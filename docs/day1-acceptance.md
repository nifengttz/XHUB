# 第 1 天验收记录

日期：2026-08-03

结论：**PASS**

## 验收结果

| 编号 | 计划验收标准 | 结果 | 证据 |
|---:|---|---|---|
| 1 | 所有签名字段、类型、字节序和哈希顺序有唯一说明 | PASS | `docs/protocol-v1.md` 第 6、7 节；三个固定长度 preimage 和确定性向量 |
| 2 | 修改金额、地址、订单号、coin id、网络或截止高度会改变承诺哈希 | PASS | `scripts/verify-day1.ps1` 对 17 个 Settlement 字段逐项变异，17/17 全部产生不同哈希 |
| 3 | 索赔与退款分支在高度上没有重叠 | PASS | simulator 验证高度 999、1000、1001；`claim: height <= 1000`，`refund: height >= 1001` |
| 4 | 明确商户错过截止高度的结果 | PASS | Voucher 必须不晚于 `C = 1000` 被 simulator 接受；到 `R = 1001` 后状态为 `CLAIM_EXPIRED`，用户可退款 |
| 5 | 没有未决 P0 协议问题 | PASS | funding coin、自绑定、签名域、金额、费用、时间边界、退款和状态诚实规则均已冻结 |

## 冻结产物

| 产物 | 内容 |
|---|---|
| `docs/protocol-v1.md` | 安全命题、对象、spend graph、字段编码、签名消息、分支条件和延后范围 |
| `docs/state-machine-v1.md` | 生命周期、终局互斥、商户显示规则、唯一性、错误码和重启恢复 |
| `test-vectors/day1-v1.json` | Invoice、Settlement、Refund 的确定性哈希与 `AGG_SIG_ME` 消息 |
| `scripts/verify-day1.ps1` | 可重复执行的规范向量和安全不变量检查 |

## 已冻结的关键决策

1. V1 只接受 `state_number = 1`，不宣称支持通用多状态通道。
2. `refund_height == claim_before_height + 1`，测试向量使用 `C = 1000`、`R = 1001`。
3. 在目标 simulator 中 Claim 最晚于 `C` 被接受；Refund 只能在 `R` 或之后被接受。
4. 付款签名截止高度至少比 `C` 提前 20 个区块。
5. funding coin 严格守恒，不承担交易费；费用只能来自独立 fee coin。
6. Voucher 必须包含可直接用于链上 `AGG_SIG_ME` 的 User、Hub 两个签名。
7. Hub 不签名时付款未成立，Merchant 不得显示 `PAID_OFFCHAIN`。
8. Settlement 必须包含 `invoice_hash`，使 Intent 和 Voucher 绑定到 Hub 授权的确切 Invoice。

## 验证命令

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File .\scripts\verify-day1.ps1
```

最终输出：

```text
DAY 1 ACCEPTANCE: PASS
```

## 第 2 天实现闸门

下面两项 CLVM 实现验证已在第 2 天完成：

- `chia-sdk-test 0.33.0` 验证了 `ASSERT_BEFORE_HEIGHT_ABSOLUTE` 在 simulator
  当前高度等于参数时仍被接受，因此引入 `C + 1` 的退款高度以保持互斥；
- CLVM 与 Rust 对固定宽度数字字段和 Settlement hash 的结果一致。

受高度修订影响的 Settlement/Refund 向量已重新生成，并继续由
`scripts/verify-day1.ps1` 自动校验。
