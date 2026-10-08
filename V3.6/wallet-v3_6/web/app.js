const $ = (selector) => document.querySelector(selector);
const $$ = (selector) => [...document.querySelectorAll(selector)];
const form = $("#terms-form");
let profile = null;
let walletProfiles = [];
let selectedWalletProfile = null;
let profileDialogMode = "create";
let activeDraft = null;
let importedWcRequest = null;
let signedReservation = null;
const ACTIVITY_LOG_KEY = "xhub-v36-activity-log-v1";
const ACTIVITY_LOG_LIMIT = 300;
const activities = (() => {
  try {
    const saved = JSON.parse(localStorage.getItem(ACTIVITY_LOG_KEY) || "[]");
    return Array.isArray(saved) ? saved.slice(0, ACTIVITY_LOG_LIMIT) : [];
  } catch { return []; }
})();
let activityPersistTimer = null;
const historyRows = [];
let chainHistoryRows = [];
let historyMode = "account";
let chainSyncTimer = null;
let chainSyncInFlight = false;
const CHAIN_SYNC_INTERVAL_MS = 10_000;
const SETTLEMENT_PENDING_GRACE_BLOCKS = 5;
const SETTLEMENT_RETRY_MIN_FEE_MOJO = 1_000;
const FUNDING_TRACKER_KEY = "xhub-v36-funding-tracker-v1";
const FUNDING_TRACKER_ACTIVE_KEY = "xhub-v36-funding-tracker-active";
const FUNDING_TRACKER_REMOVED_KEY = "xhub-v36-funding-tracker-removed-v1";
const RESERVATION_TRACKER_KEY = "xhub-v36-reservation-tracker-v1";
let fundingTrackerInFlight = false;
let settlementInFlight = false;
const fundingRegistrationInFlight = new Set();
let merchantWcPollTimer = null;
let reservationTrackerPollTimer = null;
const reservationEventSources = new Map();
let sensitiveHideTimer = null;
let preparedWalletSend = null;
let currentPeakHeight = null;

function normalizeCoinId(value) {
  return String(value || "").trim().replace(/^0x/, "").toLowerCase();
}

// —— 一次性消费 / 商户幂等键（B2 + C） ——
// 一个 WC 请求码是 "一次性" 的：同一个 request_id 只能被一个 Funding Coin 消费。
// HUB 已在服务端强制执行（REQUEST_ALREADY_CONSUMED）；钱包与商户面板在本地同样执行，
// 并且本地索引改用 (funding_coin_id, request_id) 作为幂等键，避免两个通道共用同一个
// request_id 时被并成一条记录（商户少记一笔）。
function reservationEntryCoin(entry) {
  return normalizeCoinId(entry?.funding_coin_id) || "";
}

// 两条本地记录是否指向 "同一笔付款"：request_id 相同，且 Funding Coin 相同
// （未绑定通道的待签记录与第一个绑定它的通道属于同一笔）。
function sameReservationRecord(left, right) {
  if (String(left?.request_id || "") !== String(right?.request_id || "")) return false;
  const a = reservationEntryCoin(left);
  const b = reservationEntryCoin(right);
  if (!a || !b) return true;
  return a === b;
}

function findReservationTracker(requestId, fundingCoinId) {
  const probe = { request_id: requestId, funding_coin_id: normalizeCoinId(fundingCoinId) || null };
  return loadReservationTrackers().find((entry) => sameReservationRecord(entry, probe)) || null;
}

// 本机已经用这个 request_id 消费过的全部 Funding Coin（用于一次性消费守卫）。
function reservationRequestConsumedCoins(requestId) {
  const id = String(requestId || "");
  if (!id) return [];
  const coins = new Set();
  for (const entry of loadReservationTrackers()) {
    if (String(entry?.request_id || "") !== id) continue;
    const coin = reservationEntryCoin(entry);
    if (coin) coins.add(coin);
  }
  for (const tracker of loadFundingTrackers()) {
    const summaries = Array.isArray(tracker?.reservation_summaries) ? tracker.reservation_summaries : [];
    for (const item of [...summaries, tracker?.last_reservation].filter(Boolean)) {
      if (String(item?.request_id || "") !== id || item?.ledger_written === false) continue;
      const coin = reservationEntryCoin({ funding_coin_id: tracker?.funding_coin_id });
      if (coin) coins.add(coin);
    }
  }
  return [...coins];
}

function shortCoinLabel(coinId) {
  const value = normalizeCoinId(coinId);
  return value ? `${value.slice(0, 16)}…` : "-";
}

function positiveIntegerOrUndefined(value) {
  const number = Number(value);
  return Number.isSafeInteger(number) && number > 0 ? number : undefined;
}

function loadReservationTrackers() {
  try {
    const raw = localStorage.getItem(walletScopedKey(RESERVATION_TRACKER_KEY)) || "[]";
    const items = JSON.parse(raw);
    return Array.isArray(items) ? items.filter((item) => item?.request_id) : [];
  } catch { return []; }
}

function saveReservationTracker(item) {
  // 幂等键为 (funding_coin_id, request_id)：同通道重试合并为一条，
  // 不同通道共用同一 request_id 时保留为两条（商户侧要能看到两笔）。
  const entries = loadReservationTrackers().filter((entry) => !sameReservationRecord(entry, item));
  const next = entries.slice(0, 99);
  next.unshift({
    ...item,
    wallet_fingerprint: selectedWalletProfile?.fingerprint || null,
    wallet_puzzle_hash: selectedWalletProfile?.puzzle_hash || null,
    wallet_role: selectedWalletProfile?.role || null,
    updated_at: new Date().toISOString(),
  });
  localStorage.setItem(walletScopedKey(RESERVATION_TRACKER_KEY), JSON.stringify(next));
  persistReservationTrackers(next);
  renderReservationTrackerList();
  startReservationEventStream(next[0]);
}

function applyPreauthEvent(entry, summary) {
  if (!entry || !summary) return false;
  const signature = JSON.stringify({ hub: summary.hub, towers: summary.towers, state_sequence: summary.state_sequence, funding_coin_id: summary.funding_coin_id });
  if (entry._preauth_event_signature === signature) return false;
  entry._preauth_event_signature = signature;
  entry.preAuthStatus = summary;
  entry.funding_coin_id = summary.funding_coin_id || entry.funding_coin_id || null;
  entry.state_sequence = summary.state_sequence ?? entry.state_sequence;
  if (summary.hub?.ledger_written === true) entry.ledger_written = true;
  const towers = summary.towers || {};
  entry.replica_statuses = Object.fromEntries(["A", "B", "C"].map((tower) => {
    const value = towers[tower.toLowerCase()] || {};
    return [tower, {
      present: value.accepted === true,
      accepted: value.accepted === true,
      greenlight: value.greenlight === true,
      state: value.state || "WAIT",
      checked: true,
      error: value.error || null,
    }];
  }));
  entry.updated_at = new Date().toISOString();
  const entries = loadReservationTrackers();
  const index = entries.findIndex((value) => sameReservationRecord(value, entry));
  if (index >= 0) {
    entries[index] = { ...entries[index], ...entry };
    localStorage.setItem(walletScopedKey(RESERVATION_TRACKER_KEY), JSON.stringify(entries));
  }
  renderReservationTrackerList();
  return true;
}

function startReservationEventStream(entry) {
  const requestId = entry?.request_id;
  if (!requestId || typeof EventSource === "undefined") return;
  const current = reservationEventSources.get(requestId);
  if (current) return;
  const source = new EventSource(`/api/v3.6/pos/preauth/${encodeURIComponent(requestId)}/events?protocol_version=0x0360`);
  source.addEventListener("preauth", (event) => {
    try {
      const summary = JSON.parse(event.data);
      if (applyPreauthEvent(entry, summary)) persistReservationTrackers(loadReservationTrackers());
    } catch (_) { /* polling remains the fallback for malformed events */ }
  });
  source.addEventListener("error", () => {
    // EventSource reconnects automatically. Keep the 15-second polling path
    // active so a gateway restart cannot hide a status change.
  });
  reservationEventSources.set(requestId, source);
}

function startReservationEventStreams(entries = loadReservationTrackers()) {
  entries.slice(0, 20).forEach(startReservationEventStream);
}

function persistReservationTrackers(entries = loadReservationTrackers()) {
  if (!selectedWalletProfile?.role) return;
  fetch(`/api/v3.6/wallet-profiles/reservation-trackers?role=${encodeURIComponent(selectedWalletProfile.role)}`, {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({ role: selectedWalletProfile.role, entries: entries.slice(0, 100) }),
  }).catch(() => {});
}

async function hydrateReservationTrackers() {
  if (!selectedWalletProfile?.role) return;
  try {
    const result = await request(`/api/v3.6/wallet-profiles/reservation-trackers?role=${encodeURIComponent(selectedWalletProfile.role)}`);
    const identity = String(selectedWalletProfile.fingerprint || selectedWalletProfile.wallet_public_key?.slice(0, 16) || "");
    const merchantId = identity ? `M-${identity.slice(0, 16).toUpperCase()}` : "";
    let cloudRequests = [];
    if (merchantId) {
      try {
        const history = await request(`/api/v3.6/pos/preauth/history?protocol_version=0x0360&merchant_id=${encodeURIComponent(merchantId)}&limit=100`);
        cloudRequests = (Array.isArray(history) ? history : []).map((item) => ({
          request_id: item.request_id,
          status: item.status,
          amount: item.request?.funding_amount || "-",
          note: item.request?.user_note || "",
          funding_coin_id: item.funding_coin_id || null,
          reservation_nonce: item.reservation_nonce || item.request?.reservation_nonce || null,
          merchant_puzzle_hash: item.request?.merchant_puzzle_hash || null,
          settlement_mode: "DIRECT_MERCHANT",
          direct_settlement_confirmed: true,
          ledger_written: ["SIGNED", "DELIVERED", "AUTHORIZED"].includes(String(item.status || "").toUpperCase()),
          created_at: new Date(Number(item.created_at || 0) * 1000).toISOString(),
          updated_at: new Date(Number(item.updated_at || item.created_at || 0) * 1000).toISOString(),
        }));
      } catch (_) { /* local and wallet-profile history remain usable */ }
    }
    const localEntries = loadReservationTrackers();
    // 合并键同样为 (funding_coin_id, request_id)：云侧、钱包档案与本地三份来源，
    // 只有当 request_id 与 Funding Coin 都一致时才视为同一条；首个来源优先。
    const mergedList = [];
    [...cloudRequests, ...(Array.isArray(result.entries) ? result.entries : []), ...localEntries].forEach((entry) => {
      if (!entry?.request_id) return;
      if (mergedList.some((value) => sameReservationRecord(value, entry))) return;
      mergedList.push(entry);
    });
    const entries = mergedList.slice(0, 100);
    localStorage.setItem(walletScopedKey(RESERVATION_TRACKER_KEY), JSON.stringify(entries));
    if (entries.length) persistReservationTrackers(entries);
    renderReservationTrackerList();
    startReservationEventStreams(entries);
    // Refresh historical requests as well as the currently focused request so
    // a merchant can see HUB authorization and a later direct-settlement Coin
    // without reopening the WC generator.
    refreshAllReservationTrackers();
  } catch (_) {
    // The native file is an additional persistence layer; the local index remains usable.
  }
}

function renderReservationTrackerList() {
  const target = $("#reservation-tracker-list");
  if (!target) return;
  const entries = loadReservationTrackers();
  target.innerHTML = entries.length ? entries.map((entry) => {
    const direct = entry.settlement_mode === "DIRECT_MERCHANT" && entry.direct_settlement_confirmed === true;
    const status = String(entry.status || "WC_CREATED").toUpperCase();
    const ledgerWritten = entry.ledger_written === true || (entry.ledger_written !== false && ["SIGNED", "DELIVERED", "AUTHORIZED"].includes(status));
    const received = confirmedReservationReceipt(entry);
    const securityState = preauthSecurityState(entry);
    const arrivalLabel = received
      ? `已到账 · ${entry.received_amount || entry.amount || "-"} mojo`
      : direct && ledgerWritten ? "已预扣 · 等待最终结算到账" : status === "FORWARDED" ? "已转入商户钱包（旧版 Payment Coin）" : "尚未写入 HUB 账本";
    const badgeClass = received || status === "FORWARDED" ? "good" : ledgerWritten ? "warn" : "neutral";
    return `<details class="reservation-entry"><summary><span><strong>${escapeHtml(entry.note || "未命名预扣")}</strong><code>${escapeHtml(String(entry.request_id).slice(0, 16))}…</code></span><span class="reservation-summary-right"><span class="tag ${badgeClass}">${escapeHtml(received ? "已到账" : status)}</span>${replicaLightsMarkup(entry, ledgerWritten)}</span></summary><dl class="summary-metrics"><div><dt>Request ID</dt><dd>${escapeHtml(entry.request_id)}</dd></div><div><dt>Funding Coin</dt><dd>${escapeHtml(entry.funding_coin_id || "等待用户签名")}</dd></div><div><dt>预扣金额</dt><dd>${escapeHtml(entry.amount || "-")} mojo</dd></div><div><dt>HUB 账本</dt><dd>${ledgerWritten ? `已写入${entry.state_sequence != null ? ` · state_sequence=${escapeHtml(entry.state_sequence)}` : ""}` : "等待用户授权"}</dd></div><div><dt>安全回执</dt><dd>${escapeHtml(securityState)}</dd></div><div><dt>结算方式</dt><dd>${direct ? "直接进入商户地址" : entry.payment_coin_id ? `旧版 Payment Coin：${escapeHtml(entry.payment_coin_id)}` : "旧版 Payment Coin（结算后发现）"}</dd></div><div><dt>到账追踪</dt><dd>${escapeHtml(arrivalLabel)}</dd></div>${entry.merchant_puzzle_hash ? `<div><dt>商户 Puzzle Hash</dt><dd>${escapeHtml(entry.merchant_puzzle_hash)}</dd></div>` : ""}${received ? `<div><dt>到账 Coin</dt><dd>${escapeHtml(received)}</dd></div><div><dt>到账确认高度</dt><dd>${escapeHtml(entry.received_height ?? "等待确认")}</dd></div>` : ""}<div><dt>最后更新</dt><dd>${escapeHtml(entry.updated_at || entry.created_at || "-")}</dd></div></dl>${reservationClosingCoinsMarkup(entry)}${!direct && entry.funding_coin_id && status !== "FORWARDED" ? `<button type="button" class="button primary merchant-forward-button" data-request-id="${escapeHtml(entry.request_id)}" data-funding-coin-id="${escapeHtml(entry.funding_coin_id || "")}">发现并转入商户钱包</button>` : direct ? `<div class="notice-box">${escapeHtml(received ? "已发现商户地址到账 Coin；请同时核对链上确认数。" : "新预扣在最终结算时直接创建商户地址 Coin。当前仅表示 HUB 已记录预扣，挑战期结束并完成 FINALIZE 后才会到账。")}</div>` : ""}</details>`;
  }).join("") : '<div class="empty-state">尚无预扣记录。生成 WC 后会自动出现在这里。</div>';
}

function preauthSecurityState(entry) {
  const summary = entry?.preAuthStatus?.summary;
  if (!summary) return "WAITING";
  const greenlightCount = Number(summary.greenlight_count || 0);
  const required = Number(summary.required_greenlights || 2);
  if (greenlightCount >= required) return "GREENLIGHT";
  return greenlightCount > 0 ? "PARTIAL_GREENLIGHT" : "WAITING";
}

function confirmedReservationReceipt(entry) {
  // A bare received_coin_id may come from the pre-fix amount-only detector or
  // stale local storage. The parent binding proves this Coin was created by
  // this reservation's final Closing Coin.
  return entry?.received_coin_id && entry?.received_parent_coin_id
    ? entry.received_coin_id
    : null;
}

function replicaLightsMarkup(entry, hubPresent) {
  const replicas = entry?.replica_statuses || {};
  const light = (label, value) => {
    const state = typeof value === "object" && value !== null ? value : { present: value === true };
    const hasGreenlight = Object.prototype.hasOwnProperty.call(state, "greenlight");
    const accepted = state.accepted === true || (!hasGreenlight && state.present === true);
    const cryptographicGreenlight = hasGreenlight ? state.greenlight === true : false;
    const detail = state.error
      ? `读取失败：${state.error}`
      : cryptographicGreenlight ? "密码学绿灯已满足" : accepted ? "已接收 Recovery Package，安全回执等待中" : state.checked === true ? "接口已读取，但未匹配到这个 Coin" : "等待读取";
    const title = escapeHtml(`${label}：${detail}${accepted ? "；点击查看 Recovery Package" : ""}`);
    const tone = accepted ? "present" : state.error ? "error" : state.not_applicable ? "not-applicable" : "absent";
    if (accepted && entry?.funding_coin_id && entry?.state_sequence != null) {
      return `<button type="button" class="replica-light replica-light-link ${tone}" data-package-source="${label}" data-funding-coin-id="${escapeHtml(String(entry.funding_coin_id).replace(/^0x/, ""))}" data-state-sequence="${escapeHtml(entry.state_sequence)}" title="${title}"><i></i><b>${label}</b></button>`;
    }
    return `<span class="replica-light ${tone}" title="${title}"><i></i><b>${label}</b></span>`;
  };
  return `<span class="replica-status" aria-label="预扣副本状态">${light("HUB", { present: hubPresent, checked: true })}${light("A", replicas.A)}${light("B", replicas.B)}${light("C", replicas.C)}</span>`;
}

document.addEventListener("click", async (event) => {
  const light = event.target.closest(".replica-light-link");
  if (!light) return;
  const source = light.dataset.packageSource;
  const coin = light.dataset.fundingCoinId;
  const sequence = light.dataset.stateSequence;
  if (!/^(HUB|A|B|C)$/.test(source) || !/^[0-9a-f]{64}$/i.test(coin) || !/^\d+$/.test(sequence)) return;
  const url = `https://hub.chiagame.top/recovery-package/${source}/${coin.toLowerCase()}/${sequence}`;
  light.disabled = true;
  try {
    await request("/api/v3.6/wallet-profiles/open-external", {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({ url }),
    });
  } catch (error) {
    window.alert(`无法打开 Recovery Package 页面：${error.message}`);
  } finally {
    light.disabled = false;
  }
});

async function refreshReservationEntry(entry, { detectReceipt = true } = {}) {
  if (!entry?.request_id) return entry;
  let requestResult = null;
  try {
    requestResult = await request(`/api/v3.6/hub/wallet-connect/requests/${entry.request_id}`);
  } catch (error) {
    // WC request records expire after a short hand-off window.  A reservation
    // already written to the HUB ledger remains queryable by Coin + nonce, so
    // do not discard the locally persisted linkage when the public WC expires.
    requestResult = { status: entry.status, request: {}, request_id: entry.request_id };
    entry.request_expired = true;
    entry.last_query_error = error.message;
  }
  const requestPayload = requestResult.request || {};
  entry.status = String(requestResult.status || entry.status || "WC_CREATED").toUpperCase();
  // 本地已记录的 (funding_coin_id, reservation_nonce) 是本钱包实际签过的那一笔，优先级高于
  // HUB 的 WC 行：WC 行只按 request_id 存一行，历史上被两个通道复用时只有一个答案，
  // 用它覆盖会造成两条本地记录塌陷成一条（商户少记一笔）。
  entry.funding_coin_id = entry.funding_coin_id || requestResult.funding_coin_id || null;
  entry.reservation_nonce = entry.reservation_nonce || requestResult.reservation_nonce || requestPayload.reservation_nonce || null;
  entry.amount = requestPayload.funding_amount || entry.amount || "-";
  entry.note = requestPayload.user_note || entry.note || "";
  entry.merchant_puzzle_hash = requestPayload.merchant_puzzle_hash || entry.merchant_puzzle_hash || null;
  if (entry.funding_coin_id && entry.reservation_nonce) {
    try {
      const result = await request(`/api/v3.6/hub/funding-coins/${entry.funding_coin_id.replace(/^0x/, "")}/reservations/${entry.reservation_nonce.replace(/^0x/, "")}?protocol_version=0x0360`);
      entry.status = String(result.status || entry.status).toUpperCase();
      entry.ledger_written = result.ledger_written === true || ["SIGNED", "DELIVERED"].includes(entry.status);
      entry.state_sequence = result.state_sequence ?? entry.state_sequence;
      entry.observed_peak_height = result.observed_peak_height ?? entry.observed_peak_height;
      entry.acceptance_cutoff_height = result.acceptance_cutoff_height ?? entry.acceptance_cutoff_height;
      entry.scheduled_close_height = result.scheduled_close_height ?? entry.scheduled_close_height;
      entry.recovery_package_content_hash = result.recovery_package_content_hash || entry.recovery_package_content_hash;
    } catch (_) { /* request status remains useful while the reservation record catches up */ }
  }
  await refreshTowerReplicaStatus(entry);
  await refreshReservationClosingCoins(entry);
  if (entry.ledger_written === true && !entry.authorized_at) entry.authorized_at = new Date().toISOString();
  if (detectReceipt && entry.settlement_mode === "DIRECT_MERCHANT" && entry.merchant_puzzle_hash) await detectMerchantReceipt(entry);
  entry.updated_at = new Date().toISOString();
  return entry;
}

async function refreshReservationClosingCoins(entry) {
  if (!entry?.funding_coin_id) return;
  const coin = String(entry.funding_coin_id).replace(/^0x/, "").toLowerCase();
  if (!/^[0-9a-f]{64}$/.test(coin)) return;
  try {
    const result = await request(`/api/v3.6/chain/closing-coins/${coin}?protocol_version=0x0360`);
    if (!Array.isArray(result.coins)) return;
    entry.closing_coins = result.coins.filter((item) => /^[0-9a-f]{64}$/i.test(String(item.coin_id || "").replace(/^0x/, ""))).map((item) => ({
      sequence: Number(item.sequence || 0),
      coin_id: String(item.coin_id).replace(/^0x/, "").toLowerCase(),
      amount_mojo: Number(item.amount_mojo || 0),
      confirmed_height: item.confirmed_height == null ? null : Number(item.confirmed_height),
      spent_height: item.spent_height == null ? null : Number(item.spent_height),
      status: String(item.status || "UNKNOWN").toUpperCase(),
    }));
    entry.closing_coins_peak_height = result.peak_height == null ? null : Number(result.peak_height);
    entry.closing_coins_updated_at = new Date().toISOString();
  } catch (_) { /* keep the last successful chain lineage */ }
}

function reservationClosingCoinsMarkup(entry) {
  const coins = Array.isArray(entry?.closing_coins) ? entry.closing_coins : [];
  if (!coins.length) return `<details class="reservation-closing-trace"><summary>链上 Closing Coin</summary><div class="empty-state">尚未发现，等待 Funding Coin children。</div></details>`;
  const rows = coins.map((coin) => {
    const unspent = coin.status === "UNSPENT";
    const action = unspent
      ? `<div class="reservation-closing-action"><label>手续费 <input class="closing-fee-input" type="number" min="0" step="1" value="1" inputmode="numeric" aria-label="Closing Coin 手续费 mojo"></label><button type="button" class="button secondary closing-spend-button" data-request-id="${escapeHtml(entry.request_id || "")}" data-funding-coin-id="${escapeHtml(entry.funding_coin_id || "")}" data-closing-coin-id="${escapeHtml(coin.coin_id)}">最终结算</button></div>`
      : "";
    return `<div class="reservation-closing-row"><strong>SEQ${escapeHtml(String(coin.sequence || "-"))}</strong><code>${coinsetLink(coin.coin_id)}</code><span class="tag ${unspent ? "good" : "neutral"}">${escapeHtml(coin.status)}</span><small>出生 ${escapeHtml(coin.confirmed_height ?? "-")} · 花费 ${escapeHtml(coin.spent_height ?? "-")} · ${escapeHtml(coin.amount_mojo)} mojo</small>${action}</div>`;
  }).join("");
  return `<details class="reservation-closing-trace"><summary>链上 Closing Coin（${coins.length} 枚）</summary><div class="reservation-closing-list">${rows}</div>${entry.closing_coins_peak_height != null ? `<small>Coinset peak：${escapeHtml(entry.closing_coins_peak_height)}</small>` : ""}</details>`;
}

async function refreshTowerReplicaStatus(entry) {
  entry.replica_statuses = {
    A: { present: false, checked: false },
    B: { present: false, checked: false },
    C: { present: false, checked: false },
  };
  // Tower operation logs from older deployments may omit the package hash.
  // Funding Coin + state sequence are sufficient to identify the accepted
  // replica; a missing hash must not suppress the A/B/C status lookup.
  if (!entry.funding_coin_id || entry.ledger_written !== true) {
    entry.replica_statuses = {
      A: { present: false, checked: true, not_applicable: true },
      B: { present: false, checked: true, not_applicable: true },
      C: { present: false, checked: true, not_applicable: true },
    };
    return;
  }
  const coin = String(entry.funding_coin_id).replace(/^0x/, "").toLowerCase();
  const sequence = entry.state_sequence == null ? null : Number(entry.state_sequence);
  if (entry.request_id) {
    try {
      const summary = await request(`/api/v3.6/pos/preauth/${encodeURIComponent(entry.request_id)}/status?protocol_version=0x0360`);
      const towers = summary?.towers;
      if (towers && typeof towers === "object") {
        entry.preAuthStatus = summary;
        entry.replica_statuses = Object.fromEntries(["A", "B", "C"].map((tower) => {
          const value = towers[tower.toLowerCase()] || {};
          return [tower, {
            present: value.accepted === true,
            accepted: value.accepted === true,
            greenlight: value.greenlight === true,
            state: value.state || "WAIT",
            checked: true,
            error: value.error || null,
          }];
        }));
        return;
      }
    } catch (_) {
      // Older HUB builds do not expose the aggregate endpoint yet; retain the
      // raw Tower log fallback below during the rolling upgrade.
    }
  }
  await Promise.all(["A", "B", "C"].map(async (tower) => {
    try {
      const result = await request(`/api/v3.6/tower-logs/${tower}?protocol_version=0x0360&limit=500`);
      const rows = Array.isArray(result.entries) ? result.entries : [];
      const present = rows.some((row) => {
        if (String(row.event_type || "").toUpperCase() !== "RECOVERY_PACKAGE_ACCEPTED") return false;
        if (String(row.funding_coin_id || "").replace(/^0x/, "").toLowerCase() !== coin) return false;
        if (sequence != null && Number(row.state_sequence) !== sequence) return false;
        // Older tower operations-log responses do not expose the package
        // content hash. Funding Coin + state sequence are still the signed
        // acceptance identity, so a missing hash must not make a known tower
        // replica appear absent. When a hash is present, retain the stronger
        // equality check.
        const rowHash = String(row.recovery_package_content_hash || row.content_hash || row.package_hash || "").replace(/^0x/, "").toLowerCase();
        const entryHash = String(entry.recovery_package_content_hash || "").replace(/^0x/, "").toLowerCase();
        return !rowHash || !entryHash || rowHash === entryHash;
      });
      entry.replica_statuses[tower] = { present, checked: true, entries: rows.length };
    } catch (error) {
      entry.replica_statuses[tower] = { present: false, checked: true, error: error.message || String(error) };
    }
  }));
}

async function detectMerchantReceipt(entry) {
  if (!selectedWalletProfile?.puzzle_hash || selectedWalletProfile.puzzle_hash.toLowerCase() !== String(entry.merchant_puzzle_hash || "").toLowerCase()) return;
  // A reservation is only "received" after the final Closing Coin has been
  // spent. SIGNED/DELIVERED means HUB authorization only; it cannot create a
  // merchant Coin. Require the terminal Closing Coin to be spent and bind the
  // receipt to that exact parent, instead of matching any same-amount Coin.
  const closingCoins = Array.isArray(entry.closing_coins) ? entry.closing_coins : [];
  const finalClosing = closingCoins
    .filter((coin) => Number(coin.sequence || 0) > 0)
    .sort((a, b) => Number(b.sequence || 0) - Number(a.sequence || 0))[0];
  const finalClosingId = normalizeCoinId(finalClosing?.coin_id);
  const finalSpentHeight = Number(finalClosing?.spent_height || 0);
  if (!finalClosingId || !finalSpentHeight || String(finalClosing?.status || "").toUpperCase() !== "SPENT") {
    // Records written by the old amount-only detector are not authoritative.
    // Remove only unbound results; a bound result can be retained while the
    // chain endpoint is temporarily unavailable.
    if (!entry.received_parent_coin_id) {
      delete entry.received_coin_id;
      delete entry.received_amount;
      delete entry.received_height;
      delete entry.received_status;
    }
    return;
  }
  try {
    const synced = await request("/api/v3.6/wallet-profiles/chain/sync", { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify({ role: selectedWalletProfile.role, rpc_url: "https://api.coinset.org" }) });
    const amount = Number(entry.amount);
    if (!Number.isFinite(amount) || amount <= 0 || !Array.isArray(synced.history)) return;
    const candidate = synced.history.find((coin) => Number(coin.amount_mojo) === amount
      && normalizeCoinId(coin.parent_coin_id) === finalClosingId
      && Number(coin.confirmed_height || 0) >= finalSpentHeight
      && coin.status === "UNSPENT");
    if (candidate) {
      entry.received_coin_id = candidate.coin_id;
      entry.received_amount = String(candidate.amount_mojo);
      entry.received_height = candidate.confirmed_height;
      entry.received_status = candidate.status;
      entry.received_parent_coin_id = finalClosingId;
    }
  } catch (_) { /* chain sync is best effort; do not turn a valid HUB record into an error */ }
}

async function refreshAllReservationTrackers() {
  const entries = loadReservationTrackers();
  if (!entries.length) return;
  let changed = false;
  for (const entry of entries.slice(0, 100)) {
    try { await refreshReservationEntry(entry); changed = true; } catch (_) { /* keep the last known public state */ }
  }
  if (changed) {
    localStorage.setItem(walletScopedKey(RESERVATION_TRACKER_KEY), JSON.stringify(entries));
    persistReservationTrackers(entries);
    renderReservationTrackerList();
  }
}

async function forwardMerchantPayment(entry) {
  if (!entry?.funding_coin_id) throw new Error("该预扣尚未关联 Funding Coin");
  const packageResponse = await request(`/api/v3.6/hub/funding-coins/${entry.funding_coin_id.replace(/^0x/, "")}/recovery-packages/latest?protocol_version=0x0360`);
  const packageHex = packageResponse.recovery_package_canonical_hex;
  if (!packageHex) throw new Error("HUB 尚未提供 Recovery Package");
  const discovered = await request("/api/v3.6/wallet-profiles/merchant-forward/discover", {
    method: "POST", headers: { "content-type": "application/json" },
    body: JSON.stringify({ role: selectedWalletProfile.role, rpc_url: "https://api.coinset.org", funding_coin_id: entry.funding_coin_id, recovery_package_canonical_hex: packageHex, entry_index: 0 }),
  });
      if (discovered.status === "SPENT") throw new Error("这个旧版 Payment Coin 已经被转发");
  entry.payment_coin_id = discovered.payment_coin_id;
  entry.payment_puzzle_hash = discovered.payment_puzzle_hash;
  const prepared = await request("/api/v3.6/wallet-profiles/merchant-forward/prepare", {
    method: "POST", headers: { "content-type": "application/json" },
    body: JSON.stringify({ role: selectedWalletProfile.role, rpc_url: "https://api.coinset.org", payment_coin_id: discovered.payment_coin_id, payment_puzzle_hash: discovered.payment_puzzle_hash, payment_amount_mojo: Number(discovered.payment_amount_mojo), recovery_package_canonical_hex: packageHex, entry_index: 0, fee_mojo: 0 }),
  });
  const destination = prepared.merchant_puzzle_hash || selectedWalletProfile?.puzzle_hash || "-";
  if (!window.confirm(`确认将 ${discovered.payment_amount_mojo} mojo 转入当前 SH001 商户钱包？\n目标 Puzzle Hash：${destination}\n手续费：0 mojo`)) return;
  const broadcast = await request("/api/v3.6/wallet-profiles/merchant-forward/broadcast", {
    method: "POST", headers: { "content-type": "application/json" },
    body: JSON.stringify({ confirmation_token: prepared.confirmation_token, spend_bundle_id: prepared.spend_bundle_id, confirm_broadcast: true }),
  });
  entry.status = "FORWARDED";
  entry.forward_spend_bundle_id = broadcast.spend_bundle_id || prepared.spend_bundle_id;
  saveReservationTracker(entry);
  addActivity("商户 Payment Coin 已转发", `${discovered.payment_amount_mojo} mojo · SpendBundle=${entry.forward_spend_bundle_id}`);
}

document.addEventListener("click", async (event) => {
  const closingSpend = event.target.closest(".closing-spend-button");
  if (closingSpend) {
    event.preventDefault();
    const fundingId = String(closingSpend.dataset.fundingCoinId || "").replace(/^0x/, "").toLowerCase();
    const tracker = loadFundingTrackers().find((item) => String(item.funding_coin_id || "").replace(/^0x/, "").toLowerCase() === fundingId);
    let activeTracker = tracker;
    if (!activeTracker) {
      const reservation = findReservationTracker(closingSpend.dataset.requestId, fundingId)
        || loadReservationTrackers().find((item) => item.request_id === closingSpend.dataset.requestId);
      if (!reservation) { window.alert("该预扣记录没有有效的 Funding Coin ID，无法执行最终结算。"); return; }
      activeTracker = {
        schema: "xhub.wallet.funding-tracker.v1",
        funding_coin_id: fundingId,
        tracking_wallet_puzzle_hash: selectedWalletProfile?.puzzle_hash || null,
        creator_puzzle_hash: selectedWalletProfile?.puzzle_hash || null,
        amount_mojo: Number(reservation.amount || 0),
        reserved_mojo: Number(reservation.amount || 0),
        settlement_stage: "NONE",
        closing_coins: reservation.closing_coins || [],
        created_at: reservation.created_at || new Date().toISOString(),
      };
      saveFundingTracker(activeTracker);
    }
    localStorage.setItem(walletScopedKey(FUNDING_TRACKER_ACTIVE_KEY), activeTracker.funding_coin_id);
    const fee = closingSpend.closest(".reservation-closing-row")?.querySelector(".closing-fee-input")?.value?.trim() || "1";
    if (!/^\d+$/.test(fee)) { window.alert("手续费必须是非负整数 mojo。"); return; }
    activeTracker.current_closing_coin_id = String(closingSpend.dataset.closingCoinId || "").replace(/^0x/, "").toLowerCase();
    const selectedClosing = (activeTracker.closing_coins || []).find((coin) => String(coin.coin_id || "").replace(/^0x/, "").toLowerCase() === activeTracker.current_closing_coin_id);
    if (selectedClosing?.sequence != null) {
      activeTracker.current_closing_sequence = Number(selectedClosing.sequence);
      activeTracker.current_closing_confirmed_height = selectedClosing.confirmed_height ?? activeTracker.current_closing_confirmed_height;
      activeTracker.settlement_stage = Number(selectedClosing.sequence) > 1 ? "SUBSEQUENT_CLOSING" : "INITIAL_CLOSING";
    }
    saveFundingTracker(activeTracker);
    const feeField = $("#settlement-fee");
    if (feeField) feeField.value = fee;
    // Reuse the existing authoritative settlement handler. It performs the
    // fresh Coinset/HUB checks, constructs the bundle, asks for confirmation,
    // and only then calls push_tx.
    setTimeout(() => {
      const settle = $("#settle-funding-coin");
      if (settle) {
        closingSpend.disabled = true;
        closingSpend.textContent = "正在准备...";
        settle.disabled = false;
        settle.click();
      }
    }, 0);
    return;
  }
  const button = event.target.closest(".merchant-forward-button");
  if (!button || button.disabled) return;
  const entry = findReservationTracker(button.dataset.requestId, button.dataset.fundingCoinId);
  if (!entry) return;
  button.disabled = true;
  button.textContent = "正在发现并转发...";
  try { await forwardMerchantPayment(entry); }
  catch (error) { addActivity("商户收款转发失败", error.message); window.alert(`商户收款转发失败：${error.message}`); }
  finally { renderReservationTrackerList(); }
});

function walletScopedKey(base) {
  const wallet = selectedWalletProfile?.puzzle_hash?.toLowerCase() || "unselected";
  return `${base}:${wallet}`;
}

function coinsetLink(id) {
  const value = String(id || "").replace(/^0x/, "").toLowerCase();
  if (!/^[0-9a-f]{64}$/.test(value)) return escapeHtml(id || "-");
  return `<a class="coinset-link" href="https://www.coinset.org/coin/${value}" target="_blank" rel="noopener noreferrer" title="在 Coinset 查看 Coin">${value}</a>`;
}

function linkCoinIdsIn(root = document) {
  const selectors = "code, .history-id, .address-display, dd";
  root.querySelectorAll?.(selectors).forEach((node) => {
    if (node.closest("a") || node.querySelector("a")) return;
    const text = node.textContent.trim();
    if (/^0x?[0-9a-f]{64}$/i.test(text)) node.innerHTML = coinsetLink(text);
  });
}

const coinsetLinkObserver = new MutationObserver(() => linkCoinIdsIn(document));
coinsetLinkObserver.observe(document.documentElement, { childList: true, subtree: true });

function trackerBelongsToCurrentWallet(item) {
  const wallet = selectedWalletProfile?.puzzle_hash?.toLowerCase();
  if (!wallet || !item) return false;
  if (item.imported === true) {
    const trackingWallet = item.tracking_wallet_puzzle_hash?.toLowerCase();
    // Older imported tracker records were persisted without the wallet
    // fingerprint. They are portable public Coin records, so keep them
    // visible and let the current wallet claim the record on save/import.
    return !trackingWallet || trackingWallet === wallet;
  }
  return item.creator_puzzle_hash?.toLowerCase() === wallet;
}

function encodeWcRequest(value) {
  const bytes = new TextEncoder().encode(JSON.stringify(value));
  let binary = "";
  bytes.forEach((byte) => { binary += String.fromCharCode(byte); });
  return `wc1${btoa(binary).replace(/=/g, "").replace(/\+/g, "-").replace(/\//g, "_")}`;
}

function decodeWcRequest(value) {
  const raw = value.trim();
  if (!raw.startsWith("wc1")) throw new Error("WC 请求码必须以 wc1 开头。");
  const encoded = raw.slice(3).replace(/-/g, "+").replace(/_/g, "/");
  const padded = encoded + "=".repeat((4 - (encoded.length % 4)) % 4);
  let binary;
  try { binary = atob(padded); } catch { throw new Error("WC 请求码不是有效的公开请求格式。"); }
  const bytes = Uint8Array.from(binary, (character) => character.charCodeAt(0));
  let valueObject;
  try { valueObject = JSON.parse(new TextDecoder().decode(bytes)); } catch { throw new Error("WC 请求码内容无法解析。"); }
  if (valueObject.schema !== "xhub.wc.request.v1" || valueObject.protocol_version !== "0x0360" || valueObject.issuer !== "hub" || !["funding_request", "reservation_request"].includes(valueObject.request_type)) throw new Error("只接受由 V3.6 HUB 生成的公开请求。");
  for (const field of ["funding_amount", "termination_blocks", "challenge_blocks", "freeze_blocks"]) {
    if (typeof valueObject[field] !== "string" || !/^\d+$/.test(valueObject[field])) throw new Error(`WC 请求缺少有效字段：${field}`);
  }
  return valueObject;
}

function hubReservationPayload(signed) {
  return {
    protocol_version: signed.protocol_version,
    request_id: signed.request_id,
    funding_coin_id: signed.funding_coin_id,
    merchant_puzzle_hash: signed.merchant_puzzle_hash,
    merchant_receipt_public_key: signed.merchant_receipt_public_key,
    amount: signed.amount,
    reservation_nonce: signed.reservation_nonce,
    user_authorization_signature: signed.user_authorization_signature,
  };
}

function fundingReadyForReservation(tracked) {
  return Boolean(tracked?.confirmed_draft?.confirmed
    && tracked.last_status?.status === "CONFIRMED"
    && tracked.last_status?.spent_height == null
    // A zero-reservation channel can legitimately have no tracking summary
    // yet; the HUB-signed Recovery Package is proof that registration exists.
    && (tracked.hub_tracking || tracked.recovery_package_available || tracked.auto_registered_at));
}

function fundingReservationStatusError(status) {
  const state = status?.status || "SYNC_ERROR";
  const confirmations = Number(status?.confirmations || 0);
  const required = Number(status?.required_confirmations || 1);
  if (state === "SPENT" || status?.spent_height != null) {
    return `所选 Funding Coin 已花费，不能用于预扣（spent_height=${status?.spent_height ?? "-"}）。`;
  }
  if (state === "MEMPOOL" || state === "MISSING" || state === "NOT_FOUND") {
    return "所选 Funding Coin 目前处于 MEMPOOL 或尚未进入可查询的区块；请等待至少 1 个区块确认后重试。";
  }
  if (state === "CONFIRMING") {
    return `所选 Funding Coin 尚未达到确认要求（${confirmations}/${required} confirmations），本次未签名、未提交。`;
  }
  return `所选 Funding Coin 链上状态不是 CONFIRMED（当前 ${state}，${confirmations}/${required} confirmations），本次未签名、未提交。`;
}

function isMempoolLookupError(message) {
  const text = String(message || "").toLowerCase();
  return text.includes("coin record") && text.includes("not found");
}

function mempoolStatusDetail(status) {
  const raw = status?.error ? ` · 原始 RPC：${status.error}` : "";
  return `MEMPOOL：链上尚未返回 Coin record，可能正在等待进入区块；当前未确认${raw}`;
}

async function ensureFundingReadyForReservation(tracked) {
  let status;
  try {
    status = await request("/api/v3.6/wallet-profiles/funding/status", {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({
        rpc_url: "https://api.coinset.org",
        funding_coin_id: tracked.funding_coin_id,
        funding_puzzle_hash: tracked.funding_puzzle_hash,
        funding_amount_mojo: Number(tracked.amount_mojo),
        required_confirmations: Number(tracked.required_confirmations || 1),
      }),
    });
  } catch (error) {
    const missing = isMempoolLookupError(error.message);
    tracked.last_status = {
      status: missing ? "MEMPOOL" : "SYNC_ERROR",
      confirmations: 0,
      required_confirmations: Number(tracked.required_confirmations || 1),
      peak_height: 0,
      confirmed_height: null,
      error: error.message,
    };
    saveFundingTracker(tracked);
    if (missing) throw new Error(fundingReservationStatusError(tracked.last_status));
    throw new Error(`无法确认所选 Funding Coin 的链上状态，本次未签名、未提交：${error.message}`);
  }
  tracked.last_status = {
    ...status,
    peak_height: Number(status.peak_height || 0),
    confirmed_height: status.confirmed_height == null ? null : Number(status.confirmed_height),
  };
  saveFundingTracker(tracked);
  if (status.status !== "CONFIRMED" || status.spent_height != null) {
    throw new Error(fundingReservationStatusError(status));
  }
  if (!tracked.hub_tracking && !tracked.recovery_package_available && !tracked.auto_registered_at) {
    await autoRegisterFunding(tracked);
    if (!tracked.hub_tracking && !tracked.recovery_package_available && !tracked.auto_registered_at) {
      throw new Error(`Funding Coin 已链上确认，但尚未成功登记 HUB，本次未签名、未提交${tracked.hub_registration_error ? `：${tracked.hub_registration_error}` : "，请稍后重试"}。`);
    }
  }
  saveFundingTracker(tracked);
  return tracked;
}

// 二维码扫描：优先用摄像头实时识别（Android WebView 原生 BarcodeDetector），
// 不支持时退回「从相册选择图片」由同一 API 识别。无外部依赖、离线可用。
// onFilled 在扫码成功后回调（用于触发自动解析）。
function openQrScanner({ inputId, onFilled, parse }) {
  const input = document.getElementById(inputId);
  if (!input) return;
  const overlay = document.createElement("div");
  overlay.className = "qr-scan-overlay";
  overlay.innerHTML = `
    <div class="qr-scan-box">
      <div class="qr-scan-head">
        <span>将二维码放入框内</span>
        <button type="button" class="qr-scan-close" aria-label="关闭">✕</button>
      </div>
      <div class="qr-scan-stage">
        <video class="qr-scan-video" autoplay playsinline muted></video>
        <div class="qr-scan-reticle"></div>
      </div>
      <div class="qr-scan-foot">
        <label class="button secondary qr-scan-file">从相册选择二维码图片
          <input type="file" accept="image/*" hidden>
        </label>
        <p class="qr-scan-hint">摄像头不可用时，可选取含二维码的截图或照片。</p>
      </div>
    </div>`;
  document.body.appendChild(overlay);
  const video = overlay.querySelector(".qr-scan-video");
  const fileInput = overlay.querySelector(".qr-scan-file input");
  let stream = null;
  const onKey = (e) => { if (e.key === "Escape") close(); };
  const close = () => {
    if (stream) stream.getTracks().forEach((t) => t.stop());
    document.removeEventListener("keydown", onKey);
    overlay.remove();
  };
  document.addEventListener("keydown", onKey);
  overlay.querySelector(".qr-scan-close").addEventListener("click", close);
  overlay.addEventListener("click", (e) => { if (e.target === overlay) close(); });

  // parse(raw) 负责把扫码文本规整为目标值（如 wc1 请求码或 xch 地址）；
  // 不传则默认按 wc1 提取，保持原有 Funding / 预扣扫码行为兼容。
  const normalize = typeof parse === "function"
    ? parse
    : (raw) => { const t = (raw || "").trim(); const m = t.match(/wc1[a-zA-Z0-9]+/); return m ? m[0] : t; };
  const fill = (raw) => {
    const code = normalize(raw);
    input.value = code;
    close();
    if (typeof onFilled === "function") onFilled(code);
  };

  const detectorSupported = "BarcodeDetector" in window;
  const jsQrSupported = typeof jsQR === "function";
  const hintFoot = overlay.querySelector(".qr-scan-foot");
  if (!detectorSupported && !jsQrSupported) {
    hintFoot.insertAdjacentHTML("afterbegin", '<p class="qr-scan-hint">当前环境无内置二维码识别且未加载解码库，请手动粘贴。</p>');
  } else if (!detectorSupported) {
    hintFoot.insertAdjacentHTML("afterbegin", '<p class="qr-scan-hint">已启用纯 JS 解码（兼容无 Google 服务的设备）。</p>');
  }

  // 用 jsQR 对图片 / 视频帧解码：无 BarcodeDetector 时的兜底，让无 GMS 的国产机也能扫。
  const decodeWithJsQr = async (source) => {
    if (!jsQrSupported) return null;
    const w = source.width || source.videoWidth || source.clientWidth || 0;
    const h = source.height || source.videoHeight || source.clientHeight || 0;
    if (!w || !h) return null;
    const cap = 720;
    const scale = Math.min(1, cap / Math.max(w, h));
    const cw = Math.max(1, Math.round(w * scale));
    const chh = Math.max(1, Math.round(h * scale));
    const c = document.createElement("canvas");
    c.width = cw; c.height = chh;
    const cx = c.getContext("2d");
    cx.drawImage(source, 0, 0, cw, chh);
    let img;
    try { img = cx.getImageData(0, 0, cw, chh); } catch { return null; }
    try { const res = jsQR(img.data, cw, chh); return res ? res.data : null; } catch { return null; }
  };

  // 相册图片识别
  fileInput.addEventListener("change", async (e) => {
    const file = e.target.files && e.target.files[0];
    if (!file) return;
    try {
      const bitmap = await createImageBitmap(file);
      let value = null;
      if (detectorSupported) {
        const detections = await new BarcodeDetector({ formats: ["qr_code"] }).detect(bitmap);
        if (detections[0]) value = detections[0].rawValue;
      }
      if (!value) value = await decodeWithJsQr(bitmap);
      if (value) fill(value);
      else window.alert("未能从图片中识别二维码，请手动粘贴请求码。");
    } catch (error) {
      window.alert("图片识别失败：" + error.message);
    }
  });

  // 摄像头实时识别
  if (!navigator.mediaDevices || !navigator.mediaDevices.getUserMedia) {
    hintFoot.insertAdjacentHTML("afterbegin", '<p class="qr-scan-hint">当前环境不支持摄像头，请从相册选择。</p>');
    return;
  }
  navigator.mediaDevices.getUserMedia({ video: { facingMode: "environment" } }).then(async (s) => {
    stream = s;
    video.srcObject = s;
    await video.play();
    const tick = async () => {
      if (!overlay.isConnected) return;
      if (detectorSupported) {
        try {
          const detections = await new BarcodeDetector({ formats: ["qr_code"] }).detect(video);
          if (detections[0] && detections[0].rawValue) { fill(detections[0].rawValue); return; }
        } catch { /* 帧解码失败继续轮询 */ }
      } else if (jsQrSupported) {
        const value = await decodeWithJsQr(video);
        if (value) { fill(value); return; }
      }
      requestAnimationFrame(tick);
    };
    tick();
  }).catch((error) => {
    hintFoot.insertAdjacentHTML("afterbegin", `<p class="qr-scan-hint">无法打开摄像头：${error.message}。可改用相册选择。</p>`);
  });
}

// 将扫码得到的二维码内容（这里为 wc1... 请求码）绘制到 canvas，白底黑模块。
function drawQrToCanvas(canvas, qr, { margin = 4, maxPx = 280 } = {}) {
  const count = qr.getModuleCount();
  const cell = Math.max(2, Math.floor(maxPx / (count + margin * 2)));
  const size = (count + margin * 2) * cell;
  const dpr = window.devicePixelRatio || 1;
  canvas.width = size * dpr;
  canvas.height = size * dpr;
  canvas.style.width = `${size}px`;
  canvas.style.height = `${size}px`;
  const ctx = canvas.getContext("2d");
  ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
  ctx.fillStyle = "#ffffff";
  ctx.fillRect(0, 0, size, size);
  ctx.fillStyle = "#000000";
  for (let r = 0; r < count; r++) {
    for (let c = 0; c < count; c++) {
      if (qr.isDark(r, c)) ctx.fillRect((c + margin) * cell, (r + margin) * cell, cell, cell);
    }
  }
  return size;
}

// 把「生成商务 WC 出码」文本框的内容渲染成可扫描二维码。
function renderMerchantWcQr() {
  const out = $("#merchant-wc-output");
  const box = $("#merchant-wc-qr");
  const canvas = $("#merchant-wc-qr-canvas");
  if (!out || !box || !canvas) return;
  const text = (out.value || "").trim();
  if (!text) { box.hidden = true; return; }
  try {
    if (typeof qrcode !== "function") throw new Error("二维码库未加载");
    const qr = qrcode(0, "L"); // typeNumber 0 = 自动选择版本；L 级纠错模块更少，长 WC 更易扫
    qr.addData(text);
    qr.make();
    drawQrToCanvas(canvas, qr, { margin: 2, maxPx: 440 });
    box.hidden = false;
  } catch (error) {
    box.hidden = true;
    console.warn("商户 WC 二维码渲染失败：", error);
  }
}

// 把任意文本（如钱包收款地址）渲染成二维码，用于「收款二维码」等展示场景。
function renderTextQr(canvas, text, { maxPx = 360, ecLevel = "L", margin = 2 } = {}) {
  const t = (text || "").trim();
  if (!t) return false;
  try {
    if (typeof qrcode !== "function") throw new Error("二维码库未加载");
    const qr = qrcode(0, ecLevel);
    qr.addData(t);
    qr.make();
    drawQrToCanvas(canvas, qr, { margin, maxPx });
    return true;
  } catch (error) {
    console.warn("二维码渲染失败：", error);
    return false;
  }
}

// 在钱包 App 内补齐最常用的二维码能力：收款页/总览页展示地址二维码，发送页扫码填址。
function setupReceiveAndSendEnhancements() {
  // —— 收款页：地址二维码 ——
  const copyReceiveBtn = $("#copy-receive-address");
  if (copyReceiveBtn && !copyReceiveBtn.dataset.qrEnhanced) {
    copyReceiveBtn.dataset.qrEnhanced = "1";
    const row = copyReceiveBtn.closest(".button-row") || copyReceiveBtn.parentElement;
    const qrBtn = document.createElement("button");
    qrBtn.id = "show-receive-qr";
    qrBtn.type = "button";
    qrBtn.className = "button secondary";
    qrBtn.textContent = "显示收款二维码";
    row.appendChild(qrBtn);
    const box = document.createElement("div");
    box.id = "receive-qr";
    box.className = "qr-box";
    box.hidden = true;
    box.innerHTML = `<div class="qr-frame"><canvas id="receive-qr-canvas"></canvas></div><p class="qr-caption">让对方扫描此二维码获取你的收款地址；地址为公开信息，可安全展示。</p><button id="receive-qr-download" class="button secondary full-button" type="button">下载二维码 PNG</button>`;
    (copyReceiveBtn.closest(".panel") || copyReceiveBtn.parentElement).appendChild(box);
    qrBtn.addEventListener("click", () => {
      const addr = selectedWalletProfile?.address;
      if (!addr) { window.alert("请先解锁钱包以显示收款地址。"); return; }
      const canvas = $("#receive-qr-canvas");
      if (renderTextQr(canvas, addr, { maxPx: 360, ecLevel: "L" })) {
        box.hidden = !box.hidden;
        if (!box.hidden) addActivity("收款二维码已展示", addr.slice(0, 16) + "…");
      } else window.alert("二维码生成失败。");
    });
    $("#receive-qr-download")?.addEventListener("click", () => {
      const canvas = $("#receive-qr-canvas");
      if (!canvas) return;
      const a = document.createElement("a");
      a.href = canvas.toDataURL("image/png");
      a.download = `xhub-receive-${(selectedWalletProfile?.address || "qr").slice(0, 24)}.png`;
      document.body.appendChild(a); a.click(); a.remove();
    });
  }

  // —— 总览页：同样提供地址二维码（主界面快速收款） ——
  const copyOverviewBtn = $("#copy-overview-address");
  if (copyOverviewBtn && !copyOverviewBtn.dataset.qrEnhanced) {
    copyOverviewBtn.dataset.qrEnhanced = "1";
    const row = copyOverviewBtn.closest(".button-row") || copyOverviewBtn.parentElement;
    const qrBtn = document.createElement("button");
    qrBtn.id = "show-overview-qr";
    qrBtn.type = "button";
    qrBtn.className = "button secondary";
    qrBtn.textContent = "显示收款二维码";
    row.appendChild(qrBtn);
    const box = document.createElement("div");
    box.id = "overview-qr";
    box.className = "qr-box";
    box.hidden = true;
    box.innerHTML = `<div class="qr-frame"><canvas id="overview-qr-canvas"></canvas></div><p class="qr-caption">让对方扫描此二维码获取你的收款地址。</p><button id="overview-qr-download" class="button secondary full-button" type="button">下载二维码 PNG</button>`;
    (copyOverviewBtn.closest(".panel") || copyOverviewBtn.parentElement).appendChild(box);
    qrBtn.addEventListener("click", () => {
      const addr = selectedWalletProfile?.address;
      if (!addr) { window.alert("请先解锁钱包以显示收款地址。"); return; }
      const canvas = $("#overview-qr-canvas");
      if (renderTextQr(canvas, addr, { maxPx: 360, ecLevel: "L" })) {
        box.hidden = !box.hidden;
        if (!box.hidden) addActivity("收款二维码已展示", addr.slice(0, 16) + "…");
      } else window.alert("二维码生成失败。");
    });
    $("#overview-qr-download")?.addEventListener("click", () => {
      const canvas = $("#overview-qr-canvas");
      if (!canvas) return;
      const a = document.createElement("a");
      a.href = canvas.toDataURL("image/png");
      a.download = `xhub-receive-${(selectedWalletProfile?.address || "qr").slice(0, 24)}.png`;
      document.body.appendChild(a); a.click(); a.remove();
    });
  }

  // —— 发送页：相机扫码收款地址 ——
  const sendDest = $("#send-destination");
  if (sendDest && !sendDest.dataset.scanEnhanced) {
    sendDest.dataset.scanEnhanced = "1";
    const scanBtn = document.createElement("button");
    scanBtn.id = "send-scan-address";
    scanBtn.type = "button";
    scanBtn.className = "button secondary";
    scanBtn.textContent = "📷 扫描收款地址";
    scanBtn.style.marginTop = "8px";
    scanBtn.style.width = "100%";
    sendDest.insertAdjacentElement("afterend", scanBtn);
    scanBtn.addEventListener("click", () => {
      openQrScanner({
        inputId: "send-destination",
        parse: (raw) => {
          const text = (raw || "").trim();
          const direct = text.match(/xch1[0-9a-z]{20,120}/i);
          if (direct) return direct[0].toLowerCase();
          const link = text.match(/chia:\/\/([0-9a-z]{20,120})/i);
          if (link) return link[1].toLowerCase();
          return text;
        },
        onFilled: (code) => {
          const v = code.trim().toLowerCase();
          if (/^xch1[0-9a-z]{20,120}$/.test(v)) {
            addActivity("已扫描收款地址", v.slice(0, 16) + "…");
            sendDest.focus();
          } else {
            window.alert("扫描到的内容不是有效的 Chia 收款地址（xch1...）。请确认二维码内容，或手动粘贴。");
          }
        },
      });
    });
  }
}

function installWcUi() {
  const fundingView = $("[data-view-panel='funding']");
  if (!fundingView) return;
  const fundingDescription = fundingView.querySelector(".page-description");
  if (fundingDescription) fundingDescription.textContent = "导入 HUB 发来的 WC 请求，由当前地址钱包核对条款并锁定自己的资金。";
  const fundingTitle = fundingView.querySelector("h1");
  if (fundingTitle) fundingTitle.textContent = "当前地址锁定 Funding Coin";
  const prepareButton = $("#prepare-button");
  if (prepareButton) prepareButton.textContent = "重新校验 WC 条款";
  const confirmationCheck = $("#confirm-check")?.closest("label");
  if (confirmationCheck) confirmationCheck.hidden = true;
  if (!$("#funding-final-review")) {
    $("#confirm-button")?.insertAdjacentHTML("beforebegin", '<div id="funding-final-review" class="review-box funding-final-review">粘贴完整 WC 后会自动解析并显示最终 Funding Coin 信息。</div>');
  }
  if ($("#confirm-button")) $("#confirm-button").textContent = "核对以上信息并锁定条款";
  const walletIsolationText = Array.from(document.querySelectorAll(".security-list span"))
    .find((item) => item.textContent.includes("商户身份与用户身份分开显示"));
  if (walletIsolationText) walletIsolationText.textContent = "一次登录只使用当前钱包的一套私钥";
  const merchantView = $("[data-view-panel='merchant']");
  if (merchantView) {
    const description = merchantView.querySelector(".page-description");
    if (description) description.textContent = "使用当前登录钱包生成预扣请求并追踪 HUB 授权状态；本次登录始终只使用这一套私钥。";
    const steps = merchantView.querySelectorAll(".flow-step");
    if (steps[0]) { steps[0].querySelector("strong").textContent = "商户提交预扣需求"; steps[0].querySelector("small").textContent = "将公开身份和金额交给 HUB"; }
    if (steps[1]) { steps[1].querySelector("strong").textContent = "HUB 生成 WC 请求"; steps[1].querySelector("small").textContent = "HUB 返回公开请求码"; }
    if (steps[2]) { steps[2].querySelector("strong").textContent = "当前地址锁定 Funding Coin"; steps[2].querySelector("small").textContent = "当前地址钱包独立核对并签名"; }
    if (steps[3]) { steps[3].querySelector("strong").textContent = "商户接收授权状态"; steps[3].querySelector("small").textContent = "HUB 返回 AUTHORIZED"; }
    if (!merchantView.querySelector("#merchant-wc-panel")) merchantView.insertAdjacentHTML("beforeend", `<div class="two-column merchant-wc-grid"><section class="panel form-panel" id="merchant-wc-panel"><div class="panel-heading"><div><h2>生成预扣 WC</h2><p>填写本次预扣金额；结算身份固定使用当前登录钱包，不会广播主网。</p></div><span class="tag neutral">RESERVATION REQUEST</span></div><label>订单号 / 备注<input id="merchant-wc-note" maxlength="80" placeholder="例如 order-20260821-001"></label><label>预扣金额（mojo）<input id="merchant-wc-amount" inputmode="numeric" value="1"></label><div id="merchant-wc-error" class="message error" role="alert" hidden></div><div class="button-row"><button id="merchant-wc-generate" class="button primary" type="button">生成 WC 链接</button><button id="merchant-wc-copy" class="button secondary" type="button" disabled>复制 WC</button></div><textarea id="merchant-wc-output" class="code-input" rows="5" readonly placeholder="生成后显示 wc1... 请求码"></textarea><div id="merchant-wc-qr" class="qr-box" hidden><div class="qr-frame"><canvas id="merchant-wc-qr-canvas"></canvas></div><p class="qr-caption">请使用「链下预扣 → 导入商户预扣 WC」页面的扫码功能读取此二维码</p><button id="merchant-wc-qr-download" class="button secondary full-button" type="button">下载二维码 PNG</button></div><div id="merchant-wc-summary" class="status-box">结算地址使用当前登录钱包。填写金额后可直接生成。</div></section><section class="panel form-panel" id="merchant-wc-tracking"><div class="panel-heading"><div><h2>预扣追踪</h2><p>按请求 ID 查看 HUB 当前状态，兼容 16/32 字节 Request ID。</p></div><span class="tag neutral">READ ONLY</span></div><label>请求 ID<input id="merchant-wc-request-id" spellcheck="false" placeholder="生成 WC 后自动填入"></label><button id="merchant-wc-refresh" class="button secondary full-button" type="button" disabled>刷新预扣状态</button><div id="merchant-wc-success" class="merchant-wc-success" hidden><strong>预扣成功</strong><span>链下授权已写入 HUB 账本</span><dl id="merchant-wc-success-details"></dl></div><div id="merchant-wc-status" class="status-box">尚未生成请求。</div></section></div>`);
    const trackerView = $(`[data-view-panel='reservation-tracker']`);
    const trackingPanel = merchantView.querySelector("#merchant-wc-tracking");
    if (trackerView && trackingPanel && !trackerView.contains(trackingPanel)) {
      const content = trackerView.querySelector("#reservation-tracker-content");
      if (content && !content.querySelector("#reservation-tracker-list")) content.insertAdjacentHTML("beforeend", '<section class="panel reservation-history-panel"><div class="panel-heading"><div><h2>历史预扣</h2><p>按当前钱包保存公开 Request ID 和状态。</p></div><span class="tag neutral">LOCAL INDEX</span></div><div id="reservation-tracker-list" class="reservation-entry-list"></div></section>');
      content?.appendChild(trackingPanel);
      renderReservationTrackerList();
    }
    const merchantConfig = () => {
      const profile = selectedWalletProfile;
      if (!profile?.puzzle_hash || !profile?.wallet_public_key) return null;
      const identity = String(profile.fingerprint || profile.wallet_public_key.slice(0, 16));
      return {
        id: `M-${identity.slice(0, 16).toUpperCase()}`,
        puzzle: profile.puzzle_hash.toLowerCase(),
        receipt: profile.wallet_public_key.toLowerCase(),
      };
    };
    const merchantWcError = (message) => { const box = $("#merchant-wc-error"); box.textContent = message; box.hidden = false; };
    const merchantWcSetSuccess = (result) => {
      const box = $("#merchant-wc-success");
      const details = $("#merchant-wc-success-details");
      const status = String(result.status || "").toUpperCase();
      const success = ["SIGNED", "DELIVERED", "AUTHORIZED"].includes(status) && result.ledger_written !== false;
      if (!success) { box.hidden = true; return; }
      const request = result.request || {};
      const rows = [
        ["状态", status],
        ["请求 ID", result.request_id || request.request_id || "-"],
        ["Funding Coin", result.funding_coin_id || "-"],
        ["预扣金额", `${request.funding_amount || result.amount || "-"} mojo`],
        ["Reservation nonce", result.reservation_nonce || request.reservation_nonce || "-"],
        ["账本写入", result.ledger_written === true ? "是" : "待确认"],
        ["账本序号", result.state_sequence ?? "-"],
        ["观察高度", result.observed_peak_height ?? "-"],
        ["到账状态", confirmedReservationReceipt(result) ? "已发现到账 Coin" : result.ledger_written === true ? "等待最终结算到账" : "尚未写入"],
        ...(confirmedReservationReceipt(result) ? [["到账 Coin", result.received_coin_id], ["到账确认高度", result.received_height ?? "等待确认"]] : []),
        ["最后更新", new Date().toLocaleString("zh-CN", { hour12: false })],
      ];
      details.innerHTML = rows.map(([label, value]) => `<div><dt>${escapeHtml(label)}</dt><dd>${escapeHtml(value)}</dd></div>`).join("");
      box.hidden = false;
    };
    const refreshMerchantWc = async ({ quiet = false } = {}) => {
      const id = $("#merchant-wc-request-id")?.value.trim();
      if (!id) return;
      try {
        const existingEntry = loadReservationTrackers().find((entry) => entry.request_id === id) || { request_id: id, settlement_mode: "DIRECT_MERCHANT", direct_settlement_confirmed: true };
        const entry = await refreshReservationEntry(existingEntry);
        // 商户对账（C）：幂等键为 (funding_coin_id, request_id)。同一个 request_id 若落在
        // 多个 Funding Coin 上就是多笔独立付款，必须逐笔列出，不能只显示最新一条。
        saveReservationTracker(entry);
        const boundSiblings = loadReservationTrackers().filter((item) => item.request_id === id && reservationEntryCoin(item));
        const multiCoin = boundSiblings.length > 1;
        const siblingLines = multiCoin ? boundSiblings.map((item) => `  · ${shortCoinLabel(item.funding_coin_id)}｜${item.amount || "-"} mojo｜${String(item.status || "-").toUpperCase()}｜账本${item.ledger_written === false ? "未写入" : "已写入"}`).join("\n") : "";
        $("#merchant-wc-status").textContent = `请求状态：${entry.status}\n请求 ID：${entry.request_id}\nFunding Coin：${entry.funding_coin_id || "等待用户签名"}\n预扣金额：${entry.amount || "-"} mojo\nHUB 账本：${entry.ledger_written ? "已写入" : "等待授权"}\n到账状态：${confirmedReservationReceipt(entry) ? "已到账" : entry.ledger_written ? "等待最终结算到账" : "尚未写入"}${multiCoin ? `\n⚠ 该请求码已在 ${boundSiblings.length} 个通道被消费，按 (funding_coin_id, request_id) 应分别入账：\n${siblingLines}` : ""}\n自动刷新：每 5 秒`;
        merchantWcSetSuccess({ ...entry, request: { funding_amount: entry.amount }, request_id: entry.request_id, funding_coin_id: entry.funding_coin_id });
        if (!quiet) addActivity(multiCoin ? "商户预扣：同一请求码跨通道重复消费" : "商户预扣状态已刷新", `${entry.status} · request_id=${entry.request_id}${multiCoin ? ` · ${boundSiblings.length} 个通道` : ""}`);
      } catch (error) {
        $("#merchant-wc-success").hidden = true;
        $("#merchant-wc-status").textContent = `查询失败：${error.message}`;
      }
    };
    const startMerchantWcPolling = () => {
      if (merchantWcPollTimer !== null) clearInterval(merchantWcPollTimer);
      merchantWcPollTimer = setInterval(() => refreshMerchantWc({ quiet: true }), 5000);
      refreshMerchantWc({ quiet: true });
    };
    $("#merchant-wc-generate").addEventListener("click", async () => {
      $("#merchant-wc-error").hidden = true;
      const merchant = merchantConfig();
      const amount = $("#merchant-wc-amount").value.trim();
      if (!merchant || !/^[0-9a-f]{64}$/.test(merchant.puzzle) || !/^[0-9a-f]{96}$/.test(merchant.receipt)) { merchantWcError("当前登录钱包缺少有效 Puzzle Hash 或 BLS 公钥，无法生成预扣请求。"); return; }
      if (!/^\d+$/.test(amount) || BigInt(amount) <= 0n) { merchantWcError("预扣金额必须是大于 0 的整数 mojo。"); return; }
      const button = $("#merchant-wc-generate"); button.disabled = true; button.textContent = "正在生成...";
      try {
        const result = await request("/api/v3.6/hub/wallet-connect/requests", { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify({ request_type: "reservation_request", amount_mojos: amount, termination_blocks: "12488", challenge_blocks: "6000", freeze_blocks: "200", user_note: $("#merchant-wc-note").value.trim(), merchant_id: merchant.id, merchant_puzzle_hash: merchant.puzzle, merchant_receipt_public_key: merchant.receipt }) });
        $("#merchant-wc-output").value = result.wc_request;
        renderMerchantWcQr();
        $("#merchant-wc-request-id").value = result.request_id;
        $("#merchant-wc-copy").disabled = false;
        $("#merchant-wc-refresh").disabled = false;
        $("#merchant-wc-summary").textContent = `请求已生成：${result.request_id}\n金额：${amount} mojo\n状态：${result.status}\n请把完整 wc1... 请求码交给用户钱包。`;
        sessionStorage.setItem("xhub-v36-merchant-last-request", result.request_id);
        saveReservationTracker({ request_id: result.request_id, status: result.status || "WC_CREATED", amount, note: $("#merchant-wc-note").value.trim(), funding_coin_id: null, merchant_puzzle_hash: merchant.puzzle, settlement_mode: "DIRECT_MERCHANT", direct_settlement_confirmed: true, ledger_written: false, created_at: new Date().toISOString() });
        startMerchantWcPolling();
        addActivity("商户预扣 WC 已生成", `${amount} mojo · request_id=${result.request_id}`);
      } catch (error) { merchantWcError(error.message); } finally { button.disabled = false; button.textContent = "生成 WC 链接"; }
    });
    $("#merchant-wc-copy").addEventListener("click", async () => { try { await copyTextToClipboard($("#merchant-wc-output").value); $("#merchant-wc-summary").textContent += "\nWC 已复制。"; } catch (error) { merchantWcError(`复制失败：${error.message}。已保留完整 WC，请手动按 Ctrl+C。`); $("#merchant-wc-output").focus(); $("#merchant-wc-output").select(); } });
    $("#merchant-wc-qr-download")?.addEventListener("click", () => {
      const canvas = $("#merchant-wc-qr-canvas");
      if (!canvas) return;
      const url = canvas.toDataURL("image/png");
      const a = document.createElement("a");
      a.href = url;
      a.download = `xhub-wc-${(($("#merchant-wc-request-id")?.value) || "qr").replace(/[^0-9a-zA-Z_-]/g, "").slice(0, 24)}.png`;
      document.body.appendChild(a);
      a.click();
      a.remove();
      $("#merchant-wc-summary").textContent += "\n二维码已导出 PNG。";
    });
    $("#merchant-wc-refresh").addEventListener("click", () => refreshMerchantWc());
    const lastMerchantRequest = sessionStorage.getItem("xhub-v36-merchant-last-request");
    if (lastMerchantRequest) { $("#merchant-wc-request-id").value = lastMerchantRequest; $("#merchant-wc-refresh").disabled = false; startMerchantWcPolling(); }
  }
  const reservationView = $(`[data-view-panel='reservation']`);
  if (reservationView && !reservationView.querySelector("#reservation-wc-import")) reservationView.querySelector(".page-heading")?.insertAdjacentHTML("afterend", `<section class="panel wc-import-panel" id="reservation-wc-import"><div class="panel-heading"><div><h2>导入商户预扣 WC</h2><p>用户导入商户请求后选择自己的 Funding Coin，本地签名并提交 HUB。</p></div><span class="tag neutral">USER APPROVAL</span></div><label>预扣 WC 请求码<textarea id="reservation-wc-code" rows="4" spellcheck="false" placeholder="粘贴以 wc1 开头的 reservation_request"></textarea></label><label>使用 Funding Coin<select id="reservation-wc-coin"><option value="">请先导入请求</option></select></label><div id="reservation-wc-error" class="message error" role="alert" hidden></div><div class="button-row"><button id="reservation-wc-scan" class="button secondary" type="button">📷 扫描二维码</button><button id="reservation-wc-import-button" class="button secondary" type="button">解析预扣请求</button><button id="reservation-wc-authorize" class="button primary" type="button" disabled>本地签名并提交 HUB</button></div><div id="reservation-wc-preview" class="status-box">等待导入商户 WC。</div></section>`);
  $("#reservation-wc-import-button")?.addEventListener("click", async (event) => {
    const box = $("#reservation-wc-error"); box.hidden = true;
    const button = event.currentTarget;
    button.disabled = true;
    button.textContent = "正在核验 Coin...";
    try {
      const value = decodeWcRequest($("#reservation-wc-code").value);
      if (value.request_type !== "reservation_request" || !value.merchant_puzzle_hash || !value.merchant_receipt_public_key || !value.reservation_nonce) throw new Error("这不是完整的预扣请求。");
      await refreshFundingTracker({ quiet: true });
      // 一次性消费守卫（B2）：本机若已用该 request_id 消费过某通道，就只允许在该通道重试。
      const consumedCoins = reservationRequestConsumedCoins(value.request_id);
      const readyTrackers = loadFundingTrackers().filter(fundingReadyForReservation);
      const trackers = consumedCoins.length ? readyTrackers.filter((item) => consumedCoins.includes(normalizeCoinId(item.funding_coin_id))) : readyTrackers;
      if (consumedCoins.length && !trackers.length) throw new Error(`该预扣请求码已在本机 Funding Coin ${consumedCoins.map(shortCoinLabel).join(" / ")} 上消费过，不能用于其他通道；请切换到该通道重试。`);
      $("#reservation-wc-coin").innerHTML = trackers.length ? trackers.map((item) => `<option value="${escapeHtml(item.funding_coin_id)}">${escapeHtml(item.funding_coin_id.slice(0, 16))}… · ${Number(item.amount_mojo).toLocaleString("en-US")} mojo</option>`).join("") : '<option value="">没有已完成链上确认并登记 HUB 的 Funding Coin</option>';
      const guardNote = consumedCoins.length ? `\n⚠ 该请求码本机已用于 Funding Coin ${consumedCoins.map(shortCoinLabel).join(" / ")}：只能在该通道重试，不能改投其他通道。` : "";
      $("#reservation-wc-preview").textContent = `商户：${value.merchant_id || "-"}\n订单：${value.user_note || "-"}\n金额：${value.funding_amount} mojo\n请求 ID：${value.request_id}\n请选择 Funding Coin 后签名。${guardNote}`;
      $("#reservation-wc-authorize").disabled = !trackers.length;
      $("#reservation-wc-authorize").dataset.request = JSON.stringify(value);
    } catch (error) { box.textContent = error.message; box.hidden = false; }
    finally { button.disabled = false; button.textContent = "解析预扣请求"; }
  });
  $("#reservation-wc-authorize")?.addEventListener("click", async (event) => {
    const box = $("#reservation-wc-error"); box.hidden = true;
    const button = event.currentTarget;
    button.disabled = true;
    button.textContent = "正在核验链上状态...";
    try {
      const value = JSON.parse(button.dataset.request || "{}");
      const selectedCoin = normalizeCoinId($("#reservation-wc-coin").value);
      // 一次性消费守卫（B2）：所选通道与已消费通道不一致时直接拒绝，不进入本地签名。
      const consumedCoins = reservationRequestConsumedCoins(value.request_id);
      if (consumedCoins.length && !consumedCoins.includes(selectedCoin)) throw new Error(`该预扣请求码已在本机 Funding Coin ${consumedCoins.map(shortCoinLabel).join(" / ")} 上消费过，不能用于 ${shortCoinLabel(selectedCoin)}；本次未签名、未提交。`);
      const tracked = loadFundingTrackers().find((item) => item.funding_coin_id === $("#reservation-wc-coin").value);
      if (!tracked?.confirmed_draft?.preview?.channel_terms_canonical_hex) throw new Error("所选 Funding Coin 缺少已确认条款。");
      const ready = await ensureFundingReadyForReservation(tracked);
      button.textContent = "正在本地签名并提交...";
      const signed = await request("/api/v3.6/wallet-profiles/reservation/authorize", { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify({ role: selectedWalletProfile.role, request_id: value.request_id, funding_coin_id: ready.funding_coin_id, channel_terms_canonical_hex: ready.confirmed_draft.preview.channel_terms_canonical_hex, merchant_puzzle_hash: value.merchant_puzzle_hash, merchant_receipt_public_key: value.merchant_receipt_public_key, amount: value.funding_amount, reservation_nonce: value.reservation_nonce }) });
      const result = await request("/api/v3.6/hub/reservations", { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify(hubReservationPayload(signed)) });
      $("#reservation-wc-preview").textContent = `预扣已提交 HUB\n状态：${result.status}\nFunding Coin：${ready.funding_coin_id}\n金额：${value.funding_amount} mojo\n广播：false`;
      if (result.ledger_written === true) {
        recordFundingReservation({ fundingCoinId: ready.funding_coin_id, requestId: value.request_id, reservationNonce: value.reservation_nonce, amount: value.funding_amount, merchantPuzzleHash: value.merchant_puzzle_hash, result });
        saveReservationTracker({ request_id: value.request_id, status: String(result.status || "SIGNED").toUpperCase(), amount: String(value.funding_amount), note: value.user_note || "", funding_coin_id: ready.funding_coin_id, merchant_puzzle_hash: value.merchant_puzzle_hash, reservation_nonce: value.reservation_nonce, settlement_mode: "DIRECT_MERCHANT", direct_settlement_confirmed: true, ledger_written: true, state_sequence: result.state_sequence, authorized_at: new Date().toISOString() });
      }
      addActivity("商户预扣已签名并提交", `${value.funding_amount} mojo · ${result.status} · Funding Coin=${ready.funding_coin_id} · Request ID=${value.request_id}`, ready.funding_coin_id);
      refreshFundingTracker({ quiet: true });
    } catch (error) { box.textContent = `签名或提交失败：${error.message}`; box.hidden = false; }
    finally { button.disabled = false; button.textContent = "本地签名并提交 HUB"; }
  });
  const fundingLayout = fundingView.querySelector(".two-column");
  if (fundingLayout) fundingLayout.insertAdjacentHTML("beforebegin", `<section class="panel wc-import-panel" id="wc-import-panel"><div class="panel-heading"><div><h2>导入 HUB WC 请求</h2><p>WC 请求码由 HUB 生成，只包含用户锁币参数，不包含私钥，也不是最终链上地址。</p></div><span class="tag neutral">HUB REQUEST</span></div><label>WC 请求码<textarea id="wc-request-input" rows="3" spellcheck="false" placeholder="粘贴以 wc1 开头的 HUB 请求码"></textarea></label><div id="wc-request-error" class="message error" role="alert" hidden></div><div class="button-row"><button id="wc-request-scan" class="button secondary" type="button">📷 扫描二维码</button><button id="import-wc-request" class="button secondary" type="button">导入并解析请求</button><button id="clear-wc-request" class="button secondary" type="button">清除</button></div><div id="wc-request-preview" class="status-box">等待导入 HUB 请求。</div></section>`);
  // The HUB owns the amount and timing policy. Keep the protocol fields in the
  // form for the signing API, but make them read-only and hide the manual inputs.
  for (const name of ["acceptance_blocks", "freeze_blocks", "challenge_blocks", "funding_amount"]) {
    const element = form.elements[name];
    if (!element) continue;
    element.readOnly = true;
    element.closest("label")?.setAttribute("hidden", "");
  }
  let wcParseTimer = null;
  let wcParseSequence = 0;
  const parseFundingWcRequest = async ({ quiet = false } = {}) => {
    const sequence = ++wcParseSequence;
    clearError("#wc-request-error");
    try {
      if (!selectedWalletProfile) throw new Error("请先登录一个地址钱包，再导入 HUB WC 请求。");
      const requestValue = decodeWcRequest($("#wc-request-input").value);
      if (requestValue.request_type !== "funding_request") throw new Error("这不是 Funding Coin 锁币请求。请粘贴 HUB 生成的 funding_request WC。");
      // 一次性消费（B2）：同一个锁币请求码不应在本机生成第二个 Funding Coin（= 第二个通道）。
      const reusedFunding = requestValue.request_id
        ? loadFundingTrackers().find((item) => item.hub_request_id === requestValue.request_id)
        : null;
      if (reusedFunding) throw new Error(`该 WC 请求码本机已用于 Funding Coin ${shortCoinLabel(reusedFunding.funding_coin_id)}；一个锁币请求码只能对应一个通道，请勿重复导入。`);
      const termination = BigInt(requestValue.termination_blocks);
      const freeze = BigInt(requestValue.freeze_blocks);
      if (termination <= freeze) throw new Error("WC 请求的终止高度必须大于冻结缓冲。");
      beginNewFundingDraft();
      form.elements.acceptance_blocks.value = (termination - freeze).toString();
      form.elements.freeze_blocks.value = requestValue.freeze_blocks;
      form.elements.challenge_blocks.value = requestValue.challenge_blocks;
      form.elements.funding_amount.value = requestValue.funding_amount;
      if (selectedWalletProfile?.wallet_public_key) form.elements.user_public_key.value = selectedWalletProfile.wallet_public_key;
      if (selectedWalletProfile?.puzzle_hash) form.elements.user_remainder_puzzle_hash.value = selectedWalletProfile.puzzle_hash;
      importedWcRequest = requestValue;
      updateTiming();
      $("#wc-request-preview").textContent = `WC 已自动解析，正在生成 Funding Coin 预览…\n请求 ID：${requestValue.request_id || "-"}\n锁定金额：${requestValue.funding_amount} mojo\n截止高度：${requestValue.termination_blocks} blocks\n接受窗口：${termination - freeze} blocks\n冻结缓冲：${requestValue.freeze_blocks} blocks\n挑战期：${requestValue.challenge_blocks} blocks\n当前账户：${selectedWalletProfile?.name || "未选择"}\nBLS 公钥：${selectedWalletProfile?.wallet_public_key || "-"}\n找零 Puzzle Hash：${selectedWalletProfile?.puzzle_hash || "-"}`;
      $("#prepare-button").disabled = true;
      const draft = await request("/api/v3.6/funding-drafts", { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify({ protocol_version: "0x0360", ...values() }) });
      if (sequence !== wcParseSequence) return;
      renderDraft(draft);
      $("#wc-request-preview").textContent = `WC 已自动解析并校验\n请求 ID：${requestValue.request_id || "-"}\n账户：${selectedWalletProfile?.name || "未选择"} · ${selectedWalletProfile?.fingerprint || "-"}\n锁定金额：${requestValue.funding_amount} mojo\nFunding 地址：${draft.preview.funding_address}\nFunding Puzzle Hash：${draft.preview.funding_puzzle_hash}\nChannel Terms Hash：${draft.preview.channel_terms_hash}\n接受 / 冻结 / 挑战：${termination - freeze} / ${requestValue.freeze_blocks} / ${requestValue.challenge_blocks} blocks\n\n请核对右侧完整信息，只需点击一次“核对以上信息并锁定条款”。此步骤不会构造或广播交易。`;
      $("#funding-final-review").textContent = `最终锁定核对\n账户：${selectedWalletProfile?.name || "未选择"} · ${selectedWalletProfile?.fingerprint || "-"}\n金额：${requestValue.funding_amount} mojo\nFunding Coin 地址：${draft.preview.funding_address}\nPuzzle Hash：${draft.preview.funding_puzzle_hash}\nTerms Hash：${draft.preview.channel_terms_hash}\n\n点击下方按钮只锁定条款，不广播主网。`;
      if (!quiet) addActivity("Funding WC 已自动解析", `${requestValue.funding_amount} mojo · terms=${draft.preview.channel_terms_hash.slice(0, 12)}…`);
    } catch (error) {
      if (sequence !== wcParseSequence) return;
      activeDraft = null;
      $("#confirm-button").disabled = true;
      $("#funding-final-review").textContent = "WC 尚未通过校验，不能锁定条款。";
      if (!quiet || $("#wc-request-input").value.trim().startsWith("wc1")) showError("#wc-request-error", error.message);
    } finally {
      if (sequence === wcParseSequence && profile) $("#prepare-button").disabled = false;
    }
  };
  $("#import-wc-request").addEventListener("click", () => parseFundingWcRequest());
  $("#wc-request-input").addEventListener("input", () => {
    if (wcParseTimer !== null) clearTimeout(wcParseTimer);
    const raw = $("#wc-request-input").value.trim();
    if (!raw) return;
    wcParseTimer = setTimeout(() => parseFundingWcRequest({ quiet: true }), 250);
  });
  $("#clear-wc-request").addEventListener("click", () => {
    wcParseSequence += 1;
    beginNewFundingDraft();
    importedWcRequest = null;
    activeDraft = null;
    $("#wc-request-input").value = "";
    $("#wc-request-preview").textContent = "等待导入 HUB 请求。";
    $("#funding-final-review").textContent = "粘贴完整 WC 后会自动解析并显示最终 Funding Coin 信息。";
    resetPreview();
    clearError("#wc-request-error");
  });

  // 二维码扫描按钮：扫描成功后自动触发对应面板的解析逻辑
  const wireScan = (buttonId, inputId, onFilled) => {
    const button = document.getElementById(buttonId);
    if (!button || button.dataset.wired) return;
    button.dataset.wired = "1";
    button.addEventListener("click", () => openQrScanner({ inputId, onFilled }));
  };
  wireScan("wc-request-scan", "wc-request-input", () => {
    const el = document.getElementById("wc-request-input");
    if (el) el.dispatchEvent(new Event("input"));
  });
  wireScan("reservation-wc-scan", "reservation-wc-code", () => {
    document.getElementById("reservation-wc-import-button")?.click();
  });
}

function addActivity(title, detail, fundingCoinId = null, diagnosticFingerprint = null) {
  activities.unshift({
    title,
    detail,
    time: new Date().toLocaleString("zh-CN", { hour12: false }),
    ...(fundingCoinId ? { funding_coin_id: fundingCoinId } : {}),
    ...(diagnosticFingerprint ? { diagnostic_fingerprint: diagnosticFingerprint } : {}),
  });
  if (activities.length > ACTIVITY_LOG_LIMIT) activities.length = ACTIVITY_LOG_LIMIT;
  localStorage.setItem(ACTIVITY_LOG_KEY, JSON.stringify(activities));
  scheduleActivityLogPersistence();
  renderActivities();
  renderAuditLog();
  renderFundingCoinLifecycle(loadFundingTracker());
}

function scheduleActivityLogPersistence() {
  if (activityPersistTimer !== null) clearTimeout(activityPersistTimer);
  activityPersistTimer = setTimeout(() => {
    activityPersistTimer = null;
    fetch("/api/v3.6/wallet-profiles/diagnostics", { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify({ entries: activities.slice(0, ACTIVITY_LOG_LIMIT) }) }).catch(() => {});
  }, 250);
}

async function hydrateActivityLog() {
  try {
    const response = await fetch("/api/v3.6/wallet-profiles/diagnostics");
    if (!response.ok) return;
    const body = await response.json();
    const remoteEntries = Array.isArray(body.entries) ? body.entries : [];
    const merged = [...activities];
    const seen = new Set(merged.map((entry) => `${entry.time}|${entry.title}|${entry.detail}`));
    for (const entry of remoteEntries) {
      const key = `${entry.time}|${entry.title}|${entry.detail}`;
      if (!seen.has(key)) { seen.add(key); merged.push(entry); }
    }
    merged.sort((left, right) => String(right.time).localeCompare(String(left.time)));
    activities.splice(0, activities.length, ...merged.slice(0, ACTIVITY_LOG_LIMIT));
    localStorage.setItem(ACTIVITY_LOG_KEY, JSON.stringify(activities));
    renderActivities();
    renderAuditLog();
    scheduleActivityLogPersistence();
  } catch { /* diagnostics remain available in this WebView session */ }
}

function renderActivities() {
  const target = $("#activity-list");
  if (!activities.length) {
    target.innerHTML = '<div class="empty-state">暂无页面活动。先在 Funding Coin 页面校验条款。</div>';
    return;
  }
  target.innerHTML = activities.slice(0, 6).map((item) => `<div class="activity-item"><span class="activity-dot"></span><div><strong>${escapeHtml(item.title)}</strong><small>${escapeHtml(item.detail)}</small></div><time>${escapeHtml(item.time)}</time></div>`).join("");
}

function renderAuditLog() {
  $("#audit-log").textContent = activities.length
    ? activities.map((item) => `[${item.time}] ${item.title} - ${item.detail}`).join("\n")
    : "暂无操作";
}

function activityBelongsToFundingCoin(item, tracked) {
  if (!tracked?.funding_coin_id) return false;
  const ids = [
    tracked.funding_coin_id,
    tracked.initial_closing_coin_id,
    tracked.challenge_coin_id,
    tracked.subsequent_closing_coin_id,
    tracked.settlement_coin_id,
    tracked.settlement_output_coin_id,
  ].filter(Boolean).map((value) => String(value).toLowerCase());
  if (item.funding_coin_id && ids.includes(String(item.funding_coin_id).toLowerCase())) return true;
  const text = `${item.title || ""} ${item.detail || ""}`.toLowerCase();
  return ids.some((id) => text.includes(id));
}

function fundingActivityDisplayKey(item) {
  if (item.diagnostic_fingerprint) return item.diagnostic_fingerprint;
  const detail = String(item.detail || "");
  if (item.title === "Funding Coin 链上状态" || item.title === "Funding Coin 链上状态已变化") {
    return `funding:${detail.match(/status=([^ ·]+)/)?.[1] || "-"}:${detail.match(/confirmed=([^ ·]+)/)?.[1] || "-"}:${detail.match(/spent=([^ ·]+)/)?.[1] || "-"}`;
  }
  if (item.title === "结算挑战期已结束") {
    return `challenge:ended:${detail.match(/deadline=([^ ·]+)/)?.[1] || "-"}`;
  }
  if (item.title === "结算挑战期等待" || item.title === "结算挑战期开始等待") {
    return `challenge:waiting:${detail.match(/deadline=([^ ·]+)/)?.[1] || "-"}`;
  }
  if (item.title === "HUB Recovery Package 可用") {
    return `recovery:${detail.match(/hash=([^ ·]+)/)?.[1] || "-"}`;
  }
  return `${item.time}|${item.title}|${item.detail}`;
}

function renderFundingCoinLifecycle(tracked) {
  const summary = $("#tracker-lifecycle-summary");
  const log = $("#tracker-coin-log");
  const count = $("#tracker-log-count");
  if (!summary || !log || !count) return;
  if (!tracked) {
    summary.innerHTML = "";
    log.innerHTML = '<div class="empty-state">选择一个 Funding Coin 后显示日志。</div>';
    count.textContent = "0 条日志";
    return;
  }
  const status = tracked.last_status || {};
  const laterCoins = [
    ["Initial Closing Coin", tracked.initial_closing_coin_id],
    ["Challenge Coin", tracked.challenge_coin_id || tracked.subsequent_closing_coin_id],
    ["Settlement Output Coin", tracked.settlement_coin_id || tracked.settlement_output_coin_id],
  ].filter(([, value]) => value);
  const rows = [
    ["Funding Coin", tracked.funding_coin_id],
    ["金额", `${Number(tracked.amount_mojo || 0).toLocaleString("en-US")} mojo`],
    ["已预扣金额", `${trackedReservedMojo(tracked).toLocaleString("en-US")} mojo · ${Number(tracked.hub_tracking?.reservation_count || (tracked.last_reservation ? 1 : 0))} 笔`],
    ...(tracked.last_reservation ? [["最近预扣", `${tracked.last_reservation.amount || "-"} mojo · ${tracked.last_reservation.status || "-"} · Request ${String(tracked.last_reservation.request_id || "").slice(0, 16)}…`]] : []),
    ["Funding SpendBundle", tracked.spend_bundle_id || "尚未保存"],
    ["广播 / 链上状态", `${tracked.broadcast_status || "-"} / ${status.status || "-"}`],
    ["确认 / 花费高度", `${status.confirmed_height ?? "-"} / ${status.spent_height ?? "-"}`],
    ["HUB 登记", tracked.auto_registered_at || (tracked.hub_tracking || tracked.recovery_package_available ? "已登记" : "等待登记")],
    ["Recovery Package Hash", tracked.recovery_package_content_hash || "尚未取得"],
    ["结算阶段", tracked.settlement_stage || "尚未开始"],
    ["Initial Closing Coin", tracked.initial_closing_coin_id || "尚未生成"],
    ["Initial 确认 / 花费高度", `${tracked.initial_birth_height ?? "-"} / ${tracked.initial_coin_spent_height ?? "-"}`],
    ["挑战结束高度", tracked.challenge_deadline_height ?? "尚未进入挑战期"],
    ["开始结算 SpendBundle", tracked.settlement_start_spend_bundle_id || "尚未生成"],
    ["最终结算 SpendBundle", tracked.settlement_spend_bundle_id || "尚未生成"],
    ["最终结算高度", tracked.settled_height ?? "尚未确认"],
    ...laterCoins.slice(1).map(([label, value]) => [label, value]),
  ];
  summary.innerHTML = rows.map(([label, value]) => `<div><dt>${escapeHtml(label)}</dt><dd>${escapeHtml(value)}</dd></div>`).join("");
  const seenActivityKeys = new Set();
  const coinActivities = activities.filter((item) => {
    if (!activityBelongsToFundingCoin(item, tracked)) return false;
    const key = fundingActivityDisplayKey(item);
    if (seenActivityKeys.has(key)) return false;
    seenActivityKeys.add(key);
    return true;
  });
  count.textContent = `${coinActivities.length} 条日志`;
  log.innerHTML = coinActivities.length
    ? coinActivities.map((item) => `<div class="activity-item"><span class="activity-dot"></span><div><strong>${escapeHtml(item.title)}</strong><small>${escapeHtml(item.detail)}</small></div><time>${escapeHtml(item.time)}</time></div>`).join("")
    : '<div class="empty-state">这个 Funding Coin 暂无绑定日志；刷新状态后会开始记录。</div>';
}

function recordHistory(row) {
  historyRows.unshift({ ...row, time: new Date().toLocaleString("zh-CN", { hour12: false }) });
  renderHistory();
}

function buildAccountHistory() {
  const events = new Map();
  const atHeight = (height) => {
    const key = Number(height || 0);
    if (!events.has(key)) events.set(key, { height: key, delta: 0n, coinIds: new Set(), timestamp: 0 });
    return events.get(key);
  };
  for (const coin of chainHistoryRows) {
    const created = atHeight(coin.confirmed_height);
    created.delta += BigInt(coin.amount || "0");
    created.coinIds.add(coin.id);
    created.timestamp = Math.max(created.timestamp, Number(coin.timestamp || 0));
    if (coin.spent_height) {
      const spent = atHeight(coin.spent_height);
      spent.delta -= BigInt(coin.amount || "0");
      spent.coinIds.add(coin.id);
    }
  }
  const ordered = [...events.values()].sort((a, b) => a.height - b.height);
  let balance = 0n;
  for (const event of ordered) {
    balance += event.delta;
    event.balance = balance;
    event.coinIds = [...event.coinIds];
  }
  return ordered.reverse();
}

function renderHistory() {
  const body = $("#history-body");
  const head = $("#history-head");
  const note = $("#history-note");
  if (!body || !head || !note) return;
  if (historyMode === "account") {
    const rows = buildAccountHistory();
    head.innerHTML = "<tr><th>时间 / 区块</th><th>方向</th><th>净变化</th><th>余额</th><th>涉及 Coin</th><th>状态</th></tr>";
    note.textContent = "账户交易按同一区块内本地址 Coin 的创建与花费聚合，找零会自动抵消，只显示净余额变化。";
    if (!rows.length) {
      body.innerHTML = '<tr><td colspan="6"><div class="empty-state">暂无账户交易。等待桌面核心同步当前地址。</div></td></tr>';
      $("#history-sync-state").textContent = "等待桌面核心同步";
      return;
    }
    $("#history-sync-state").textContent = `${rows.length} 笔账户净变化`;
    body.innerHTML = rows.map((row) => {
      const incoming = row.delta > 0n;
      const outgoing = row.delta < 0n;
      const sign = incoming ? "+" : "";
      const direction = incoming ? "收入" : outgoing ? "支出" : "内部整理";
      const time = row.timestamp ? formatSyncTime(row.timestamp) : `区块 ${row.height.toLocaleString("en-US")}`;
      const ids = row.coinIds.map((id) => id.slice(0, 10)).join(", ");
      return `<tr><td><strong>${escapeHtml(time)}</strong><span class="history-sub">高度 ${escapeHtml(row.height)}</span></td><td><span class="account-direction ${incoming ? "income" : outgoing ? "expense" : "neutral"}">${direction}</span></td><td><strong class="account-amount ${incoming ? "income" : outgoing ? "expense" : ""}">${sign}${escapeHtml(row.delta)} mojo</strong></td><td>${escapeHtml(row.balance)} mojo</td><td><span class="history-id">${escapeHtml(ids || "-")}</span><span class="history-sub">${row.coinIds.length} 个 Coin</span></td><td><span class="history-status">已确认</span></td></tr>`;
    }).join("");
    return;
  }
  const onchainRows = [...chainHistoryRows];
  const knownCoinIds = new Set(onchainRows.map((row) => row.id));
  for (const tracked of loadFundingTrackers()) {
    if (!tracked.funding_coin_id) continue;
    if (!knownCoinIds.has(tracked.funding_coin_id)) {
      const status = tracked.last_status?.status || (tracked.broadcast_status === "SUCCESS" ? "PENDING" : tracked.broadcast_status || "PENDING");
      const created = tracked.created_at ? new Date(tracked.created_at) : null;
      onchainRows.push({
        kind: "onchain",
        subtype: "Funding Coin",
        status,
        amount: String(tracked.amount_mojo ?? "-"),
        id: tracked.funding_coin_id,
        time: created && !Number.isNaN(created.getTime()) ? created.toLocaleString("zh-CN", { hour12: false }) : "-",
        confirmed_height: tracked.last_status?.confirmed_height ?? null,
        spent_height: tracked.last_status?.spent_height ?? null,
        effect: status === "SPENT" ? "已转入 Initial Closing，尚未返还" : "Funding 通道资金",
      });
      knownCoinIds.add(tracked.funding_coin_id);
    }
    if (tracked.initial_closing_coin_id && !knownCoinIds.has(tracked.initial_closing_coin_id)) {
      const initialStatus = tracked.initial_coin_spent_height || tracked.settlement_stage === "SETTLED" ? "SPENT" : "UNSPENT";
      onchainRows.push({
        kind: "onchain",
        subtype: "Initial Closing",
        status: initialStatus,
        amount: String(tracked.amount_mojo ?? "-"),
        id: tracked.initial_closing_coin_id,
        time: "-",
        confirmed_height: tracked.initial_birth_height ?? null,
        spent_height: tracked.initial_coin_spent_height ?? null,
        effect: initialStatus === "SPENT" ? "最终结算已执行" : "等待用户执行最终结算",
      });
      knownCoinIds.add(tracked.initial_closing_coin_id);
    }
  }
  const rows = onchainRows;
  head.innerHTML = "<tr><th>Coin 类型</th><th>状态</th><th>金额</th><th>Coin ID</th><th>创建信息</th><th>花费信息</th></tr>";
  note.textContent = "Coin 明细逐个显示本地址 Coin、Funding Coin 与 Initial Closing Coin；SPENT 表示该 Coin 已被花费，不等同于资金丢失。";
  if (!rows.length) {
    body.innerHTML = '<tr><td colspan="6"><div class="empty-state">暂无 Coin 明细。链上历史需要本地桌面核心同步。</div></td></tr>';
    $("#history-sync-state").textContent = "等待桌面核心同步";
    return;
  }
  $("#history-sync-state").textContent = `${rows.length} 个 Coin`;
  body.innerHTML = rows.map((row) => `<tr><td><span class="history-type">${escapeHtml(row.subtype || "钱包地址 Coin")}</span><span class="history-sub">Chia Mainnet</span></td><td><span class="history-status ${["PENDING", "SUBMITTING", "UNKNOWN"].includes(row.status) ? "pending" : ""}">${escapeHtml(row.status)}</span></td><td>${escapeHtml(row.amount || "-")} mojo</td><td><span class="history-id">${escapeHtml(row.id || "-")}</span></td><td>${escapeHtml(row.time || "-")}<span class="history-sub">高度 ${escapeHtml(row.confirmed_height ?? "-")}</span></td><td>${row.spent_height ? `高度 ${escapeHtml(row.spent_height)}` : escapeHtml(row.effect || "未花费")}</td></tr>`).join("");
}

function formatSyncTime(unixSeconds) {
  if (!Number.isFinite(Number(unixSeconds)) || Number(unixSeconds) <= 0) return "-";
  return new Date(Number(unixSeconds) * 1000).toLocaleString("zh-CN", { hour12: false });
}

function loadRemovedFundingTrackerIds() {
  try {
    const parsed = JSON.parse(localStorage.getItem(walletScopedKey(FUNDING_TRACKER_REMOVED_KEY)) || "[]");
    return new Set(Array.isArray(parsed) ? parsed : []);
  } catch { return new Set(); }
}

function saveRemovedFundingTrackerIds(ids) {
  localStorage.setItem(walletScopedKey(FUNDING_TRACKER_REMOVED_KEY), JSON.stringify([...ids]));
}

function restoreFundingTrackerId(fundingCoinId) {
  const removed = loadRemovedFundingTrackerIds();
  if (!removed.delete(fundingCoinId)) return;
  saveRemovedFundingTrackerIds(removed);
}

function loadFundingTrackers() {
  try {
    const scopedKey = walletScopedKey(FUNDING_TRACKER_KEY);
    const raw = localStorage.getItem(scopedKey) || localStorage.getItem(FUNDING_TRACKER_KEY) || "[]";
    const parsed = JSON.parse(raw);
    const trackers = Array.isArray(parsed) ? parsed : parsed && parsed.funding_coin_id ? [parsed] : [];
    const removed = loadRemovedFundingTrackerIds();
    return trackers.filter((item) => trackerBelongsToCurrentWallet(item) && !removed.has(item.funding_coin_id));
  } catch { return []; }
}

function loadFundingTracker() {
  const trackers = loadFundingTrackers();
  const active = localStorage.getItem(walletScopedKey(FUNDING_TRACKER_ACTIVE_KEY));
  return trackers.find((item) => item.funding_coin_id === active) || trackers[0] || null;
}

function saveFundingTracker(value) {
  value.tracking_wallet_puzzle_hash ||= selectedWalletProfile?.puzzle_hash || null;
  const existing = loadFundingTrackers().find((item) => item.funding_coin_id === value.funding_coin_id);
  const nextValue = existing ? mergeFundingTrackerRecord(existing, value) : value;
  const trackers = loadFundingTrackers().filter((item) => item.funding_coin_id !== value.funding_coin_id);
  trackers.push(nextValue);
  localStorage.setItem(walletScopedKey(FUNDING_TRACKER_KEY), JSON.stringify(trackers));
  localStorage.setItem(walletScopedKey(FUNDING_TRACKER_ACTIVE_KEY), value.funding_coin_id);
  persistFundingTrackers(trackers);
  renderFundingTrackerTabs();
}

function removeFundingTracker(fundingCoinId) {
  const removed = loadRemovedFundingTrackerIds();
  removed.add(fundingCoinId);
  saveRemovedFundingTrackerIds(removed);
  const trackers = loadFundingTrackers().filter((item) => item.funding_coin_id !== fundingCoinId);
  localStorage.setItem(walletScopedKey(FUNDING_TRACKER_KEY), JSON.stringify(trackers));
  if (localStorage.getItem(walletScopedKey(FUNDING_TRACKER_ACTIVE_KEY)) === fundingCoinId) {
    if (trackers.length) localStorage.setItem(walletScopedKey(FUNDING_TRACKER_ACTIVE_KEY), trackers[0].funding_coin_id);
    else localStorage.removeItem(walletScopedKey(FUNDING_TRACKER_ACTIVE_KEY));
  }
  persistFundingTrackers(trackers);
  renderFundingTrackerTabs();
  renderActiveFundingTracker();
  addActivity("已取消 Funding Coin 追踪", `${fundingCoinId.slice(0, 12)}… · 仅删除本地记录`);
}

async function persistFundingTrackers(trackers) {
  const removed = loadRemovedFundingTrackerIds();
  const liveTrackers = trackers.filter((item) => !removed.has(item.funding_coin_id));
  try {
    await request("/api/v3.6/wallet-profiles/funding/trackers", { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify({ trackers: liveTrackers, removed_ids: [...removed] }) });
  } catch (error) {
    // Local persistence remains authoritative when an older Gateway rejects
    // an extended tracker field; retry with the canonical minimal envelope.
    if (error.message.includes("funding/trackers")) {
      try {
        await request("/api/v3.6/wallet-profiles/funding/trackers", { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify({ trackers: liveTrackers.map((item) => ({ funding_coin_id: item.funding_coin_id, amount_mojo: item.amount_mojo, settlement_stage: item.settlement_stage, current_closing_coin_id: item.current_closing_coin_id, current_closing_sequence: item.current_closing_sequence, current_closing_confirmed_height: item.current_closing_confirmed_height, current_closing_deadline_height: item.current_closing_deadline_height, challenge_deadline_height: item.challenge_deadline_height, recovery_package_canonical_hex: item.recovery_package_canonical_hex })), removed_ids: [...removed] }) });
        return;
      } catch (_) { /* report the original failure below */ }
    }
    addActivity("追踪记录持久化失败", error.message);
  }
}

function hasConfirmedSettlement(tracked) {
  return Number(tracked?.settled_height || 0) > 0 || Number(tracked?.initial_coin_spent_height || 0) > 0;
}

function normalizeLegacySettlementStage(tracked) {
  if (tracked?.settlement_stage === "SETTLED"
      && !hasConfirmedSettlement(tracked)
      && tracked.initial_closing_coin_id
      && tracked.settlement_spend_bundle_id) {
    tracked.settlement_stage = "FINAL_SUBMITTED";
    return true;
  }
  return false;
}

function settlementStageRank(tracked) {
  const stage = String(tracked?.settlement_stage || "NONE");
  if (["START_PENDING", "START_SUBMITTED", "START_DROPPED"].includes(stage)) return 1;
  if (["INITIAL_CLOSING", "SUBSEQUENT_CLOSING"].includes(stage)) return 2;
  if (["FINALIZE", "FINAL_PENDING", "FINAL_SUBMITTED", "FINAL_DROPPED"].includes(stage)) return 3;
  if (stage === "SETTLED") return hasConfirmedSettlement(tracked) ? 4 : 2;
  return 0;
}

function mergeFundingTrackerRecord(existing, incoming) {
  const merged = { ...existing, ...incoming };
  if (existing?.diagnostic_fingerprints || incoming?.diagnostic_fingerprints) {
    merged.diagnostic_fingerprints = { ...(existing?.diagnostic_fingerprints || {}), ...(incoming?.diagnostic_fingerprints || {}) };
  }
  if (existing?.last_status && incoming?.last_status) {
    const oldPeak = Number(existing.last_status.peak_height ?? -1);
    const newPeak = Number(incoming.last_status.peak_height ?? -1);
    const statusRank = { SYNC_ERROR: 1, MEMPOOL: 2, NOT_FOUND: 2, PENDING: 3, SUBMITTING: 3, CONFIRMED: 4, SPENT: 5 };
    if (oldPeak > newPeak || (oldPeak === newPeak && (statusRank[existing.last_status.status] || 0) > (statusRank[incoming.last_status.status] || 0))) merged.last_status = existing.last_status;
  } else if (existing?.last_status && !incoming?.last_status) {
    merged.last_status = existing.last_status;
  }
  const oldStage = String(existing?.settlement_stage || "NONE");
  const newStage = String(incoming?.settlement_stage || "NONE");
  if (settlementStageRank(existing) > settlementStageRank(incoming)) {
    merged.settlement_stage = oldStage;
    for (const field of ["initial_closing_coin_id", "current_closing_coin_id", "current_closing_sequence", "current_closing_confirmed_height", "initial_birth_height", "challenge_deadline_height", "settlement_start_spend_bundle_id", "settlement_spend_bundle_id", "settlement_final_submitted_peak_height", "settlement_final_submitted_at", "settlement_final_fee_mojo", "settlement_final_retry_fee_mojo", "settled_height", "initial_coin_spent_height"]) {
      if (existing[field] !== undefined) merged[field] = existing[field];
    }
  }
  if (settlementStageRank(existing) >= settlementStageRank(incoming)) {
    for (const field of ["current_closing_coin_id", "current_closing_sequence", "current_closing_confirmed_height", "challenge_deadline_height"]) {
      if ((merged[field] === undefined || merged[field] === null || merged[field] === "") && existing[field] !== undefined) merged[field] = existing[field];
    }
  }
  // A second client can finish a refresh with an older HUB snapshot.  Do not
  // let that snapshot erase reservation totals or the protocol heights just
  // fetched by the current writer.
  const oldHub = existing?.hub_tracking;
  const newHub = incoming?.hub_tracking;
  if (oldHub && newHub) {
    const oldSequence = Number(oldHub.state_sequence ?? -1);
    const newSequence = Number(newHub.state_sequence ?? -1);
    const oldReserved = Number(oldHub.total_reserved_mojo ?? 0);
    const newReserved = Number(newHub.total_reserved_mojo ?? 0);
    if (oldSequence > newSequence || (oldSequence === newSequence && oldReserved > newReserved)) {
      merged.hub_tracking = { ...newHub, ...oldHub };
    }
    for (const field of ["acceptance_cutoff_height", "scheduled_close_height", "funding_birth_height", "observed_peak_height", "state_sequence", "reservation_count", "total_reserved_mojo"]) {
      const oldValue = Number(oldHub[field]);
      const newValue = Number(merged.hub_tracking?.[field]);
      if (Number.isFinite(oldValue) && (!Number.isFinite(newValue) || (field !== "total_reserved_mojo" && oldValue > newValue) || (field === "total_reserved_mojo" && oldValue > newValue))) {
        merged.hub_tracking[field] = oldValue;
      }
    }
  } else if (oldHub && !newHub) {
    merged.hub_tracking = oldHub;
  }
  // Keep the legacy top-level field in step with the authoritative HUB
  // snapshot. Older clients only wrote reserved_mojo, while newer clients
  // retain the full tracking object; without this mirror a second client can
  // re-save a stale zero and make the summary card appear to lose a reserve.
  const hubReserved = Number(merged.hub_tracking?.total_reserved_mojo);
  if (Number.isFinite(hubReserved) && hubReserved >= 0) merged.reserved_mojo = hubReserved;
  for (const field of ["acceptance_cutoff_height", "scheduled_close_height", "funding_birth_height"]) {
    const oldValue = Number(existing?.[field]);
    const newValue = Number(merged[field]);
    if (Number.isFinite(oldValue) && (!Number.isFinite(newValue) || oldValue > newValue)) merged[field] = oldValue;
  }
  return merged;
}

function trackedReservedMojo(tracked) {
  const hubReserved = Number(tracked?.hub_tracking?.total_reserved_mojo);
  if (Number.isFinite(hubReserved) && hubReserved >= 0) return hubReserved;
  const localReserved = Number(tracked?.reserved_mojo);
  return Number.isFinite(localReserved) && localReserved >= 0 ? localReserved : 0;
}

function recordFundingReservation({ fundingCoinId, requestId, reservationNonce, amount, merchantPuzzleHash, result }) {
  const coinId = String(fundingCoinId || "").replace(/^0x/, "").toLowerCase();
  const amountMojo = Number(amount);
  if (!/^[0-9a-f]{64}$/.test(coinId) || !Number.isFinite(amountMojo) || amountMojo <= 0 || result?.ledger_written !== true) return false;
  const tracked = loadFundingTrackers().find((item) => String(item.funding_coin_id || "").replace(/^0x/, "").toLowerCase() === coinId);
  if (!tracked) return false;

  const summary = {
    request_id: requestId || null,
    reservation_nonce: reservationNonce || null,
    amount: String(amount),
    status: String(result.status || "SIGNED").toUpperCase(),
    ledger_written: true,
    state_sequence: result.state_sequence ?? null,
    merchant_puzzle_hash: merchantPuzzleHash || null,
    updated_at: new Date().toISOString(),
  };
  const summaries = Array.isArray(tracked.reservation_summaries) ? tracked.reservation_summaries.slice() : [];
  if (tracked.last_reservation && !summaries.some((entry) => entry.request_id === tracked.last_reservation.request_id || (entry.reservation_nonce && entry.reservation_nonce === tracked.last_reservation.reservation_nonce))) summaries.push(tracked.last_reservation);
  const existingIndex = summaries.findIndex((entry) => (requestId && entry.request_id === requestId) || (reservationNonce && entry.reservation_nonce === reservationNonce));
  if (existingIndex >= 0) summaries[existingIndex] = { ...summaries[existingIndex], ...summary };
  else summaries.push(summary);
  tracked.reservation_summaries = summaries.slice(-64);

  const knownReserved = tracked.reservation_summaries.reduce((total, entry) => {
    if (entry.ledger_written === false) return total;
    const value = Number(entry.amount);
    return Number.isFinite(value) && value > 0 ? total + value : total;
  }, 0);
  const responseReserved = Number(result.total_reserved_mojo);
  const reservedMojo = Math.min(Number(tracked.amount_mojo), Number.isFinite(responseReserved) && responseReserved >= 0 ? responseReserved : Math.max(trackedReservedMojo(tracked), knownReserved));
  tracked.reserved_mojo = reservedMojo;
  tracked.hub_tracking = {
    ...(tracked.hub_tracking || {}),
    total_reserved_mojo: reservedMojo,
    reservation_count: Math.max(Number(tracked.hub_tracking?.reservation_count || 0), Number(result.reservation_count || 0), tracked.reservation_summaries.length),
    state_sequence: result.state_sequence ?? tracked.hub_tracking?.state_sequence,
  };
  tracked.last_reservation = summary;
  if (result.acceptance_cutoff_height != null && Number.isFinite(Number(result.acceptance_cutoff_height))) tracked.acceptance_cutoff_height = Number(result.acceptance_cutoff_height);
  if (result.scheduled_close_height != null && Number.isFinite(Number(result.scheduled_close_height))) tracked.scheduled_close_height = Number(result.scheduled_close_height);
  saveFundingTracker(tracked);
  return true;
}

function reconcileFundingReservationsFromHistory() {
  for (const entry of loadReservationTrackers()) {
    if (entry.ledger_written !== true || !entry.funding_coin_id || !entry.amount) continue;
    recordFundingReservation({
      fundingCoinId: entry.funding_coin_id,
      requestId: entry.request_id,
      reservationNonce: entry.reservation_nonce,
      amount: entry.amount,
      merchantPuzzleHash: entry.merchant_puzzle_hash,
      result: {
        status: entry.status || "SIGNED",
        ledger_written: true,
        state_sequence: entry.state_sequence,
        acceptance_cutoff_height: entry.acceptance_cutoff_height,
        scheduled_close_height: entry.scheduled_close_height,
      },
    });
  }
}

function mergeFundingTrackerLists(base, incoming) {
  const merged = [];
  const positions = new Map();
  for (const tracker of [...base, ...incoming]) {
    if (!tracker?.funding_coin_id) continue;
    const position = positions.get(tracker.funding_coin_id);
    if (position === undefined) {
      positions.set(tracker.funding_coin_id, merged.length);
      merged.push({ ...tracker });
    } else {
      merged[position] = mergeFundingTrackerRecord(merged[position], tracker);
    }
  }
  return merged;
}

async function hydrateFundingTrackers() {
  if (!selectedWalletProfile) return;
  try {
    const remote = await request("/api/v3.6/wallet-profiles/funding/trackers");
    const remoteTrackers = (Array.isArray(remote.trackers) ? remote.trackers : []).filter(trackerBelongsToCurrentWallet);
    const localTrackers = loadFundingTrackers();
    const removed = loadRemovedFundingTrackerIds();
    const merged = mergeFundingTrackerLists(remoteTrackers, localTrackers).filter((item) => !removed.has(item.funding_coin_id));
    localStorage.setItem(walletScopedKey(FUNDING_TRACKER_KEY), JSON.stringify(merged));
    renderFundingTrackerTabs();
    renderActiveFundingTracker();
    renderHistory();
    if ((localTrackers.length && remoteTrackers.length === 0) || remoteTrackers.some((item) => removed.has(item.funding_coin_id))) persistFundingTrackers(merged);
  } catch { /* older desktop builds do not expose persistence; local fallback remains usable */ }
}

function renderFundingTrackerTabs() {
  const target = $("#tracker-tabs");
  if (!target) return;
  const trackers = loadFundingTrackers().slice().sort((left, right) => {
    const rightHeight = Number(right.last_status?.confirmed_height || right.last_status?.peak_height || 0);
    const leftHeight = Number(left.last_status?.confirmed_height || left.last_status?.peak_height || 0);
    if (rightHeight !== leftHeight) return rightHeight - leftHeight;
    return String(right.created_at || "").localeCompare(String(left.created_at || ""));
  });
  const active = loadFundingTracker()?.funding_coin_id;
  target.innerHTML = `${trackers.map((item, index) => {
    const logCount = new Set(activities.filter((activity) => activityBelongsToFundingCoin(activity, item)).map(fundingActivityDisplayKey)).size;
    const reserved = trackedReservedMojo(item);
    const reservationLabel = reserved > 0 ? ` · 已预扣 ${reserved.toLocaleString("en-US")} mojo` : "";
    return `<button class="tracker-tab ${item.funding_coin_id === active ? "active" : ""}" data-tracker-id="${escapeHtml(item.funding_coin_id)}" type="button"><span>Coin ${index + 1}</span><code>${escapeHtml(item.funding_coin_id.slice(0, 12))}…</code><small>${logCount} 条日志 · ${escapeHtml(item.settlement_stage || item.last_status?.status || "等待同步")}${reservationLabel}</small><span class="tracker-remove" data-remove-tracker="${escapeHtml(item.funding_coin_id)}" role="button" tabindex="0" title="取消追踪此 Coin" aria-label="取消追踪 Coin ${index + 1}">×</span></button>`;
  }).join("")}<button id="add-funding-coin" class="tracker-add-button" type="button">＋ 添加 FUNDCOIN</button>`;
  $$(".tracker-tab").forEach((button) => button.addEventListener("click", () => {
    localStorage.setItem(walletScopedKey(FUNDING_TRACKER_ACTIVE_KEY), button.dataset.trackerId);
    renderFundingTrackerTabs();
    renderActiveFundingTracker();
    refreshFundingTracker({ quiet: true });
  }));
  $$('[data-remove-tracker]').forEach((removeButton) => {
    const remove = (event) => {
      event.stopPropagation();
      const coinId = removeButton.dataset.removeTracker;
      if (coinId && window.confirm("取消追踪只会删除本地记录，不会花费或删除链上 Coin。是否继续？")) removeFundingTracker(coinId);
    };
    removeButton.addEventListener("click", remove);
    removeButton.addEventListener("keydown", (event) => {
      if (event.key === "Enter" || event.key === " ") { event.preventDefault(); remove(event); }
    });
  });
  $("#add-funding-coin")?.addEventListener("click", () => { const panel = $("#tracker-import-panel"); if (panel) { panel.open = true; panel.scrollIntoView({ behavior: "smooth", block: "center" }); } });
}

function installFundingTrackerImport() {
  const view = $("[data-view-panel='funding-tracker']");
  if (!view || $("#tracker-import-panel")) return;
  view.querySelector(".page-heading").insertAdjacentHTML("afterend", `<div id="tracker-tabs" class="tracker-tabs" role="tablist"></div><details id="tracker-import-panel" class="panel"><summary>手工添加或导入 FUNDCOIN</summary><p class="page-description">只填写公开字段；不会读取私钥、签名或广播。只填 Coin ID 时，金额和 Funding Puzzle Hash 会从主网只读查询；时间窗口可稍后补充。</p><div class="field-grid"><label>Funding Coin ID<input id="tracker-import-id" spellcheck="false" placeholder="必填，64 位十六进制"></label><label>Funding Puzzle Hash（可选）<input id="tracker-import-puzzle" spellcheck="false" placeholder="留空则按 Coin ID 查询"></label><label>创建者 Puzzle Hash（可选）<input id="tracker-import-creator" spellcheck="false" placeholder="当前钱包会自动填入，用于身份比对"></label><label>Funding 金额（mojo，可选）<input id="tracker-import-amount" inputmode="numeric" placeholder="留空则按 Coin ID 查询"></label><label>结束预扣（相对区块，可选）<input id="tracker-import-termination" inputmode="numeric"></label><label>挑战期（区块，可选）<input id="tracker-import-challenge" inputmode="numeric"></label></div><div id="tracker-import-error" class="message error" role="alert" hidden></div><button id="tracker-import-save" class="button primary" type="button">保存并开始追踪</button></details>`);
  $("#tracker-import-save").addEventListener("click", async () => {
    clearError("#tracker-import-error");
    const coin = $("#tracker-import-id").value.trim().replace(/^0x/, "").toLowerCase();
    let puzzle = $("#tracker-import-puzzle").value.trim().replace(/^0x/, "").toLowerCase();
    const creator = $("#tracker-import-creator").value.trim().replace(/^0x/, "").toLowerCase();
    let amount = $("#tracker-import-amount").value.trim();
    const termination = $("#tracker-import-termination").value.trim();
    const challenge = $("#tracker-import-challenge").value.trim();
    if (!/^[0-9a-f]{64}$/.test(coin) || (creator && !/^[0-9a-f]{64}$/.test(creator))) { showError("#tracker-import-error", "Coin ID 和创建者 Puzzle Hash 必须是 64 位十六进制。"); return; }
    if (!puzzle || !amount) {
      const button = $("#tracker-import-save");
      button.disabled = true;
      button.textContent = "正在查询主网 Coin...";
      try {
        const discovered = await request("/api/v3.6/wallet-profiles/funding/discover", { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify({ rpc_url: "https://api.coinset.org", funding_coin_id: coin }) });
        // A Funding Coin is normally imported before settlement, but after A
        // broadcasts the start spend it is expected to be SPENT. Its record
        // still contains the public puzzle hash and amount needed to recover
        // the Initial Closing Coin, so SPENT must remain importable.
        if (!["FOUND", "SPENT"].includes(discovered.status)) throw new Error("主网未找到这个 Coin，请确认 Coin ID 完整且已广播。");
        puzzle = discovered.funding_puzzle_hash || "";
        amount = String(discovered.funding_amount_mojo ?? "");
        if (!creator && discovered.creator_puzzle_hash) {
          $("#tracker-import-creator").value = discovered.creator_puzzle_hash;
        }
        $("#tracker-import-puzzle").value = puzzle;
        $("#tracker-import-amount").value = amount;
      } catch (error) { showError("#tracker-import-error", `只读查询失败：${error.message}`); return; }
      finally { button.disabled = false; button.textContent = "保存并开始追踪"; }
    }
    if (!/^[0-9a-f]{64}$/.test(puzzle)) { showError("#tracker-import-error", "Funding Puzzle Hash 必须是 64 位十六进制，或留空让程序按 Coin ID 查询。"); return; }
    if (!/^\d+$/.test(amount) || BigInt(amount) <= 0n) { showError("#tracker-import-error", "Funding 金额必须是大于 0 的整数，或留空让程序按 Coin ID 查询。"); return; }
    if (termination && (!/^\d+$/.test(termination) || BigInt(termination) <= 0n)) { showError("#tracker-import-error", "结束预扣区块必须是正整数，或留空表示暂不显示窗口。"); return; }
    if (challenge && (!/^\d+$/.test(challenge) || BigInt(challenge) <= 0n)) { showError("#tracker-import-error", "挑战期必须是正整数，或留空表示暂不显示窗口。"); return; }
    const discoveredCreator = $("#tracker-import-creator").value.trim().replace(/^0x/, "").toLowerCase();
    if (discoveredCreator && !/^[0-9a-f]{64}$/.test(discoveredCreator)) { showError("#tracker-import-error", "创建者 Puzzle Hash 查询结果无效。"); return; }
    restoreFundingTrackerId(coin);
    saveFundingTracker({ schema: "xhub.wallet.funding-tracker.v1", funding_coin_id: coin, funding_puzzle_hash: puzzle, creator_puzzle_hash: discoveredCreator || null, tracking_wallet_puzzle_hash: selectedWalletProfile?.puzzle_hash || null, amount_mojo: Number(amount), reserved_mojo: 0, termination_blocks: termination ? Number(termination) : 0, challenge_blocks: challenge ? Number(challenge) : 0, required_confirmations: Number(profile?.funding_confirmation_blocks || 1), created_at: new Date().toISOString(), imported: true });
    $("#tracker-import-panel").open = false;
    refreshFundingTracker();
  });
}

function renderActiveFundingTracker() {
  const reset = (selector, value = "-") => { const element = $(selector); if (element) element.textContent = value; };
  reset("#tracker-total"); reset("#tracker-reserved"); reset("#tracker-remaining"); reset("#tracker-chain-status", "未载入"); reset("#tracker-confirmations", "等待同步"); reset("#tracker-peak"); reset("#tracker-cutoff-height"); reset("#tracker-cutoff-remaining"); reset("#tracker-challenge-height"); reset("#tracker-challenge-remaining"); reset("#tracker-confirmed-height"); reset("#tracker-coin-id");
  const tracked = loadFundingTracker();
  if (!tracked) {
    $("#tracker-sync-state").textContent = "没有记录";
    $("#tracker-chain-status").textContent = "未载入";
    $("#tracker-creator").textContent = "尚未添加 Coin";
    updateSettlementAction(null);
    renderFundingCoinLifecycle(null);
    return;
  }
  const totalMojo = Number(tracked.amount_mojo);
  const reservedMojo = trackedReservedMojo(tracked);
  $("#tracker-total").textContent = `${Number.isFinite(totalMojo) ? totalMojo.toLocaleString("en-US") : "-"} mojo`;
  $("#tracker-reserved").textContent = `${reservedMojo.toLocaleString("en-US")} mojo`;
  $("#tracker-remaining").textContent = `${Math.max((Number.isFinite(totalMojo) ? totalMojo : 0) - reservedMojo, 0).toLocaleString("en-US")} mojo`;
  $("#tracker-coin-id").textContent = tracked.funding_coin_id;
  const ownPuzzle = selectedWalletProfile?.puzzle_hash?.toLowerCase();
  const creatorPuzzle = tracked.creator_puzzle_hash?.toLowerCase();
  const hubRegistered = Boolean(tracked.hub_tracking || tracked.auto_registered_at || tracked.recovery_package_available);
  const creator = ownPuzzle && creatorPuzzle && ownPuzzle === creatorPuzzle ? "本钱包创建（当前私钥可验证）" : creatorPuzzle ? "其他钱包创建" : "暂无法验证（缺少创建者 Puzzle Hash）";
  $("#tracker-creator").textContent = creator;
  $("#tracker-creator").className = ownPuzzle && creatorPuzzle && ownPuzzle === creatorPuzzle ? "tag good" : "tag warn";
  if (tracked.last_status) {
    const status = tracked.last_status;
    if (status.status === "MEMPOOL") {
      $("#tracker-chain-status").textContent = "MEMPOOL · 等待上链";
      $("#tracker-sync-state").textContent = "MEMPOOL · 等待上链";
      $("#tracker-sync-state").className = "tag warn";
      $("#tracker-confirmations").textContent = mempoolStatusDetail(status);
      $("#tracker-cutoff-height").textContent = "未同步";
      $("#tracker-cutoff-remaining").textContent = "未同步";
      $("#tracker-challenge-height").textContent = "未同步";
      $("#tracker-challenge-remaining").textContent = "未同步";
      renderFundingCoinLifecycle(tracked);
      updateSettlementAction(tracked);
      return;
    }
    if (status.status === "SYNC_ERROR") {
      $("#tracker-chain-status").textContent = "同步失败";
      $("#tracker-confirmations").textContent = status.error || "无法读取该 Coin";
      $("#tracker-cutoff-height").textContent = "未同步";
      $("#tracker-cutoff-remaining").textContent = "未同步";
      $("#tracker-challenge-height").textContent = "未同步";
      $("#tracker-challenge-remaining").textContent = "未同步";
      renderFundingCoinLifecycle(tracked);
      return;
    }
    $("#tracker-chain-status").textContent = ({ CONFIRMED: "已确认", CONFIRMING: "确认中", SPENT: "已花费", MISSING: "尚未发现" })[status.status] || status.status;
    $("#tracker-confirmations").textContent = `${status.confirmations}/${status.required_confirmations} confirmations`;
    $("#tracker-peak").textContent = Number(status.peak_height).toLocaleString("en-US");
    $("#tracker-confirmed-height").textContent = status.confirmed_height == null ? "等待确认" : Number(status.confirmed_height).toLocaleString("en-US");
    const cutoffHeight = tracked.hub_tracking?.acceptance_cutoff_height ?? tracked.acceptance_cutoff_height;
    const challengeHeight = tracked.hub_tracking?.scheduled_close_height ?? tracked.scheduled_close_height;
    const cutoffLabel = cutoffHeight == null ? (hubRegistered ? "已登记 · 等待同步" : "HUB 未登记") : Number(cutoffHeight).toLocaleString("en-US");
    const challengeLabel = challengeHeight == null ? (hubRegistered ? "已登记 · 等待同步" : "HUB 未登记") : Number(challengeHeight).toLocaleString("en-US");
    $("#tracker-cutoff-height").textContent = cutoffLabel;
    $("#tracker-challenge-height").textContent = challengeLabel;
    $("#tracker-cutoff-remaining").textContent = cutoffHeight == null ? cutoffLabel : formatRemainingBlocks(Number(cutoffHeight), Number(status.peak_height));
    $("#tracker-challenge-remaining").textContent = challengeHeight == null ? challengeLabel : formatRemainingBlocks(Number(challengeHeight), Number(status.peak_height));
  }
  updateSettlementAction(tracked);
  renderFundingCoinLifecycle(tracked);
}

function updateSettlementAction(tracked) {
  const button = $("#settle-funding-coin");
  if (!button) return;
  button.disabled = true;
  if (!tracked) { button.textContent = "结算这个 Coin"; button.title = "尚未选择 Funding Coin"; return; }
  const hubRegistered = Boolean(tracked.hub_tracking || tracked.auto_registered_at || tracked.recovery_package_available);
  if (tracked.auto_registered_at && !tracked.hub_tracking && !tracked.recovery_package_available) {
    logFundingDiagnostic(tracked, "HUB 登记状态与摘要不同步", `Coin=${tracked.funding_coin_id} · 已有登记时间=${tracked.auto_registered_at} · tracking-summary=未返回 · 继续使用已登记状态`, `hub-ui-mismatch:${tracked.auto_registered_at}`);
  }
  normalizeLegacySettlementStage(tracked);
  if (tracked.settlement_stage === "SETTLED") { button.textContent = "已完成结算"; button.title = `该 Coin 已完成结算${tracked.settled_height ? `，确认高度 ${tracked.settled_height}` : ""}`; return; }
  if (["START_PENDING", "START_SUBMITTED"].includes(tracked.settlement_stage)) { button.textContent = "核验开始结算交易"; button.title = "正在核验 Funding Coin、Initial Closing Coin 和 mempool 状态"; return; }
  if (tracked.settlement_stage === "START_DROPPED") {
    button.disabled = false;
    button.textContent = "重新准备开始结算";
    button.title = "原交易已不在 mempool，Funding Coin 仍未花费，Initial Closing Coin 尚未生成";
    const fee = $("#settlement-fee");
    const suggestedFee = Math.max(Number(tracked.settlement_retry_fee_mojo || 0), SETTLEMENT_RETRY_MIN_FEE_MOJO);
    if (fee && Number(fee.value || 0) < suggestedFee) fee.value = String(suggestedFee);
    return;
  }
  if (["FINAL_PENDING", "FINAL_SUBMITTED"].includes(tracked.settlement_stage)) { button.textContent = "核验最终结算交易"; button.title = "正在核验 Initial Closing Coin 和 mempool 状态"; return; }
  if (tracked.settlement_stage === "FINAL_DROPPED") {
    button.disabled = false;
    button.textContent = "重新准备最终结算";
    button.title = "原最终结算交易已不在 mempool，Initial Closing Coin 仍未花费";
    const fee = $("#settlement-fee");
    const suggestedFee = Math.max(Number(tracked.settlement_final_retry_fee_mojo || 0), SETTLEMENT_RETRY_MIN_FEE_MOJO);
    if (fee && Number(fee.value || 0) < suggestedFee) fee.value = String(suggestedFee);
    return;
  }
  const deadline = Number(tracked.challenge_deadline_height || 0);
  const peak = Number(tracked.last_status?.peak_height || 0);
  if (tracked.initial_coin_spent_height && !tracked.current_closing_coin_id) { button.textContent = "已完成结算"; button.title = `最终 Closing Coin 已在高度 ${tracked.initial_coin_spent_height} 花费`; return; }
  if (tracked.settlement_stage === "INITIAL_CLOSING" && deadline > 0 && peak < deadline) { button.textContent = `挑战中 · ${deadline - peak} blocks`; button.title = `挑战期结束高度 ${deadline}`; return; }
  if (tracked.settlement_stage === "INITIAL_CLOSING" && deadline > 0 && peak >= deadline) { button.disabled = false; button.textContent = "挑战期已结束 · 确认最终结算"; button.title = "挑战期已结束；点击后核对最终结算明细，再确认是否广播"; return; }
  if (tracked.settlement_stage === "INITIAL_CLOSING") { button.textContent = "挑战期参数未确认"; button.title = "必须先取得 HUB Recovery Package 中的 challenge_blocks"; return; }
  if (tracked.last_status?.status === "CONFIRMED" && hubRegistered) { button.disabled = false; button.textContent = "准备开始结算"; button.title = tracked.hub_tracking ? "从 Funding Coin 构造 Initial Closing SpendBundle" : "HUB 已登记；可读取 Recovery Package 并构造结算 SpendBundle"; return; }
  if (tracked.last_status?.status === "CONFIRMED" && tracked.confirmed_draft) { button.disabled = false; button.textContent = "注册 HUB 并准备结算"; button.title = "把已确认条款登记到 HUB，再读取通道状态和 Recovery Package"; return; }
  button.textContent = tracked.last_status?.status === "CONFIRMED" ? "缺少原始条款，无法注册 HUB" : "等待 Funding Coin 确认";
  button.title = tracked.last_status?.status === "CONFIRMED" ? "该 Coin 未在 HUB 登记，本地也没有已确认条款；请重新导入原始 WC 请求" : "Funding Coin 需要先达到确认深度";
}

function formatRemainingBlocks(targetHeight, peakHeight) {
  if (!Number.isFinite(targetHeight) || !Number.isFinite(peakHeight)) return "等待链上确认";
  const blocks = Math.max(targetHeight - peakHeight, 0);
  if (blocks === 0) return "已结束";
  const minutes = Math.max(Math.round(blocks * 18.75 / 60), 1);
  return `${blocks.toLocaleString("en-US")} blocks（约 ${minutes.toLocaleString("en-US")} 分钟）`;
}

async function autoRegisterFunding(tracked) {
  if (!selectedWalletProfile
      || tracked.last_status?.status !== "CONFIRMED"
      || tracked.hub_tracking
      || tracked.recovery_package_available
      || !tracked.confirmed_draft?.confirmed
      || fundingRegistrationInFlight.has(tracked.funding_coin_id)) return;
  fundingRegistrationInFlight.add(tracked.funding_coin_id);
  try {
    const registration = await request("/api/v3.6/wallet-profiles/funding/register", {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({ role: selectedWalletProfile.role, funding_coin_id: tracked.funding_coin_id, confirmed_draft: tracked.confirmed_draft })
    });
    const hub = registration.hub_response || registration;
    tracked.hub_tracking = {
      acceptance_cutoff_height: hub.acceptance_cutoff_height,
      scheduled_close_height: hub.scheduled_close_height,
      funding_birth_height: hub.funding_birth_height,
      chain_state: hub.chain_state,
      total_reserved_mojo: 0
    };
    tracked.auto_registered_at = new Date().toISOString();
    tracked.hub_registration_error = null;
    logFundingDiagnostic(tracked, "Funding Coin 已自动登记 HUB", `Coin=${tracked.funding_coin_id} · confirmations=${tracked.last_status.confirmations}/${tracked.last_status.required_confirmations} · chain_state=${hub.chain_state || "-"} · push_tx=false`, `auto-register:${tracked.funding_coin_id}:${hub.chain_state || "-"}`);
  } catch (error) {
    tracked.hub_registration_error = error.message;
    logFundingDiagnostic(tracked, "Funding Coin 自动登记等待重试", `Coin=${tracked.funding_coin_id} · confirmations=${tracked.last_status?.confirmations || 0}/${tracked.last_status?.required_confirmations || 1} · ${error.message}`, `auto-register-error:${tracked.funding_coin_id}:${error.message}`);
  } finally {
    fundingRegistrationInFlight.delete(tracked.funding_coin_id);
  }
}

function logFundingDiagnostic(tracked, title, detail, fingerprint) {
  const category = fingerprint.split(":", 1)[0];
  tracked.diagnostic_fingerprints = tracked.diagnostic_fingerprints || {};
  const alreadyLogged = activities.some((item) => item.funding_coin_id === tracked.funding_coin_id
    && item.diagnostic_fingerprint === fingerprint);
  if (tracked.diagnostic_fingerprints[category] === fingerprint || alreadyLogged) return;
  tracked.diagnostic_fingerprints[category] = fingerprint;
  addActivity(title, detail, tracked.funding_coin_id, fingerprint);
}

function applyHubTrackingSnapshot(tracked, hub) {
  if (!hub || typeof hub !== "object") return;
  tracked.hub_tracking = hub;
  tracked.recovery_package_available = false;
  const reservedMojo = Number(hub.total_reserved_mojo);
  if (Number.isFinite(reservedMojo) && reservedMojo >= 0) tracked.reserved_mojo = reservedMojo;
  for (const [source, target] of [["acceptance_cutoff_height", "acceptance_cutoff_height"], ["scheduled_close_height", "scheduled_close_height"], ["funding_birth_height", "funding_birth_height"], ["challenge_blocks", "challenge_blocks"], ["funding_amount_mojo", "amount_mojo"], ["acceptance_blocks", "termination_blocks"], ["freeze_blocks", "freeze_blocks"]]) {
    if (hub[source] != null && Number.isFinite(Number(hub[source]))) tracked[target] = Number(hub[source]);
  }
  if (hub.channel_terms_canonical_hex) tracked.channel_terms_canonical_hex = hub.channel_terms_canonical_hex;
  if (hub.funding_puzzle_reveal_hex) tracked.funding_puzzle_reveal_hex = hub.funding_puzzle_reveal_hex;
  if (hub.funding_puzzle_hash) tracked.funding_puzzle_hash = hub.funding_puzzle_hash;
}

async function refreshHubTrackingSnapshot(tracked) {
  try {
    const before = `${tracked.hub_tracking?.state_sequence ?? "-"}:${tracked.hub_tracking?.total_reserved_mojo ?? "-"}:${tracked.acceptance_cutoff_height ?? "-"}:${tracked.scheduled_close_height ?? "-"}`;
    const hub = await request(`/api/v3.6/hub/funding-coins/${tracked.funding_coin_id}/tracking`, { headers: { "x-xhub-protocol-version": "0x0360" } });
    applyHubTrackingSnapshot(tracked, hub);
    const after = `${hub.state_sequence ?? "-"}:${hub.total_reserved_mojo ?? "-"}:${hub.acceptance_cutoff_height ?? "-"}:${hub.scheduled_close_height ?? "-"}`;
    if (before !== after) logFundingDiagnostic(tracked, "HUB 信息已同步到 Coin 追踪", `Coin=${tracked.funding_coin_id} · sequence=${hub.state_sequence ?? "-"} · reserved=${hub.total_reserved_mojo ?? 0} · cutoff=${hub.acceptance_cutoff_height ?? "-"} · challenge=${hub.scheduled_close_height ?? "-"}`, `hub-sync:${after}`);
    return true;
  } catch (_) {
    return false;
  }
}

async function refreshSettlementDiscovery(tracked) {
  if (!tracked?.funding_coin_id) return false;
  const packageResponse = await request(`/api/v3.6/hub/funding-coins/${tracked.funding_coin_id}/recovery-packages/latest`, { headers: { "x-xhub-protocol-version": "0x0360" } });
  const packageHex = packageResponse.recovery_package_canonical_hex;
  if (!packageHex) return false;
  tracked.recovery_package_available = true;
  tracked.recovery_package_canonical_hex = packageHex;
  tracked.recovery_package_content_hash = packageResponse.recovery_package_content_hash || tracked.recovery_package_content_hash || null;
  if (packageResponse.challenge_blocks != null) {
    const packageChallengeBlocks = Number(packageResponse.challenge_blocks);
    if (Number.isFinite(packageChallengeBlocks) && packageChallengeBlocks > 0) tracked.challenge_blocks = packageChallengeBlocks;
  }
  const discovered = await request("/api/v3.6/wallet-profiles/settlement/discover", {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({ rpc_url: "https://api.coinset.org", funding_coin_id: tracked.funding_coin_id, recovery_package_canonical_hex: packageHex }),
  });
  tracked.initial_closing_coin_id = discovered.initial_closing_coin_id || tracked.initial_closing_coin_id || null;
  tracked.initial_birth_height = discovered.initial_confirmed_height == null ? tracked.initial_birth_height || null : Number(discovered.initial_confirmed_height);
  tracked.initial_coin_spent_height = discovered.initial_spent_height == null ? tracked.initial_coin_spent_height || null : Number(discovered.initial_spent_height);
  tracked.closing_coins = Array.isArray(discovered.closing_coins) ? discovered.closing_coins : (tracked.closing_coins || []);
  if (discovered.current_closing_coin_id) {
    tracked.current_closing_coin_id = normalizeCoinId(discovered.current_closing_coin_id);
  } else if (discovered.suggested_stage === "SETTLED") {
    // A null current coin is authoritative once the discovered chain is
    // settled; do not keep a stale Closing Coin that can be spent again.
    tracked.current_closing_coin_id = null;
    tracked.current_closing_sequence = null;
    tracked.current_closing_confirmed_height = null;
  } else {
    tracked.current_closing_coin_id = tracked.current_closing_coin_id || tracked.initial_closing_coin_id || null;
  }
  if (discovered.current_closing_sequence != null) tracked.current_closing_sequence = Number(discovered.current_closing_sequence);
  else if (tracked.current_closing_sequence == null && tracked.current_closing_coin_id) tracked.current_closing_sequence = 1;
  if (discovered.current_closing_confirmed_height != null) tracked.current_closing_confirmed_height = Number(discovered.current_closing_confirmed_height);
  const discoveredChallengeBlocks = Number(discovered.challenge_blocks);
  if (Number.isFinite(discoveredChallengeBlocks) && discoveredChallengeBlocks > 0) tracked.challenge_blocks = discoveredChallengeBlocks;
  if (discovered.challenge_deadline_height != null) {
    const discoveredDeadline = Number(discovered.challenge_deadline_height);
    // Do not replace a previously verified non-zero challenge window with a
    // transient zero-value response from an older gateway/package endpoint.
    if (Number.isFinite(discoveredDeadline) && (discoveredChallengeBlocks > 0 || !tracked.challenge_deadline_height)) {
      tracked.challenge_deadline_height = discoveredDeadline;
    }
  }
  // Initial and Subsequent Closing Coins share the Initial challenge
  // deadline. Keep this authoritative value ahead of legacy current-coin
  // derived fields.
  if (discovered.challenge_deadline_height != null) {
    const initialDeadline = Number(discovered.challenge_deadline_height);
    if (Number.isSafeInteger(initialDeadline) && initialDeadline > 0) tracked.challenge_deadline_height = initialDeadline;
  }
  // Recovery Package checkpoint is a HUB-signed global ledger snapshot. It
  // must remain the summary source even when the optional tracking endpoint
  // is unavailable, otherwise each address falls back to its local records.
  if (discovered.total_reserved_mojo != null) {
    const globalReserved = Number(discovered.total_reserved_mojo);
    if (Number.isFinite(globalReserved) && globalReserved >= 0) {
      tracked.reserved_mojo = globalReserved;
      tracked.hub_tracking = {
        ...(tracked.hub_tracking || {}),
        total_reserved_mojo: globalReserved,
        reservation_count: Number(discovered.ledger_entry_count || 0),
        checkpoint_source: "RECOVERY_PACKAGE",
      };
    }
  }
  const current = String(tracked.settlement_stage || "NONE");
  const transient = ["START_PENDING", "START_SUBMITTED", "START_DROPPED", "FINAL_PENDING", "FINAL_SUBMITTED", "FINAL_DROPPED"];
  if (!transient.includes(current)) {
    if (discovered.suggested_stage === "SETTLED" && !discovered.current_closing_coin_id && discovered.initial_spent_height != null) {
      tracked.settlement_stage = "SETTLED";
      tracked.settled_height = Number(discovered.initial_spent_height);
      tracked.settled_at = tracked.settled_at || new Date().toISOString();
    } else if (["INITIAL_CLOSING", "SUBSEQUENT_CLOSING", "FINALIZE"].includes(discovered.suggested_stage)) {
      tracked.settlement_stage = discovered.suggested_stage;
    }
  }
  if (tracked.funding_coin_id === loadFundingTracker()?.funding_coin_id) {
    logFundingDiagnostic(tracked, "已自动发现 Closing Coin 链", `Funding Coin=${tracked.funding_coin_id} · Initial=${tracked.initial_closing_coin_id} · Current=${tracked.current_closing_coin_id} · sequence=${tracked.current_closing_sequence || 1} · funding=${discovered.funding_state} · initial=${discovered.initial_state} · deadline=${tracked.challenge_deadline_height || "-"}`, `settlement-discover:${discovered.funding_state}:${discovered.initial_state}:${tracked.current_closing_coin_id}:${tracked.current_closing_sequence || 1}:${tracked.initial_coin_spent_height || "-"}`);
  }
  return true;
}

async function refreshPendingSettlement(tracked) {
  const startPending = ["START_PENDING", "START_SUBMITTED"].includes(tracked.settlement_stage);
  const finalPending = ["FINAL_PENDING", "FINAL_SUBMITTED"].includes(tracked.settlement_stage);
  const spendBundleId = finalPending ? tracked.settlement_spend_bundle_id : tracked.settlement_start_spend_bundle_id;
  if ((!startPending && !finalPending) || !tracked.initial_closing_coin_id || !spendBundleId) return false;
  const stage = finalPending ? "FINALIZE" : "START";
  const status = await request("/api/v3.6/wallet-profiles/settlement/pending-status", {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({
      rpc_url: "https://api.coinset.org",
      stage,
      funding_coin_id: tracked.funding_coin_id,
      initial_closing_coin_id: tracked.initial_closing_coin_id,
      spend_bundle_id: spendBundleId,
    }),
  });
  const now = new Date().toISOString();
  if (finalPending) {
    tracked.settlement_final_pending_status = status;
    tracked.settlement_final_pending_checked_at = now;
    if (status.initial_state === "SPENT" && status.initial_spent_height != null) {
      tracked.settlement_stage = "SETTLED";
      tracked.initial_coin_spent_height = Number(status.initial_spent_height);
      tracked.settled_height = Number(status.initial_spent_height);
      tracked.settled_at = now;
      logFundingDiagnostic(tracked, "最终结算已确认上链", `Initial Coin=${tracked.initial_closing_coin_id} · spent_height=${status.initial_spent_height} · SpendBundle=${spendBundleId}`, `settled:${status.initial_spent_height}`);
      return true;
    }
    if (status.tx_in_mempool) {
      tracked.settlement_stage = "FINAL_SUBMITTED";
      return true;
    }
    const submittedPeak = Number(tracked.settlement_final_submitted_peak_height);
    const blocksWaited = Number.isFinite(submittedPeak) && submittedPeak > 0 ? Math.max(Number(status.peak_height) - submittedPeak, 0) : SETTLEMENT_PENDING_GRACE_BLOCKS;
    if (status.safe_to_retry === true && blocksWaited >= SETTLEMENT_PENDING_GRACE_BLOCKS) {
      tracked.settlement_stage = "FINAL_DROPPED";
      tracked.settlement_final_dropped_at = now;
      tracked.settlement_final_retry_fee_mojo = Math.max(Number(tracked.settlement_final_fee_mojo || 0) * 10, SETTLEMENT_RETRY_MIN_FEE_MOJO);
      logFundingDiagnostic(tracked, "最终结算交易已掉单", `SpendBundle=${spendBundleId} · funding=SPENT · initial=UNSPENT · mempool=false · waited=${blocksWaited} blocks · 可重新准备`, `settlement-final-dropped:${spendBundleId}`);
      saveFundingTracker(tracked);
    }
    return true;
  }
  tracked.settlement_pending_status = status;
  tracked.settlement_pending_checked_at = now;
  if (["UNSPENT", "SPENT"].includes(status.initial_state)) {
    tracked.initial_birth_height = Number(status.initial_confirmed_height);
    tracked.challenge_deadline_height = tracked.initial_birth_height + Number(tracked.challenge_blocks || 0);
    tracked.settlement_stage = status.initial_state === "SPENT" ? "SETTLED" : "INITIAL_CLOSING";
    if (status.initial_spent_height != null) {
      tracked.initial_coin_spent_height = Number(status.initial_spent_height);
      tracked.settled_height = Number(status.initial_spent_height);
    }
    return true;
  }
  if (status.tx_in_mempool) {
    tracked.settlement_stage = "START_SUBMITTED";
    return true;
  }
  const submittedPeak = Number(tracked.settlement_start_submitted_peak_height);
  const blocksWaited = Number.isFinite(submittedPeak) && submittedPeak > 0 ? Math.max(Number(status.peak_height) - submittedPeak, 0) : SETTLEMENT_PENDING_GRACE_BLOCKS;
  if (status.safe_to_retry === true && blocksWaited >= SETTLEMENT_PENDING_GRACE_BLOCKS) {
    tracked.settlement_stage = "START_DROPPED";
    tracked.settlement_dropped_at = new Date().toISOString();
    tracked.settlement_retry_fee_mojo = Math.max(Number(tracked.settlement_start_fee_mojo || 0) * 10, SETTLEMENT_RETRY_MIN_FEE_MOJO);
    logFundingDiagnostic(tracked, "结算开始交易已掉单", `SpendBundle=${tracked.settlement_start_spend_bundle_id} · funding=UNSPENT · initial=MISSING · mempool=false · waited=${blocksWaited} blocks · 可重新准备`, `settlement-dropped:${tracked.settlement_start_spend_bundle_id}`);
    saveFundingTracker(tracked);
    return true;
  }
  return true;
}

async function repairClosingDeadline(tracked) {
  if (!tracked?.initial_closing_coin_id) return false;
  const blocks = Number(tracked.challenge_blocks || tracked.hub_tracking?.challenge_blocks || 0);
  if (!Number.isFinite(blocks) || blocks <= 0) return false;
  try {
    const c = await request("/api/v3.6/wallet-profiles/funding/discover", { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify({ rpc_url: "https://api.coinset.org", funding_coin_id: tracked.initial_closing_coin_id }) });
    if (c.confirmed_height == null) return false;
    tracked.initial_birth_height = Number(c.confirmed_height);
    tracked.challenge_blocks = blocks;
    tracked.challenge_deadline_height = tracked.initial_birth_height + blocks;
    if (c.status === "SPENT" && c.spent_height != null) tracked.settlement_stage = "SETTLED";
    else if (tracked.settlement_stage !== "SETTLED") tracked.settlement_stage = "INITIAL_CLOSING";
    return true;
  } catch (_) { return false; }
}

// Multiple desktop clients can share the same profile directory.  WebView2's
// Web Locks API gives those clients one writer for the asynchronous refresh;
// followers keep their UI but must not write a stale snapshot over the writer.
async function refreshFundingTracker({ quiet = false, _skipWriterLock = false } = {}) {
  if (!_skipWriterLock && navigator.locks?.request) {
    let acquired = false;
    await navigator.locks.request("xhub-v3.6-funding-tracker-writer", { mode: "exclusive", ifAvailable: true }, async (lock) => {
      if (!lock) return;
      acquired = true;
      await refreshFundingTracker({ quiet, _skipWriterLock: true });
    });
    if (!acquired && !quiet) {
      const state = $("#tracker-sync-state");
      if (state) {
        state.textContent = "跟随另一客户端同步";
        state.className = "tag muted";
      }
    }
    return;
  }
  if (fundingTrackerInFlight || document.body.classList.contains("auth-locked")) return;
  reconcileFundingReservationsFromHistory();
  const trackers = loadFundingTrackers();
  if (!trackers.length) {
    $("#tracker-sync-state").textContent = "没有记录";
    $("#tracker-chain-status").textContent = "未载入";
    renderFundingTrackerTabs();
    return;
  }
  clearError("#funding-tracker-error");
  fundingTrackerInFlight = true;
  const activeFundingCoinId = loadFundingTracker()?.funding_coin_id;
  try {
    for (const tracked of trackers) {
      normalizeLegacySettlementStage(tracked);
      // HUB/DATA is an independent source of protocol heights. Refresh it
      // before the chain RPC so a temporary Coinset error cannot hide them.
      await refreshHubTrackingSnapshot(tracked);
      let status;
      try {
        status = await request("/api/v3.6/wallet-profiles/funding/status", { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify({ rpc_url: "https://api.coinset.org", funding_coin_id: tracked.funding_coin_id, funding_puzzle_hash: tracked.funding_puzzle_hash, funding_amount_mojo: Number(tracked.amount_mojo), required_confirmations: Number(tracked.required_confirmations || 1) }) });
        if (status.status === "MISSING") status = { ...status, status: "MEMPOOL", mempool: true };
      } catch (error) {
        const mempool = isMempoolLookupError(error.message);
        tracked.last_status = { status: mempool ? "MEMPOOL" : "SYNC_ERROR", confirmations: 0, required_confirmations: Number(tracked.required_confirmations || 1), peak_height: 0, confirmed_height: null, error: error.message, ...(mempool ? { mempool: true } : {}) };
        if (tracked.funding_coin_id === activeFundingCoinId) {
          logFundingDiagnostic(tracked, mempool ? "Funding Coin 处于 MEMPOOL" : "Funding Coin 状态查询失败", mempool ? `Coin=${tracked.funding_coin_id} · MEMPOOL · Coin record 尚未可查询，可能仍在 mempool；等待至少 1 个区块确认 · ${error.message}` : `Coin=${tracked.funding_coin_id} · endpoint=/api/v3.6/wallet-profiles/funding/status · ${error.message}`, mempool ? "mempool:coin-record-not-found" : `status-error:${error.message}`);
        }
        continue;
      }
      const confirmedHeight = status.confirmed_height !== null && status.confirmed_height !== undefined && Number.isFinite(Number(status.confirmed_height)) ? Number(status.confirmed_height) : null;
      const peakHeight = Number(status.peak_height);
      const cutoffHeight = tracked.acceptance_cutoff_height !== null && tracked.acceptance_cutoff_height !== undefined && Number.isFinite(Number(tracked.acceptance_cutoff_height)) ? Number(tracked.acceptance_cutoff_height) : confirmedHeight !== null && Number(tracked.termination_blocks || 0) > 0 ? confirmedHeight + Number(tracked.termination_blocks) : null;
      const challengeHeight = tracked.scheduled_close_height !== null && tracked.scheduled_close_height !== undefined && Number.isFinite(Number(tracked.scheduled_close_height)) ? Number(tracked.scheduled_close_height) : cutoffHeight !== null && Number(tracked.challenge_blocks || 0) > 0 ? cutoffHeight + Number(tracked.challenge_blocks) : null;
      tracked.acceptance_cutoff_height = cutoffHeight;
      tracked.scheduled_close_height = challengeHeight;
      tracked.last_status = { ...status, peak_height: peakHeight, confirmed_height: confirmedHeight };
      // When Funding Coin is spent, derive Initial Closing Coin from its
      // actual chain outputs instead of trusting a stale local prediction.
      if (String(status.status).toUpperCase() === "SPENT") {
        let chainFunding = status;
        try { chainFunding = await request("/api/v3.6/wallet-profiles/funding/discover", { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify({ rpc_url: "https://api.coinset.org", funding_coin_id: tracked.funding_coin_id }) }); } catch (_) {}
        const candidates = Array.isArray(chainFunding.children) ? chainFunding.children.filter((child) => Number(child.amount_mojo) === Number(tracked.amount_mojo || 0)) : [];
        if (candidates.length === 1) {
          const actual = String(candidates[0].coin_id || "").toLowerCase();
          if (/^[0-9a-f]{64}$/.test(actual)) tracked.initial_closing_coin_id = actual;
        }
      }
      if (tracked.funding_coin_id === activeFundingCoinId) {
        logFundingDiagnostic(tracked, status.status === "MEMPOOL" ? "Funding Coin 处于 MEMPOOL" : "Funding Coin 链上状态已变化", status.status === "MEMPOOL" ? `Coin=${tracked.funding_coin_id} · MEMPOOL · Coin record 尚未可查询，可能仍在 mempool；等待至少 1 个区块确认 · observed_peak=${peakHeight}` : `Coin=${tracked.funding_coin_id} · status=${status.status} · observed_peak=${peakHeight} · confirmed=${confirmedHeight ?? "-"} · spent=${status.spent_height ?? "-"}`, status.status === "MEMPOOL" ? "mempool:coin-record-not-found" : `funding:${status.status}:${confirmedHeight ?? "-"}:${status.spent_height ?? "-"}`);
      }
      await autoRegisterFunding(tracked);
      try {
        const hub = await request(`/api/v3.6/hub/funding-coins/${tracked.funding_coin_id}/tracking`, { headers: { "x-xhub-protocol-version": "0x0360" } });
        applyHubTrackingSnapshot(tracked, hub);
      } catch {
        // /tracking is absent for a zero-reservation channel even though
        // registration and its Recovery Package are already valid.
        if (!tracked.hub_tracking && !tracked.recovery_package_available && !tracked.auto_registered_at) tracked.hub_tracking = null;
        // A zero-reservation Coin can have a HUB-signed empty-ledger package
        // without a normal tracking summary record.
        try {
          const packageResponse = await request(`/api/v3.6/hub/funding-coins/${tracked.funding_coin_id}/recovery-packages/latest`, { headers: { "x-xhub-protocol-version": "0x0360" } });
          tracked.recovery_package_available = Boolean(packageResponse.recovery_package_canonical_hex);
          tracked.recovery_package_content_hash = packageResponse.recovery_package_content_hash || tracked.recovery_package_content_hash || null;
          if (packageResponse.challenge_blocks) tracked.challenge_blocks = Number(packageResponse.challenge_blocks);
          if (tracked.funding_coin_id === activeFundingCoinId) {
            logFundingDiagnostic(tracked, "HUB Recovery Package 可用", `Coin=${tracked.funding_coin_id} · hash=${tracked.recovery_package_content_hash || "-"} · tracking-summary=404 · settlement-allowed=${tracked.recovery_package_available}`, `recovery:${tracked.recovery_package_content_hash}:${tracked.recovery_package_available}`);
          }
        } catch {
          if (!tracked.auto_registered_at) tracked.recovery_package_available = false;
        }
      }
      // Recovery Package is the portable hand-off artifact. Use it to derive
      // the Initial Closing Coin for imported Funding Coins, including C's
      // first read-only sync where no local settlement fields exist yet.
      try {
        await refreshSettlementDiscovery(tracked);
      } catch (error) {
        if (tracked.funding_coin_id === activeFundingCoinId) {
          logFundingDiagnostic(tracked, "Initial Closing Coin 自动发现等待重试", `Funding Coin=${tracked.funding_coin_id} · ${error.message}`, `settlement-discover-error:${error.message}`);
        }
      }
      await repairClosingDeadline(tracked);
      if (["START_PENDING", "START_SUBMITTED", "FINAL_PENDING", "FINAL_SUBMITTED"].includes(tracked.settlement_stage)) {
        try {
          await refreshPendingSettlement(tracked);
        } catch (error) {
          logFundingDiagnostic(tracked, "开始结算状态核验失败", `SpendBundle=${tracked.settlement_start_spend_bundle_id || "-"} · ${error.message}`, `pending-check-error:${error.message}`);
        }
      }
      if (tracked.settlement_stage === "INITIAL_CLOSING" && tracked.initial_closing_coin_id) {
        try {
          const closing = await request("/api/v3.6/wallet-profiles/funding/discover", { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify({ rpc_url: "https://api.coinset.org", funding_coin_id: tracked.initial_closing_coin_id }) });
          if (closing.status === "FOUND" && closing.confirmed_height != null && closing.spent_height == null) {
            tracked.initial_birth_height = Number(closing.confirmed_height);
            tracked.challenge_deadline_height = tracked.initial_birth_height + Number(tracked.challenge_blocks || 0);
            tracked.settlement_stage = "INITIAL_CLOSING";
            logFundingDiagnostic(tracked, "Initial Closing Coin 已确认", `Coin=${tracked.initial_closing_coin_id} · confirmed=${tracked.initial_birth_height} · challenge_blocks=${tracked.challenge_blocks || 0} · deadline=${tracked.challenge_deadline_height}`, `initial-confirmed:${tracked.initial_birth_height}:${tracked.challenge_deadline_height}`);
          } else if (closing.status === "SPENT" && closing.spent_height != null) {
            tracked.settlement_stage = "SETTLED";
            tracked.settled_height = Number(closing.spent_height);
            tracked.initial_coin_spent_height = Number(closing.spent_height);
            logFundingDiagnostic(tracked, "最终结算已确认上链", `Initial Coin=${tracked.initial_closing_coin_id} · spent_height=${closing.spent_height} · SpendBundle=${tracked.settlement_spend_bundle_id || "已由链上确认"}`, `settled:${closing.spent_height}`);
          }
        } catch (error) {
          logFundingDiagnostic(tracked, "等待开始结算确认", `SpendBundle=${tracked.settlement_start_spend_bundle_id || "-"} · Initial Coin=${tracked.initial_closing_coin_id} · peak=${peakHeight} · query=${error.message}`, `initial-wait:${peakHeight}:${error.message}`);
        }
      }
      if (tracked.settlement_stage === "INITIAL_CLOSING" && tracked.funding_coin_id === activeFundingCoinId) {
        const deadline = Number(tracked.challenge_deadline_height || 0);
        const remaining = Math.max(deadline - peakHeight, 0);
        const challengeState = remaining > 0 ? "waiting" : "ended";
        logFundingDiagnostic(tracked, remaining > 0 ? "结算挑战期开始等待" : "结算挑战期已结束", `Initial Coin=${tracked.initial_closing_coin_id} · birth=${tracked.initial_birth_height} · observed_peak=${peakHeight} · deadline=${deadline} · remaining=${remaining} blocks`, `challenge:${challengeState}:${deadline}`);
      }
    }
    // A Funding Coin can be created or manually added while this async refresh is
    // waiting on chain/HUB requests. Re-read and merge before committing results.
    const latestTrackers = loadFundingTrackers();
    const removed = loadRemovedFundingTrackerIds();
    const liveTrackers = mergeFundingTrackerLists(trackers, latestTrackers).filter((item) => !removed.has(item.funding_coin_id));
    localStorage.setItem(walletScopedKey(FUNDING_TRACKER_KEY), JSON.stringify(liveTrackers));
    persistFundingTrackers(liveTrackers);
    renderFundingTrackerTabs();
    renderActiveFundingTracker();
    const activeAfterSync = loadFundingTracker();
    if (activeAfterSync?.last_status?.status === "MEMPOOL") {
      $("#tracker-sync-state").textContent = "MEMPOOL · 等待上链";
      $("#tracker-sync-state").className = "tag warn";
    } else {
      $("#tracker-sync-state").textContent = `${new Date().toLocaleTimeString("zh-CN", { hour12: false })} 已同步 ${trackers.length} 个 Coin`;
      $("#tracker-sync-state").className = "tag good";
    }
    if (!quiet) addActivity("Funding Coin 状态已同步", `${trackers.length} 个 Coin`);
  } catch (error) {
    $("#tracker-sync-state").textContent = "同步失败";
    $("#tracker-sync-state").className = "tag warn";
    $("#tracker-confirmations").textContent = error.message;
    if (!quiet) addActivity("Funding Coin 同步失败", error.message);
  } finally { fundingTrackerInFlight = false; }
}

async function syncChainState({ quiet = false } = {}) {
  if (chainSyncInFlight || document.body.classList.contains("auth-locked") || !selectedWalletProfile) return;
  chainSyncInFlight = true;
  try {
    const result = await request("/api/v3.6/wallet-profiles/chain/sync", { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify({ role: selectedWalletProfile.role, rpc_url: "https://api.coinset.org" }) });
    const balance = Number(result.confirmed_balance_mojo || 0);
    currentPeakHeight = result.peak_height == null ? null : Number(result.peak_height);
    const topbarPeak = $("#topbar-peak-height");
    if (topbarPeak) topbarPeak.textContent = `区块高度：${currentPeakHeight == null ? "-" : currentPeakHeight.toLocaleString("en-US")}`;
    $("#overview-funding").textContent = `${balance.toLocaleString("en-US")} mojo`;
    $("#overview-funding-detail").textContent = `当前地址 · ${result.unspent_coin_count ?? 0} 个未花费 Coin · 主网高度 ${result.peak_height ?? "-"}`;
    const metricLabel = $("[data-view-panel='overview'] .metric-card:first-child .metric-label");
    if (metricLabel) metricLabel.textContent = "当前地址余额";
    chainHistoryRows = (Array.isArray(result.history) ? result.history : []).map((entry) => ({ kind: "onchain", status: entry.status, amount: String(entry.amount_mojo), id: entry.coin_id, timestamp: Number(entry.timestamp || 0), time: formatSyncTime(entry.timestamp), confirmed_height: entry.confirmed_height, spent_height: entry.spent_height, coinbase: entry.coinbase === true }));
    renderHistory();
    $("#history-sync-state").textContent = `${chainHistoryRows.length} 条链上 Coin · ${new Date().toLocaleTimeString("zh-CN", { hour12: false })} 更新`;
    refreshFundingTracker({ quiet: true });
    if (!quiet) addActivity("余额和交易历史已同步", `${balance} mojo · ${chainHistoryRows.length} 条 Coin`);
  } catch (error) {
    $("#overview-funding-detail").textContent = `同步失败：${error.message}`;
    $("#history-sync-state").textContent = "链上同步失败，稍后自动重试";
    if (!quiet) addActivity("链上同步失败", error.message);
  } finally {
    chainSyncInFlight = false;
  }
}

function startChainSync() {
  if (chainSyncTimer === null) chainSyncTimer = setInterval(() => syncChainState({ quiet: true }), CHAIN_SYNC_INTERVAL_MS);
  syncChainState({ quiet: true });
}

function escapeHtml(value) {
  return String(value).replace(/[&<>"']/g, (character) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" })[character]);
}

async function copyTextToClipboard(text) {
  const value = String(text || "");
  if (!value) throw new Error("没有可复制的文本。");
  if (navigator.clipboard?.writeText) {
    try {
      await navigator.clipboard.writeText(value);
      return;
    } catch (_) { /* xhub:// WebView may expose Clipboard API without permission */ }
  }
  const helper = document.createElement("textarea");
  helper.value = value;
  helper.setAttribute("readonly", "");
  helper.style.position = "fixed";
  helper.style.left = "-9999px";
  helper.style.top = "0";
  document.body.appendChild(helper);
  try {
    helper.focus();
    helper.select();
    helper.setSelectionRange(0, value.length);
    if (!document.execCommand("copy")) throw new Error("当前 WebView 不支持系统复制");
  } finally {
    helper.remove();
  }
}

async function request(url, options = {}) {
  const timeoutMs = Number.isFinite(Number(options.timeoutMs)) ? Number(options.timeoutMs) : 45_000;
  const controller = new AbortController();
  const timeoutId = setTimeout(() => controller.abort(), timeoutMs);
  const { timeoutMs: _ignoredTimeoutMs, ...fetchOptions } = options;
  let response;
  try {
    response = await fetch(url, { ...fetchOptions, signal: controller.signal });
  } catch (error) {
    if (error?.name === "AbortError") throw new Error(`请求超时（${timeoutMs}ms）：${url}`);
    throw error;
  } finally {
    clearTimeout(timeoutId);
  }
  const rawBody = await response.text();
  let body = {};
  try { body = rawBody ? JSON.parse(rawBody) : {}; } catch (_) { body = {}; }
  if (!response.ok) {
    const code = body.code || body.error?.code;
    if (code === "RECOVERY_PACKAGE_NOT_FOUND") {
      throw new Error("HUB 尚未生成可验证的 Recovery Package。请先提交一笔有效链下预扣；如果当前确实是 0 预扣，请重启到最新 Wallet 后再试。" );
    }
    if (response.status === 404 && url.includes("wallet-profiles/settlement")) {
      throw new Error("当前 Wallet 程序版本没有结算接口。请关闭旧窗口后，重新打开最新的 Gateway-v2。" );
    }
    const detail = body.message || body.error?.message || rawBody.trim();
    throw new Error(`${detail || `请求失败（HTTP ${response.status}）`} [${response.status} ${url}]`);
  }
  return body;
}

async function loadBuildInfo() {
  const labels = $$(".build-time");
  try {
    const build = await request("/api/v3.6/build");
    const buildUnix = Number(build.build_unix);
    if (!Number.isFinite(buildUnix) || buildUnix <= 0) throw new Error("构建时间无效");
    const buildTime = new Intl.DateTimeFormat("zh-CN", {
      timeZone: "Asia/Shanghai",
      year: "numeric",
      month: "2-digit",
      day: "2-digit",
      hour: "2-digit",
      minute: "2-digit",
      second: "2-digit",
      hour12: false,
    }).format(new Date(buildUnix * 1000));
    labels.forEach((label) => { label.textContent = `代码构建时间：${buildTime}（北京时间）`; });
  } catch (_) {
    labels.forEach((label) => { label.textContent = "代码构建时间：无法读取"; });
  }
}

function selectedRole() {
  const role = sessionStorage.getItem("xhub-v36-wallet-role");
  return selectedWalletProfile?.role || role || "address";
}

function renderWalletProfiles() {
  const savedRole = sessionStorage.getItem("xhub-v36-wallet-role");
  selectedWalletProfile = walletProfiles.find((item) => item.role === savedRole) || walletProfiles[0] || null;
  const target = $("#saved-wallets");
  if (!walletProfiles.length) {
    target.innerHTML = '<div class="wallets-empty"><strong>还没有钱包</strong><span>创建新钱包或导入 24 词助记词。</span></div>';
  } else {
    target.innerHTML = walletProfiles.map((item) => `<button class="saved-wallet-card" type="button" data-wallet-role="${escapeHtml(item.role)}"><span class="saved-wallet-avatar">X</span><span class="saved-wallet-main"><strong>${escapeHtml(item.name)}</strong><small>地址钱包 · ${escapeHtml(item.fingerprint)}</small><code>${escapeHtml(item.address.slice(0, 20))}…</code></span><span class="saved-wallet-enter">进入</span></button>`).join("");
    $$(".saved-wallet-card").forEach((button) => button.addEventListener("click", () => enterSavedWallet(button.dataset.walletRole)));
  }
  updateReceiveIdentity();
}

async function enterSavedWallet(role) {
  selectedWalletProfile = walletProfiles.find((item) => item.role === role) || null;
  if (!selectedWalletProfile) return;
  try {
    await request("/api/v3.6/wallet-profiles/unlock", { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify({ role }) });
    $("#login-error").hidden = true;
    unlockWorkspace();
  } catch (error) { showError("#login-error", error.message); }
}

function updateReceiveIdentity() {
  const profile = selectedWalletProfile;
  $("#receive-address").textContent = profile?.address || "请先解锁地址钱包";
  $("#receive-role-label").textContent = profile ? `当前地址 · ${profile.name}` : "尚未解锁地址钱包";
  $("#receive-public-key").textContent = profile?.wallet_public_key || "-";
  $("#receive-fingerprint").textContent = profile?.fingerprint || "-";
  $("#copy-receive-address").disabled = !profile?.address;
  $("#overview-address").textContent = profile?.address || "-";
  $("#overview-public-key").textContent = profile?.wallet_public_key || "-";
  $("#overview-puzzle-hash").textContent = profile?.puzzle_hash || "-";
  $("#overview-fingerprint").textContent = profile?.fingerprint || "-";
  const walletLabel = $("#topbar-wallet-label");
  if (walletLabel) walletLabel.textContent = profile ? `${profile.name || "未命名钱包"} · ${profile.fingerprint || "-"}` : "未登录钱包";
  $("#copy-overview-address").disabled = !profile?.address;
  hideSensitiveWalletInfo();
}

function hideSensitiveWalletInfo() {
  if (sensitiveHideTimer !== null) { clearTimeout(sensitiveHideTimer); sensitiveHideTimer = null; }
  if ($("#overview-mnemonic")) $("#overview-mnemonic").textContent = "•••••••• •••••••• ••••••••";
  if ($("#overview-private-key")) $("#overview-private-key").textContent = "••••••••••••••••••••••••••••••••";
  if ($("#sensitive-state")) { $("#sensitive-state").textContent = "敏感信息已遮蔽"; $("#sensitive-state").className = "tag neutral"; }
  if ($("#reveal-sensitive")) $("#reveal-sensitive").hidden = false;
  if ($("#hide-sensitive")) $("#hide-sensitive").hidden = true;
}

function showView(view) {
  $$('.nav-item').forEach((item) => item.classList.toggle('active', item.dataset.view === view));
  $$('.view').forEach((panel) => panel.classList.toggle('active', panel.dataset.viewPanel === view));
}

async function loadWalletProfiles() {
  try {
    const result = await request("/api/v3.6/wallet-profiles");
    walletProfiles = Array.isArray(result.profiles) ? result.profiles : [];
    renderWalletProfiles();
  } catch (error) {
    walletProfiles = [];
    renderWalletProfiles();
    showError("#login-error", `钱包档案载入失败：${error.message}`);
  }
}

function openProfileDialog(mode) {
  profileDialogMode = mode;
  $("#profile-dialog-title").textContent = mode === "create" ? "创建钱包档案" : "导入钱包档案";
  $("#profile-form-submit").textContent = mode === "create" ? "创建到本次会话" : "导入到本次会话";
  $("#profile-mnemonic-field").hidden = mode !== "import";
  $("#profile-mnemonic").required = mode === "import";
  $("#profile-mnemonic").value = "";
  $("#profile-form-role").value = "address";
  $("#profile-form-name").value = mode === "create" ? "V3.6 Address Wallet" : "";
  $("#profile-form-error").hidden = true;
  $("#profile-dialog").hidden = false;
  $("#profile-form-name").focus();
}

function closeProfileDialog() {
  $("#profile-dialog").hidden = true;
}

function showBackupPhrase(phrase) {
  $("#backup-phrase").value = phrase;
  $("#backup-confirm").checked = false;
  $("#backup-close").disabled = true;
  $("#backup-dialog").hidden = false;
}

function values() {
  const data = Object.fromEntries(new FormData(form).entries());
  const formValue = (name) => {
    const value = form.elements[name]?.value;
    return typeof value === "string" ? value.trim() : "";
  };
  // Locked drafts disable their controls, and disabled controls are omitted by
  // FormData. Always bind a new draft to the selected wallet and current HUB
  // profile instead of inheriting the previous form's disabled state.
  const userPublicKey = selectedWalletProfile?.wallet_public_key || formValue("user_public_key");
  const remainderPuzzleHash = selectedWalletProfile?.puzzle_hash || formValue("user_remainder_puzzle_hash");
  if (userPublicKey) data.user_public_key = userPublicKey;
  if (remainderPuzzleHash) data.user_remainder_puzzle_hash = remainderPuzzleHash;
  for (const name of ["hub_state_public_key_a", "state_rules_hash"]) {
    const value = formValue(name);
    if (value) data[name] = value;
  }
  // The network field is readonly and can be empty during the first profile
  // refresh. Funding drafts must always carry the canonical mainnet ID.
  if (!data.network_id) {
    data.network_id = formValue("network_id") || profile?.network_id || "ccd5bb71183532bff220ba46c268991a3ff07eb358e8255a65c30a2dce0e5fbb";
  }
  // Do not rely on FormData to retain hidden/readonly timing inputs. When a
  // Funding WC is imported, the HUB request is the authoritative source for
  // these required fields and must be copied explicitly into the draft body.
  if (importedWcRequest?.request_type === "funding_request") {
    const termination = BigInt(importedWcRequest.termination_blocks);
    const freeze = BigInt(importedWcRequest.freeze_blocks);
    data.acceptance_blocks = (termination - freeze).toString();
    data.freeze_blocks = importedWcRequest.freeze_blocks;
    data.challenge_blocks = importedWcRequest.challenge_blocks;
    data.funding_amount = importedWcRequest.funding_amount;
  }
  return data;
}

function updateTiming() {
  const data = values();
  const acceptance = /^\d+$/.test(data.acceptance_blocks) ? BigInt(data.acceptance_blocks) : null;
  const freeze = /^\d+$/.test(data.freeze_blocks) ? BigInt(data.freeze_blocks) : null;
  const close = acceptance !== null && freeze !== null ? acceptance + freeze : null;
  $("#close-delay").textContent = close === null ? "-" : close.toString();
  $("#summary-close").textContent = close === null ? "-" : `${close} blocks`;
  if (activeDraft && !activeDraft.confirmed) resetPreview();
}

function resetPreview() {
  activeDraft = null;
  $("#summary-state").textContent = "等待重新校验";
  $("#terms-hash").textContent = "校验后生成";
  $("#funding-address").textContent = "校验后生成";
  $("#funding-puzzle-hash").textContent = "校验后生成";
  $("#canonical-hex").textContent = "校验后生成";
  $("#confirm-check").checked = false;
  $("#confirm-check").disabled = true;
  $("#confirm-button").disabled = true;
  if ($("#one-click-funding")) $("#one-click-funding").disabled = !importedWcRequest;
}

function showError(selector, message) {
  const target = $(selector);
  target.textContent = message;
  target.hidden = false;
}

function clearError(selector) {
  const target = $(selector);
  target.hidden = true;
  target.textContent = "";
}

function setGatewayState(label, healthy) {
  $("#gateway-label").textContent = label;
  $("#network-dot").classList.toggle("pending", !healthy);
  $("#network-dot").classList.toggle("error", !healthy);
}

function lockForm() {
  form.querySelectorAll("input, textarea, button").forEach((element) => { element.disabled = true; });
  $("#confirm-check").disabled = true;
  $("#confirm-button").disabled = true;
  $("#confirm-button").textContent = "条款已确认并锁定";
  $("#summary-state").textContent = "条款已锁定";
  $("#lock-state").textContent = "已确认";
  $("#lock-state").className = "tag good";
  $("#funding-lock-tag").textContent = "已锁定";
  $("#funding-lock-tag").className = "tag good";
}

function beginNewFundingDraft() {
  if (!activeDraft?.confirmed) return;
  activeDraft = null;
  form.querySelectorAll("input, textarea, button").forEach((element) => { element.disabled = false; });
  $("#prepare-button").disabled = !profile;
  $("#confirm-check").checked = false;
  $("#confirm-check").disabled = true;
  $("#confirm-button").disabled = true;
  $("#confirm-button").textContent = "确认并锁定条款";
  $("#summary-state").textContent = "等待校验";
  $("#lock-state").textContent = "草稿";
  $("#lock-state").className = "tag neutral";
  $("#funding-lock-tag").textContent = "草稿";
  $("#funding-lock-tag").className = "tag neutral";
  const oneClickFunding = $("#one-click-funding");
  if (oneClickFunding) {
    delete oneClickFunding.dataset.broadcasted;
    oneClickFunding.disabled = true;
    oneClickFunding.textContent = "确认并广播 Funding Coin";
  }
  window.__xhubPreparedFunding = null;
}

function installFundingChainUi() {
  const summary = $("[data-view-panel='funding'] .summary-panel");
  if (!summary || $("#prepare-funding-chain")) return;
  summary.insertAdjacentHTML("beforeend", `<div id="funding-chain-actions" class="chain-actions"><div class="section-divider"></div><div class="panel-heading compact-heading"><div><h2>链上 Funding Coin</h2><p>条款锁定后，先在本地构造并签名；广播仍需最终确认。</p></div><span id="funding-chain-state" class="tag neutral">未构造</span></div><label>Fee（mojo）<input id="funding-fee" inputmode="numeric" value="1"></label><button id="prepare-funding-chain" class="button primary full-button" type="button" disabled>构造并验证 SpendBundle</button><div id="funding-chain-preview" class="review-box">等待已锁定的 Funding 条款。</div><button id="broadcast-funding-chain" class="button confirm full-button" type="button" disabled>最终确认并广播</button><div class="notice-box">构造步骤会读取用户钱包未花费 Coin 并本地签名，但不会广播。只有最终弹窗选择“是”才会调用 push_tx。</div></div>`);
  $("#funding-chain-actions").insertAdjacentHTML("afterbegin", '<button id="one-click-funding" class="button confirm full-button" type="button" disabled>确认并广播 Funding Coin</button><p class="panel-footnote">粘贴 WC 后，点击一次即本地签名并提交主网；不会在粘贴时自动广播。</p>');
  $("#prepare-funding-chain").hidden = true;
  $("#broadcast-funding-chain").hidden = true;
  $("#one-click-funding").addEventListener("click", async () => {
    const button = $("#one-click-funding");
    if (!activeDraft || !importedWcRequest || !selectedWalletProfile) return;
    clearError("#funding-error");
    button.disabled = true;
    button.textContent = "正在校验、签名并广播...";
    try {
      if (!activeDraft.confirmed) {
        const locked = await request(`/api/v3.6/funding-drafts/${activeDraft.draft_id}/confirm`, { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify({ protocol_version: "0x0360", channel_terms_hash: activeDraft.preview.channel_terms_hash, user_confirmed: true }) });
        renderDraft(locked);
        addActivity("Funding 条款已锁定", "一键广播流程已完成条款确认");
      }
      const fee = $("#funding-fee").value.trim();
      if (!/^\d+$/.test(fee)) throw new Error("Fee 必须是非负整数 mojo。");
      const prepared = await request("/api/v3.6/wallet-profiles/funding/prepare", { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify({ role: selectedWalletProfile.role, rpc_url: "https://api.coinset.org", wallet_service_url: "https://wallet.chiagame.top", hub_base_url: "https://hub.chiagame.top", fee_mojo: Number(fee), confirmed_draft: activeDraft, hub_request_id: importedWcRequest.request_id || "", wc_request: $("#wc-request-input").value.trim() }) });
      window.__xhubPreparedFunding = prepared;
      const coinId = prepared.funding?.predicted_funding_coin_id || "-";
      $("#funding-chain-state").textContent = "已签名，正在广播";
      $("#funding-chain-state").className = "tag warn";
      $("#funding-chain-preview").textContent = `状态：已本地签名，准备广播\nFunding Coin ID（预测）：${coinId}\n金额：${prepared.amount_mojo} mojo\n费用：${prepared.fee_mojo} mojo\nSpendBundle ID：${prepared.spend_bundle_id}`;
      const pendingTracker = { schema: "xhub.wallet.funding-tracker.v1", funding_coin_id: coinId, funding_puzzle_hash: prepared.destination_puzzle_hash, creator_puzzle_hash: prepared.source_puzzle_hash || selectedWalletProfile?.puzzle_hash || null, tracking_wallet_puzzle_hash: selectedWalletProfile?.puzzle_hash || null, imported: false, hub_request_id: importedWcRequest.request_id || null, confirmed_draft: activeDraft, amount_mojo: Number(prepared.amount_mojo), reserved_mojo: 0, termination_blocks: Number(importedWcRequest.termination_blocks), challenge_blocks: Number(importedWcRequest.challenge_blocks), required_confirmations: Number(prepared.funding?.required_confirmations || profile?.funding_confirmation_blocks || 1), spend_bundle_id: prepared.spend_bundle_id, broadcast_status: "SUBMITTING", created_at: new Date().toISOString() };
      saveFundingTracker(pendingTracker);
      const result = await request("/api/v3.6/wallet-profiles/funding/broadcast", { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify({ confirmation_token: prepared.confirmation_token, spend_bundle_id: prepared.spend_bundle_id, funding_coin_id: coinId, confirm_broadcast: true }) });
      const broadcastStatus = result.status || "SUCCESS";
      const actualCoinId = result.funding_coin_id || coinId;
      saveFundingTracker({ ...pendingTracker, funding_coin_id: actualCoinId, spend_bundle_id: result.spend_bundle_id || prepared.spend_bundle_id, broadcast_status: broadcastStatus });
      $("#funding-chain-state").textContent = broadcastStatus.toUpperCase() === "PENDING" ? "MEMPOOL · 等待上链" : "已广播";
      $("#funding-chain-state").className = broadcastStatus.toUpperCase() === "PENDING" ? "tag warn" : "tag good";
      $("#funding-chain-preview").textContent += `\n\n主网提交：${broadcastStatus}\nFunding Coin ID：${actualCoinId}\nSpendBundle ID：${result.spend_bundle_id || prepared.spend_bundle_id}`;
      addActivity("Funding Coin 已广播", `${actualCoinId} · push_tx=true`, actualCoinId);
      window.__xhubPreparedFunding = null;
      button.dataset.broadcasted = "true";
      button.disabled = true;
      button.textContent = "Funding Coin 已广播";
      refreshFundingTracker({ quiet: true });
    } catch (error) {
      showError("#funding-error", `一键广播失败或结果不确定：${error.message}。已保留本地追踪信息，请勿重复提交。`);
    } finally {
      if (button.dataset.broadcasted !== "true") {
        button.disabled = false;
        button.textContent = "再次确认并广播 Funding Coin";
      }
    }
  });
  $("#prepare-funding-chain").addEventListener("click", async () => {
    if (!activeDraft?.confirmed || !importedWcRequest || !selectedWalletProfile) return;
    clearError("#funding-error");
    const fee = $("#funding-fee").value.trim();
    if (!/^\d+$/.test(fee)) { showError("#funding-error", "费用必须是非负整数 mojo。"); return; }
    const button = $("#prepare-funding-chain");
    button.disabled = true;
    button.textContent = "正在本地构造和验证...";
    try {
      const prepared = await request("/api/v3.6/wallet-profiles/funding/prepare", { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify({ role: selectedWalletProfile.role, rpc_url: "https://api.coinset.org", wallet_service_url: "https://wallet.chiagame.top", hub_base_url: "https://hub.chiagame.top", fee_mojo: Number(fee), confirmed_draft: activeDraft, hub_request_id: importedWcRequest.request_id || "", wc_request: $("#wc-request-input").value.trim() }) });
      window.__xhubPreparedFunding = prepared;
      $("#funding-chain-state").textContent = "已签名，未广播";
      $("#funding-chain-state").className = "tag warn";
      $("#funding-chain-preview").textContent = `状态：SpendBundle 已本地签名验证\nFunding Coin ID（预测）：${prepared.funding?.predicted_funding_coin_id || "-"}\nFunding 地址：${prepared.destination_address || "-"}\n金额：${prepared.amount_mojo} mojo\n费用：${prepared.fee_mojo} mojo\n输入合计：${prepared.input_total_mojo} mojo\n找零：${prepared.change_mojo} mojo\nSpendBundle ID：${prepared.spend_bundle_id}\n本地验证：consensus=${prepared.consensus_conditions_verified} / signature=${prepared.aggregate_signature_verified}\n广播：false`;
      $("#broadcast-funding-chain").disabled = false;
      addActivity("Funding SpendBundle 已本地签名", `${prepared.amount_mojo} mojo · 广播=false`, prepared.funding?.predicted_funding_coin_id || null);
    } catch (error) { showError("#funding-error", `构造 Funding SpendBundle 失败：${error.message}`); }
    finally { button.disabled = false; button.textContent = "重新构造并验证 SpendBundle"; }
  });
  $("#broadcast-funding-chain").addEventListener("click", async () => {
    const prepared = window.__xhubPreparedFunding;
    if (!prepared) return;
    const coinId = prepared.funding?.predicted_funding_coin_id || "-";
    const message = `即将向 Chia Mainnet 提交 Funding Coin。\n\n金额：${prepared.amount_mojo} mojo\nFunding Coin ID（预测）：${coinId}\nSpendBundle ID：${prepared.spend_bundle_id}\n费用：${prepared.fee_mojo} mojo\n\n只有选择“确定”才会调用 push_tx。该操作不可撤销。`;
    if (!window.confirm(message)) { addActivity("已取消 Funding 广播", "没有调用 push_tx"); return; }
    const button = $("#broadcast-funding-chain");
    button.disabled = true;
    const pendingTracker = {
      schema: "xhub.wallet.funding-tracker.v1",
      funding_coin_id: coinId,
      funding_puzzle_hash: prepared.destination_puzzle_hash,
      creator_puzzle_hash: prepared.source_puzzle_hash || selectedWalletProfile?.puzzle_hash || null,
      tracking_wallet_puzzle_hash: selectedWalletProfile?.puzzle_hash || null,
      imported: false,
      hub_request_id: importedWcRequest.request_id || null,
      confirmed_draft: activeDraft,
      amount_mojo: Number(prepared.amount_mojo),
      reserved_mojo: 0,
      termination_blocks: Number(importedWcRequest.termination_blocks),
      challenge_blocks: Number(importedWcRequest.challenge_blocks),
      required_confirmations: Number(prepared.funding?.required_confirmations || profile?.funding_confirmation_blocks || 1),
      spend_bundle_id: prepared.spend_bundle_id,
      broadcast_status: "SUBMITTING",
      created_at: new Date().toISOString(),
    };
    // Persist before push_tx. An RPC timeout can hide a successful broadcast,
    // so chain polling must retain enough public metadata to recover it.
    saveFundingTracker(pendingTracker);
    try {
      const result = await request("/api/v3.6/wallet-profiles/funding/broadcast", { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify({ confirmation_token: prepared.confirmation_token, spend_bundle_id: prepared.spend_bundle_id, funding_coin_id: coinId, confirm_broadcast: true }) });
      const broadcastStatus = result.status || "SUCCESS";
      const pending = broadcastStatus.toUpperCase() === "PENDING";
      $("#funding-chain-state").textContent = pending ? "已提交，等待确认" : "已广播";
      $("#funding-chain-state").className = pending ? "tag warn" : "tag good";
      $("#funding-chain-preview").textContent += `\n\n主网提交：${broadcastStatus}${pending ? "（不是失败，等待 RPC/主网确认）" : ""}\nFunding Coin ID：${result.funding_coin_id || coinId}\nSpendBundle ID：${result.spend_bundle_id || prepared.spend_bundle_id}`;
      saveFundingTracker({
        ...pendingTracker,
        funding_coin_id: result.funding_coin_id || coinId,
        spend_bundle_id: result.spend_bundle_id || prepared.spend_bundle_id,
        broadcast_status: broadcastStatus,
      });
      window.__xhubPreparedFunding = null;
      addActivity(pending ? "Funding Coin 已提交，等待确认" : "Funding Coin 已广播", `${result.funding_coin_id || coinId} · push_tx=true`, result.funding_coin_id || coinId);
      refreshFundingTracker({ quiet: true });
    } catch (error) {
      saveFundingTracker({ ...pendingTracker, broadcast_status: "UNKNOWN", broadcast_error: error.message });
      refreshFundingTracker({ quiet: true });
      showError("#funding-error", `广播结果不确定或失败：${error.message}。已保存待确认追踪器，钱包会按 Coin ID 自动查询，禁止重复提交。`);
    }
  });
}

async function loadProfile() {
  try {
    profile = await request("/api/v3.6/config");
    form.elements.network_id.value = profile.network_id;
    form.elements.acceptance_blocks.value = profile.acceptance_blocks;
    form.elements.freeze_blocks.value = profile.freeze_blocks;
    form.elements.challenge_blocks.value = profile.challenge_blocks;
    form.elements.hub_state_public_key_a.value = profile.hub_state_public_key_a;
    form.elements.state_rules_hash.value = profile.state_rules_hash;
    $("#network-label").textContent = "Chia Mainnet · V3.6";
    $("#confirmation-blocks").textContent = `${profile.funding_confirmation_blocks} blocks`;
    $("#delivery-threshold").textContent = `${profile.delivery_threshold}-of-${profile.delivery_participants}`;
    $("#prepare-button").disabled = false;
    updateTiming();
    addActivity("主网配置已载入", `profile ${profile.profile_id}`);
    startChainSync();
  } catch (error) {
    $("#network-label").textContent = "主网配置载入失败";
    setGatewayState("HUB 网关不可用", false);
    showError("#funding-error", error.message);
    return;
  }
  try {
    const hub = await request("/api/v3.6/hub/health");
    const ready = hub.status === "READY";
    setGatewayState(ready ? "HUB 网关 READY" : "HUB 网关异常", ready);
    addActivity("HUB 网关状态已检查", ready ? "READY" : "异常");
  } catch (error) {
    setGatewayState("HUB 网关不可用", false);
    showError("#funding-error", `HUB 网关：${error.message}`);
  }
}

function renderDraft(draft) {
  activeDraft = draft;
  const preview = draft.preview;
  $("#summary-state").textContent = draft.confirmed ? "条款已锁定" : "校验通过";
  $("#terms-hash").textContent = preview.channel_terms_hash;
  $("#funding-address").textContent = preview.funding_address;
  $("#funding-puzzle-hash").textContent = preview.funding_puzzle_hash;
  $("#canonical-hex").textContent = preview.channel_terms_canonical_hex;
  $("#confirm-check").disabled = draft.confirmed;
  $("#confirm-button").disabled = draft.confirmed || !$("#confirm-check").checked;
  const prepareFundingChain = $("#prepare-funding-chain");
  if (prepareFundingChain) prepareFundingChain.disabled = !draft.confirmed;
  const oneClickFunding = $("#one-click-funding");
  if (oneClickFunding) oneClickFunding.disabled = !importedWcRequest;
  if (draft.confirmed) {
    lockForm();
  } else {
    $("#lock-state").textContent = "待确认";
    $("#lock-state").className = "tag neutral";
    $("#funding-lock-tag").textContent = "待确认";
    $("#funding-lock-tag").className = "tag neutral";
  }
}

form.addEventListener("input", updateTiming);
form.addEventListener("submit", async (event) => {
  event.preventDefault();
  clearError("#funding-error");
  clearError("#funding-tracker-error");
  if (!importedWcRequest) {
    showError("#funding-error", "请先粘贴并解析 HUB 生成的 wc1... 请求码；金额和区块高度由 HUB 提供。");
    return;
  }
  $("#prepare-button").disabled = true;
  $("#prepare-button").textContent = "正在校验...";
  try {
    const draft = await request("/api/v3.6/funding-drafts", { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify({ protocol_version: "0x0360", ...values() }) });
    renderDraft(draft);
    addActivity("Funding 条款校验通过", `terms ${draft.preview.channel_terms_hash.slice(0, 12)}…`);
  } catch (error) {
    showError("#funding-error", error.message);
  } finally {
    if (!activeDraft?.confirmed && profile) $("#prepare-button").disabled = false;
    $("#prepare-button").textContent = "校验并生成条款";
  }
});

$("#confirm-check").addEventListener("change", () => { $("#confirm-button").disabled = !$("#confirm-check").checked || !activeDraft || activeDraft.confirmed; });
$("#confirm-button").addEventListener("click", async () => {
  if (!activeDraft || activeDraft.confirmed) return;
  clearError("#funding-error");
  $("#confirm-button").disabled = true;
  $("#confirm-button").textContent = "正在锁定...";
  try {
    const draft = await request(`/api/v3.6/funding-drafts/${activeDraft.draft_id}/confirm`, { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify({ protocol_version: "0x0360", channel_terms_hash: activeDraft.preview.channel_terms_hash, user_confirmed: true }) });
    renderDraft(draft);
    addActivity("Funding 条款已锁定", "参数不可修改；尚未创建链上交易");
  } catch (error) {
    showError("#funding-error", error.message);
    $("#confirm-button").disabled = false;
    $("#confirm-button").textContent = "确认并锁定条款";
  }
});

function parseSignedReservation() {
  const raw = $("#signed-request").value.trim();
  if (!raw) throw new Error("请先导入本地签名器生成的 JSON");
  const value = JSON.parse(raw);
  const required = ["protocol_version", "request_id", "funding_coin_id", "merchant_puzzle_hash", "merchant_receipt_public_key", "amount", "reservation_nonce", "user_authorization_signature"];
  for (const field of required) if (typeof value[field] !== "string" || !value[field]) throw new Error(`签名请求缺少字段：${field}`);
  if (value.protocol_version !== "0x0360") throw new Error("只接受 V3.6 协议请求");
  if (value.user_authorization_signature === "null") throw new Error("请求尚未完成用户签名");
  return value;
}

$("#preview-reservation").addEventListener("click", () => {
  clearError("#reservation-error");
  try {
    signedReservation = parseSignedReservation();
    $("#reservation-preview").hidden = false;
    $("#reservation-preview").textContent = `Funding Coin: ${signedReservation.funding_coin_id}\n金额: ${signedReservation.amount} mojo\nReservation nonce: ${signedReservation.reservation_nonce}\nAuthorization hash: ${signedReservation.authorization_hash || "由 HUB 校验"}\n用户签名: 已存在\nSpendBundle: false\n广播: false`;
    $("#submit-reservation").disabled = false;
    $("#status-coin").value = signedReservation.funding_coin_id;
    $("#status-nonce").value = signedReservation.reservation_nonce;
    $("#status-reservation").disabled = false;
    addActivity("签名预扣请求已预览", `${signedReservation.amount} mojo · 未提交`);
  } catch (error) {
    signedReservation = null;
    $("#submit-reservation").disabled = true;
    showError("#reservation-error", error.message);
  }
});

$("#submit-reservation").addEventListener("click", async () => {
  if (!signedReservation) return;
  clearError("#reservation-error");
  if (!window.confirm("即将向 HUB 提交链下预扣。此操作不创建 SpendBundle、不调用 push_tx、不广播主网。是否继续？")) return;
  $("#submit-reservation").disabled = true;
  try {
    const hubRequest = hubReservationPayload(signedReservation);
    const result = await request("/api/v3.6/hub/reservations", { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify(hubRequest) });
    $("#reservation-status").textContent = `HUB 状态：${result.status}\nledger_written=${result.ledger_written}\nstate_sequence=${result.state_sequence}\n广播=false`;
    $("#overview-reservation").textContent = result.status || "已提交";
    $("#overview-reservation-detail").textContent = `state_sequence=${result.state_sequence ?? "-"} · 广播=false`;
    recordHistory({ kind: "offchain", status: result.status || "SIGNED", amount: signedReservation.amount, id: signedReservation.reservation_nonce });
      const fundingCoinId = signedReservation.funding_coin_id.replace(/^0x/, "").toLowerCase();
      if (result.ledger_written) {
        recordFundingReservation({ fundingCoinId, requestId: signedReservation.request_id, reservationNonce: signedReservation.reservation_nonce, amount: signedReservation.amount, merchantPuzzleHash: signedReservation.merchant_puzzle_hash, result });
        const tracker = findReservationTracker(signedReservation.request_id, fundingCoinId) || { request_id: signedReservation.request_id, settlement_mode: "DIRECT_MERCHANT", direct_settlement_confirmed: true };
        saveReservationTracker({ ...tracker, status: String(result.status || "SIGNED").toUpperCase(), amount: String(signedReservation.amount), funding_coin_id: fundingCoinId, merchant_puzzle_hash: signedReservation.merchant_puzzle_hash, reservation_nonce: signedReservation.reservation_nonce, ledger_written: true, state_sequence: result.state_sequence, authorized_at: new Date().toISOString() });
        refreshFundingTracker({ quiet: true });
      }
    addActivity("链下预扣已提交 HUB", `${result.status} · ledger_written=${result.ledger_written}`);
  } catch (error) {
    showError("#reservation-error", `提交失败；未广播：${error.message}`);
    $("#submit-reservation").disabled = false;
  }
});

$("#status-reservation").addEventListener("click", async () => {
  const coin = $("#status-coin").value.trim();
  const nonce = $("#status-nonce").value.trim();
  if (!/^(0x)?[0-9a-fA-F]{64}$/.test(coin) || !/^(0x)?[0-9a-fA-F]{64}$/.test(nonce)) { $("#reservation-status").textContent = "Coin ID 和 nonce 必须是 32 字节十六进制。"; return; }
  try {
    const result = await request(`/api/v3.6/hub/funding-coins/${coin.replace(/^0x/, "")}/reservations/${nonce.replace(/^0x/, "")}?protocol_version=0x0360`);
    $("#reservation-status").textContent = `HUB 状态：${result.status}\nledger_written=${result.ledger_written}\nstate_sequence=${result.state_sequence}\n广播=false`;
    addActivity("已查询原 nonce 状态", `${result.status} · 未创建新请求`);
  } catch (error) { $("#reservation-status").textContent = `查询失败；未广播：${error.message}`; }
});

$("#refresh-button").addEventListener("click", () => { loadProfile(); syncChainState(); addActivity("状态刷新已执行", "配置、余额和 HUB 健康检查"); });
const activityHeading = $("#audit-log")?.closest(".panel")?.querySelector(".panel-heading");
if (activityHeading) {
  activityHeading.querySelector("h2").textContent = "详细操作日志";
  activityHeading.querySelector("p").textContent = "跨重启保留最近 300 条诊断，不包含助记词、私钥或完整 Recovery Package。";
  $("#clear-activity").insertAdjacentHTML("beforebegin", '<button id="copy-activity" class="button secondary" type="button">复制日志</button>');
}
$("#copy-activity")?.addEventListener("click", async () => {
  try {
    const text = $("#audit-log").textContent;
    if (navigator.clipboard?.writeText) {
      await navigator.clipboard.writeText(text);
    } else {
      const helper = document.createElement("textarea");
      helper.value = text;
      helper.style.position = "fixed";
      helper.style.opacity = "0";
      document.body.appendChild(helper);
      helper.focus();
      helper.select();
      if (!document.execCommand("copy")) throw new Error("当前 WebView 不支持剪贴板复制，请手动选择日志文本");
      helper.remove();
    }
    addActivity("诊断日志已复制", `${activities.length} 条记录`);
  } catch (error) { addActivity("诊断日志复制失败", error.message); }
});
$("#clear-activity").addEventListener("click", () => { activities.splice(0); localStorage.removeItem(ACTIVITY_LOG_KEY); scheduleActivityLogPersistence(); renderActivities(); renderAuditLog(); });
$("#lock-button").addEventListener("click", () => {
  if (chainSyncTimer !== null) { clearInterval(chainSyncTimer); chainSyncTimer = null; }
  chainHistoryRows = [];
  sessionStorage.removeItem("xhub-v36-ui-session");
  sessionStorage.removeItem("xhub-v36-wallet-role");
  document.body.classList.add("auth-locked");
  $("#auth-screen").style.display = "grid";
  loadWalletProfiles();
});
$$('.history-mode').forEach((button) => button.addEventListener('click', () => {
  historyMode = button.dataset.historyMode;
  $$('.history-mode').forEach((item) => {
    const active = item === button;
    item.classList.toggle('active', active);
    item.setAttribute('aria-selected', active ? 'true' : 'false');
  });
  renderHistory();
}));
$("#refresh-transactions").addEventListener("click", () => { syncChainState(); addActivity("交易历史刷新已请求", "正在读取当前地址 Coin 历史"); });
$("#settle-funding-coin")?.addEventListener("click", async () => {
  if (settlementInFlight) return;
  const requestedSpentContext = window.__xhubSpentFinalize;
  let tracked = loadFundingTracker();
  const requestedFundingId = normalizeCoinId(requestedSpentContext?.funding_coin_id);
  if (requestedFundingId) {
    // SPENT COIN is an explicit FINALIZE request. Never fall back to whatever
    // Funding tracker happens to be active in the wallet UI.
    const contextTracker = loadFundingTrackers().find((item) => normalizeCoinId(item.funding_coin_id) === requestedFundingId);
    tracked = contextTracker || null;
    if (tracked) localStorage.setItem(walletScopedKey(FUNDING_TRACKER_ACTIVE_KEY), tracked.funding_coin_id);
  }
  if (!tracked) {
    const missingFundingId = requestedFundingId || normalizeCoinId(requestedSpentContext?.funding_coin_id);
    const missingClosingId = normalizeCoinId(requestedSpentContext?.closing_coin_id);
    const detail = `stage=FINALIZE · funding_coin_id=${missingFundingId || "-"} · current_closing_coin_id=${missingClosingId || "-"} · tracker 未能保存或不属于当前钱包`;
    addActivity("SPENT 结算失败", detail, missingFundingId || null);
    showError("#funding-tracker-error", "SPENT 结算失败：临时 Funding Tracker 未能保存或不属于当前钱包，请重新执行 Closing Coin 反查。\n" + detail);
    window.__xhubSpentFinalize = null;
    return;
  }
  const spentFinalize = requestedSpentContext
    && requestedFundingId === normalizeCoinId(tracked.funding_coin_id)
    && /^[0-9a-f]{64}$/.test(normalizeCoinId(requestedSpentContext.closing_coin_id))
    ? requestedSpentContext
    : null;
  if (spentFinalize) {
    tracked.current_closing_coin_id = normalizeCoinId(spentFinalize.closing_coin_id);
    tracked.initial_closing_coin_id ||= tracked.current_closing_coin_id;
    const contextSequence = positiveIntegerOrUndefined(spentFinalize.sequence);
    const contextConfirmedHeight = positiveIntegerOrUndefined(spentFinalize.confirmed_height);
    const contextDeadline = positiveIntegerOrUndefined(spentFinalize.deadline_height);
    if (contextSequence !== undefined) tracked.current_closing_sequence = contextSequence;
    if (contextConfirmedHeight !== undefined) tracked.current_closing_confirmed_height = contextConfirmedHeight;
    if (contextDeadline !== undefined) tracked.challenge_deadline_height = contextDeadline;
    tracked.settlement_stage = tracked.current_closing_sequence > 1 ? "SUBSEQUENT_CLOSING" : "INITIAL_CLOSING";
    // Consume the page-specific context so a later ordinary Funding action
    // cannot inherit an old Closing Coin from the hidden SPENT form.
    window.__xhubSpentFinalize = null;
  }
  normalizeLegacySettlementStage(tracked);
  settlementInFlight = true;
  clearError("#funding-error");
  clearError("#funding-tracker-error");
  // Re-discover the portable settlement state before choosing START or
  // FINALIZE. This is important immediately after B/C import a Funding Coin,
  // when the background refresh may not have populated the Initial Coin yet.
  if (!spentFinalize && !["SETTLED", "START_PENDING", "START_SUBMITTED", "START_DROPPED", "FINAL_PENDING", "FINAL_SUBMITTED", "FINAL_DROPPED"].includes(tracked.settlement_stage)) {
    try { await refreshSettlementDiscovery(tracked); } catch (_) { /* the normal path below reports the actionable error */ }
  }
  normalizeLegacySettlementStage(tracked);
  let stage = "UNKNOWN";
  const finalStages = ["FINALIZE", "INITIAL_CLOSING", "SUBSEQUENT_CLOSING", "FINAL_PENDING", "FINAL_SUBMITTED", "FINAL_DROPPED"];
  stage = spentFinalize || finalStages.includes(tracked.settlement_stage) ? "FINALIZE" : "START";
  addActivity("结算操作开始", `stage=${stage} · Funding Coin=${tracked.funding_coin_id} · local-stage=${tracked.settlement_stage || "NONE"} · peak=${tracked.last_status?.peak_height || "-"}`, tracked.funding_coin_id);
  try {
    if (["START_PENDING", "START_SUBMITTED", "FINAL_PENDING", "FINAL_SUBMITTED"].includes(tracked.settlement_stage)) {
      await refreshPendingSettlement(tracked);
      saveFundingTracker(tracked);
      renderActiveFundingTracker();
      addActivity("结算交易状态已核验", `stage=${stage} · local-stage=${tracked.settlement_stage} · 未构造重复交易`, tracked.funding_coin_id);
      return;
    }
    if (tracked.settlement_stage === "SETTLED") {
      addActivity("结算已完成，拒绝重复执行", `Coin=${tracked.funding_coin_id} · settled_height=${tracked.settled_height || tracked.initial_coin_spent_height || "-"}`, tracked.funding_coin_id);
      return;
    }
    if (["START_DROPPED", "FINAL_DROPPED"].includes(tracked.settlement_stage)) {
      const retryingFinal = tracked.settlement_stage === "FINAL_DROPPED";
      const oldSpendBundleId = retryingFinal ? tracked.settlement_spend_bundle_id : tracked.settlement_start_spend_bundle_id;
      const retryStatus = await request("/api/v3.6/wallet-profiles/settlement/pending-status", {
        method: "POST",
        headers: { "content-type": "application/json" },
        body: JSON.stringify({ rpc_url: "https://api.coinset.org", stage: retryingFinal ? "FINALIZE" : "START", funding_coin_id: tracked.funding_coin_id, initial_closing_coin_id: tracked.initial_closing_coin_id, spend_bundle_id: oldSpendBundleId }),
      });
      if (retryStatus.safe_to_retry !== true) {
        if (retryStatus.initial_state === "SPENT" && retryStatus.initial_spent_height != null) {
          tracked.settlement_stage = "SETTLED";
          tracked.initial_coin_spent_height = Number(retryStatus.initial_spent_height);
          tracked.settled_height = Number(retryStatus.initial_spent_height);
        } else if (retryStatus.tx_in_mempool) {
          tracked.settlement_stage = retryingFinal ? "FINAL_SUBMITTED" : "START_SUBMITTED";
        } else if (!retryingFinal && retryStatus.initial_state === "UNSPENT") {
          tracked.settlement_stage = "INITIAL_CLOSING";
        }
        saveFundingTracker(tracked);
        throw new Error(`原交易状态已变化，已停止重试：funding=${retryStatus.funding_state} / initial=${retryStatus.initial_state} / mempool=${retryStatus.tx_in_mempool}`);
      }
      const suggestedFee = Math.max(Number((retryingFinal ? tracked.settlement_final_retry_fee_mojo : tracked.settlement_retry_fee_mojo) || 0), SETTLEMENT_RETRY_MIN_FEE_MOJO);
      const fee = $("#settlement-fee");
      if (fee && Number(fee.value || 0) < suggestedFee) fee.value = String(suggestedFee);
      addActivity("掉单重试条件已确认", `stage=${retryingFinal ? "FINALIZE" : "START"} · 旧 SpendBundle=${oldSpendBundleId} · funding=${retryStatus.funding_state} · initial=${retryStatus.initial_state} · mempool=false · 新 Fee=${fee?.value || suggestedFee} mojo · 尚未广播`, tracked.funding_coin_id);
    }
    if (stage === "FINALIZE" && (tracked.current_closing_coin_id || tracked.initial_closing_coin_id)) {
      const currentCoinId = normalizeCoinId(tracked.current_closing_coin_id || tracked.initial_closing_coin_id);
      const initial = await request("/api/v3.6/wallet-profiles/funding/discover", { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify({ rpc_url: "https://api.coinset.org", funding_coin_id: currentCoinId }) });
      if (initial.status === "SPENT" && initial.spent_height != null) {
        tracked.settlement_stage = "SETTLED";
        tracked.settled_height = Number(initial.spent_height);
        tracked.initial_coin_spent_height = Number(initial.spent_height);
        tracked.settled_at = new Date().toISOString();
        saveFundingTracker(tracked);
        addActivity("最终结算已确认上链", `Current Closing Coin=${currentCoinId} · spent_height=${initial.spent_height} · 不再重复构造`, tracked.funding_coin_id);
        renderActiveFundingTracker();
        return;
      }
    }
    if (!tracked.hub_tracking && tracked.confirmed_draft && !tracked.recovery_package_available) {
      if (!window.confirm("该 Funding Coin 尚未注册到 HUB。将提交公开 Funding Puzzle 和锁定条款到 HUB，仍不会广播主网。是否继续？")) return;
      const registration = await request("/api/v3.6/wallet-profiles/funding/register", { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify({ role: selectedWalletProfile.role, funding_coin_id: tracked.funding_coin_id, confirmed_draft: tracked.confirmed_draft }) });
      tracked.hub_tracking = { acceptance_cutoff_height: registration.hub_response?.acceptance_cutoff_height || registration.acceptance_cutoff_height, scheduled_close_height: registration.hub_response?.scheduled_close_height || registration.scheduled_close_height, funding_birth_height: registration.hub_response?.funding_birth_height || registration.funding_birth_height, chain_state: registration.hub_response?.chain_state || registration.chain_state, total_reserved_mojo: 0 };
      saveFundingTracker(tracked);
    }
    // A HUB-signed Recovery Package is sufficient for settlement. Do not
    // reject an existing package merely because the tracking summary endpoint
    // is unavailable for a zero-reservation Coin.
    const feeText = $("#settlement-fee")?.value.trim() || "1";
    if (!/^\d+$/.test(feeText)) throw new Error("Fee 必须是非负整数 mojo。");
    const feeMojos = Number(feeText);
    // A cached SPENT-COIN context can contain the package that was used to
    // create an older Closing Coin. Always reload the latest HUB package for
    // FINALIZE so a Subsequent Coin is built from the checkpoint sequence that
    // actually produced the current chain child.
    const packageResponse = await request(`/api/v3.6/hub/funding-coins/${tracked.funding_coin_id}/recovery-packages/latest`, { headers: { "x-xhub-protocol-version": "0x0360" }, timeoutMs: 30_000 });
    if (!packageResponse.recovery_package_canonical_hex) throw new Error("HUB 没有返回 Recovery Package；请先注册 Funding Coin 并提交至少一条有效通道状态。");
    addActivity("Recovery Package 已读取", `Coin=${tracked.funding_coin_id} · sequence=${packageResponse.state_sequence ?? "-"} · hash=${packageResponse.recovery_package_content_hash || "-"} · bytes=${Math.floor(packageResponse.recovery_package_canonical_hex.length / 2)} · source=HUB_LATEST`, tracked.funding_coin_id);
    tracked.recovery_package_content_hash = packageResponse.recovery_package_content_hash;
    tracked.recovery_package_canonical_hex = packageResponse.recovery_package_canonical_hex;
    // RecoveryPackage responses from older HUB deployments do not expose
    // challenge_blocks as a top-level JSON field. The value is nevertheless
    // committed in the package and mirrored by the deployed wallet profile.
    // Keep it on the tracker so FINALIZE can derive the deadline locally.
    const packageChallengeBlocks = Number(packageResponse.challenge_blocks || 0);
    let discoveredSettlement = null;
    try {
      discoveredSettlement = await request("/api/v3.6/wallet-profiles/settlement/discover", {
        method: "POST",
        headers: { "content-type": "application/json" },
        body: JSON.stringify({
          rpc_url: "https://api.coinset.org",
          funding_coin_id: tracked.funding_coin_id,
          recovery_package_canonical_hex: packageResponse.recovery_package_canonical_hex,
        }),
        timeoutMs: 60_000,
      });
    } catch (error) {
      addActivity("Settlement Discovery 失败", `Funding=${tracked.funding_coin_id} · ${error.message}`, tracked.funding_coin_id);
    }
    const discoveredChallengeBlocks = Number(discoveredSettlement?.challenge_blocks || 0);
    if (!tracked.challenge_blocks && packageChallengeBlocks > 0) tracked.challenge_blocks = packageChallengeBlocks;
    if (discoveredChallengeBlocks > 0) tracked.challenge_blocks = discoveredChallengeBlocks;
    // Never use the current deployment profile for an existing Funding Coin.
    // Its historical challenge window is committed in RecoveryPackage.
    if (discoveredSettlement?.challenge_deadline_height != null) {
      const discoveredDeadline = Number(discoveredSettlement.challenge_deadline_height);
      if (Number.isSafeInteger(discoveredDeadline) && discoveredDeadline > 0) tracked.challenge_deadline_height = discoveredDeadline;
    }
    saveFundingTracker(tracked);
    const startBirthHeight = tracked.hub_tracking?.acceptance_cutoff_height ?? tracked.acceptance_cutoff_height;
    const currentClosingCoinId = normalizeCoinId(tracked.current_closing_coin_id || tracked.initial_closing_coin_id);
    const currentClosingSequence = positiveIntegerOrUndefined(tracked.current_closing_sequence);
    const currentClosingConfirmedHeight = positiveIntegerOrUndefined(tracked.current_closing_confirmed_height);
    const challengeBlocks = Number(discoveredChallengeBlocks || packageChallengeBlocks || 0);
    const derivedClosingDeadline = currentClosingSequence <= 1 && currentClosingConfirmedHeight !== undefined && challengeBlocks > 0
      ? currentClosingConfirmedHeight + challengeBlocks
      : undefined;
    const discoveredClosingDeadline = positiveIntegerOrUndefined(discoveredSettlement?.challenge_deadline_height);
    const currentClosingDeadline = discoveredClosingDeadline ?? derivedClosingDeadline;
    if (stage === "FINALIZE") {
      if (!/^[0-9a-f]{64}$/.test(currentClosingCoinId)) throw new Error("FINALIZE 缺少有效的 Closing Coin ID，已阻止回退到 START");
      if (currentClosingSequence === undefined) throw new Error("FINALIZE 缺少 Closing Coin sequence，已阻止构造错误输入");
      if (currentClosingConfirmedHeight === undefined) throw new Error("FINALIZE 缺少 Closing Coin confirmed height，已阻止构造错误输入");
      if (currentClosingDeadline === undefined) {
        addActivity("Closing Coin deadline 由链核心推导", `confirmed_height=${currentClosingConfirmedHeight} · RecoveryPackage 内含历史 challenge_blocks`, tracked.funding_coin_id);
      }
    }
    const settlementRequest = {
      role: selectedWalletProfile.role,
      rpc_url: "https://api.coinset.org",
      stage,
      funding_coin_id: tracked.funding_coin_id,
      recovery_package_canonical_hex: packageResponse.recovery_package_canonical_hex,
      initial_birth_height: stage === "START" ? startBirthHeight : undefined,
      current_closing_coin_id: stage === "FINALIZE" ? currentClosingCoinId : undefined,
      current_closing_sequence: stage === "FINALIZE" ? currentClosingSequence : undefined,
      current_closing_confirmed_height: stage === "FINALIZE" ? currentClosingConfirmedHeight : undefined,
      current_closing_deadline_height: stage === "FINALIZE" ? currentClosingDeadline : undefined,
      fee_mojo: feeMojos,
    };
    addActivity("结算请求已锁定", `stage=${settlementRequest.stage} · funding_coin_id=${settlementRequest.funding_coin_id} · current_closing_coin_id=${settlementRequest.current_closing_coin_id || "-"} · current_closing_sequence=${settlementRequest.current_closing_sequence ?? "-"} · current_closing_confirmed_height=${settlementRequest.current_closing_confirmed_height ?? "-"} · current_closing_deadline_height=${settlementRequest.current_closing_deadline_height ?? "-"} · fee_mojo=${settlementRequest.fee_mojo}`, tracked.funding_coin_id);
    const settlementInputCoin = normalizeCoinId(stage === "START" ? tracked.funding_coin_id : currentClosingCoinId);
    addActivity("结算请求已发送", `stage=${stage} · prepare-settlement · spend_input=${settlementInputCoin}`, tracked.funding_coin_id);
    const prepared = await request("/api/v3.6/wallet-profiles/settlement/prepare", { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify(settlementRequest), timeoutMs: 60_000 });
    const preparedStage = String(prepared.stage || "").trim().toUpperCase();
    if (preparedStage !== stage) throw new Error(`链核心返回 stage=${preparedStage || "缺失"}，请求为 ${stage}；已阻止广播`);
    const preparedInputCoin = normalizeCoinId(prepared.spend_input_coin_id);
    if (preparedInputCoin !== settlementInputCoin) throw new Error(`链核心返回 SpendBundle 输入 ${preparedInputCoin || "缺失"} 与请求输入 ${settlementInputCoin} 不一致；已阻止广播`);
    if (stage === "FINALIZE" && normalizeCoinId(prepared.closing_coin_id) !== currentClosingCoinId) throw new Error("链核心返回的 Closing Coin 与输入不一致，已阻止广播");
    const runtime = prepared.runtime_settlement_request;
    if (prepared.runtime_binary_path || prepared.runtime_command) addActivity("链核心运行时已确认", `binary=${prepared.runtime_binary_path || "-"} · command=${prepared.runtime_command || "-"} · request_stage=${runtime?.stage || stage} · request_current_closing_coin_id=${runtime?.current_closing_coin_id || "-"}`, tracked.funding_coin_id);
    addActivity("结算 SpendBundle 本地验证通过", `stage=${prepared.stage} · SpendBundle=${prepared.spend_bundle_id} · input=${settlementInputCoin} · predicted-output=${prepared.closing_coin_id} · amount=${prepared.funding_amount_mojo} · fee=${prepared.fee_mojo} · broadcast=false`, tracked.funding_coin_id);
    window.__xhubPreparedSettlement = prepared;
    const outputs = Array.isArray(prepared.outputs) && prepared.outputs.length ? prepared.outputs.map((output) => `${output.kind}: ${output.amount_mojo} mojo -> ${output.puzzle_hash}`).join("\n") : `Initial Closing Coin ID: ${prepared.closing_coin_id}`;
    const message = `结算交易已在本地构造并通过共识/签名验证。\n\n阶段：${prepared.stage}\n输入 Coin：${settlementInputCoin}\n预测后续 Coin：${prepared.closing_coin_id}\n结算金额：${prepared.funding_amount_mojo} mojo\n费用：${prepared.fee_mojo} mojo\n钱包费用找零：${prepared.fee_change_mojo || 0} mojo\nSpendBundle ID：${prepared.spend_bundle_id}\n\n输出地址 / Puzzle Hash：\n${outputs}\n\n选择“确定”后才会调用 push_tx；取消不会广播。`;
    if (!window.confirm(message)) { addActivity("已取消结算广播", `SpendBundle=${prepared.spend_bundle_id} · 本地已构造 · push_tx=false`, tracked.funding_coin_id); return; }
    addActivity("用户确认结算广播", `SpendBundle=${prepared.spend_bundle_id} · 即将调用 push_tx`, tracked.funding_coin_id);
    const result = await request("/api/v3.6/wallet-profiles/settlement/broadcast", { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify({ confirmation_token: prepared.confirmation_token, spend_bundle_id: prepared.spend_bundle_id, confirm_broadcast: true }), timeoutMs: 30_000 });
    const broadcastStatus = String(result.status || "SUCCESS").toUpperCase();
    if (stage === "START") {
      if (!["SUCCESS", "PENDING"].includes(broadcastStatus)) throw new Error(`节点拒绝结算交易：${broadcastStatus}${result.node_error ? ` · ${result.node_error}` : ""}`);
      tracked.settlement_stage = broadcastStatus === "SUCCESS" ? "START_SUBMITTED" : "START_PENDING";
      tracked.predicted_initial_closing_coin_id = prepared.closing_coin_id;
      // Keep a chain-discovered Initial Closing Coin authoritative. The
      // prepared output is only a candidate until Funding Coin spend outputs
      // confirm its actual identity.
      if (!tracked.initial_closing_coin_id) tracked.initial_closing_coin_id = prepared.closing_coin_id;
      tracked.settlement_start_spend_bundle_id = prepared.spend_bundle_id;
      tracked.settlement_start_submitted_peak_height = Number(tracked.last_status?.peak_height || 0);
      tracked.settlement_start_submitted_at = new Date().toISOString();
      tracked.settlement_start_fee_mojo = Number(prepared.fee_mojo || 0);
    } else {
      if (broadcastStatus !== "SUCCESS") throw new Error(`最终结算尚未被节点接受：${broadcastStatus}${result.node_error ? ` · ${result.node_error}` : ""}`);
      tracked.settlement_stage = "FINAL_SUBMITTED";
      tracked.settlement_spend_bundle_id = prepared.spend_bundle_id;
      tracked.finalized_closing_coin_id = currentClosingCoinId;
      tracked.settlement_final_submitted_peak_height = Number(tracked.last_status?.peak_height || 0);
      tracked.settlement_final_submitted_at = new Date().toISOString();
      tracked.settlement_final_fee_mojo = Number(prepared.fee_mojo || 0);
    }
    saveFundingTracker(tracked);
    try {
      const pending = await request("/api/v3.6/wallet-profiles/settlement/pending-status", {
        method: "POST",
        headers: { "content-type": "application/json" },
        body: JSON.stringify({
          rpc_url: "https://api.coinset.org",
          stage,
          funding_coin_id: tracked.funding_coin_id,
          initial_closing_coin_id: tracked.initial_closing_coin_id || currentClosingCoinId,
          spend_bundle_id: prepared.spend_bundle_id,
        }),
        timeoutMs: 30_000,
      });
      addActivity("广播后状态核验", `stage=${stage} · SpendBundle=${prepared.spend_bundle_id} · mempool=${pending.tx_in_mempool === true} · funding=${pending.funding_state || "-"} · closing=${pending.initial_state || "-"}`, tracked.funding_coin_id);
    } catch (error) {
      addActivity("广播后状态核验失败", `stage=${stage} · SpendBundle=${prepared.spend_bundle_id} · ${error.message}`, tracked.funding_coin_id);
    }
    const activityTitle = stage === "START" && broadcastStatus === "PENDING" ? "结算开始交易等待节点接收" : stage === "START" ? "结算开始交易已提交" : "最终结算交易已提交";
    addActivity(activityTitle, `push_tx=${broadcastStatus} · SpendBundle=${prepared.spend_bundle_id} · next=${stage === "START" ? `核验 mempool 并等待 Closing Coin ${prepared.closing_coin_id} 确认` : `核验 mempool 并等待当前 Closing Coin ${currentClosingCoinId} 花费确认`}`, tracked.funding_coin_id);
    refreshFundingTracker({ quiet: true });
  } catch (error) {
    const failureInput = stage === "FINALIZE"
      ? normalizeCoinId(tracked.current_closing_coin_id || tracked.initial_closing_coin_id)
      : normalizeCoinId(tracked.funding_coin_id);
    addActivity("结算操作失败", `stage=${stage} · spend_input=${failureInput || "-"} · funding_coin_id=${normalizeCoinId(tracked.funding_coin_id)} · ${error.message}`);
    showError("#funding-tracker-error", `结算准备或广播失败：${error.message}`);
  } finally { settlementInFlight = false; updateSettlementAction(loadFundingTracker()); }
});
$("#refresh-funding-tracker").addEventListener("click", () => refreshFundingTracker());

$$('.nav-item').forEach((button) => button.addEventListener('click', () => {
  showView(button.dataset.view);
}));

installWcUi();
installFundingChainUi();
installFundingTrackerImport();

function unlockWorkspace() {
  sessionStorage.setItem("xhub-v36-ui-session", "unlocked");
  sessionStorage.setItem("xhub-v36-wallet-role", selectedWalletProfile?.role || "user");
  document.body.classList.remove("auth-locked");
  $("#auth-screen").style.display = "none";
  updateReceiveIdentity();
  hydrateReservationTrackers();
  if (reservationTrackerPollTimer !== null) clearInterval(reservationTrackerPollTimer);
  reservationTrackerPollTimer = setInterval(() => refreshAllReservationTrackers(), 15_000);
  startReservationEventStreams();
  hydrateFundingTrackers();
  showView("overview");
  loadProfile();
}

$("#create-profile").addEventListener("click", () => openProfileDialog("create"));
$("#import-profile").addEventListener("click", () => openProfileDialog("import"));
$("#profile-dialog-close").addEventListener("click", closeProfileDialog);
$("#profile-form-cancel").addEventListener("click", closeProfileDialog);
$("#profile-form").addEventListener("submit", async (event) => {
  event.preventDefault();
  const role = $("#profile-form-role").value;
  const name = $("#profile-form-name").value.trim();
  $("#profile-form-submit").disabled = true;
  try {
    const endpoint = profileDialogMode === "create" ? "/api/v3.6/wallet-profiles/create" : "/api/v3.6/wallet-profiles/import";
    const body = { role, name };
    if (profileDialogMode === "import") body.mnemonic = $("#profile-mnemonic").value.trim();
    const result = await request(endpoint, { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify(body) });
    closeProfileDialog();
    sessionStorage.setItem("xhub-v36-wallet-role", result.profile?.role || role);
    await loadWalletProfiles();
    selectedWalletProfile = walletProfiles.find((item) => item.role === (result.profile?.role || role)) || selectedWalletProfile;
    addActivity(profileDialogMode === "create" ? "钱包已创建" : "钱包已导入", `${role} · 账户名和助记词已保存到本地 JSON`);
    if (result.backup_phrase) showBackupPhrase(result.backup_phrase);
  } catch (error) {
    showError("#profile-form-error", error.message);
  } finally {
    $("#profile-form-submit").disabled = false;
  }
});
$("#backup-confirm").addEventListener("change", () => { $("#backup-close").disabled = !$("#backup-confirm").checked; });
$("#backup-close").addEventListener("click", () => { $("#backup-phrase").value = ""; $("#backup-dialog").hidden = true; });
$("#copy-receive-address").addEventListener("click", async () => {
  if (!selectedWalletProfile?.address) return;
  try {
    await navigator.clipboard.writeText(selectedWalletProfile.address);
    $("#receive-copy-status").textContent = "收款地址已复制到剪贴板。";
    addActivity("收款地址已复制", selectedWalletProfile.address.slice(0, 16) + "…");
  } catch {
    $("#receive-copy-status").textContent = "复制失败，请手动选择地址复制。";
  }
});
$("#copy-overview-address").addEventListener("click", async () => {
  if (!selectedWalletProfile?.address) return;
  try { await navigator.clipboard.writeText(selectedWalletProfile.address); addActivity("钱包地址已复制", selectedWalletProfile.address.slice(0, 16) + "…"); }
  catch { addActivity("钱包地址复制失败", "请手动选择地址复制"); }
});
$("#reveal-sensitive").addEventListener("click", async () => {
  if (!selectedWalletProfile) return;
  if (!window.confirm("助记词和 BLS 私钥可以控制本钱包资金。请确认周围无人窥屏，并且当前电脑可信。是否显示 60 秒？")) return;
  const button = $("#reveal-sensitive"); button.disabled = true;
  try {
    const value = await request("/api/v3.6/wallet-profiles/sensitive", { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify({ role: selectedWalletProfile.role, confirm_reveal: true }) });
    $("#overview-mnemonic").textContent = value.mnemonic;
    $("#overview-private-key").textContent = value.wallet_bls_private_key;
    $("#sensitive-state").textContent = "显示中 · 60 秒后隐藏";
    $("#sensitive-state").className = "tag warn";
    button.hidden = true;
    $("#hide-sensitive").hidden = false;
    sensitiveHideTimer = setTimeout(hideSensitiveWalletInfo, 60_000);
  } catch (error) { addActivity("敏感信息读取失败", error.message); }
  finally { button.disabled = false; }
});
$("#hide-sensitive").addEventListener("click", hideSensitiveWalletInfo);
$("#overview-balance-card").addEventListener("click", () => showView("send"));
$("#send-back-overview").addEventListener("click", () => showView("overview"));
$("#send-form").addEventListener("submit", async (event) => {
  event.preventDefault();
  clearError("#send-error");
  const destination = $("#send-destination").value.trim().toLowerCase();
  const amount = $("#send-amount").value.trim();
  const fee = $("#send-fee").value.trim();
  const memo = $("#send-memo").value.trim();
  if (!/^xch1[0-9a-z]{20,120}$/.test(destination)) { showError("#send-error", "请输入有效的 Chia xch 收款地址。"); return; }
  if (!/^\d+$/.test(amount) || BigInt(amount) <= 0n) { showError("#send-error", "金额必须是大于 0 的整数 mojo。"); return; }
  if (!/^\d+$/.test(fee)) { showError("#send-error", "费用必须是非负整数 mojo。"); return; }
  const button = $("#prepare-send"); button.disabled = true; button.textContent = "正在本地构造和验证...";
  preparedWalletSend = null;
  $("#broadcast-send").disabled = true;
  try {
    const prepared = await request("/api/v3.6/wallet-profiles/send/prepare", { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify({ role: selectedWalletProfile.role, rpc_url: "https://api.coinset.org", destination_address: destination, amount_mojo: amount, fee_mojo: fee, memo }) });
    preparedWalletSend = prepared;
    $("#broadcast-send-fee").value = String(prepared.fee_mojo ?? fee);
    $("#send-state").textContent = "已签名，未广播";
    $("#send-state").className = "tag warn";
    $("#send-preview").textContent = `收款地址：${prepared.destination_address}\n金额：${prepared.amount_mojo} mojo\n费用：${prepared.fee_mojo} mojo\n输入合计：${prepared.input_total_mojo} mojo\n找零：${prepared.change_mojo} mojo\n输入 Coin：${prepared.selected_coins?.length || 0} 个\nSpendBundle ID：${prepared.spend_bundle_id}\n本地验证：consensus=${prepared.consensus_conditions_verified} / signature=${prepared.aggregate_signature_verified}\n广播：false`;
    $("#broadcast-send").disabled = false;
    addActivity("普通转账已本地签名", `${amount} mojo · SpendBundle=${prepared.spend_bundle_id} · 广播=false`);
  } catch (error) { showError("#send-error", `构造转账失败：${error.message}`); }
  finally { button.disabled = false; button.textContent = "重新构造并验证"; }
});
$("#broadcast-send").addEventListener("click", async () => {
  const prepared = preparedWalletSend;
  if (!prepared) return;
  const fee = $("#broadcast-send-fee").value.trim();
  if (!/^\d+$/.test(fee)) { showError("#send-error", "Fee 必须是非负整数 mojo。"); return; }
  if (Number(fee) !== Number(prepared.fee_mojo)) { showError("#send-error", "Fee 已改变，请重新构造并验证 SpendBundle。"); return; }
  if (!window.confirm(`即将广播普通转账。\n\n收款地址：${prepared.destination_address}\n金额：${prepared.amount_mojo} mojo\n费用：${prepared.fee_mojo} mojo\nSpendBundle ID：${prepared.spend_bundle_id}\n\n该操作不可撤销。是否继续？`)) { addActivity("已取消普通转账广播", "没有调用 push_tx"); return; }
  const button = $("#broadcast-send"); button.disabled = true;
  try {
    const result = await request("/api/v3.6/wallet-profiles/send/broadcast", { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify({ confirmation_token: prepared.confirmation_token, spend_bundle_id: prepared.spend_bundle_id, confirm_broadcast: true }) });
    $("#send-state").textContent = "已提交主网";
    $("#send-state").className = "tag good";
    $("#send-preview").textContent += `\n\n主网提交：${result.status || "SUCCESS"}\n广播：true`;
    preparedWalletSend = null;
    addActivity("普通转账已广播", `${prepared.amount_mojo} mojo · SpendBundle=${prepared.spend_bundle_id}`);
    syncChainState({ quiet: true });
  } catch (error) { showError("#send-error", `广播失败或结果不确定：${error.message}。请先按 SpendBundle ID 查询，避免重复发送。`); }
});
document.body.classList.toggle("auth-locked", sessionStorage.getItem("xhub-v36-ui-session") !== "unlocked");
loadBuildInfo();
loadWalletProfiles();
hydrateFundingTrackers();
hydrateActivityLog();
if (document.body.classList.contains("auth-locked")) $("#auth-screen").style.display = "grid";
else { $("#auth-screen").style.display = "none"; loadProfile(); }
renderHistory();
async function refreshClosingCoinTrace() {
  const body = $("#tower-closing-trace-body");
  const summary = $("#tower-closing-trace-summary");
  const count = $("#tower-closing-trace-count");
  if (!body) return;
  const tracked = loadFundingTracker();
  // The trace panel must refresh the authoritative Coinset-backed chain
  // itself; relying only on a previously hydrated local tracker leaves the
  // panel at 0 entries after launching a fresh EXE.
  if (tracked?.funding_coin_id) {
    try {
      const funding = String(tracked.funding_coin_id).replace(/^0x/, "").toLowerCase();
      const chain = await request(`/api/v3.6/chain/closing-coins/${funding}?protocol_version=0x0360`);
      if (Array.isArray(chain.coins)) {
        tracked.closing_coins = chain.coins;
        tracked.closing_coins_peak_height = chain.peak_height ?? null;
        saveFundingTracker(tracked);
      }
    } catch (_) { /* render the last known snapshot and retry on the interval */ }
  }
  // Only render coins returned by settlement/discover (Coinset-backed). Do
  // not treat a locally predicted Initial ID as an on-chain Closing Coin.
  const ids = Array.isArray(tracked?.closing_coins)
    ? tracked.closing_coins.map((coin) => ({ sequence: Number(coin.sequence || 0), kind: Number(coin.sequence || 0) === 1 ? "INITIAL" : "SUBSEQUENT", id: coin.coin_id, snapshot: coin }))
      .filter((entry) => /^[0-9a-f]{64}$/i.test(String(entry.id || "")))
    : [];
  if (!ids.length) { body.innerHTML = '<div class="empty-state">尚未发现 Closing Coin。</div>'; if (summary) summary.textContent = "等待同步 Funding Coin"; if (count) count.textContent = "0 条"; return; }
  const rows = await Promise.all(ids.map(async (entry) => {
    let status = {};
    try { status = await request("/api/v3.6/wallet-profiles/funding/discover", { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify({ rpc_url: "https://api.coinset.org", funding_coin_id: entry.id }) }); } catch (error) { status = { status: "UNKNOWN", error: error.message }; }
    const state = String(entry.snapshot?.status || status.status || "UNKNOWN").toUpperCase();
    const label = state === "FOUND" ? "UNSPENT" : state === "SPENT" ? "SPENT" : state;
    return `<article class="closing-trace-row"><div class="closing-trace-seq">SEQ${entry.sequence}</div><div class="closing-trace-main"><strong>${entry.kind === "INITIAL" ? "Initial Closing Coin" : "Subsequent Closing Coin"}</strong><code>${entry.id}</code></div><div class="closing-trace-meta"><span class="tag ${state === "FOUND" ? "good" : state === "SPENT" ? "neutral" : "warn"}">${label}</span><small>确认 ${status.confirmed_height ?? "-"} · 花费 ${status.spent_height ?? "-"}</small></div></article>`;
  }));
  body.innerHTML = rows.join("");
  if (summary) summary.textContent = `已发现 ${ids.length} 枚，当前链上状态已同步`;
  if (count) count.textContent = `${ids.length} 条`;
}

function setupTowerSeq1TestEntry() {
  const confirm = $("#tower-test-confirm");
  const prepare = $("#tower-test-prepare");
  const broadcast = $("#tower-test-broadcast");
  const state = $("#tower-test-state");
  if (!confirm || !prepare || !broadcast || !state) return;
  const trace = $("#tower-closing-trace");
  const testBody = document.querySelector("#tower-seq1-test .test-panel-body");
  if (trace && testBody) testBody.append(trace);
  const tracker = document.querySelector('[data-view-panel="funding-tracker"]');
  const lifecycle = document.querySelector("#tracker-lifecycle");
  const testPanel = document.querySelector("#tower-seq1-test");
  if (tracker && lifecycle && testPanel) tracker.insertBefore(testPanel, lifecycle);
  const finalizeWrap = document.createElement("div");
  finalizeWrap.className = "finalize-test-action";
  finalizeWrap.innerHTML = `<button id="tower-test-finalize" class="button secondary full-button" type="button" disabled>花费已过挑战期的 Closing Coin</button><p id="tower-test-finalize-state" class="field-hint">等待检测 Closing Coin 和挑战截止高度。</p>`;
  const notice = testPanel.querySelector(".notice-box");
  if (notice && !testPanel.querySelector("#tower-test-finalize")) notice.before(finalizeWrap);
  const finalize = testPanel.querySelector("#tower-test-finalize");
  const finalizeState = testPanel.querySelector("#tower-test-finalize-state");
  const refreshFinalizeEntry = () => {
    if (!finalize || !finalizeState) return;
    const tracked = loadFundingTracker();
    const stage = String(tracked?.settlement_stage || "");
    const deadline = Number(tracked?.challenge_deadline_height || 0);
    const peak = Number(tracked?.last_status?.peak_height || 0);
    const eligible = Boolean(tracked?.initial_closing_coin_id)
      && ((stage === "INITIAL_CLOSING" && deadline > 0 && peak >= deadline) || stage === "FINAL_DROPPED");
    finalize.disabled = !eligible;
    finalizeState.textContent = eligible
      ? `已确认挑战期结束（peak ${peak.toLocaleString("en-US")} / deadline ${deadline.toLocaleString("en-US")}），点击后重新核验并确认 FINALIZE。`
      : tracked?.initial_closing_coin_id
        ? `当前 Closing Coin 仍在挑战期内，或尚未同步截止高度（peak ${peak || "-"} / deadline ${deadline || "-"}）。`
        : "尚未发现 Closing Coin。";
  };
  finalize?.addEventListener("click", () => {
    refreshFinalizeEntry();
    if (!finalize.disabled) $("#settle-funding-coin")?.click();
  });
  refreshFinalizeEntry();
  refreshClosingCoinTrace();
  setInterval(refreshFinalizeEntry, 10000);
  setInterval(refreshClosingCoinTrace, 15000);
  let prepared = null;
  confirm.addEventListener("change", () => { prepare.disabled = !confirm.checked; });
  prepare.addEventListener("click", async () => {
    const tracked = loadFundingTracker();
    if (!tracked?.funding_coin_id || !selectedWalletProfile) {
      state.textContent = "请先选择已追踪且已解锁的钱包 Funding Coin。";
      return;
    }
    let chain = tracked.last_status || {};
    const coin = tracked.funding_coin_id.replace(/^0x/, "").toLowerCase();
    try {
      chain = await request("/api/v3.6/wallet-profiles/funding/discover", { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify({ rpc_url: "https://api.coinset.org", funding_coin_id: coin }) });
      tracked.last_status = chain;
      saveFundingTracker(tracked);
    } catch (_) { /* settlement/prepare performs the authoritative check if discovery is unavailable */ }
    const spent = String(chain.funding_state ?? chain.status ?? "").toUpperCase() === "SPENT" || chain.spent_height != null;
    if (spent) {
      state.textContent = "Coinset 确认该 Funding Coin 已花费，拒绝作为 SEQ1 测试 Coin。";
      return;
    }
    prepare.disabled = true; broadcast.disabled = true;
    state.textContent = "正在读取 SEQ1 Recovery Package，并本地构造 Closing SpendBundle...";
    try {
      const packageResponse = await request(`/api/v3.6/hub/funding-coins/${coin}/recovery-packages/1`, { headers: { "x-xhub-protocol-version": "0x0360" } });
      if (Number(packageResponse.state_sequence) !== 1) throw new Error("HUB 返回的 Recovery Package 不是 SEQ1");
      const fee = 1;
      const startBirthHeight = tracked.hub_tracking?.acceptance_cutoff_height ?? tracked.acceptance_cutoff_height;
      prepared = await request("/api/v3.6/wallet-profiles/settlement/prepare", { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify({ role: selectedWalletProfile.role, rpc_url: "https://api.coinset.org", stage: "START", funding_coin_id: coin, recovery_package_canonical_hex: packageResponse.recovery_package_canonical_hex, initial_birth_height: startBirthHeight, fee_mojo: fee }) });
      if (Number(prepared.fee_mojo) !== 1 || Number(prepared.state_sequence ?? 1) !== 1) throw new Error("本地构造结果不满足 SEQ1 / 1 mojo 门限");
      window.__xhubPreparedTowerSeq1 = prepared;
      state.textContent = `SEQ1 Closing SpendBundle 已构造并通过共识/签名校验，尚未广播。\nSpendBundle ID：${prepared.spend_bundle_id}\n预测 Closing Coin：${prepared.closing_coin_id}\n请确认 Tower 已保存 SEQ2 后再点击延迟广播。`;
      broadcast.disabled = false;
      addActivity("Tower SEQ1 测试交易已准备", `SpendBundle=${prepared.spend_bundle_id} · fee=1 mojo · broadcast=false`, coin);
    } catch (error) {
      prepared = null; window.__xhubPreparedTowerSeq1 = null;
      const detail = String(error.message || "");
      state.textContent = detail.includes("404") ? "HUB 未保存这个 Funding Coin 的 SEQ1 Recovery Package，无法固定构造 SEQ1。请使用已保存 SEQ1 且仍在挑战窗口内的专用测试 Coin。" : `SEQ1 构造失败：${detail}`;
      addActivity("Tower SEQ1 测试构造失败", error.message, tracked.funding_coin_id);
    } finally { prepare.disabled = !confirm.checked; }
  });
  broadcast.addEventListener("click", async () => {
    if (!prepared || prepared !== window.__xhubPreparedTowerSeq1) return;
    if (!window.confirm(`仅广播指定 Funding Coin 的 SEQ1 Closing SpendBundle。\n\nSpendBundle：${prepared.spend_bundle_id}\n费用：1 mojo\n这会调用主网 push_tx，且不可撤销。是否继续？`)) return;
    broadcast.disabled = true;
    try {
      const result = await request("/api/v3.6/wallet-profiles/settlement/broadcast", { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify({ confirmation_token: prepared.confirmation_token, spend_bundle_id: prepared.spend_bundle_id, confirm_broadcast: true }) });
      state.textContent = `SEQ1 Closing SpendBundle 已提交主网。\n状态：${result.status || "SUCCESS"}\nSpendBundle ID：${prepared.spend_bundle_id}`;
      addActivity("Tower SEQ1 测试交易已广播", `push_tx=${result.status || "SUCCESS"} · SpendBundle=${prepared.spend_bundle_id}`, loadFundingTracker()?.funding_coin_id);
      prepared = null; window.__xhubPreparedTowerSeq1 = null;
    } catch (error) { state.textContent = `SEQ1 广播失败或结果不确定：${error.message}`; broadcast.disabled = false; }
  });
}

function installSpentCoinPage() {
  const auditNav = document.querySelector('.nav-item[data-view="audit"]');
  const main = document.querySelector('main');
  if (!auditNav || !main || document.querySelector('[data-view-panel="spent-coin"]')) return;
  auditNav.insertAdjacentHTML('afterend', '<button class="nav-item spent-coin-nav" data-view="spent-coin" type="button"><span class="nav-icon">SP</span><span>SPENT COIN</span></button>');
  main.insertAdjacentHTML('beforeend', '<section class="view" data-view-panel="spent-coin"><div class="page-heading"><div><p class="eyebrow">SPENT COIN</p><h1>SPENT COIN</h1><p class="page-description">输入 Closing Coin ID，核验后构造并广播最终结算。</p></div><span class="tag warn">REAL BROADCAST</span></div><section class="panel spent-coin-panel"><div class="panel-heading"><div><h2>Closing Coin 广播</h2><p>仅对指定 Closing Coin 执行，手续费默认 1 mojo。</p></div></div><div class="field-grid compact"><label>Closing Coin ID<input id="spent-coin-id" spellcheck="false" placeholder="64 位十六进制 Coin ID"></label><label>FEE COIN（mojo）<input id="spent-coin-fee" type="number" min="0" step="1" inputmode="numeric" value="1"></label></div><div class="field-grid compact"><label>Funding Coin（链上反查结果）<input id="spent-coin-funding" spellcheck="false" readonly placeholder="尚未识别"></label><label>Recovery Package（来源 / hash）<input id="spent-coin-package" spellcheck="false" readonly placeholder="尚未获取"></label></div><pre id="spent-coin-log" class="status-box" aria-live="polite">等待输入 Closing Coin ID。</pre><button id="spent-coin-broadcast" class="button confirm full-button" type="button">广播</button></section></section>');
  document.querySelector('.spent-coin-nav').addEventListener('click', () => showView('spent-coin'));
  $('#spent-coin-broadcast').addEventListener('click', async () => {
    const id = $('#spent-coin-id').value.trim().replace(/^0x/, '').toLowerCase();
    const fee = $('#spent-coin-fee').value.trim() || '1';
    const state = $('#spent-coin-log');
    const lines = [];
    const log = (phase, detail) => { const stamp = new Date().toLocaleString('zh-CN', { hour12: false }); lines.push(`[${stamp}] ${phase} - ${detail}`); if (state) state.textContent = lines.join('\n'); };
    if (!/^[0-9a-f]{64}$/.test(id)) { log('输入校验失败', 'Closing Coin ID 必须是 64 位十六进制'); return; }
    if (!/^\d+$/.test(fee)) { log('输入校验失败', 'FEE COIN 必须是非负整数 mojo'); return; }
    log('开始', `Closing Coin=${id} · fee=${fee} mojo · 仅花费输入 Coin`);
    try {
      const fundingField = $('#spent-coin-funding');
      const packageField = $('#spent-coin-package');
      if (fundingField) fundingField.value = '正在从链上 Closing Coin 反查祖先...';
      if (packageField) packageField.value = '等待 Funding Coin 识别';
      const trackers = loadFundingTrackers();
      let tracker = trackers.find((item) => (item.closing_coins || []).some((item) => String(item.coin_id || '').replace(/^0x/, '').toLowerCase() === id));
      let packageResponse = null;
      if (!tracker) {
        log('Funding 反查', '未找到本地追踪记录，正在从 Coinset 回溯 Closing Coin 祖先');
        const discovered = await request('/api/v3.6/wallet-profiles/spent-coin/discover', {
          method: 'POST', headers: { 'content-type': 'application/json' },
          body: JSON.stringify({ rpc_url: 'https://api.coinset.org', closing_coin_id: id })
        });
        const ancestors = Array.isArray(discovered.ancestors) ? discovered.ancestors : [];
        log('Funding 反查结果', `ancestors=${ancestors.length}`);
        for (const ancestor of ancestors) {
          const fundingId = String(ancestor.coin_id || '').replace(/^0x/, '').toLowerCase();
          if (!/^[0-9a-f]{64}$/.test(fundingId)) continue;
          try {
            const candidate = await request(`/api/v3.6/hub/funding-coins/${fundingId}/recovery-packages/latest`, { headers: { 'x-xhub-protocol-version': '0x0360' } });
            if (candidate?.recovery_package_canonical_hex) {
              if (fundingField) fundingField.value = fundingId;
              const packageHash = candidate.recovery_package_content_hash || candidate.content_hash || 'hash 未由 HUB 返回';
              if (packageField) packageField.value = `HUB · ${packageHash}`;
              packageResponse = candidate;
      tracker = {
                schema: 'xhub.wallet.funding-tracker.v1',
                funding_coin_id: fundingId,
                // This record is created by SPENT COIN from public chain/HUB
                // data. Mark it imported and bind it to the selected wallet so
                // save/load filtering cannot discard it before FINALIZE.
                imported: true,
                creator_puzzle_hash: selectedWalletProfile?.puzzle_hash || null,
                tracking_wallet_puzzle_hash: selectedWalletProfile?.puzzle_hash || null,
                closing_coins: [],
                current_closing_coin_id: id,
                current_closing_sequence: 1,
                current_closing_confirmed_height: null,
                settlement_stage: 'FINALIZE',
                recovery_package_canonical_hex: candidate.recovery_package_canonical_hex,
                recovery_package_source: 'HUB',
                challenge_blocks: Number(candidate.challenge_blocks || 0) || undefined
              };
              log('Funding Coin 已识别', `${fundingId} · ancestor_height=${ancestor.confirmed_block_height ?? ancestor.confirmed_height ?? '-'} · Funding 是否 SPENT 由链上结果决定`);
              log('Recovery Package 已获取', `source=HUB · sequence=${candidate.state_sequence ?? 'latest'} · content_hash=${packageHash} · bytes=${Math.floor(String(candidate.recovery_package_canonical_hex).length / 2)}`);
              break;
            }
          } catch (_) { /* try the next ancestor */ }
        }
        if (!tracker) throw new Error('Funding/Package 反查失败：链上找不到对应 Funding Coin，或 HUB 未返回 Recovery Package');
      }
      if (!$('#spent-coin-funding').value && tracker?.funding_coin_id) $('#spent-coin-funding').value = tracker.funding_coin_id;
      if (!$('#spent-coin-package').value && tracker?.recovery_package_canonical_hex) $('#spent-coin-package').value = `本地记录 · ${tracker.recovery_package_content_hash || 'hash 未记录'}`;
      log('Closing Coin 链查询', `funding_coin=${tracker.funding_coin_id} · 请求 /chain/closing-coins`);
      const chain = await request(`/api/v3.6/chain/closing-coins/${String(tracker.funding_coin_id).replace(/^0x/, '').toLowerCase()}?protocol_version=0x0360`);
      log('Closing Coin 链结果', `coins=${Array.isArray(chain.coins) ? chain.coins.length : 0}`);
      const coin = (chain.coins || []).find((item) => String(item.coin_id || '').replace(/^0x/, '').toLowerCase() === id);
      if (!coin) throw new Error('Closing Coin 校验失败：该 Coin 不在对应 Funding Coin 的 Closing Coin 链上记录中，可能是 puzzle hash/金额不匹配');
      log('Closing Coin 已确认', `coin=${id} · sequence=${coin.sequence ?? '-'} · status=${coin.status ?? '-'} · confirmed_height=${coin.confirmed_height ?? '-'} · spent_height=${coin.spent_height ?? '-'}`);
      if (String(coin.status).toUpperCase() !== 'UNSPENT') throw new Error(`Closing Coin 已花费：status=${coin.status} · spent_height=${coin.spent_height ?? '-'}，不能重复花费`);
      const closingDeadline = chain.challenge_deadline_height
        ?? chain.current_closing_deadline_height
        ?? coin.deadline_height
        ?? (Number(coin.sequence || 1) <= 1 && Number(coin.confirmed_height || 0) > 0 && Number(tracker.challenge_blocks || profile?.challenge_blocks || 0) > 0
          ? Number(coin.confirmed_height) + Number(tracker.challenge_blocks || profile.challenge_blocks)
          : undefined);
      if (closingDeadline != null) {
        tracker.challenge_deadline_height = Number(closingDeadline);
        log('Deadline 校验', `deadline=${closingDeadline} · peak=${chain.peak_height ?? chain.peak?.height ?? '-'} · ${chain.current_closing_deadline_height ?? chain.challenge_deadline_height ?? coin.deadline_height ? '由接口返回' : '由 confirmed_height + challenge_blocks 推导'}`);
      }
      tracker.current_closing_coin_id = id; tracker.current_closing_sequence = Number(coin.sequence || 1); tracker.current_closing_confirmed_height = coin.confirmed_height;
      tracker.settlement_stage = tracker.current_closing_sequence > 1 ? 'SUBSEQUENT_CLOSING' : 'INITIAL_CLOSING';
      window.__xhubSpentFinalize = { funding_coin_id: tracker.funding_coin_id, closing_coin_id: id, sequence: tracker.current_closing_sequence, confirmed_height: coin.confirmed_height, deadline_height: tracker.challenge_deadline_height, recovery_package_canonical_hex: packageResponse?.recovery_package_canonical_hex || tracker.recovery_package_canonical_hex, recovery_package_content_hash: packageResponse?.recovery_package_content_hash || tracker.recovery_package_content_hash, state_sequence: packageResponse?.state_sequence };
      if (packageResponse?.recovery_package_canonical_hex) tracker.recovery_package_canonical_hex = packageResponse.recovery_package_canonical_hex;
      saveFundingTracker(tracker); localStorage.setItem(walletScopedKey(FUNDING_TRACKER_ACTIVE_KEY), tracker.funding_coin_id);
      $('#settlement-fee').value = fee; log('准备结算', `stage=FINALIZE · spend_input=${id} · funding=${tracker.funding_coin_id}（仅用于反查 Package） · fee=${fee} mojo`); log('SpendBundle', '正在构造并提交 BroadcastExecutor');
      setTimeout(() => { const settle = $('#settle-funding-coin'); if (settle) { settle.disabled = false; settle.click(); } }, 0);
      // Only consume activity records created by this click.  The audit log is
      // persisted across restarts, so a plain find() can report an older
      // START failure as the result of this FINALIZE request.
      const activitiesBeforeOperation = new Set(activities);
      const handledOperationActivities = new Set();
      let checks = 0;
      const resultTimer = setInterval(() => {
        checks += 1;
        const error = $('#funding-tracker-error');
        const activity = activities.find((item) => !activitiesBeforeOperation.has(item) && !handledOperationActivities.has(item)
          && (item.title === '结算请求已发送' || item.title === '最终结算交易已提交' || item.title === '结算操作失败' || item.title === 'SPENT 结算失败' || item.title === '已取消结算广播' || item.title === '结算开始交易已提交' || item.title === '结算开始交易等待节点接收' || item.title === '广播后状态核验' || item.title === '广播后状态核验失败'));
        if (activity) handledOperationActivities.add(activity);
        if (error && !error.hidden && error.textContent) { log('失败', error.textContent); clearInterval(resultTimer); }
        else if (activity?.title === '结算请求已发送') { log('结算请求已发送', activity.detail); }
        else if (activity?.title === '最终结算交易已提交') { log('广播成功', `${activity.detail} · 请在 Coinset 核对 Closing Coin 花费状态`); clearInterval(resultTimer); }
        else if (activity?.title === '广播后状态核验') { log('MEMPOOL 状态', activity.detail); }
        else if (activity?.title === '广播后状态核验失败') { log('MEMPOOL 状态未知', activity.detail); clearInterval(resultTimer); }
        else if (activity?.title === 'SPENT 结算失败') { log('结算失败', activity.detail); clearInterval(resultTimer); }
        else if (activity?.title === '已取消结算广播') { log('已取消广播', activity.detail); clearInterval(resultTimer); }
        else if (activity?.title === '结算操作失败') { log('广播失败', activity.detail); clearInterval(resultTimer); }
        else if (activity?.title === '结算开始交易已提交' || activity?.title === '结算开始交易等待节点接收') { log('阶段异常', `后端报告 ${activity.detail}，本页面请求应为 FINALIZE，请检查 Gateway 版本`); clearInterval(resultTimer); }
        else if (checks >= 240) { log('等待超时', '结算请求仍在处理中；请查看审计日志中的 SpendBundle ID 和节点响应'); clearInterval(resultTimer); }
      }, 500);
    } catch (error) { log('准备/广播失败', error.message || String(error)); }
  });
}

document.addEventListener("DOMContentLoaded", setupTowerSeq1TestEntry);
document.addEventListener("DOMContentLoaded", installSpentCoinPage);
document.addEventListener("DOMContentLoaded", setupReceiveAndSendEnhancements);
