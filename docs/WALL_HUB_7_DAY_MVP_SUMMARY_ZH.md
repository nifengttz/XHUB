# Wall-Hub 七天 MVP 论证总结报告

日期：2026-08-03  
项目目录：`C:\Users\CJOE\Documents\XHUB`  
最终结论：**MVP PASS，生产就绪度 NOT READY**

## 1. 执行摘要

本轮七天 MVP 要回答的核心问题是：

> User 与 Hub 已共同签发付款 Voucher 后，即使二者随后离线，Merchant 能否在限定高度内，仅凭 Voucher 独立从预锁定的 10 mojo 中领取 1 mojo，并将剩余 9 mojo 返回 User；如果没有 Voucher，User 能否在退款高度后取回完整 10 mojo？

在 Chia 本地 simulator、固定的一次性单向支付模型下，答案是 **可以**。

最终自动化结果：

```text
CLVM compilation: PASS
Protocol vectors: PASS
Offline Merchant Claim: PASS
No-Voucher User Refund: PASS
34 tests passed; 0 failed
Clippy -D warnings: PASS
Final MVP result: PASS
```

该结论证明了核心密码学承诺、链上执行路径、离线兑现、超时退款、持久化恢复和主要攻击边界在当前模型中能够闭环。它不等于主网生产可用。

## 2. MVP 模型与边界

本次实现的是一次性、限时、单向支付通道：

| 项目 | 当前实现 |
|---|---|
| 参与方 | 1 User、1 Hub、1 Merchant |
| Funding | 单个 10 mojo coin |
| 支付次数 | 1 次 |
| 支付金额 | Merchant 1 mojo |
| Claim 输出 | Merchant 1 + User 9 mojos |
| Refund 输出 | User 10 mojos |
| Claim 窗口 | `claim_before_height`，含该高度 |
| Refund 起点 | `refund_height = claim_before_height + 1` |
| 手续费 | 独立 fee coin，不侵蚀通道输出 |
| 环境 | `chia-sdk-test 0.33.0` simulator |
| 状态存储 | SQLite + WAL + 原子事务 |

它不是可反复更新余额的通用状态通道，也不包含多商户聚合、挑战期或 watcher。

## 3. 七天交付与论证结果

| 天数 | 核心工作 | 得到的证据 | 结论 |
|---:|---|---|---|
| 1 | 冻结协议、签名字段、规范编码、coin spend graph | 固定测试向量；所有承诺字段改变都会改变 hash；Claim/Refund 高度不重叠 | PASS |
| 2 | 实现 CLVM funding puzzle 和 simulator 测试 | 双签 Claim 产生 1/9 mojo；Refund 产生 10 mojo；错误金额、签名、coin、分支均拒绝 | PASS |
| 3 | 实现 Invoice、Intent、Voucher 生命周期 | User/Hub 对同一 commitment 签名；Rust 验证结果与 CLVM 一致；未双签不显示已支付 | PASS |
| 4 | 实现 SQLite 状态机和原子持久化 | order/nonce 去重；并发最多签发一个 Voucher；重启后状态和签名产物可恢复 | PASS |
| 5 | 打通结算、退款、fee coin 和确认跟踪 | 广播后保持 Submitted；链上 children 精确匹配后才进入终态 | PASS |
| 6 | 建立篡改、重放、竞争和恢复测试矩阵 | 跨通道/网络重放拒绝；重复领取 DoubleSpend；边界竞争只有一个赢家 | PASS |
| 7 | 建立干净环境一键演示和最终报告 | 两条业务链路可重复运行，输出 coin id、commitment hash、bundle id 和最终余额 | PASS |

## 4. 已经证明了什么

### 4.1 Merchant 不依赖在线 Hub 才能收款

Voucher 一旦完成 User + Hub 双签，Merchant 持有 funding coin 信息、公开通道参数和 Voucher 即可构造 Claim SpendBundle。User 与 Hub 的私钥和在线进程都不再参与结算。

这证明当前模型不是“Merchant 结算时仍需 Hub 批准”的托管式流程。

### 4.2 资金输出受链上程序和签名共同约束

SettlementCommitment 绑定网络、funding coin、channel、订单、nonce、双方 puzzle hash、双方金额、状态编号和高度边界。CLVM 同时固定 funding 金额和输出结构。

有效结果只有两种：

```text
Claim:  Merchant 1 mojo + User 9 mojos
Refund: User 10 mojos
```

独立 fee coin 不改变通道 coin 的两个结算输出。

### 4.3 Claim 与 Refund 在高度上互斥

目标 simulator 中：

```text
height 25: Claim 接受，Refund 拒绝
height 26: Claim 拒绝，Refund 接受
```

两个分支没有同时有效的高度。funding coin 的单次花费规则进一步阻止两组最终输出同时出现。

### 4.4 签发与状态持久化具备最小原子性

SQLite 使用 WAL、`BEGIN IMMEDIATE` 和按 channel 隔离的 order/nonce 唯一约束。Hub 在取得事务写锁、确认状态合法后才签名并持久化 Voucher，因此并发请求最多获得一个有效 Voucher。

状态机区分 Submitted 和链上确认终态，避免“广播即成功”的错误记账：

```text
VOUCHER_ISSUED -> CLAIM_SUBMITTED -> SETTLED
REFUNDABLE -> REFUND_SUBMITTED -> REFUNDED
```

### 4.5 主要篡改和重放路径已被自动化覆盖

测试覆盖所有 Settlement 和 Invoice 签名字段、错误网络、错误 funding coin、错误公钥、跨通道重放、重复广播、输出确认不匹配、重启恢复和 Claim/Refund 边界竞争。失败都有确定错误或状态，不使用模糊结果。

## 5. 尚未证明什么

以下内容不在本轮 PASS 的含义内：

- 未在 Chia 主网或公开 testnet 节点广播、确认和处理重组。
- 未验证真实 mempool 费用、成本上限、手续费估算和 fee coin 选择策略。
- 未实现多次余额更新、旧状态覆盖和挑战期，因此还不是通用状态通道。
- 未实现多商户、多订单聚合或批量结算。
- 未实现 watcher，Merchant 错过 Claim 截止高度仍会永久失去该 Voucher 的链上执行权。
- 未实现生产级密钥托管、硬件签名、备份、轮换和权限隔离。
- 未提供对外 API、CLI 产品接口、认证、限流、审计日志、指标和告警。
- 未测试链重组、RPC 故障、数据库损坏、磁盘耗尽、进程崩溃点注入等运维故障。
- 未经过独立 CLVM、Rust、密码学和协议安全审计。
- 未完成隐私、合规、资金风控和商业运营验证。

因此当前准确表述应是：**核心原理在 simulator 中成立，具备进入工程化原型阶段的条件。**

## 6. 下一步建议

### 阶段 A：公共 testnet 闭环，建议 1 至 2 周

这是最高优先级。目标是把 simulator 假设替换为真实节点行为。

1. 接入 Chia RPC，完成真实 coin 查询、广播、mempool 状态和确认高度跟踪。
2. 在 testnet 创建真实 funding coin，分别运行 Claim 和 Refund。
3. 验证真实 `ASSERT_BEFORE_HEIGHT_ABSOLUTE` 边界、确认延迟和重组后的状态回滚。
4. 实现 fee estimation、独立 fee coin 选择、找零和失败重试。
5. 记录 transaction id、区块高度、coin lineage、费用和最终 children，形成 testnet 验收报告。

退出标准：两条场景各连续成功至少 20 次，并完成至少一次短重组或确认回滚演练。

### 阶段 B：最小服务化与 watcher，建议 1 至 2 周

1. 将 User、Hub、Merchant 拆成独立进程或 CLI，使用版本化 JSON/API 交换 Invoice、Intent 和 Voucher。
2. 增加 Merchant watcher，在安全余量高度前自动广播 Claim，并持续 bump fee 或重试。
3. 增加 User refund watcher，在退款高度后自动恢复资金。
4. 持久化广播记录、SpendBundle id、mempool 状态、确认块和重组历史。
5. 定义幂等 API、稳定错误码、审计日志和基础指标。

退出标准：杀进程、断 RPC、重启数据库连接后，状态能够自动恢复且不会重复签发或错误终结。

### 阶段 C：安全加固，建议 2 至 4 周

1. 建立属性测试和模糊测试，覆盖二进制解码、金额溢出、畸形 CLVM solution 和状态迁移。
2. 加入故障注入：事务提交前后崩溃、磁盘写失败、重复事件、乱序确认和链重组。
3. 建立密钥隔离方案，Hub 私钥不得与 API/数据库进程处于同一信任域。
4. 对 CLVM 成本、签名域、coin lineage、时间边界和资金守恒开展独立审计。
5. 建立可复现构建、依赖锁定、SBOM 和依赖漏洞扫描。

退出标准：无未解决 P0/P1 资金安全问题，审计发现有明确关闭证据。

### 阶段 D：决定产品方向

完成 testnet 和安全加固后，再决定是否扩展协议：

| 方向 | 适用场景 | 主要新增风险 |
|---|---|---|
| 保持一次性 Voucher | 单笔保证金、单次授权支付 | coin 数量、用户体验、手续费 |
| 多商户聚合结算 | Hub 下大量小额商户支付 | 聚合证明、批量失败、数据可用性 |
| 真正多状态通道 | 高频连续支付 | 旧状态发布、挑战期、watcher 强依赖 |
| Hub 托管/信用模式 | 优先追求产品速度 | 对手方风险、合规和资金托管责任 |

不建议现在直接跳到多状态通道。应先用公共 testnet 证明当前一次性模型的真实网络闭环，再根据手续费和业务数据决定协议复杂度是否值得增加。

## 7. 推荐的近期执行清单

接下来最务实的十项工作是：

1. 选择并固定目标 Chia testnet、full node 版本和共识常量。
2. 增加 RPC adapter，但保持现有 simulator adapter 作为快速测试后端。
3. 将确认逻辑改为支持确认深度和 reorg 回滚，而非一次 children 查询。
4. 建立真实 fee coin 选择、找零和费用不足错误。
5. 实现 Merchant watcher 和安全广播余量配置。
6. 提供三个独立 CLI：`user`、`hub`、`merchant`。
7. 为所有外部消息增加 schema version 和严格解析。
8. 增加 crash/restart/reorg 集成测试。
9. 在 testnet 连续执行并保存 20 轮 Claim 与 Refund 证据。
10. 在扩大协议功能前安排一次独立安全评审。

## 8. 最终判断

七天 MVP 达成了预定论证目标：**一次性、单商户、限时 Voucher 的非托管离线兑现原理成立。**

下一阶段不应立即追求 UI 或复杂多状态协议。最关键的是接入公共 testnet、实现 watcher 与重组感知确认，并验证真实手续费和运维故障。只有这些完成后，项目才有依据从“协议原型 PASS”升级为“可试点系统”。
