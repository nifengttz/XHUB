# XHUB Puzzles V3.6

本目录保存 X-Hub V3.6 的 CLVM 源码、可复现编译产物、模块哈希和执行测试。

当前状态为 `VECTOR_READY`。以下接口和哈希是测试网候选冻结快照；在独立安全评审、双机可复现构建和主网上线审查完成前，不得标记为主网 `FROZEN`。

## 规范表示

- 除 `mode` 外，所有共识整数均以固定 8 字节 `u64_be` atom 传入。
- `mode = 1` 表示 `CHALLENGE`，`mode = 2` 表示 `FINALIZE`。
- `bytes32`、BLS 公钥和 nonce 分别是 32、48 和 32 字节 atom。
- `entries` 是按 `entry_index` 排序的列表；每项结构为 `(entry_index merchant_puzzle_hash merchant_receipt_public_key amount reservation_nonce)`。
- curry 参数和 solution 字段均为位置接口；调整顺序、增删字段或修改含义都属于不兼容变更。

## Funding Puzzle

模块：`xhub_funding_v3_6.clsp`

Curry 参数顺序：

```text
(NETWORK_ID ACCEPTANCE_BLOCKS FREEZE_BLOCKS CLOSE_DELAY_BLOCKS CHALLENGE_BLOCKS
 USER_PUBLIC_KEY HUB_PUBLIC_KEY STATE_RULES_HASH FUNDING_AMOUNT
 USER_REMAINDER_PUZZLE_HASH MAX_LEDGER_ENTRIES INITIAL_CLOSING_MOD_HASH
 SUBSEQUENT_CLOSING_MOD_HASH PAYMENT_MOD_HASH)
```

Solution 结构：

```text
(funding_coin_id state_sequence previous_checkpoint_hash manifest_root
 entry_count reserved_total user_remainder entries)
```

该 spend 即 `START_CLOSE`。Puzzle 校验完整账本、金额守恒、nonce 唯一性、Merkle root、State 0/正式状态签名条件和 Funding Coin 身份，并以 `ASSERT_HEIGHT_RELATIVE(CLOSE_DELAY_BLOCKS)` 创建唯一的 Initial Closing Coin。Funding Coin 本身不扣除手续费。

## Initial Closing Puzzle

模块：`xhub_initial_closing_v3_6.clsp`

Curry 参数顺序：

```text
(CURRENT_COMMITMENT)
```

Solution 结构：

```text
(mode initial_birth_height challenge_deadline_height
 network_id acceptance_blocks freeze_blocks close_delay_blocks challenge_blocks
 user_public_key hub_public_key state_rules_hash_value funding_amount
 user_remainder_puzzle_hash max_ledger_entries initial_closing_mod_hash
 subsequent_closing_mod_hash payment_mod_hash funding_coin_id
 current_sequence current_previous_checkpoint_hash current_manifest_root
 current_entry_count current_reserved_total current_user_remainder current_entries
 new_sequence new_previous_checkpoint_hash new_manifest_root new_entry_count
 new_reserved_total new_user_remainder new_entries)
```

`challenge_deadline_height` 必须等于 `initial_birth_height + challenge_blocks`。`CHALLENGE` 要求新状态序号严格增大，并创建带固定 deadline 承诺的 Subsequent Closing Coin；`FINALIZE` 在 deadline 到达后按当前完整账本逐条创建 Merchant Payment Coin，并返还用户余款。两个分支均使用 `ASSERT_MY_BIRTH_HEIGHT(initial_birth_height)`。

## Subsequent Closing Puzzle

模块：`xhub_subsequent_closing_v3_6.clsp`

Curry 参数顺序：

```text
(CURRENT_COMMITMENT)
```

Solution 结构：

```text
(mode challenge_deadline_height
 network_id acceptance_blocks freeze_blocks close_delay_blocks challenge_blocks
 user_public_key hub_public_key state_rules_hash_value funding_amount
 user_remainder_puzzle_hash max_ledger_entries initial_closing_mod_hash
 subsequent_closing_mod_hash payment_mod_hash funding_coin_id
 current_sequence current_previous_checkpoint_hash current_manifest_root
 current_entry_count current_reserved_total current_user_remainder current_entries
 new_sequence new_previous_checkpoint_hash new_manifest_root new_entry_count
 new_reserved_total new_user_remainder new_entries)
```

`CHALLENGE` 必须继承同一个 `challenge_deadline_height`，且新状态序号严格增大；`FINALIZE` 使用当前完整账本逐条结算。Closing Coin 本身不扣除手续费。

## Merchant Payment Puzzle

模块：`xhub_merchant_payment_v3_6.clsp`

Curry 参数顺序：

```text
(PROTOCOL_VERSION_ARG NETWORK_ID FUNDING_COIN_ID CHANNEL_TERMS_HASH ENTRY_INDEX
 RESERVATION_NONCE MERCHANT_PUZZLE_HASH)
```

Solution 结构：

```text
(payment_coin_amount)
```

Puzzle 使用 `ASSERT_MY_AMOUNT(payment_coin_amount)`，并将同一原额通过 `CREATE_COIN(MERCHANT_PUZZLE_HASH, payment_coin_amount)` 转给固定商户地址。它不提供手续费扣除、找零或替换收款地址的分支。

## VECTOR_READY 模块哈希

编译器为 `run 0.4.0` / `opc 0.4.0`，固定参数为 `--strict --optimize`。

```text
xhub_funding_v3_6             e2945105091602fb91db08af00525153604007791be6e673372e33880eb2e6ce
xhub_initial_closing_v3_6     95d2aa194ef302ac4637280031e3492736e231490a4883cc2ace090551c18b59
xhub_subsequent_closing_v3_6  e1a73c7381c56817159558594e13558c22d5d7ac8a8c5c81a53132335e8d1e29
xhub_merchant_payment_v3_6    b53e39fa4960713ced442f21331672ea38d73c763cc690f37089ebd3aee5ffe1
```

可复现构建与验证：

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File .\compile-puzzles.ps1
cargo test --manifest-path .\Cargo.toml
```

`module-hashes.json` 是机器可读清单；测试会重新计算每个 `.clsp.hex` 的 tree hash，并与清单逐项比较。
