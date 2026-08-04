# 第 5 天验收记录

日期：2026-08-03

结论：**PASS**

## 验收结果

| 编号 | V2 计划验收标准 | 结果 | 自动化证据 |
|---:|---|---|---|
| 1 | Voucher 签发后 User/Hub 离线，Merchant 在截止高度前领取 1 mojo，User 得到 9 mojos | PASS | `merchant_claims_after_user_and_hub_go_offline` 丢弃签名方对象后，仅用 funding coin、公开参数和持久化 Voucher 构造并提交 Claim；确认两个输出金额和地址 |
| 2 | 无 Voucher 时，退款高度后 User 取回 10 mojos | PASS | `user_refunds_without_voucher_after_refund_height` 在高度 26 提交 Refund，确认唯一输出为 User 10 mojos |
| 3 | 退款高度后 Claim 失败 | PASS | `claim_fails_at_refund_height` 在高度 26 返回 `AssertBeforeHeightAbsoluteFailed` |
| 4 | 退款高度前 Refund 失败 | PASS | `refund_fails_before_refund_height` 在高度 25 返回 `AssertHeightAbsoluteFailed` |
| 5 | 独立 fee coin 不改变通道结算输出 | PASS | `external_fee_coin_preserves_channel_outputs` 聚合独立标准 coin，通道 funding coin 的 children 仍严格为 Merchant 1 + User 9 mojos |
| 6 | 只有链上确认后状态才变为 `SETTLED/REFUNDED` | PASS | 成功提交后状态保持 `CLAIM_SUBMITTED/REFUND_SUBMITTED`；确认 children 后才进入终态；`confirmation_mismatch_does_not_finalize` 证明缺失或错误输出不会终结状态 |

## 状态确认规则

广播只记录为 `CLAIM_SUBMITTED` 或 `REFUND_SUBMITTED`。确认函数要求 funding coin 的链上 children 与协议输出在父 coin、puzzle hash、金额和数量上精确一致，才写入 `SETTLED` 或 `REFUNDED`。缺失、多余或错误输出返回 `ConfirmationMismatch`，状态保持 Submitted。

## 测试结果

第 5 天新增 6 个集成测试，连同前四天共 29 个测试：

```text
29 passed; 0 failed
```

验证命令：

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File .\scripts\compile-puzzles.ps1
powershell -NoProfile -ExecutionPolicy Bypass -File .\scripts\verify-day1.ps1
cargo test --all-targets
cargo clippy --all-targets -- -D warnings
```
