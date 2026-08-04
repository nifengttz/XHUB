# 第 2 天验收记录

日期：2026-08-03

结论：**PASS**

## 验收结果

| 编号 | V2 计划验收标准 | 结果 | 自动化证据 |
|---:|---|---|---|
| 1 | funding coin 金额严格为 10 mojo | PASS | `funding_amount_is_exact`：错误金额触发 `AssertMyAmountFailed` |
| 2 | 有效双签 Voucher 创建 Merchant 1 mojo 和 User 9 mojos | PASS | `merchant_submits_claim_without_user_or_hub_keys`：断言两个 coin 的地址、金额及总额 |
| 3 | User 与 Hub 离线后 Merchant 可独立构造和广播 SpendBundle | PASS | 测试先取得双签并释放包含私钥的 `TestChannel`，再仅用 Voucher 数据构造 spend 并提交 simulator |
| 4 | 篡改金额、任一 puzzle hash、coin id 或高度后被拒绝 | PASS | `funding_amount_is_exact`、`modified_merchant_output_invalidates_signature`、`curried_destination_and_heights_cannot_be_changed`、`signature_cannot_replay_on_another_funding_coin` |
| 5 | 同一 funding coin 第二次花费被拒绝 | PASS | `same_voucher_cannot_be_claimed_twice`：第二次提交触发 `DoubleSpend` |
| 6 | 输出总额严格等于输入金额 | PASS | Claim 输出 `1 + 9 = 10 mojos`；Refund 输出 `10 mojos` |

## 额外验证

| 项目 | 结果 | 证据 |
|---|---|---|
| Rust 与 CLVM Settlement hash 一致 | PASS | `claim_conditions_match_rust_commitment` 核对两个 `AGG_SIG_ME` 原始消息 |
| Hub 签名必需 | PASS | `hub_signature_is_required_before_voucher_issuance` 返回 `MissingKey` |
| Claim 高度边界 | PASS | `claim_cutoff_is_inclusive_and_then_expires` 在 `C` 接受、在 `R` 拒绝 |
| Refund 高度边界 | PASS | `refund_starts_after_claim_cutoff` 在 `C` 拒绝、在 `R` 接受 |

## 高度边界修订

`chia-sdk-test 0.33.0` 的 simulator 在当前高度等于
`ASSERT_BEFORE_HEIGHT_ABSOLUTE C` 时仍接受交易。若沿用第 1 天的
`claim_before_height == refund_height`，两个分支会在 simulator 的边界高度重叠。

因此实现和规范统一修订为：

```text
refund_height = claim_before_height + 1
Claim（simulator）：height <= claim_before_height
Refund：height >= refund_height
```

测试使用 `C = 25`、`R = 26`；确定性协议向量使用 `C = 1000`、`R = 1001`。
该差异已在 `docs/protocol-v1.md` 中记录为 simulator 兼容性约束；部署到其他
simulator 或真实网络前必须重新验证精确共识边界。

## 交付物

| 产物 | 内容 |
|---|---|
| `puzzles/wall_hub_channel_v1.clsp` | Claim/Refund funding puzzle |
| `puzzles/wall_hub_channel_v1.clsp.hex` | `clvm_tools_rs 0.4.0` 编译产物 |
| `src/lib.rs` | 参数编码、hash、puzzle curry、coin spend 与 10 个 simulator 测试 |
| `scripts/compile-puzzles.ps1` | 可重复执行的 Chialisp 编译脚本 |
| `test-vectors/day1-v1.json` | 按新高度边界重新生成的 Settlement/Refund 向量 |

## 验证命令

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File .\scripts\compile-puzzles.ps1
powershell -NoProfile -ExecutionPolicy Bypass -File .\scripts\verify-day1.ps1
cargo test --all-targets
cargo clippy --all-targets -- -D warnings
```

验收要求为 10 个 Rust/simulator 测试全部通过，Day 1 向量校验通过，且
Clippy 在 `-D warnings` 下无告警。
