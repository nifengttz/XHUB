# 第 3 天验收记录

日期：2026-08-03

结论：**PASS**

## 验收结果

| 编号 | V2 计划验收标准 | 结果 | 自动化证据 |
|---:|---|---|---|
| 1 | User 与 Hub 对完全相同的 commitment bytes 签名 | PASS | `PaymentIntent` 与 `PaymentVoucher` 共用同一个 `SettlementCommitment::claim_signature_message`；`voucher_signatures_match_clvm_and_settle_in_simulator` 验证完整流程 |
| 2 | 正确双签通过 Rust 验证并满足 CLVM 条件 | PASS | Voucher 的两个签名聚合后直接作为 SpendBundle 签名提交 simulator，成功创建 Merchant 1 mojo 与 User 9 mojos |
| 3 | 逐项篡改所有签名字段必须失败 | PASS | `every_settlement_field_is_signature_bound` 覆盖 16 个 Settlement 字段；`every_invoice_field_is_signature_bound` 覆盖 9 个 Invoice 字段，25/25 全部拒绝 |
| 4 | 错误网络、funding coin、公钥和过期 Invoice 必须拒绝 | PASS | `wrong_context_and_expired_invoice_are_rejected` 与 `wrong_signing_keys_are_rejected` 返回类型化 `ProtocolError` |
| 5 | 只有双签完成后才能显示 `PAID_OFFCHAIN` | PASS | Invoice 为 `Pending`，Intent 为 `PendingHub`，只有验证完整 Voucher 后返回 `PaidOffchain` |
| 6 | Hub 不签名时为 `PENDING/EXPIRED`，不得显示已付款 | PASS | `incomplete_or_expired_payment_never_looks_paid` 在截止高度内返回 `PendingHub`，过期后返回 `Expired` |

## 实现对象

| Rust 类型 | 职责 |
|---|---|
| `InvoiceFields` | Invoice 的固定顺序规范字段和 `WALL_HUB_INVOICE_V1` hash |
| `MerchantInvoice` | Hub Invoice 签名、网络/coin/channel/金额/过期验证 |
| `SettlementCommitment` | 329 字节 Settlement 语义、CLVM 一致 hash 和 `AGG_SIG_ME` 最终消息 |
| `PaymentIntent` | User 对 Claim 消息的单独签名与验证 |
| `PaymentVoucher` | Hub 验证 Intent、签署同一消息、双签验证和聚合 |
| `MerchantPaymentStatus` | `Pending`、`PendingHub`、`PaidOffchain`、`Expired` |
| `ProtocolError` | 版本、网络、coin、channel、公钥、字段、签名、过期和窗口错误 |

## 签名边界

三种签名使用不同消息域：

```text
Hub Invoice signature:
    invoice_hash

User/Hub Claim signatures:
    settlement_hash || funding_coin_id || agg_sig_me_additional_data

User Refund signature:
    refund_hash || funding_coin_id || agg_sig_me_additional_data
```

`signature_domains_are_distinct` 验证 Invoice、Settlement 和 Refund hash
互不相同。`voucher_signatures_match_clvm_and_settle_in_simulator` 进一步证明
Rust 生成的 User/Hub 聚合签名可直接满足 puzzle 的两个 `AGG_SIG_ME` 条件。

## 测试结果

Day 3 新增 7 个测试，连同前两天测试共 17 个：

```text
17 passed; 0 failed
```

验证命令：

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File .\scripts\compile-puzzles.ps1
powershell -NoProfile -ExecutionPolicy Bypass -File .\scripts\verify-day1.ps1
cargo test --all-targets
cargo clippy --all-targets -- -D warnings
```

第 3 天不包含数据库持久化、并发控制或重启恢复；这些属于 V2 第 4 天范围。
