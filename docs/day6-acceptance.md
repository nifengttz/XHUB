# 第 6 天验收记录

日期：2026-08-03

结论：**PASS**

## 攻击与边界矩阵

| 编号 | V2 验收标准 | 结果 | 自动化证据与确定结果 |
|---:|---|---|---|
| 1 | 篡改金额、地址、订单、nonce、state、网络或 coin id 全部拒绝 | PASS | `every_settlement_field_is_signature_bound` 逐项覆盖 16 个结算字段；`every_invoice_field_is_signature_bound` 覆盖 9 个 Invoice 字段；错误返回 `ProtocolError` 或 `BadAggregateSignature` |
| 2 | 同一 Voucher 重复广播不能产生第二笔商户输出 | PASS | `same_voucher_cannot_be_claimed_twice` 第二次提交确定返回 `DoubleSpend` |
| 3 | Voucher 不能跨 channel 或网络重放 | PASS | `voucher_replay_is_rejected_across_channel_and_network` 分别返回 `WrongFundingCoin`、`WrongNetwork`，错误 funding coin 的 bundle 构造返回 `SettlementWorkflowError::WrongFundingCoin` |
| 4 | Hub 重启不丢失已签 Voucher | PASS | `persisted_voucher_survives_restart_and_claims_at_cutoff` 关闭 SQLite 后重新打开，恢复 Voucher，并在高度 25 完成 Claim 和确认 |
| 5 | Refund 与 Claim 不能同时成功，最终只有一组输出 | PASS | `claim_cutoff_race_has_one_winner_and_one_output_set` 在高度 25 仅 Claim 成功；`refund_height_race_has_one_winner_and_one_output_set` 在高度 26 仅 Refund 成功 |
| 6 | 商户在截止高度恢复仍可索赔；退款高度明确为 `CLAIM_EXPIRED` | PASS | 高度 25 恢复并索赔成功；`voucher_status_becomes_claim_expired_at_refund_height` 在高度 26 返回 `MerchantPaymentStatus::ClaimExpired` |
| 7 | 每个失败场景有确定状态和错误码 | PASS | 高度边界返回 `AssertBeforeHeightAbsoluteFailed` / `AssertHeightAbsoluteFailed`；重放返回 `DoubleSpend`、`WrongFundingCoin` 或 `WrongNetwork`；确认不匹配返回 `ConfirmationMismatch` 且保持 Submitted |

## 状态语义修正

`payment_expiry_height` 只限制 Invoice、Intent 和 Voucher 的签发时间。Voucher 已完成双签后，在 `claim_before_height`（含）之前仍为 `PAID_OFFCHAIN`，不会因为签发期限已过而错误显示失效。只有当前高度超过 Claim 截止高度时，状态才明确变为 `CLAIM_EXPIRED`。

## 资金竞争结果

```text
height 25: Claim = accepted, Refund = AssertHeightAbsoluteFailed
           outputs = Merchant 1 mojo + User 9 mojos

height 26: Claim = AssertBeforeHeightAbsoluteFailed, Refund = accepted
           output = User 10 mojos
```

两种情况下 funding coin 只产生一组最终 children，金额总和均严格为 10 mojos。

## 测试结果

第 6 天新增 5 个攻击与恢复测试，并修正 1 个 Voucher 生命周期测试。全量结果：

```text
34 passed; 0 failed
```

验证命令：

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File .\scripts\compile-puzzles.ps1
powershell -NoProfile -ExecutionPolicy Bypass -File .\scripts\verify-day1.ps1
cargo test --all-targets
cargo clippy --all-targets -- -D warnings
```
