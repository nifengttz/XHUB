const form = document.querySelector("#terms-form");
const prepareButton = document.querySelector("#prepare-button");
const confirmButton = document.querySelector("#confirm-button");
const confirmCheck = document.querySelector("#confirm-check");
const errorBox = document.querySelector("#error");
const gatewayDot = document.querySelector("#gateway-dot");
let activeDraft = null;
let profile = null;

const values = () => Object.fromEntries(new FormData(form).entries());

function updateTiming() {
  const data = values();
  const acceptance = /^\d+$/.test(data.acceptance_blocks) ? BigInt(data.acceptance_blocks) : null;
  const freeze = /^\d+$/.test(data.freeze_blocks) ? BigInt(data.freeze_blocks) : null;
  const close = acceptance !== null && freeze !== null ? acceptance + freeze : null;
  document.querySelector("#close-delay").textContent = close === null ? "-" : close.toString();
  document.querySelector("#summary-close").textContent = close === null ? "-" : `${close} blocks`;
  document.querySelector("#a-relation").textContent = acceptance === null ? "-" : `F + ${acceptance}`;
  document.querySelector("#s-relation").textContent = freeze === null ? "-" : `A + ${freeze}`;
  if (activeDraft && !activeDraft.confirmed) resetPreview();
}

function resetPreview() {
  activeDraft = null;
  document.querySelector("#summary-state").textContent = "等待重新校验";
  document.querySelector("#terms-hash").textContent = "校验后生成";
  document.querySelector("#canonical-hex").textContent = "校验后生成";
  confirmCheck.checked = false;
  confirmCheck.disabled = true;
  confirmButton.disabled = true;
}

function showError(message) {
  errorBox.textContent = message;
  errorBox.hidden = false;
}

function clearError() {
  errorBox.hidden = true;
  errorBox.textContent = "";
}

async function request(url, options) {
  const response = await fetch(url, options);
  const body = await response.json().catch(() => ({}));
  if (!response.ok) throw new Error(body.message || `请求失败（HTTP ${response.status}）`);
  return body;
}

function setGatewayState(state, healthy) {
  document.querySelector("#gateway-state").textContent = state;
  gatewayDot.classList.toggle("pending", !healthy);
  gatewayDot.classList.toggle("error", state === "不可用");
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
    document.querySelector("#profile-label").textContent = "V3.6 主网 Canary";
    document.querySelector("#profile-id").textContent = profile.profile_id;
    document.querySelector("#confirmation-blocks").textContent = `${profile.funding_confirmation_blocks} blocks`;
    document.querySelector("#delivery-threshold").textContent = `${profile.delivery_threshold}-of-${profile.delivery_participants}（单 VPS）`;
    prepareButton.disabled = false;
    updateTiming();
  } catch (error) {
    document.querySelector("#profile-label").textContent = "主网配置载入失败";
    setGatewayState("不可用", false);
    showError(error.message);
    return;
  }

  try {
    const hub = await request("/api/v3.6/hub/health");
    setGatewayState(hub.status === "READY" ? "READY" : "异常", hub.status === "READY");
  } catch (error) {
    setGatewayState("不可用", false);
    showError(`HUB 网关：${error.message}`);
  }
}

form.addEventListener("input", updateTiming);
form.addEventListener("submit", async (event) => {
  event.preventDefault();
  clearError();
  prepareButton.disabled = true;
  prepareButton.textContent = "正在校验...";
  try {
    activeDraft = await request("/api/v3.6/funding-drafts", {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({ protocol_version: "0x0360", ...values() }),
    });
    const preview = activeDraft.preview;
    document.querySelector("#summary-state").textContent = activeDraft.confirmed ? "条款已锁定" : "校验通过";
    document.querySelector("#terms-hash").textContent = preview.channel_terms_hash;
    document.querySelector("#canonical-hex").textContent = preview.channel_terms_canonical_hex;
    confirmCheck.disabled = activeDraft.confirmed;
    confirmButton.disabled = true;
    if (activeDraft.confirmed) lockForm();
  } catch (error) {
    showError(error.message);
  } finally {
    if (!activeDraft?.confirmed && profile) prepareButton.disabled = false;
    prepareButton.textContent = "校验并生成条款";
  }
});

confirmCheck.addEventListener("change", () => {
  confirmButton.disabled = !confirmCheck.checked || !activeDraft || activeDraft.confirmed;
});

confirmButton.addEventListener("click", async () => {
  if (!activeDraft || !confirmCheck.checked) return;
  clearError();
  confirmButton.disabled = true;
  confirmButton.textContent = "正在锁定...";
  try {
    activeDraft = await request(`/api/v3.6/funding-drafts/${activeDraft.draft_id}/confirm`, {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({
        protocol_version: "0x0360",
        channel_terms_hash: activeDraft.preview.channel_terms_hash,
        user_confirmed: true,
      }),
    });
    lockForm();
  } catch (error) {
    showError(error.message);
    confirmButton.disabled = false;
    confirmButton.textContent = "确认并锁定条款";
  }
});

function lockForm() {
  form.querySelectorAll("input, textarea, button").forEach((element) => { element.disabled = true; });
  confirmCheck.disabled = true;
  confirmButton.disabled = true;
  confirmButton.textContent = "条款已确认并锁定";
  document.querySelector("#summary-state").textContent = "条款已锁定";
  const lockState = document.querySelector("#lock-state");
  lockState.textContent = "已确认";
  lockState.classList.add("locked");
}

loadProfile();
