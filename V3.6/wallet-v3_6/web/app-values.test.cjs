const assert = require("node:assert/strict");
const fs = require("node:fs");
const path = require("node:path");
const test = require("node:test");
const vm = require("node:vm");

function loadValues(context) {
  const source = fs.readFileSync(path.join(__dirname, "app.js"), "utf8");
  const start = source.indexOf("function values() {");
  const end = source.indexOf("\nfunction updateTiming()", start);
  assert.notEqual(start, -1, "values() must exist in app.js");
  assert.notEqual(end, -1, "values() must end before updateTiming()");
  vm.runInNewContext(`${source.slice(start, end)}\nresult = values();`, context);
  return JSON.parse(JSON.stringify(context.result));
}

test("locked funding controls still produce a complete WC draft body", () => {
  const fields = {
    network_id: { value: "mainnet-id", disabled: true },
    user_public_key: { value: "stale-user-key", disabled: true },
    user_remainder_puzzle_hash: { value: "stale-remainder", disabled: true },
    hub_state_public_key_a: { value: "hub-key", disabled: true },
    state_rules_hash: { value: "rules-hash", disabled: true },
  };
  class LockedFormData {
    entries() { return [][Symbol.iterator](); }
  }
  const result = loadValues({
    FormData: LockedFormData,
    form: { elements: fields },
    profile: { network_id: "profile-mainnet-id" },
    selectedWalletProfile: { wallet_public_key: "current-user-key", puzzle_hash: "current-remainder" },
    importedWcRequest: {
      request_type: "funding_request",
      termination_blocks: "20",
      freeze_blocks: "1",
      challenge_blocks: "20",
      funding_amount: "22",
    },
  });

  assert.deepEqual(result, {
    user_public_key: "current-user-key",
    user_remainder_puzzle_hash: "current-remainder",
    hub_state_public_key_a: "hub-key",
    state_rules_hash: "rules-hash",
    network_id: "mainnet-id",
    acceptance_blocks: "19",
    freeze_blocks: "1",
    challenge_blocks: "20",
    funding_amount: "22",
  });
});
