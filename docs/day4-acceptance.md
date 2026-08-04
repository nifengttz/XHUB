# 第 4 天验收记录

日期：2026-08-03

结论：**PASS**

## 验收结果

| 编号 | V2 计划验收标准 | 结果 | 自动化证据 |
|---:|---|---|---|
| 1 | 同一 `order_id` 只能成功一次 | PASS | SQLite `PRIMARY KEY(channel_id, order_id)` 加事务内预检；`duplicate_order_and_nonce_are_rejected` 返回 `DuplicateOrder` |
| 2 | 同一 `nonce` 不能重复使用 | PASS | SQLite `PRIMARY KEY(channel_id, nonce)` 加事务内预检；重复值返回 `DuplicateNonce` |
| 3 | 并发两个 Intent 最多一个进入 `VOUCHER_ISSUED` | PASS | `concurrent_vouchers_commit_at_most_once` 使用两个线程、两个 SQLite 连接同时提交；Hub 只在 `BEGIN IMMEDIATE` 获得写锁后签名，结果严格为一个成功、一个失败 |
| 4 | Voucher 后余额恒为 Merchant 1 mojo、User 9 mojos | PASS | `voucher_persists_conserved_balances` 从数据库恢复并断言 `1 + 9 = 10 mojos` |
| 5 | 每个状态点强制终止并重启后可恢复 | PASS | `every_state_and_signed_artifact_survives_restart` 在每次迁移后关闭连接并重新打开数据库，状态、Intent 和 Voucher 均逐字节一致 |
| 6 | 非法状态迁移返回明确错误码 | PASS | `illegal_transitions_return_explicit_error` 返回包含来源和目标状态的 `IllegalStateTransition` |

## 持久化状态机

```text
FUNDED
  -> INTENT_SIGNED
  -> VOUCHER_ISSUED
  -> CLAIM_SUBMITTED
  -> SETTLED

FUNDED / INTENT_SIGNED / VOUCHER_ISSUED / CLAIM_SUBMITTED
  -> REFUNDABLE
  -> REFUND_SUBMITTED
  -> REFUNDED
```

`SETTLED` 和 `REFUNDED` 是互斥终态。`REFUND_SUBMITTED` 保留“已广播但尚未
确认”的状态，避免在第 5 天接入链上确认时提前标记退款完成。

## 原子性设计

- SQLite 使用外键、WAL 和 5 秒 busy timeout。
- order 与 nonce 使用按 channel 隔离的复合主键。
- 状态迁移使用 `BEGIN IMMEDIATE`，读取、唯一性判断、签名和写入在同一事务完成。
- 并发签发 API 从 Intent 推导 `channel_id`，调用方不能传入不一致的通道标识。
- Hub 只在事务确认通道仍为 `FUNDED` 后签署 Voucher；失败请求不会获得有效 Hub 签名。
- Voucher 入库前再次执行网络、coin、channel、字段、User/Hub 签名和过期验证。

## 恢复格式

Intent 和 Voucher 使用版本化固定宽度二进制格式：

| 对象 | Magic | 长度 | 内容 |
|---|---|---:|---|
| PaymentIntent | `WHI1` | 455 bytes | Settlement 字段、User 公钥、User 签名 |
| PaymentVoucher | `WHV1` | 603 bytes | 完整 Intent、Hub 公钥、Hub 签名 |

解码器拒绝错误 magic、截断数据、附加数据以及无效 BLS 公钥/签名。

## 测试结果

第 4 天新增 6 个测试，连同前三天测试共 23 个：

```text
23 passed; 0 failed
```

验证命令：

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File .\scripts\compile-puzzles.ps1
powershell -NoProfile -ExecutionPolicy Bypass -File .\scripts\verify-day1.ps1
cargo test --all-targets
cargo clippy --all-targets -- -D warnings
```

第 4 天只保证本地 SQLite 状态与签名产物的原子持久化。真实 Claim/Refund
广播、SpendBundle 确认跟踪和链上状态对账属于第 5 天范围。
