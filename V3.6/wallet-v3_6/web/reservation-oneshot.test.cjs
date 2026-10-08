// 回归测试：WC 请求码一次性消费（B2）与商户幂等键 (funding_coin_id, request_id)（C）。
//
// 只加载 app.js 里的纯函数块（normalizeCoinId .. positiveIntegerOrUndefined 之前），
// 用桩替换 localStorage 派生的读取函数，因此不需要 DOM。
//
// 运行：node reservation-oneshot.test.cjs
const assert = require("node:assert/strict");
const fs = require("node:fs");
const path = require("node:path");
const test = require("node:test");
const vm = require("node:vm");

function loadHelpers({ reservationTrackers = [], fundingTrackers = [] } = {}) {
  const source = fs.readFileSync(path.join(__dirname, "app.js"), "utf8");
  const start = source.indexOf("function normalizeCoinId(value) {");
  const end = source.indexOf("\nfunction positiveIntegerOrUndefined(value) {", start);
  assert.notEqual(start, -1, "normalizeCoinId must exist in app.js");
  assert.notEqual(end, -1, "helper block must end before positiveIntegerOrUndefined()");
  const context = {
    state: { reservationTrackers, fundingTrackers },
    loadReservationTrackers() { return context.state.reservationTrackers; },
    loadFundingTrackers() { return context.state.fundingTrackers; },
  };
  vm.runInNewContext(`${source.slice(start, end)}\nresult = { normalizeCoinId, reservationEntryCoin, sameReservationRecord, findReservationTracker, reservationRequestConsumedCoins, shortCoinLabel };`, context);
  return { ...context.result, state: context.state };
}

const COIN_A = "7e131ed5a8dc149592fb71ab3be178cb33adefea0b2c3fe91092a5e9aee54de9";
const COIN_B = "76b040b7edf219d8beb9f6fc9a03c705bc223994fe9d1a2fda8c1b869f14b839";
const RID = "b1136d001eea05942725e576bac7d2f5f6fc76dc5523892aab1bb5912feaa5dc";

test("同一 request_id 落在两个 Funding Coin 上是两笔记录，不塌陷", () => {
  const h = loadHelpers();
  const rowA = { request_id: RID, funding_coin_id: COIN_A, amount: "2" };
  const rowB = { request_id: RID, funding_coin_id: COIN_B, amount: "2" };
  assert.equal(h.sameReservationRecord(rowA, rowB), false, "不同通道必须视为两笔");
  assert.equal(h.sameReservationRecord(rowA, { ...rowA }), true, "同通道同请求必须合并");
  assert.equal(h.sameReservationRecord(rowA, { request_id: "other", funding_coin_id: COIN_A }), false);
});

test("未绑定通道的待签记录与首个绑定它的通道属于同一笔", () => {
  const h = loadHelpers();
  const pending = { request_id: RID, funding_coin_id: null };
  const bound = { request_id: RID, funding_coin_id: COIN_A };
  assert.equal(h.sameReservationRecord(pending, bound), true);
  assert.equal(h.sameReservationRecord(bound, pending), true);
});

test("一次性消费：本机已消费的通道可被查出，未消费通道会被守卫拦住", () => {
  const h = loadHelpers({
    reservationTrackers: [
      { request_id: RID, funding_coin_id: COIN_A, ledger_written: true },
      { request_id: "unrelated", funding_coin_id: COIN_B, ledger_written: true },
    ],
  });
  const consumed = [...h.reservationRequestConsumedCoins(RID)];
  assert.deepEqual(consumed, [COIN_A], "只应报告消费过该 request_id 的通道");
  // 同通道重试：放行
  assert.equal(consumed.includes(h.normalizeCoinId(COIN_A)), true);
  // 改投其他通道：守卫必须拦下
  assert.equal(consumed.length > 0 && !consumed.includes(h.normalizeCoinId(COIN_B)), true);
  // 完全没消费过的请求码：不该拦
  assert.deepEqual([...h.reservationRequestConsumedCoins("never-used")], []);
});

test("funding tracker 的预扣摘要同样计入已消费通道", () => {
  const h = loadHelpers({
    reservationTrackers: [],
    fundingTrackers: [
      { funding_coin_id: COIN_A, last_reservation: { request_id: RID, ledger_written: true } },
      { funding_coin_id: COIN_B, reservation_summaries: [{ request_id: RID, ledger_written: true }] },
    ],
  });
  assert.deepEqual([...h.reservationRequestConsumedCoins(RID)].sort(), [COIN_A, COIN_B].sort());
});

test("按 (funding_coin_id, request_id) 精确取回对应记录", () => {
  const h = loadHelpers({
    reservationTrackers: [
      { request_id: RID, funding_coin_id: COIN_A, amount: "1" },
      { request_id: RID, funding_coin_id: COIN_B, amount: "2" },
    ],
  });
  assert.equal(h.findReservationTracker(RID, COIN_A).amount, "1");
  assert.equal(h.findReservationTracker(RID, `0x${COIN_B}`).amount, "2");
  assert.equal(h.findReservationTracker(RID, "0011".repeat(16)), null);
});
