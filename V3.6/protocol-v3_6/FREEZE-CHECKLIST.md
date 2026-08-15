# X-Hub V3.6 协议冻结清单

> 对应协议：X-Hub V3.6 协议书  
> 当前阶段：设计草案  
> 目标：在编码、测试网部署和主网上线三个阶段分别冻结必要事项，保证 HUB、钱包、瞭望塔和 CLVM 使用完全相同的协议解释。

## 1. 使用规则

状态只能使用以下取值：

- `OPEN`：尚未决定。
- `DRAFTED`：已有方案，尚未形成测试向量。
- `VECTOR_READY`：已有跨实现测试向量。
- `REVIEWED`：已完成代码和安全评审。
- `FROZEN`：已经冻结；不兼容修改必须升级协议版本，不得继续使用 V3.6 标识。

冻结记录必须包含：

```text
decision:
status:
owner:
reviewer:
decision_date:
evidence:
```

## 2. 编码前必须冻结（P0）

以下项目未冻结前，可以制作原型，但不得把产物标记为规范 V3.6 实现。

### 2.1 哈希规范

- [x] 冻结哈希函数及输出长度。
- [x] 确认所有域分离字符串均为协议列出的 ASCII 原始字节，且不带结尾零字节。
- [x] 冻结多字段哈希的输入规则：直接拼接、长度前缀或规范容器编码。
- [x] 冻结空值、可选值和零长度数组的编码规则。
- [ ] 冻结 `curry_hash` 的计算方法，并与实际 CLVM curry 结果交叉验证。
- [ ] 为每个协议哈希提供至少一个正常向量和一个单字节变更向量。

涉及的哈希至少包括：

```text
channel_terms_hash
state_zero_hash
authorization_hash
entry_hash
leaf_hash
node_hash
empty_root
checkpoint_hash
hub_state_hash
merchant_payment_puzzle_hash
recovery_package_content_hash
delivery_confirmation_hash
double_sign_evidence_hash
reservation_result_hash
conflicting_result_evidence_hash
```

### 2.2 规范二进制编码

- [x] 冻结 `protocol_version = u16_be(0x0360)` 的编码。
- [x] 冻结共识整数为 `u64_be`，并限制在 `0..=2^63-1`。
- [x] 冻结数组编码：元素数量、元素顺序、长度前缀和最大长度。
- [x] 冻结布尔值、枚举、状态码和可选字段的编码。
- [x] 冻结 `network_id` 的类型、长度和值来源。
- [x] 冻结 bytes32、BLS 公钥 48 字节、BLS 签名 96 字节和 nonce 32 字节的解析规则。
- [x] 明确 JSON 仅为 API 表示还是共识编码；不得用普通 JSON 序列化直接生成共识哈希。
- [x] 拒绝非最短、重复字段、未知字段或尾随字节的规则形成测试向量。

### 2.3 BLS 签名规范

- [x] 冻结 BLS 方案：Basic、Augmented 或 Proof of Possession 中的唯一一种。
- [x] 冻结使用的曲线、库兼容基线和签名 DST。
- [x] 冻结签名输入：签原始规范编码还是签固定长度哈希。
- [x] 冻结公钥和签名的压缩编码。
- [x] 冻结非法曲线点、无穷远点、非规范编码和非子群点的拒绝规则。
- [x] 冻结是否允许签名聚合；未明确允许的地方默认不得聚合替代逐条验证。
- [x] 为用户授权、HUB 状态、送达确认和结果签名分别提供正反测试向量。

### 2.4 协议数据结构

- [x] 冻结核心结构的字段集合、顺序、类型、范围及必填规则。
- [x] 冻结 `LedgerEntry`、`LedgerCheckpoint`、`OfficialState` 和 `StateZero`。
- [x] 冻结完整 `RecoveryPackage` 的字段、嵌套顺序和内容哈希覆盖范围。
- [x] 冻结 `DeliveryConfirmation` 的签名消息和接收方身份绑定。
- [x] 冻结链下 `CustodyAttestation` 的签名消息；它绑定商户回执和 RecoveryPackage，但不进入 ChannelTerms 或 CLVM。
- [x] 冻结 `ReservationResult` 的全部状态分支和签名覆盖字段。
- [x] 冻结 `DoubleSignEvidence` 与 `ConflictingResultEvidence` 的规范结构。
- [ ] 冻结 `SealStatement`；明确其始终是可选运营声明，不能替代 OfficialState 或 RecoveryPackage。
- [x] 明确所有 ID 是原始 bytes32 还是带 `0x` 的展示字符串；展示形式不得进入共识哈希。

### 2.5 Merkle 账本

- [x] 冻结叶子顺序为不可变 `entry_index` 顺序。
- [x] 冻结空树根。
- [x] 冻结奇数叶子的处理规则。
- [x] 冻结 padding 规则及 padding 是否进入证明。
- [x] 冻结 proof 中左右方向位的编码。
- [x] 冻结单叶、双叶、奇数叶和 64 叶的根及 proof 向量。
- [x] 冻结 proof 验证失败条件：错方向、错索引、漏节点、多节点和尾随数据。
- [x] 确认同一商户、同一金额的多条记录仍是不同叶子，不得合并或重排。

### 2.6 金额与账本不变量

- [x] 冻结所有金额范围和 checked arithmetic 规则。
- [x] 冻结 `amount > 0`。
- [x] 冻结 `entry_count <= 64`。
- [x] 冻结同一 Funding Coin 内 `reservation_nonce` 全局唯一。
- [x] 冻结 append-only 比较算法；旧记录的每个字段均不可变。
- [x] 冻结 `reserved_total = checked_sum(entries.amount)`。
- [x] 冻结 `reserved_total + user_remainder == funding_amount`。
- [ ] 冻结每条 LedgerEntry 独立生成一枚 Merchant Payment Coin，禁止按商户或金额合并。

### 2.7 状态签名器原子性

- [x] 冻结持久化键 `(funding_coin_id, latest_sequence, latest_checkpoint_hash)`。
- [x] 冻结第一状态必须从 `state_zero_hash` 推进至 sequence 1。
- [x] 冻结相邻状态规则，不允许跳号或跨 checkpoint 签名。
- [x] 冻结“先持久化签名意图，再返回签名”的事务顺序。
- [x] 冻结崩溃恢复、WAL 回放和签名返回丢失时的幂等行为。
- [ ] 冻结同序号不同内容的拒绝、告警和双签证据生成规则。
- [ ] 冻结 HUB A 密钥加载、备份、轮换和紧急停用流程；轮换不得改变现有 Funding Coin 固定的公钥。

### 2.8 高度和链状态

- [x] 建立测试网向量默认 Funding 参数：`acceptance_blocks = 12288`、`freeze_blocks = 200`、`challenge_blocks = 6000`；主网值仍为 `OPEN`。
- [x] 明确这三个值在创建 Funding Coin 的 XHUB 界面中作为“本通道条款参数”输入，而不是运行中可修改的全局常量。
- [x] `close_delay_blocks` 不提供独立输入框，始终由 `acceptance_blocks + freeze_blocks` 自动计算并由服务端校验。
- [x] 三个用户输入值写入 Funding 条款和 `channel_terms_hash`；确认后旧 draft 不可编辑，参数变化生成新 draft。
- [x] 建立测试网参数配置文件、协议有效范围和默认值；主网安全范围与配置文件仍为 `OPEN`。
- [ ] 主网界面必须使用已审核的参数配置文件限制范围，不得允许用户创建低于安全下限的挑战期。
- [x] 冻结 Funding Coin 实际出生高度 `F` 的可信数据来源。
- [x] 冻结 `height >= A` 时拒绝预扣的提交边界和锁顺序。
- [x] 冻结 `effective_A = min(old_A, new_A)` 的重组处理。
- [ ] 冻结初始 Closing Coin 使用 `ASSERT_MY_BIRTH_HEIGHT(C0)` 的规则。
- [ ] 冻结所有后继 Closing Coin 继承同一个 `D`，不得重新计时。
- [x] 冻结节点未同步、RPC 不可用和链状态不确定时暂停预扣的行为。

### 2.9 CLVM 接口

- [x] 冻结 Funding Puzzle 的 curry 参数顺序和 solution 结构。
- [x] 冻结初始 Closing Puzzle 的 curry 参数顺序和 solution 结构。
- [x] 冻结后继 Closing Puzzle 的 curry 参数顺序和 solution 结构。
- [x] 冻结 START_CLOSE、CHALLENGE 和 FINALIZE 的完整账本验证输入。
- [x] 冻结 State 0 的独立空状态验证分支。
- [x] 冻结 Merchant Payment Puzzle 的 curry 参数和无托管原额转发条件。
- [ ] 冻结所有预期 Chia condition 的精确集合，拒绝额外可改变资金流向的分支。
- [ ] 冻结外部 fee Coin 的组合规则；Funding/Closing/Payment Coin 本身不得扣费。

### 2.10 API 和幂等语义

- [x] 冻结预扣请求、状态查询、恢复包获取、投递和投递状态接口的版本字段。
- [x] 冻结 `(funding_coin_id, reservation_nonce)` 作为幂等键。
- [x] 冻结相同 nonce 相同内容返回原结果，相同 nonce 不同内容返回 `NONCE_CONFLICT`。
- [x] 冻结哪些拒绝码保证未写账，哪些错误必须使用原 nonce 查询。
- [x] 冻结 `UNKNOWN`、`RPC_UNAVAILABLE` 和 `INTERNAL_ERROR` 不代表未写账。
- [x] 冻结用户和商户收到相同确定性结果的查询与重试规则。
- [x] 冻结 HTTP JSON 传输表示与规范二进制编码之间的转换规则；CBOR 保持 `OPEN`。

## 3. 测试网实例创建前冻结（P1）

测试网实例一旦创建，以下值不得在该实例中修改：

- [x] `protocol_version = 0x0360`。
- [x] `network_id` 作为 32 字节条款输入并进入规范编码；实例须使用所连接 Full Node 的 genesis challenge。
- [x] Funding、Closing 和 Merchant Payment 的 curry 参数与模块构建版本写入测试网发布清单。
- [x] 创建时由用户确认 `acceptance_blocks`，默认值为 `12288`。
- [x] 创建时由用户确认 `freeze_blocks`，默认值为 `200`。
- [x] `close_delay_blocks` 由前两项自动计算为 `acceptance_blocks + freeze_blocks`，不得手工覆盖。
- [x] 创建时由用户确认 `challenge_blocks`，默认值为 `6000`；参数变化必须创建新 draft/Funding Coin。
- [x] UI 在提交前显示完整参数、计算出的 `A`/`S` 关系和最终 `channel_terms_hash`，要求用户明确确认。
- [x] 服务端拒绝零值、负值、超过 `2^63-1`、非规范十进制和加法溢出；主网安全下限仍为 `OPEN`。
- [x] `max_ledger_entries = 64`。
- [x] 测试激活策略 `funding_confirmation_blocks_test = 32`。
- [x] 测试送达要求一份有效商户回执；托管证明保留 `1-of-3`、`2-of-3`、`3-of-3` 一致性测试。
- [x] HUB A、钱包用户和商户/瞭望塔确认测试公钥清单。
- [ ] 测试 RPC 网络、Genesis/Network ID 校验和最小同步要求。

测试网发布包必须包含：

- [x] 规范文档版本和文档 SHA-256。
- [x] 协议库版本和源码提交状态；未提交组件明确记录为 `UNCOMMITTED`。
- [x] CLVM 源码、编译器版本、构建命令、hex 和模块哈希。
- [ ] 完整跨实现测试向量。
- [ ] 正常关闭、旧状态挑战、State 0 退出和 Merchant Payment 转发样例。
- [x] 已知限制、未冻结主网参数和禁止用于主网的显著声明。

## 4. 主网上线前冻结（P2）

### 4.1 最终参数

- [ ] 对 `challenge_blocks = 6000` 完成书面安全评审并决定接受或升级协议版本。
- [ ] 评估高度对应的实际时间分布、长重组、网络拥堵和手续费市场。
- [ ] 评估瞭望塔最长故障恢复时间、商户离线时间和挑战重试预算。
- [ ] 冻结生产 Delivery 运营身份；候选门槛为一份商户回执加跨故障域 `2-of-3` 独立托管证明。
- [ ] 冻结重组安全余量、RecoveryPackage 保留周期和 Merchant Payment 确认策略。
- [ ] 冻结 fee Coin 最低余额、告警阈值和自动补充/人工补充流程。

### 4.2 最终 CLVM 模块哈希

- [ ] Funding Puzzle 模块哈希。
- [ ] 初始 Closing Puzzle 模块哈希。
- [ ] 后继 Closing Puzzle 模块哈希。
- [ ] CHALLENGE/FINALIZE 所用模块哈希。
- [ ] Merchant Payment Puzzle 模块哈希。
- [ ] `state_rules_hash` 的生成和发布规则。
- [ ] 可复现构建在两台独立机器上得到相同 hex 和模块哈希。

### 4.3 生产部署与故障域

- [ ] HUB 签名器和公开 API 权限分离。
- [ ] HUB 签名密钥不进入日志、普通配置、钱包或瞭望塔。
- [ ] 至少一个 RecoveryPackage 副本由 HUB 之外的参与者保存。
- [ ] 生产瞭望塔满足跨故障域要求；同一 VPS 的多个实例只算一个故障域。
- [ ] 每个生产故障域有独立链状态来源、可用 fee Coin 和独立广播能力。
- [ ] 完成备份恢复、密钥恢复、冷备接管、脑裂和安全停运移交演练。
- [ ] 完成日志脱敏、存储哈希巡检、访问审计和告警演练。

### 4.4 上线测试证据

- [ ] 完成协议书第 20 节全部上线门槛，并为每项保存可复核证据。
- [ ] 验证 0、1、10、64 条记录的 START_CLOSE、CHALLENGE 和 FINALIZE。
- [ ] 测量上述规模下的真实 CLVM cost 和 Spend Bundle 大小。
- [ ] 验证 State 0、最高正式状态和连续多次挑战的全部高度边界。
- [ ] 验证错误签名、Coin ID、条款、金额、nonce、Root、找零和 RecoveryPackage 均失败。
- [ ] 验证 HUB 永久离线后第三方仍可独立关闭、挑战和结算。
- [ ] 验证回复丢失、重复请求、冻结竞态、RPC 冲突、节点落后和链重组。
- [ ] 由至少一名未参与核心实现的评审者复核协议、Rust 和 CLVM。
- [ ] 发布最终安全评审结论和剩余风险清单。

## 5. 跨代码库一致性冻结

HUB、钱包和瞭望塔不得各自维护一份不同的协议算法。

- [x] `protocol-v3_6` 是规范类型、编码、哈希、签名和测试向量的唯一来源。
- [x] 钱包、HUB 和瞭望塔通过路径依赖使用同一 `protocol-v3_6` 实现，并由跨库测试核对共识哈希。
- [ ] 所有代码库 CI 均运行同一份 golden vectors。
- [ ] CLVM hex 和模块哈希由同一可复现构建产物发布，不允许手工复制后修改。
- [ ] API 兼容矩阵明确记录客户端、HUB、瞭望塔和协议版本组合。
- [ ] V3.6 不读取或生成 V3.5 及更早版本的 Funding Coin、Closing Coin、哈希域或签名域。

### 5.1 Funding 参数输入协议

Funding 创建界面必须采用以下交互规则：

```text
acceptance_blocks  = 用户输入，默认 12288
freeze_blocks      = 用户输入，默认 200
close_delay_blocks = acceptance_blocks + freeze_blocks（自动计算）
challenge_blocks   = 用户输入，默认 6000
```

- [x] 用户确认前显示测试网参数配置名称、协议有效范围和禁止主网使用提示；主网配置仍为 `OPEN`。
- [ ] 用户确认后，客户端将参数原样发送给 HUB，HUB 重新校验，不信任客户端校验结果。
- [ ] HUB 计算并核对 `close_delay_blocks`，拒绝客户端传入的不一致值。
- [ ] Funding Puzzle、钱包和 HUB 对参数使用同一规范编码和同一 `channel_terms_hash`。
- [x] 已确认 draft 的参数不可编辑，不会改变已有通道或正式账本。
- [x] 参数变更生成新的协议条款和 draft；不能通过 UI 编辑已确认条款。

## 6. 最小测试向量集合

- [x] ChannelTerms 正常向量、输入边界和默认测试网参数向量。
- [x] State 0 向量。
- [x] 单条和多条 UserAuthorization 向量。
- [x] 0、1、2、3、10、64 叶 Merkle root/proof 向量。
- [ ] 同商户同金额不同 nonce 的独立 Payment Coin 向量。
- [x] OfficialState sequence 1、连续推进、跳号和错误 previous hash 向量。
- [ ] RecoveryPackage 完整、截断、篡改、重放和同序号冲突向量。
- [ ] ReservationResult 成功、确定拒绝、UNKNOWN 和冲突证据向量。
- [ ] A-1、A、A+1、S-1、S、C0、D-1、D、D+1 高度边界向量。
- [x] A-1、A、A+1、提交时跨 A、RPC/同步故障和 `effective_A` HUB 门控向量。
- [ ] State 0 START_CLOSE/FINALIZE 向量。
- [ ] 非空状态 START_CLOSE、CHALLENGE、FINALIZE 向量。
- [ ] 外部 fee Coin 组合和 fee Coin 不足向量。
- [x] 无效 BLS 公钥、签名、曲线点和编码向量。

## 7. 冻结签署表

`VECTOR_READY` 的当前证据范围以 [实现规范附录](IMPLEMENTATION-SPEC.md) 第 6、7 节为准；未勾选项目仍属于后续阶段，不因同类核心模块达到 `VECTOR_READY` 而自动完成。

| 冻结对象 | 状态 | 决策人 | 评审人 | 日期 | 证据/版本 |
|---|---|---|---|---|---|
| 哈希与规范编码 | VECTOR_READY | Codex 实现 | 待独立评审 | 2026-08-12 | `IMPLEMENTATION-SPEC.md`; vectors-1 |
| BLS 签名规范 | VECTOR_READY | Codex 实现 | 待独立评审 | 2026-08-12 | `chia-bls 0.36.1`; vectors-1 |
| 协议数据结构 | VECTOR_READY | Codex 实现 | 待独立评审 | 2026-08-12 | 第 6 节所列核心结构；vectors-1 |
| Merkle 规则 | VECTOR_READY | Codex 实现 | 待独立评审 | 2026-08-12 | duplicate-last；1/2/3/10/64 叶测试 |
| 状态签名器原子性 | VECTOR_READY | Codex 实现 | 待独立评审 | 2026-08-12 | `../hub-v3_6`; WAL/FULL/IMMEDIATE；2 个故障注入点；6 写入者并发测试 |
| HTTP API 与幂等语义 | VECTOR_READY | Codex 实现 | 待独立评审 | 2026-08-12 | `../hub-v3_6/HTTP-API.md`; `src/api.rs`; `tests/http_api.rs` |
| 瞭望塔包验证与测试绿灯聚合 | VECTOR_READY | Codex 实现 | 待独立评审 | 2026-08-12 | `../watchtower-v3_6/src/lib.rs`; `tests/watchtower.rs`; `tests/http_api.rs` |
| Reservation 幂等核心 | VECTOR_READY | Codex 实现 | 待独立评审 | 2026-08-12 | `../hub-v3_6/test-vectors/hub-v3_6.json`; 相同请求/nonce 冲突/冻结拒绝 |
| 高度与链状态门控 | VECTOR_READY | Codex 实现 | 待独立评审 | 2026-08-12 | `../hub-v3_6/src/chain.rs`; `tests/chain_gate.rs`; 主备源、A 边界、32 确认、重组测试 |
| 测试网参数与钱包创建流程 | VECTOR_READY | Codex 实现 | 待独立评审 | 2026-08-12 | `../wallet-v3_6/config/testnet-vector-profile-v1.json`; `tests/funding_terms.rs`; `tests/http_api.rs` |
| 跨代码库兼容性 | VECTOR_READY | Codex 实现 | 待独立评审 | 2026-08-12 | `../wallet-v3_6/tests/cross_repo.rs`; 钱包/HUB/瞭望塔四类共识哈希一致 |
| 测试网发布清单 | VECTOR_READY | Codex 实现 | 待独立评审 | 2026-08-12 | `../release/testnet-release-v3_6.json`; SHA-256 生成脚本；源码状态 `UNCOMMITTED` |
| 测试网服务部署适配 | VECTOR_READY | Codex 实现 | 待独立评审 | 2026-08-12 | `../deploy/testnet`; HUB/Watchtower 可执行入口；Bearer 认证；本地三服务 smoke |
| HUB 到瞭望塔 HTTP 投递 | VECTOR_READY | Codex 实现 | 待独立评审 | 2026-08-12 | `../hub-v3_6/src/transport.rs`; `tests/watchtower_transport.rs`; token/hash/失败分类 |
| CLVM 接口 | VECTOR_READY | Codex 实现 | 待独立评审 | 2026-08-12 | `../puzzles-v3_6/README.md`; `tests/puzzles.rs`; 8 项执行测试 |
| CLVM 模块哈希 | VECTOR_READY | Codex 实现 | 待独立评审 | 2026-08-12 | `../puzzles-v3_6/module-hashes.json`; 4 个 `.clsp.hex`; 可复现编译脚本 |
| 瞭望塔只读链监控与 CHALLENGE 预演 | VECTOR_READY | Codex 实现 | 待独立评审 | 2026-08-12 | `../watchtower-v3_6/src/monitor.rs`; `src/rpc.rs`; `tests/chain_monitor.rs`; 不创建 SpendBundle、不广播 |
| 不可广播 CHALLENGE SpendBundle 构造与完整验证 | VECTOR_READY | Codex 实现 | 待独立评审 | 2026-08-12 | `../puzzles-v3_6/src/closing.rs`; `../watchtower-v3_6/src/bundle.rs`; `tests/offline_bundle.rs`; 测试 fee Coin；不导出、不广播 |
| 监控计划到离线准备的失效状态机 | VECTOR_READY | Codex 实现 | 待独立评审 | 2026-08-13 | `../watchtower-v3_6/src/preparation.rs`; `tests/chain_monitor.rs`; 不保存 bundle/私钥；不批准、不广播 |
| 主网挑战参数 | OPEN |  |  |  |  |
| 生产瞭望塔门槛 | OPEN |  |  |  |  |
| 全部上线测试证据 | OPEN |  |  |  |  |

### 7.1 CLVM 第二阶段决策记录

```text
decision: 冻结 Funding、Initial Closing、Subsequent Closing 和 Merchant Payment 的位置接口，作为 V3.6 测试向量基线
status: VECTOR_READY
owner: Codex 实现
reviewer: 待独立评审
decision_date: 2026-08-12
evidence: ../puzzles-v3_6/README.md; ../puzzles-v3_6/tests/puzzles.rs; cargo test（8 passed）
```

```text
decision: 固定四个 CLVM 编译产物的候选模块哈希；当前不是主网最终哈希
status: VECTOR_READY
owner: Codex 实现
reviewer: 待独立评审
decision_date: 2026-08-12
evidence: ../puzzles-v3_6/module-hashes.json; ../puzzles-v3_6/compile-puzzles.ps1; run/opc 0.4.0 --strict --optimize
```

当前候选模块哈希：

```text
Funding:            e2945105091602fb91db08af00525153604007791be6e673372e33880eb2e6ce
Initial Closing:    95d2aa194ef302ac4637280031e3492736e231490a4883cc2ace090551c18b59
Subsequent Closing: e1a73c7381c56817159558594e13558c22d5d7ac8a8c5c81a53132335e8d1e29
Merchant Payment:   b53e39fa4960713ced442f21331672ea38d73c763cc690f37089ebd3aee5ffe1
```

第 4.2 节的主网最终模块哈希、双机可复现构建和独立安全评审仍保持未完成；任何影响上述接口、hex 或哈希的修改都必须同步更新向量与本记录，且不得修改已经创建的测试实例。

### 7.2 HUB 第三阶段决策记录

```text
decision: 固定 V3.6 状态签名器的持久化键、相邻状态、append-only 校验和先落盘后签名事务顺序
status: VECTOR_READY
owner: Codex 实现
reviewer: 待独立评审
decision_date: 2026-08-12
evidence: ../hub-v3_6/src/lib.rs; ../hub-v3_6/tests/hub_state.rs; WAL/FULL/IMMEDIATE; 2 个故障注入点；6 写入者并发测试
```

```text
decision: 固定 reservation 幂等键、相同请求原结果返回、nonce 内容冲突拒绝及确定性冻结拒绝
status: VECTOR_READY
owner: Codex 实现
reviewer: 待独立评审
decision_date: 2026-08-12
evidence: ../hub-v3_6/test-vectors/hub-v3_6.json; ../hub-v3_6/tests/golden_vectors.rs
```

```text
decision: 固定 SignedReservationResult、DoubleSignEvidence 和 ConflictingResultEvidence 的规范编码、排序和验证规则
status: VECTOR_READY
owner: Codex 实现
reviewer: 待独立评审
decision_date: 2026-08-12
evidence: IMPLEMENTATION-SPEC.md; test-vectors/protocol-v3_6.json; tests/evidence.rs
```

第三阶段不包含 HTTP/CBOR 传输、UNKNOWN/RPC 错误查询流程、HUB A 密钥轮换、外部 RecoveryPackage 投递或瞭望塔绿灯；可信链峰值读取和重组门控已在第四阶段补齐。

### 7.3 HUB 第四阶段决策记录

```text
decision: Funding Coin 出生高度和提交高度只接受 ChainStateProvider 从 Chia Full Node RPC 读取并验证的链状态
status: VECTOR_READY
owner: Codex 实现
reviewer: 待独立评审
decision_date: 2026-08-12
evidence: ../hub-v3_6/src/chain.rs; get_network_info/get_blockchain_state/get_coin_record_by_name; HTTPS/mTLS
```

```text
decision: 预扣在 BEGIN IMMEDIATE 写锁内执行第二次链快照，最终以该快照判定 height < A；A 及之后拒绝且不写账
status: VECTOR_READY
owner: Codex 实现
reviewer: 待独立评审
decision_date: 2026-08-12
evidence: ../hub-v3_6/tests/chain_gate.rs; A-1/A/A+1；请求到达 A 前但提交时到 A
```

```text
decision: 激活后重组使用 effective_A = min(previous_effective_A, new_F + acceptance_blocks)，不得延长接受期
status: VECTOR_READY
owner: Codex 实现
reviewer: 待独立评审
decision_date: 2026-08-12
evidence: ../hub-v3_6/test-vectors/hub-v3_6.json; Funding Coin 消失/重现；主备 RPC 冲突；32 确认激活测试
```

第四阶段完成后，HTTP/CBOR API、UNKNOWN 查询流程、主网 RPC 高度差阈值、主网 Funding 确认深度、HUB A 密钥轮换、外部 RecoveryPackage 投递和瞭望塔绿灯仍保持 `OPEN`。

### 7.4 HUB 第五阶段决策记录

```text
decision: 固定 /api/v3.6 的预扣提交、原 nonce 状态查询、RecoveryPackage 获取及投递状态接口
status: VECTOR_READY
owner: Codex 实现
reviewer: 待独立评审
decision_date: 2026-08-12
evidence: ../hub-v3_6/HTTP-API.md; ../hub-v3_6/src/api.rs; ../hub-v3_6/tests/http_api.rs
```

```text
decision: JSON 仅是传输表示；响应 hex 固定为无 0x 小写形式，并同时公开 SignedReservationResult 与 RecoveryPackage 的规范二进制 hex
status: VECTOR_READY
owner: Codex 实现
reviewer: 待独立评审
decision_date: 2026-08-12
evidence: ../hub-v3_6/tests/http_api.rs; 版本头与版本字段双重校验；严格金额和定长 hex 解析
```

```text
decision: UNKNOWN、RPC_UNAVAILABLE 和 INTERNAL_ERROR 返回 ledger_written = null，客户端必须使用原 nonce 查询；确定性拒绝才返回 false
status: VECTOR_READY
owner: Codex 实现
reviewer: 待独立评审
decision_date: 2026-08-12
evidence: ../hub-v3_6/HTTP-API.md; RPC/UNKNOWN/确定性拒绝集成测试
```

第五阶段只冻结 HTTP JSON 测试向量接口。CBOR、认证/TLS/限流部署、真实商户与瞭望塔网络适配器、DeliveryConfirmation 绿灯聚合、主网兼容矩阵和独立安全评审仍保持 `OPEN`。

### 7.5 瞭望塔第六阶段决策记录

```text
decision: 瞭望塔先完整验证 RecoveryPackage，再以 WAL/FULL SQLite 持久化有效包；失败包进入隔离区
status: VECTOR_READY
owner: Codex 实现
reviewer: 待独立评审
decision_date: 2026-08-12
evidence: ../watchtower-v3_6/src/lib.rs; ../watchtower-v3_6/tests/watchtower.rs; 截断/篡改/同序号冲突/append-only 测试
```

```text
decision: DeliveryConfirmation 只允许使用目标 LedgerEntry 的 merchant_receipt_public_key，并绑定相同内容哈希
status: VECTOR_READY
owner: Codex 实现
reviewer: 待独立评审
decision_date: 2026-08-12
evidence: ../watchtower-v3_6/src/lib.rs; ../watchtower-v3_6/tests/watchtower.rs; 未持久化包禁止签确认
```

```text
decision: 商户 DeliveryConfirmation 只贡献一份交付回执；生产候选绿灯另外按不同 Watchtower 公钥和不同故障域聚合 CustodyAttestation，禁止重复公钥伪造 2-of-3
status: VECTOR_READY
owner: Codex 实现
reviewer: 待独立评审
decision_date: 2026-08-14
evidence: ../watchtower-v3_6/src/custody.rs; ../watchtower-v3_6/tests/watchtower.rs; 重复公钥和同故障域均不满足 2-of-3
```

第六阶段仍不包含生产身份认证、TLS/限流、真实跨运营者故障域证明和主网绿灯运营批准。CustodyAttestation 只解决密码学身份分离与本地聚合，不证明实际运营独立性。

### 7.6 钱包与测试网发布第七阶段决策记录

```text
decision: Funding 创建界面固定由用户输入 acceptance_blocks、freeze_blocks 和 challenge_blocks；close_delay_blocks 只读自动计算并由协议库重新校验
status: VECTOR_READY
owner: Codex 实现
reviewer: 待独立评审
decision_date: 2026-08-12
evidence: ../wallet-v3_6/web; ../wallet-v3_6/src/lib.rs; ../wallet-v3_6/tests/funding_terms.rs; ../wallet-v3_6/tests/http_api.rs
```

```text
decision: 用户必须在服务端生成 channel_terms_hash 后明确确认；确认后的 draft 不可编辑，参数变化生成新 draft 和新 Funding Coin
status: VECTOR_READY
owner: Codex 实现
reviewer: 待独立评审
decision_date: 2026-08-12
evidence: ../wallet-v3_6/src/api.rs; ../wallet-v3_6/web/app.js; confirm hash mismatch/immutable draft tests
```

```text
decision: 钱包、HUB 和瞭望塔使用同一 protocol-v3_6 类型与算法，并端到端核对 channel terms、checkpoint、RecoveryPackage content 和 authorization 哈希
status: VECTOR_READY
owner: Codex 实现
reviewer: 待独立评审
decision_date: 2026-08-12
evidence: ../wallet-v3_6/tests/cross_repo.rs; HUB 生成 RecoveryPackage；瞭望塔验包并聚合 1-of-3 测试绿灯
```

```text
decision: 测试网发布清单由脚本计算协议、向量、配置、CLVM 源码和 hex 的 SHA-256；未提交源码不得引用旧 Git HEAD
status: VECTOR_READY
owner: Codex 实现
reviewer: 待独立评审
decision_date: 2026-08-12
evidence: ../release/generate-release.ps1; ../release/testnet-release-v3_6.json; source_commits=UNCOMMITTED
```

第七阶段只建立测试网向量基线。主网参数安全范围、`challenge_blocks` 安全评审、生产 Watchtower 托管证明 `2-of-3` 门槛、最终主网 CLVM 模块哈希和独立安全评审继续保持 `OPEN`。

### 7.7 测试网部署适配第八阶段决策记录

```text
decision: HUB 和瞭望塔提供独立可执行服务，默认只允许 loopback 监听；外部 TLS 由反向代理终止
status: VECTOR_READY
owner: Codex 实现
reviewer: 待独立评审
decision_date: 2026-08-12
evidence: ../hub-v3_6/src/main.rs; ../watchtower-v3_6/src/main.rs; ../deploy/testnet/Caddyfile; 非 loopback 启动拒绝
```

```text
decision: HUB 和瞭望塔 /api/v3.6 接口要求 Bearer token，token 只从独立文件读取；缺失或错误 token 返回 401
status: VECTOR_READY
owner: Codex 实现
reviewer: 待独立评审
decision_date: 2026-08-12
evidence: authenticated_router; HUB/Watchtower HTTP API 认证测试；本地 smoke unauthenticated_requests_rejected=true
```

```text
decision: HUB 使用真实 HTTP 适配器向指定瞭望塔投递规范 RecoveryPackage，携带版本头、Bearer token 和幂等键，并核对响应内容哈希
status: VECTOR_READY
owner: Codex 实现
reviewer: 待独立评审
decision_date: 2026-08-12
evidence: ../hub-v3_6/src/transport.rs; ../hub-v3_6/tests/watchtower_transport.rs; 正确 token 成功、401 最终失败、错误接收者不发送
```

```text
decision: Funding Coin 注册由 HUB 从 Full Node RPC 重新读取 genesis、同步状态、CoinRecord、出生高度、puzzle hash、金额和 32 区块确认深度
status: VECTOR_READY
owner: Codex 实现
reviewer: 待独立评审
decision_date: 2026-08-12
evidence: POST /api/v3.6/funding-coins; xhub-rpc-preflight; ../deploy/testnet/rpc-preflight.ps1; 现有 ChainStateProvider 边界测试
```

本阶段完成的是部署适配和本机联调证据。真实测试网 Full Node 预检、实际 Funding Coin 注册、外部 TLS 证书验证、限流压测、跨 VPS 故障域和链上完整生命周期仍保持 `OPEN`，需要真实测试网端点与 Coin ID 后执行。

### 7.8 瞭望塔链监控与挑战预演第九阶段决策记录

```text
decision: 瞭望塔从独立 Full Node RPC 读取 Funding Coin、CoinSpend 和 Closing Coin，不接受调用方声明的链上状态序号或 puzzle hash
status: VECTOR_READY
owner: Codex 实现
reviewer: 待独立评审
decision_date: 2026-08-12
evidence: ../watchtower-v3_6/src/rpc.rs; src/monitor.rs; Funding reveal/CoinRecord、solution/RecoveryPackage、Closing puzzle hash/Coin ID 三重绑定
```

```text
decision: 仅当链上 Closing State 低于本地最新完整 RecoveryPackage 且 peak < D 时运行真实 CHALLENGE CLVM 并持久化幂等预演计划
status: VECTOR_READY
owner: Codex 实现
reviewer: 待独立评审
decision_date: 2026-08-12
evidence: ../puzzles-v3_6/src/closing.rs simulate_challenge; ../watchtower-v3_6/tests/chain_monitor.rs; Initial/Subsequent、D 边界、重组、UNKNOWN、FINALIZED 测试
```

```text
decision: 当前挑战计划不创建 SpendBundle、不加载私钥或 fee Coin、不连接广播端点；RPC UNKNOWN 和重组状态禁止行动
status: VECTOR_READY
owner: Codex 实现
reviewer: 待独立评审
decision_date: 2026-08-12
evidence: spend_bundle_created=false; broadcast_ready=false; chain_broadcast=false; SIMULATED_ONLY/RETRY_SCHEDULED 持久化状态
```

第九阶段不包含可广播 SpendBundle 构造、签名聚合、外部 fee Coin 选择、mempool 提交、确认跟踪或跨故障域传播。这些生产广播能力和主网安全策略仍保持 `OPEN`。

### 7.9 不可广播 SpendBundle 第十阶段决策记录

```text
decision: Closing puzzle reveal、CHALLENGE solution 和协议签名材料只由 puzzles-v3_6 构造；瞭望塔不得复制 CLVM solution 布局
status: VECTOR_READY
owner: Codex 实现
reviewer: 待独立评审
decision_date: 2026-08-12
evidence: ../puzzles-v3_6/src/closing.rs ChallengeSpendMaterial; ../watchtower-v3_6/tests/offline_bundle.rs
```

```text
decision: 离线候选使用真实 Chia CoinSpend/SpendBundle，由 chia_consensus 验证条件、cost、金额守恒、RESERVE_FEE、重复 Coin 和聚合 BLS 签名
status: VECTOR_READY
owner: Codex 实现
reviewer: 待独立评审
decision_date: 2026-08-12
evidence: ../watchtower-v3_6/src/bundle.rs; State 0/Initial/Subsequent、D-1/D、Coin/fee/重组负例
```

```text
decision: fee sponsor 仅使用 P2DelegatedConditions 测试向量 Coin 和测试密钥；候选只存在内存，不导出规范 bytes，不提供 push_tx 或广播客户端
status: VECTOR_READY
owner: Codex 实现
reviewer: 待独立评审
decision_date: 2026-08-12
evidence: broadcast_enabled=false; broadcast_ready=false; chain_broadcast=false; 生产 fee sponsor 和真实钱包私钥保持 OPEN
```

第十阶段不改变第九阶段已持久化计划的 `SIMULATED_ONLY` 行为，也不自动接入监控器。生产 fee Coin 选择、密钥托管、主网共识常量、广播审批、`push_tx`、mempool/确认跟踪和跨故障域传播继续保持 `OPEN`。

### 7.10 监控计划到离线准备第十一阶段决策记录

```text
decision: 只有已持久化的 SIMULATED_ONLY 挑战计划才能显式进入离线准备；准备时重新绑定 RecoveryPackage、Closing Coin、C0、D、链峰值和测试 fee Coin
status: VECTOR_READY
owner: Codex 实现
reviewer: 待独立评审
decision_date: 2026-08-13
evidence: ../watchtower-v3_6/src/preparation.rs; ../watchtower-v3_6/tests/chain_monitor.rs
```

```text
decision: 离线验证成功只进入 OFFLINE_VERIFIED_AWAITING_APPROVAL；SQLite 仅保存验证报告和链快照，不保存 SpendBundle bytes、测试私钥或签名材料
status: VECTOR_READY
owner: Codex 实现
reviewer: 待独立评审
decision_date: 2026-08-13
evidence: v36_offline_challenge_preparations; broadcast_enabled=false; broadcast_ready=false; chain_broadcast=false
```

```text
decision: 新峰值、重组、Closing Coin 已花费、截止高度和 RPC UNKNOWN 必须撤销先前离线验证结论；只能基于新快照重新准备
status: VECTOR_READY
owner: Codex 实现
reviewer: 待独立评审
decision_date: 2026-08-13
evidence: INVALIDATED_CHAIN_CHANGE; CHAIN_RECHECK_REQUIRED; same-snapshot/idempotency/new-peak/RPC-UNKNOWN tests
```

第十一阶段没有人工批准写入接口、生产 fee Coin 选择、密钥托管、bundle 导出、`push_tx`、mempool 提交或确认跟踪。它只建立广播前的强制失效边界，所有生产广播能力继续保持 `OPEN`。

### 7.11 人工审批与双人复核第十二阶段决策记录

```text
decision: 审批声明使用独立 V3.6 域和规范二进制签名，绑定 preparation/Closing/Funding/fee Coin ID、报告哈希、链峰值、D、审批身份、故障域、时间窗与 nonce
status: VECTOR_READY
owner: Codex 实现
reviewer: 待独立评审
decision_date: 2026-08-13
evidence: ../watchtower-v3_6/src/approval.rs; 签名篡改、字段绑定、过期、重复身份/公钥/nonce 测试
```

```text
decision: 双人复核要求两个不同审批者且属于两个不同故障域；单票为 PARTIALLY_APPROVED，双票只进入 DUAL_APPROVED_RECHECK_REQUIRED
status: VECTOR_READY
owner: Codex 实现
reviewer: 待独立评审
decision_date: 2026-08-13
evidence: v36_challenge_approvals; ../watchtower-v3_6/tests/chain_monitor.rs; broadcast_enabled=false; broadcast_ready=false; chain_broadcast=false
```

```text
decision: RPC UNKNOWN、新峰值、同高度重组、Closing Coin 变化或 peak >= D 撤销全部有效审批；重新准备生成新 epoch，旧凭证保留审计但不得重放
status: VECTOR_READY
owner: Codex 实现
reviewer: 待独立评审
decision_date: 2026-08-13
evidence: APPROVAL_REVOKED_CHAIN_CHANGE; D-1/D、RPC UNKNOWN、same-height reorg、fresh preparation tests
```

第十二阶段没有接入真实审批密钥托管、生产身份注册、SpendBundle 持久化或导出、最终链上重检、`push_tx`、mempool 提交和确认跟踪。即使满足双人复核，所有广播能力仍保持 `OPEN`。

### 7.12 最终链上重检第十三阶段决策记录

```text
decision: 双人批准后必须由 WatchtowerChainProvider 从 Full Node RPC 自行读取并推导 Funding/Closing 谱系；不信任调用者声明的当前链状态
status: VECTOR_READY
owner: Codex 实现
reviewer: 待独立评审
decision_date: 2026-08-13
evidence: ../watchtower-v3_6/src/final_recheck.rs poll_final_chain_recheck; RPC-derived success/unsynced UNKNOWN tests
```

```text
decision: 最终重检重新解码和验证两张 BLS 审批，核对签名字段与 SQLite 索引列，绑定 approval_set_hash、preparation、Coin ID、报告哈希、峰值和 D
status: VECTOR_READY
owner: Codex 实现
reviewer: 待独立评审
decision_date: 2026-08-13
evidence: verified_approval_set; v36_final_chain_rechecks; approval index tampering test
```

```text
decision: 通过状态仅为 FINAL_RECHECK_VERIFIED_NO_BROADCAST，有效期取 30 秒和审批最早过期时间的较小值；RPC UNKNOWN、重组、快照变化或 peak >= D 立即失效
status: VECTOR_READY
owner: Codex 实现
reviewer: 待独立评审
decision_date: 2026-08-13
evidence: expiry/idempotency/D-boundary/reorg/RPC-UNKNOWN tests; broadcast_enabled=false; broadcast_ready=false; chain_broadcast=false
```

第十三阶段仍没有生产 fee sponsor、真实密钥托管、SpendBundle 持久化或导出、`push_tx`、mempool 提交和确认跟踪。最终链上重检记录不是广播授权，所有生产广播能力继续保持 `OPEN`。

### 7.13 SpendBundle 承诺绑定第十四阶段决策记录

```text
decision: 使用 XHUB_SPEND_BUNDLE_COMMITMENT_V3_6 域，按 CoinSpend 原顺序承诺数量、parent、puzzle hash、u64 amount、u64 长度前缀的完整 puzzle reveal/solution，最后承诺 96 字节聚合签名
status: VECTOR_READY
owner: Codex 实现
reviewer: 待独立评审
decision_date: 2026-08-13
evidence: ../watchtower-v3_6/src/bundle.rs spend_bundle_commitment; 稳定性、顺序、solution、fee material 测试
```

```text
decision: bundle_commitment 必须贯穿 offline preparation、preparation ID、双人审批规范声明和最终链上重检；任一处不一致均拒绝继续
status: VECTOR_READY
owner: Codex 实现
reviewer: 待独立评审
decision_date: 2026-08-13
evidence: v36_offline_challenge_preparations; v36_final_chain_rechecks; commitment tampering/end-to-end binding tests
```

```text
decision: 只持久化 32 字节承诺，不保存或导出 SpendBundle bytes；旧数据库增加可空迁移列，旧准备必须重建并重新审批，不得从旧报告推测承诺
status: VECTOR_READY
owner: Codex 实现
reviewer: 待独立评审
decision_date: 2026-08-13
evidence: migrate_bundle_commitment_columns; pre-commitment database migration test; no bundle export/push_tx
```

第十四阶段没有改变不可广播边界。生产 fee sponsor、真实密钥托管、SpendBundle 导出、最终执行授权、`push_tx`、mempool 提交和确认跟踪继续保持 `OPEN`。

### 7.14 广播前 Execution Manifest 第十五阶段决策记录

```text
decision: 仅有效 FINAL_RECHECK_VERIFIED_NO_BROADCAST 可签发 XHUB_EXECUTION_MANIFEST_V3_6；签发时二次核对 preparation、双审批和重算 approval_set_hash
status: VECTOR_READY
owner: Codex 实现
reviewer: 待独立评审
decision_date: 2026-08-13
evidence: ../watchtower-v3_6/src/manifest.rs; tampered final recheck binding test
```

```text
decision: Manifest 绑定 recheck/preparation、Closing/Funding/fee Coin、report、bundle commitment、approval set、peak/header、D 和时间窗；TTL 为 10 秒与 recheck 剩余时间的较小值
status: VECTOR_READY
owner: Codex 实现
reviewer: 待独立评审
decision_date: 2026-08-13
evidence: v36_execution_manifests; binding/idempotency/expiry tests
```

```text
decision: 新重检将旧清单标为 MANIFEST_SUPERSEDED；RPC UNKNOWN、重组、快照变化或重建准备标为 MANIFEST_INVALIDATED_CHAIN_CHANGE；清单始终不包含 SpendBundle bytes 或广播能力
status: VECTOR_READY
owner: Codex 实现
reviewer: 待独立评审
decision_date: 2026-08-13
evidence: supersede/UNKNOWN/reorg tests; broadcast_enabled=false; broadcast_ready=false; chain_broadcast=false
```

第十五阶段仍不提供最终执行授权、SpendBundle 导出、真实 fee sponsor、私钥托管、`push_tx`、mempool 提交或确认跟踪。Execution Manifest 只是短期审计对象，所有广播能力继续保持 `OPEN`。

### 7.15 最终执行授权闸门第十六阶段决策记录

```text
decision: 仅有当前有效的 MANIFEST_VERIFIED_NO_BROADCAST 才能签发 EXECUTION_AUTHORIZED_SIMULATED_ONLY；签发时重新核对最终重检、离线准备、双故障域审批集合及全部执行哈希，授权 TTL 最多 5 秒
status: VECTOR_READY
owner: Codex 实现
reviewer: 待独立评审
decision_date: 2026-08-13
evidence: ../watchtower-v3_6/src/authorization.rs; execution authorization binding/expiry/supersede/UNKNOWN/tamper tests
```

```text
decision: 授权只允许记录模拟提交次数和时间，不保存或导出 SpendBundle，不接入 push_tx、mempool 或真实广播端点；Manifest、RPC UNKNOWN、重组和准备重建会传播失效
status: VECTOR_READY
owner: Codex 实现
reviewer: 待独立评审
decision_date: 2026-08-13
evidence: v36_execution_authorizations; broadcast_enabled=false; broadcast_ready=false; chain_broadcast=false; strict clippy
```

第十六阶段仍不提供生产私钥托管、真实 fee sponsor、SpendBundle 持久化或导出、主网广播和确认跟踪。授权对象只是模拟执行前的短期审计闸门，所有广播能力继续保持 `OPEN`。

### 7.16 执行授权 HTTP API 第十七阶段决策记录

```text
decision: 使用 /api/v3.6 提供 ExecutionAuthorization 的签发、查询和模拟提交接口；协议版本必须同时出现在 x-xhub-protocol-version 头和请求体或查询参数中
status: VECTOR_READY
owner: Codex 实现
reviewer: 待独立评审
decision_date: 2026-08-13
evidence: ../watchtower-v3_6/src/api.rs; versioned authorization HTTP route tests
```

```text
decision: HTTP 响应只暴露授权绑定哈希、Coin ID、链快照、状态、时间窗和模拟计数；请求拒绝未知字段及 SpendBundle bytes，NOT_FOUND/NOT_AVAILABLE 均 fail-closed
status: VECTOR_READY
owner: Codex 实现
reviewer: 待独立评审
decision_date: 2026-08-13
evidence: execution authorization HTTP negative tests; deny_unknown_fields; authenticated_router
```

第十七阶段没有增加真实执行入口。HTTP 模拟提交仍只更新审计计数，`broadcast_enabled=false`、`broadcast_ready=false`、`chain_broadcast=false`，没有 `push_tx`、mempool、SpendBundle 导出或确认跟踪。

### 7.17 模拟提交单次消费与防重放第十八阶段决策记录

```text
decision: 模拟提交必须携带 32 字节 submission_nonce；首次提交原子消费 ExecutionAuthorization 并产生 XHUB_SIMULATED_SUBMISSION_RECEIPT_V3_6，同授权同 nonce 重试幂等，不同 nonce 或全局 nonce 重用拒绝
status: VECTOR_READY
owner: Codex 实现
reviewer: 待独立评审
decision_date: 2026-08-13
evidence: ../watchtower-v3_6/src/authorization.rs; v36_simulated_submission_receipts; idempotency/replay tests
```

```text
decision: 一个 Execution Manifest 全生命周期最多产生一张模拟提交收据，重新签发授权不得绕过；收据只保存哈希、nonce、时间和状态，三个广播字段由 SQLite 约束固定为 false
status: VECTOR_READY
owner: Codex 实现
reviewer: 待独立评审
decision_date: 2026-08-13
evidence: UNIQUE(manifest_id); replacement authorization replay test; simulated receipt HTTP query
```

第十八阶段仍是模拟审计能力，不会提交任何交易。生产 SpendBundle、真实 fee sponsor、私钥、RPC 广播、mempool 与确认跟踪继续保持 `OPEN`。

### 7.18 执行审计哈希链第十九阶段决策记录

```text
decision: 使用 XHUB_EXECUTION_AUDIT_V3_6 追加哈希链承诺 Manifest 签发、Authorization 签发和模拟收据消费；每项绑定序号、前序哈希、事件类型、主体 ID、绑定哈希、状态和时间
status: VECTOR_READY
owner: Codex 实现
reviewer: 待独立评审
decision_date: 2026-08-13
evidence: ../watchtower-v3_6/src/audit.rs; v36_execution_audit_heads; v36_execution_audit_events; normal/tamper verification tests
```

```text
decision: 只读 /api/v3.6/execution-audit 返回链头、事件数和 valid，不导出事件执行材料；SQLite 约束继续固定三个广播字段为 false，链头外部锚定用于抵御完整数据库回滚仍保持 OPEN
status: VECTOR_READY
owner: Codex 实现
reviewer: 待独立评审
decision_date: 2026-08-13
evidence: execution audit HTTP test; audit event tampering test; broadcast_enabled=false; broadcast_ready=false; chain_broadcast=false
```

第十九阶段只增加本地可验证审计链，不增加任何交易执行能力。外部链头锚定、真实主网广播、SpendBundle 导出、私钥和确认跟踪继续保持 `OPEN`。

### 7.19 审计事件与执行状态原子一致性第二十阶段决策记录

```text
decision: 新 Manifest 签发、Execution Authorization 签发和模拟收据消费，分别与对应 execution audit event 及 audit head 在同一 SQLite 事务内提交；审计写入失败时不得保留该次业务状态
status: VECTOR_READY
owner: Codex 实现
reviewer: 待独立评审
decision_date: 2026-08-13
evidence: ../watchtower-v3_6/src/manifest.rs; ../watchtower-v3_6/src/authorization.rs; ../watchtower-v3_6/src/audit.rs; execution_audit_write_failures_roll_back_each_execution_state_change
```

第二十阶段仅证明上述三条执行状态路径的本地事务原子性。它不增加广播能力、不导出 SpendBundle、不读取私钥，也不解决整库回滚；外部链头锚定继续保持 `OPEN`。

### 7.20 SQLite 并发与 WAL 故障恢复第二十一阶段决策记录

```text
decision: Watchtower 数据库固定使用 journal_mode=WAL、synchronous=FULL 和 10 秒 busy timeout；独立连接并发接收同一后继 RecoveryPackage 必须收敛到一个 append-only head，重复提交保持幂等
status: VECTOR_READY
owner: Codex 实现
reviewer: 待独立评审
decision_date: 2026-08-13
evidence: ../watchtower-v3_6/src/lib.rs; concurrent_store_instances_preserve_a_single_append_only_head; wal_restart_preserves_packages_quarantine_and_durability_settings
```

```text
decision: 子进程在未提交的大事务中强制异常退出后，重启必须保留先前已提交 RecoveryPackage、丢弃全部未提交隔离记录，并通过 PRAGMA integrity_check
status: VECTOR_READY
owner: Codex 实现
reviewer: 待独立评审
decision_date: 2026-08-13
evidence: wal_recovery_discards_an_aborted_uncommitted_transaction; journal_mode=wal; synchronous=2; integrity_check=ok
```

第二十一阶段只验证单机 SQLite 的连接级并发、正常重启和未提交事务异常退出恢复。磁盘扇区损坏、目录整体回滚、跨主机副本、备份恢复和外部审计链头锚定仍保持 `OPEN`；所有广播能力继续禁用。

### 7.21 外部审计链头锚定与回滚检测第二十二阶段决策记录

```text
decision: 为 XHUB_EXECUTION_AUDIT_V3_6 提供基于事件数、链头哈希和时间的幂等 anchor_id；锚点创建前必须验证本地审计链，验证时使用调用方保存的外部锚点材料检查当前链是否仍包含该前缀
status: VECTOR_READY
owner: Codex 实现
reviewer: 待独立评审
decision_date: 2026-08-13
evidence: ../watchtower-v3_6/src/audit.rs; v36_execution_audit_anchors; execution_audit_anchor_detects_backup_restore_and_accepts_descendants
```

```text
decision: 当前事件数低于外部锚点事件数时必须返回 rollback_detected=true；锚点之后的合法追加只要锚点前缀哈希一致即可通过，整库恢复到锚点之前的副本必须失败关闭
status: VECTOR_READY
owner: Codex 实现
reviewer: 待独立评审
decision_date: 2026-08-13
evidence: backup copy before authorization; descendant simulation; restored database rollback check
```

第二十二阶段只提供本地锚点生成与验证，外部存储、跨主机复制、备份编排和独立密钥签名仍需部署阶段完成；锚点不构成交易执行授权，所有广播能力继续禁用。

### 7.22 可验证数据库备份与恢复第二十三阶段决策记录

```text
decision: 使用 SQLite VACUUM INTO 生成独立备份文件；DatabaseBackupManifest 固定备份文件哈希、文件大小、审计事件数、审计链头、可选外部 anchor_id 和创建时间，恢复前必须先通过文件校验
status: VECTOR_READY
owner: Codex 实现
reviewer: 待独立评审
decision_date: 2026-08-13
evidence: ../watchtower-v3_6/src/backup.rs; database_backup_manifest_detects_corruption_and_validates_restored_state
```

```text
decision: 通过文件校验后，恢复副本必须重新打开并验证 execution audit chain；提供外部锚点时还必须验证锚点前缀，篡改备份不得进入恢复流程
status: VECTOR_READY
owner: Codex 实现
reviewer: 待独立评审
decision_date: 2026-08-13
evidence: backup hash/size check; restored audit_valid=true; anchor_valid=true; corrupted backup hash mismatch
```

第二十三阶段只完成单机可验证备份与恢复前检查。备份加密、密钥轮换、跨故障域远程复制、异地恢复演练和自动化保留策略仍保持 `OPEN`；备份接口不增加任何广播能力。

### 7.23 加密备份、密钥轮换与副本一致性第二十四阶段决策记录

```text
decision: XHUB_WATCHTOWER_ENCRYPTED_BACKUP_V1 使用 XChaCha20-Poly1305、32 字节外部提供密钥、24 字节 OS 随机 nonce 和 32 字节 key_id；AAD 绑定域、V3.6 协议版本和 key_id，密钥不得写入数据库、备份文件或日志
status: VECTOR_READY
owner: Codex 实现
reviewer: 待独立评审
decision_date: 2026-08-13
evidence: ../watchtower-v3_6/src/backup.rs; encrypted_backup_rejects_wrong_material_and_supports_key_rotation
```

```text
decision: 错误 key_id、错误密钥、密文或认证标签篡改必须在写出明文前失败；轮换通过旧密钥解密后用新 key_id/密钥重新加密，旧包和新包均可由对应密钥独立恢复
status: VECTOR_READY
owner: Codex 实现
reviewer: 待独立评审
decision_date: 2026-08-13
evidence: wrong-key/wrong-id/tamper tests; v1-to-v2 key rotation; restored backup manifest verification
```

```text
decision: 跨故障域副本一致性比较解密后的 DatabaseBackupManifest 文件哈希、大小、审计事件数、链头和 anchor_id，不比较使用随机 nonce 生成的密文字节；任一字段分歧必须拒绝作为一致副本
status: VECTOR_READY
owner: Codex 实现
reviewer: 待独立评审
decision_date: 2026-08-13
evidence: backup_replicas_are_consistent; matching and divergent audit-head tests
```

第二十四阶段不提供 KMS/HSM、密钥分发、密钥销毁证明、远程传输或自动备份调度；这些生产运维能力仍保持 `OPEN`。所有交易广播能力继续禁用。

### 7.24 原子加密备份与恢复发布第二十五阶段决策记录

```text
decision: BackupKeyProvider 是唯一密钥获取边界；key 使用 Zeroizing 内存包装，Watchtower 不持久化 key。加密备份先写随机临时路径，成功后原子重命名到目标，目标存在或发布失败均不得覆盖/替换现有文件
status: VECTOR_READY
owner: Codex 实现
reviewer: 待独立评审
decision_date: 2026-08-13
evidence: ../watchtower-v3_6/src/backup.rs; atomic_encrypted_backup_cleans_temporary_plaintext_and_verifies_before_publish; atomic_encrypted_backup_failure_leaves_no_plaintext_or_temp_files
```

```text
decision: 恢复流程先校验 envelope hash、AEAD、DatabaseBackupManifest、execution audit chain 和可选外部 anchor，全部通过后才发布数据库；失败时明文恢复临时文件必须清理
status: VECTOR_READY
owner: Codex 实现
reviewer: 待独立评审
decision_date: 2026-08-13
evidence: restore_encrypted_database_backup; restored audit_valid=true; temporary plaintext cleanup test
```

第二十五阶段只收紧单机备份/恢复的文件发布和密钥边界。KMS/HSM 实现、密钥生命周期策略、远程传输、跨主机原子发布和异地恢复演练仍保持 `OPEN`；所有广播能力继续禁用。

## 8. 版本变更规则

- `OPEN` 至 `REVIEWED` 阶段可以修改，但必须同步更新规范、实现和测试向量。
- 测试网实例创建后，影响 Coin ID、Puzzle Hash、签名或状态哈希的修改不得应用到已有实例。
- 任一共识相关项目进入 `FROZEN` 后，任何不兼容修改必须使用新的协议版本、域字符串和模块哈希。
- 主网上线后不得在 V3.6 名义下替换 CLVM hex、哈希算法、编码、签名方案或挑战计时规则。

### 7.25 版本化加密备份清单与恢复交接第二十六阶段决策记录
```text
decision: EncryptedBackupArtifact 使用 XHUBAM01 magic、V3.6 协议版本和加密备份域进行 canonical 编码；清单固定绑定 DatabaseBackupManifest、envelope_hash 与 key_id，可跨进程、跨副本交接
status: VECTOR_READY
owner: Codex 实现
reviewer: 待独立评审
decision_date: 2026-08-13
evidence: ../watchtower-v3_6/src/backup.rs; encrypted_backup_artifact_manifest_is_stable_and_fail_closed; strict canonical decode rejects wrong version, truncation, trailing bytes and manifest tampering
```

```text
decision: 清单不包含任何 key bytes；解析失败、backup_id 派生不一致或清单字段篡改均不得进入恢复流程。KMS/HSM、远程密钥分发和真实跨 VPS 复制保持 OPEN；broadcast_enabled=false、broadcast_ready=false、chain_broadcast=false
status: VECTOR_READY
owner: Codex 实现
reviewer: 待独立评审
decision_date: 2026-08-13
evidence: decode_encrypted_backup_artifact; restore_encrypted_database_backup manifest validation; broadcast safety scan
```

### 7.26 备份清单交接审计与拒绝状态第二十七阶段决策记录
```text
decision: 交接记录只保存 artifact_hash、backup_id、envelope_hash、key_id、manifest_bytes_hash、时间和 RECEIVED/VERIFIED/REJECTED 状态；重复接收幂等，内容冲突和已拒绝清单均 fail-closed
status: VECTOR_READY
owner: Codex 实现
reviewer: 待独立评审
decision_date: 2026-08-13
evidence: ../watchtower-v3_6/src/backup.rs; backup_artifact_handoff_is_atomic_idempotent_and_restartable; backup_artifact_handoff_rejects_changed_envelope_and_cannot_recover
```

```text
decision: 交接验证重新执行 envelope、AEAD、DatabaseBackupManifest、执行审计链和可选 anchor 校验；业务状态与 BACKUP_ARTIFACT_RECEIVED/VERIFIED/REJECTED 审计事件在同一 SQLite 事务中提交，密钥不落盘，三项广播字段固定为 false
status: VECTOR_READY
owner: Codex 实现
reviewer: 待独立评审
decision_date: 2026-08-13
evidence: append_execution_audit_event_in_transaction; restart persistence; strict clippy; broadcast safety scan
```

### 7.27 跨副本一致性与恢复演练第二十八阶段决策记录
```text
decision: 加密副本一致性比较以解密后的 DatabaseBackupManifest 为准，比较 file_hash、size、audit event count/head、anchor 和完整 backup_id；随机 nonce、envelope_hash 与独立 key_id 不参与，以支持合法密钥轮换
status: VECTOR_READY
owner: Codex 实现
reviewer: 待独立评审
decision_date: 2026-08-13
evidence: encrypted_backup_replicas_are_consistent; encrypted_backup_replica_comparison_ignores_nonce_and_key_rotation
```

```text
decision: 交接副本比较要求全部为 VERIFIED，且 backup_id、envelope_hash 与 manifest_bytes_hash 一致；RECEIVED、REJECTED 或任一哈希分歧均 fail-closed。远程复制、自动调度、KMS/HSM 和真实跨 VPS 恢复演练保持 OPEN
status: VECTOR_READY
owner: Codex 实现
reviewer: 待独立评审
decision_date: 2026-08-13
evidence: verified_backup_handoffs_are_consistent; verified_handoff_comparison_rejects_unverified_or_divergent_replicas; broadcast_enabled=false; broadcast_ready=false; chain_broadcast=false
```

### 7.28 恢复演练报告与保留候选第二十九阶段决策记录
```text
decision: run_backup_restore_drill 只接受 VERIFIED 交接，重新执行 AEAD、DatabaseBackupManifest、执行审计链和可选 anchor 验证；记录 drill_id、backup_id、耗时、各项校验结果和失败原因，临时明文完成后必清理
status: VECTOR_READY
owner: Codex 实现
reviewer: 待独立评审
decision_date: 2026-08-13
evidence: ../watchtower-v3_6/src/backup.rs; backup_restore_drill_records_audited_result_and_cleans_plaintext; BACKUP_RESTORE_DRILL_COMPLETED audit event
```

```text
decision: backup_retention_candidates 只返回已通过恢复演练、超过 minimum_age_seconds 且不属于最新 keep_latest 份的 backup_id；未来删除仍需人工二次核对，当前实现不调用文件删除、不启用自动调度或远程复制
status: VECTOR_READY
owner: Codex 实现
reviewer: 待独立评审
decision_date: 2026-08-13
evidence: backup_retention_only_returns_old_drilled_candidates; retention policy fail-closed for future timestamps and keep_latest=0; broadcast_enabled=false; broadcast_ready=false; chain_broadcast=false
```

### 7.29 恢复演练与保留候选 HTTP API 第三十阶段决策记录
```text
decision: 通过 /api/v3.6/backup-restore-drills/{drill_id} 查询持久化演练报告，及 /api/v3.6/backup-retention-candidates 计算保留候选；请求和响应均绑定 x-xhub-protocol-version=0x0360，未知报告、错误清单和错误版本 fail-closed
status: VECTOR_READY
owner: Codex 实现
reviewer: 待独立评审
decision_date: 2026-08-13
evidence: ../watchtower-v3_6/src/api.rs; backup_drill_query_is_versioned_and_fail_closed; backup_retention_http_is_versioned_and_never_deletes
```

```text
decision: HTTP 层不获取密钥、不执行解密、不删除文件、不接收 SpendBundle 或广播材料；保留候选响应固定 deletion_performed=false，广播字段继续固定为 false
status: VECTOR_READY
owner: Codex 实现
reviewer: 待独立评审
decision_date: 2026-08-13
evidence: deny_unknown_fields; deletion_performed=false response; strict clippy; broadcast safety scan
```

### 7.30 跨模块部署前评审第三十一阶段决策记录
```text
decision: protocol、puzzles、hub、watchtower、wallet 五库完成离线全 target 测试；严格 clippy -D warnings 通过；cargo fmt --check 通过；协议版本、HTTP 前缀和版本头一致；Rust 源码广播/SpendBundle 导出安全扫描无命中；git diff --check 通过
status: REVIEWED
owner: Codex 实现
reviewer: 待独立外部评审
decision_date: 2026-08-13
evidence: cargo test --offline --all-targets (five crates); cargo clippy --offline --all-targets -- -D warnings; cargo fmt -- --check; version scan; source security scan; git diff --check
```

```text
decision: REVIEWED 仅表示 V3.6 代码主线完成当前回归与接口冻结复核，不表示生产上线或主网广播授权；主网参数、KMS/HSM、跨 VPS 复制、TLS/限流、真实测试网生命周期、外部安全评审和广播审批继续保持 OPEN
status: REVIEWED
owner: Codex 实现
reviewer: 待独立外部评审
decision_date: 2026-08-13
evidence: broadcast_enabled=false; broadcast_ready=false; chain_broadcast=false; no push_tx or SpendBundle export in Rust source
```

### 7.31 测试网部署配置门禁第三十二阶段决策记录
```text
decision: 部署配置在启动、RPC 预检和 smoke test 前统一执行静态 fail-closed 校验；拒绝缺失字段、非 loopback 监听、端口冲突、错误 Network ID 格式、非 HTTP(S) RPC、非 loopback 明文 RPC、证书/私钥单边配置、瞭望塔 URL 分歧、共用数据库和共用秘密文件
status: VECTOR_READY
owner: Codex 实现
reviewer: 待独立评审
decision_date: 2026-08-13
evidence: ../deploy/testnet/common.ps1; ../deploy/testnet/validate-config.ps1; ../deploy/testnet/test-config-validation.ps1; CONFIG_VALIDATION_TESTS_OK
```

```text
decision: 配置门禁只验证本地部署输入，不联网、不启动服务、不验证证书链，也不证明真实测试网 Coin 生命周期；TLS/限流压测、跨 VPS、KMS/HSM、外部安全评审和广播审批继续保持 OPEN
status: VECTOR_READY
owner: Codex 实现
reviewer: 待独立评审
decision_date: 2026-08-13
evidence: validate-config output contains no secrets; start-local/rpc-preflight/smoke-test share Assert-DeploymentConfig; broadcast safety boundary unchanged
```

### 7.32 主网金丝雀只读预检准备第三十三阶段决策记录
```text
decision: 新增 deploy/mainnet 隔离目录，仅提供主网 HTTPS RPC 静态配置校验与只读 preflight；主网配置要求真实 64-hex Network ID、真实 Funding Coin ID、mTLS 证书/私钥和独立接收者 ID，拒绝测试网 smoke 值与所有占位符
status: VECTOR_READY
owner: Codex 实现
reviewer: 待独立评审
decision_date: 2026-08-14
evidence: ../deploy/mainnet/config.mainnet.example.json; ../deploy/mainnet/validate-config.ps1; ../deploy/mainnet/rpc-preflight.ps1; ../deploy/mainnet/README.md; placeholder rejection
```

```text
decision: 主网目录不提供服务启动脚本、不保存真实密钥、不导出 SpendBundle、不调用 push_tx；只读 preflight 仅验证 Network ID、同步状态、峰值、Funding Coin 和确认深度，broadcast_enabled=false、broadcast_ready=false、chain_broadcast=false 继续固定
status: VECTOR_READY
owner: Codex 实现
reviewer: 待独立评审
decision_date: 2026-08-14
evidence: mainnet README; mainnet script scan; mainnet placeholder rejection; no broadcast entrypoint
```

### 7.33 主网金丝雀审批计划第三十四阶段决策记录
```text
decision: 新增审批前金丝雀计划格式与 fail-closed 校验；计划绑定真实 Funding Coin ID、Puzzle Hash、协议版本和必需证据，max_total_mojo 固定为 1，必须人工批准，broadcast_enabled 固定为 false，并明确禁止私钥、助记词、SpendBundle canonical hex 和 push_tx
status: VECTOR_READY
owner: Codex 实现
reviewer: 待独立评审
decision_date: 2026-08-14
evidence: ../deploy/mainnet/canary-plan.example.json; ../deploy/mainnet/validate-canary-plan.ps1; ../deploy/mainnet/test-canary-plan.ps1; CANARY_PLAN_TESTS_OK
```

```text
decision: 金丝雀计划只生成审批前审计对象，不构造或导出 SpendBundle，不启动主网服务，不执行广播；真实主网交易仍须在只读 preflight、参数/哈希复核、RecoveryPackage 模拟投递和双人审批完成后另行授权
status: VECTOR_READY
owner: Codex 实现
reviewer: 待独立评审
decision_date: 2026-08-14
evidence: VALID_APPROVAL_PLAN_ONLY; max_total_mojo=1; production_broadcast=false; broadcast safety scan
```

### 7.34 主网金丝雀证据包校验第三十五阶段决策记录
```text
decision: 新增不可广播证据包模板与校验器；证据绑定批准计划 SHA-256，核对主网 Network ID、Funding Coin/Puzzle Hash、RPC 快照摘要、四个 CLVM 模块哈希、RecoveryPackage 模拟结果和两个不同故障域的 APPROVED 记录；证据包拒绝私钥、助记词、SpendBundle bytes 和广播授权
status: VECTOR_READY
owner: Codex 实现
reviewer: 待独立评审
decision_date: 2026-08-14
evidence: ../deploy/mainnet/canary-evidence.example.json; ../deploy/mainnet/validate-canary-evidence.ps1; ../deploy/mainnet/test-canary-evidence.ps1; CANARY_EVIDENCE_TESTS_OK
```

```text
decision: 证据包只生成 VALID_EVIDENCE_ONLY 审计摘要，不构造、签名、导出或广播交易；production_broadcast、broadcast_enabled 和 external_broadcast_authorized 固定为 false
status: VECTOR_READY
owner: Codex 实现
reviewer: 待独立评审
decision_date: 2026-08-14
evidence: evidence validator output; broadcast-enabled rejection; source safety scan
```

### 7.35 主网候选参数门禁第三十六阶段决策记录
```text
decision: 主网候选参数配置固定协议算术、Funding 后不可变、max_ledger_entries=64、一份商户回执和生产 2-of-3 CustodyAttestation 策略；参数仅为 candidate，challenge 安全评审保持 PENDING_EXTERNAL_REVIEW，mainnet_approved=false
status: VECTOR_READY
owner: Codex 实现
reviewer: 待独立评审
decision_date: 2026-08-14
evidence: ../deploy/mainnet/mainnet-parameters.candidate.json; validate-mainnet-parameters.ps1; MAINNET_PARAMETER_TESTS_OK
```

### 7.36 金丝雀工件哈希清单第三十七阶段决策记录
```text
decision: 对协议、向量、CLVM 模块清单、主网候选参数、配置、RPC、计划和证据校验器生成文件大小与 SHA-256 清单；缺失、篡改、重复或不安全路径均 fail-closed
status: VECTOR_READY
owner: Codex 实现
reviewer: 待独立评审
decision_date: 2026-08-14
evidence: ../deploy/mainnet/mainnet-canary-artifacts.json; generate/verify-artifact-manifest.ps1; ARTIFACT_MANIFEST_TESTS_OK
```

### 7.37 主网秘密文件隔离第三十八阶段决策记录
```text
decision: 主网秘密预检只检查 API token、HUB BLS 私钥和 RPC 私钥文件的存在、分离、大小与 Windows ACL；不读取或输出秘密内容，真实文件未提供前不得声称 SECRETS_ISOLATED
status: VECTOR_READY
owner: Codex 实现
reviewer: 待独立评审
decision_date: 2026-08-14
evidence: ../deploy/mainnet/check-secret-isolation.ps1; test-secret-isolation.ps1; SECRET_ISOLATION_STATIC_TESTS_OK
```

### 7.38 主网准备度聚合第三十九阶段决策记录
```text
decision: 准备度报告聚合参数、工件、配置、RPC、计划、证据、秘密隔离、Watchtower 身份、TLS、运行时、生成配置、真实部署、在线端点和外部安全评审；全部满足也只进入 READY_FOR_MANUAL_REVIEW，永不授权广播；当前模板有十五项外部输入待完成，状态 BLOCKED_EXTERNAL_INPUT
status: VECTOR_READY
owner: Codex 实现
reviewer: 待独立评审
decision_date: 2026-08-14
evidence: ../deploy/mainnet/evaluate-readiness.ps1; readiness-input.example.json; READINESS_TESTS_OK; pending_count=15
```

### 7.39 不可广播主网候选发布包第四十阶段决策记录
```text
decision: 候选发布包仅包含公开配置模板、参数、校验器和 CLVM 模块哈希，排除 local/private/secrets、数据库、助记词和 SpendBundle；逐文件 SHA-256 校验，状态固定 CANDIDATE_VERIFIED_NOT_APPROVED
status: VECTOR_READY
owner: Codex 实现
reviewer: 待独立评审
decision_date: 2026-08-14
evidence: ../deploy/mainnet/generate-candidate-release.ps1; verify-candidate-release.ps1; CANDIDATE_RELEASE_TESTS_OK; mainnet_approved=false; production_broadcast=false
```

### 7.40 主网 RPC 结构化证据第四十一阶段决策记录
```text
decision: 验证 Rust xhub-rpc-preflight 的结构化输出并绑定主网配置；要求 Network ID 一致、节点同步、有峰值、Funding Coin 已确认未花费且达到确认深度，输出只含摘要并固定 broadcast_enabled=false
status: VECTOR_READY
owner: Codex 实现
reviewer: 待独立评审
decision_date: 2026-08-14
evidence: ../deploy/mainnet/validate-rpc-preflight-output.ps1; test-chain-evidence.ps1; CHAIN_EVIDENCE_TESTS_OK
```

### 7.41 Funding Coin 与 Puzzle 绑定第四十二阶段决策记录
```text
decision: 将主网配置 Funding Coin ID、1-mojo 金丝雀计划、RPC Coin/Puzzle/amount/confirmations 和候选参数文件哈希绑定；任一 Coin ID、Network ID、Puzzle Hash、金额或确认策略分歧均 fail-closed
status: VECTOR_READY
owner: Codex 实现
reviewer: 待独立评审
decision_date: 2026-08-14
evidence: ../deploy/mainnet/validate-funding-binding.ps1; FUNDING_BINDING_VERIFIED; negative ready=false test
```

### 7.42 RecoveryPackage 模拟证据第四十三阶段决策记录
```text
decision: 验证钱包 mainnet closing simulation 报告与计划和 Funding binding 一致；要求 RecoveryPackage、全部 CLVM 条件通过，金额固定 1 mojo，spend_bundle_created、broadcast_ready 和 chain_broadcast 均为 false
status: VECTOR_READY
owner: Codex 实现
reviewer: 待独立评审
decision_date: 2026-08-14
evidence: ../deploy/mainnet/validate-recovery-simulation.ps1; RECOVERY_SIMULATION_VERIFIED; CHAIN_EVIDENCE_TESTS_OK
```

### 7.43 双人审批记录与故障域第四十四阶段决策记录
```text
decision: 审批记录要求两个不同 reviewer_id 和两个不同 failure_domain 对同一 evidence SHA-256 明确 APPROVED；拒绝占位身份、哈希分歧、重复故障域、秘密材料和任何 broadcast_authorized=true
status: VECTOR_READY
owner: Codex 实现
reviewer: 待独立评审
decision_date: 2026-08-14
evidence: ../deploy/mainnet/approval-records.example.json; validate-approval-records.ps1; APPROVAL_RECORD_TESTS_OK
```

### 7.44 主网金丝雀统一演练编排第四十五阶段决策记录
```text
decision: run-canary-dry-run 串行执行参数、RPC、计划、Funding binding、Recovery simulation、证据包和双人审批七个门禁，记录全部输入 SHA-256；任一步失败立即停止，最高状态仅 DRY_RUN_COMPLETE_MANUAL_REVIEW_REQUIRED
status: VECTOR_READY
owner: Codex 实现
reviewer: 待独立评审
decision_date: 2026-08-14
evidence: ../deploy/mainnet/run-canary-dry-run.ps1; test-dry-run-orchestrator.ps1; functional 1-mojo fixture; no service start/signing/broadcast
```

### 7.45 Watchtower 独立身份第四十六阶段决策记录
```text
decision: 主网候选身份清单固定三套不同 BLS 公钥、运营者、基础设施供应商、区域、故障域、API 主机和 TLS 证书指纹，并与 Watchtower 加载的 custody attester 配置逐项一致；任何重复或漂移均 fail-closed
status: VECTOR_READY
owner: Codex 实现
reviewer: 待独立评审
decision_date: 2026-08-14
evidence: ../deploy/mainnet/validate-watchtower-identities.ps1; test-watchtower-identities.ps1; WATCHTOWER_IDENTITY_TESTS_OK; production_approved=false
```

### 7.46 Watchtower TLS 配置第四十七阶段决策记录
```text
decision: 三个公开 Watchtower 入口必须使用 TLS1.3、强制验证客户端证书、绑定身份清单中的证书 SHA-256、限制请求速率和大小，并且只转发到回环 HTTP 上游；本阶段只验证配置，不读取私钥或声称在线端点已验证
status: VECTOR_READY
owner: Codex 实现
reviewer: 待独立评审
decision_date: 2026-08-14
evidence: ../deploy/mainnet/validate-watchtower-tls-profile.ps1; test-watchtower-tls-profile.ps1; WATCHTOWER_TLS_PROFILE_TESTS_OK; live_endpoint_check_performed=false
```

### 7.47 Watchtower 在线 TLS 探针第四十八阶段决策记录
```text
decision: 在线探针对三个公开 DNS 端点分别强制 TLS1.3、mTLS、系统证书链和叶证书 SHA-256 pin，使用独立 API Token/客户端证书查询 health 与最新 RecoveryPackage；三节点的 Funding Coin、状态、checkpoint 和内容哈希必须完全一致
status: VECTOR_READY
owner: Codex 实现
reviewer: 待独立评审
decision_date: 2026-08-14
evidence: ../deploy/mainnet/verify-watchtower-tls-endpoints.ps1; test-watchtower-endpoint-probe.ps1; WATCHTOWER_ENDPOINT_PROBE_TESTS_OK; PlanOnly 不联网且不读取秘密
```

### 7.48 Watchtower 三运营者部署证据第四十九阶段决策记录
```text
decision: 部署证据绑定身份清单 SHA-256、24 小时内 TLS 端点报告 SHA-256、三套唯一运营者/故障域/部署 ID 和两名不同故障域复核人；任一端点未验证、内容分歧、重复部署、过期报告或哈希漂移均 fail-closed
status: VECTOR_READY
owner: Codex 实现
reviewer: 待独立评审
decision_date: 2026-08-14
evidence: ../deploy/mainnet/validate-watchtower-deployment-evidence.ps1; test-watchtower-deployment-evidence.ps1; WATCHTOWER_DEPLOYMENT_EVIDENCE_TESTS_OK; production_broadcast=false
```

### 7.49 Watchtower 运行时硬化第五十阶段决策记录
```text
decision: 三节点 Linux 运行时绑定同一 Watchtower 二进制 SHA-256、公开身份主机和 TLS 回环上游；强制非管理员账户、回环监听、独立数据/配置路径、资源上限、NoNewPrivileges、PrivateTmp、ProtectSystem=strict、ProtectHome 和空 capability 集
status: VECTOR_READY
owner: Codex 实现
reviewer: 待独立评审
decision_date: 2026-08-14
evidence: ../deploy/mainnet/validate-watchtower-runtime.ps1; test-watchtower-runtime.ps1; WATCHTOWER_RUNTIME_TESTS_OK; production_broadcast=false
```

### 7.50 systemd 与 Nginx 配置生成第五十一阶段决策记录
```text
decision: 由已验证运行时和 TLS 清单生成三份 systemd unit 与三份 Nginx 配置，逐文件 SHA-256 留痕并复验硬化项；生成器不安装、不启动、不 reload、不嵌入秘密或广播能力
status: VECTOR_READY
owner: Codex 实现
reviewer: 待独立评审
decision_date: 2026-08-14
evidence: ../deploy/mainnet/generate-watchtower-systemd-units.ps1; generate-watchtower-nginx-configs.ps1; verify-watchtower-generated-configs.ps1; WATCHTOWER_DEPLOYMENT_GENERATION_TESTS_OK
```

### 7.51 单 VPS 测试绿灯第五十二阶段决策记录
```text
decision: 资源受限测试可通过独立 single-vps-test API 只按不同 Watchtower BLS 公钥计算 2-of-3 CustodyAttestation，不要求不同故障域；响应固定 failure_domain_enforced=false、test_only=true、production_ready=false，生产绿灯逻辑保持不变
status: VECTOR_READY
owner: Codex 实现
reviewer: 待独立评审
decision_date: 2026-08-14
evidence: ../watchtower-v3_6/src/custody.rs; src/api.rs; tests/watchtower.rs; tests/http_api.rs; 同故障域测试通过且生产绿灯失败
```

### 7.52 单 VPS Docker 编排第五十三阶段决策记录
```text
decision: 在单一 Linux VPS 上生成三个 host-network Watchtower 容器，分别绑定不同回环端口、数据库、API Token 和 BLS 公钥；容器只读、非 root、drop ALL capabilities、no-new-privileges，生成器不启动容器且不开放公网 ports
status: VECTOR_READY
owner: Codex 实现
reviewer: 待独立评审
decision_date: 2026-08-14
evidence: ../deploy/mainnet/docker-single-vps/Dockerfile; validate-single-vps-docker-profile.ps1; generate-single-vps-docker-compose.ps1; test-single-vps-docker.ps1; SINGLE_VPS_DOCKER_TESTS_OK; ../mainnet-experiment/three-watchtower-canary/closing-state-1/readonly-monitor-deployment-evidence.json; readonly-monitor-alert-lifecycle-evidence.json; docker_validation_performed=true
```

### 7.53 单 VPS 聚合告警运维闭环第五十四阶段决策记录
```text
decision: 聚合器告警事件使用 SQLite 持久化和状态指纹去重，仅人工确认接口要求独立 Bearer token；单 VPS 运维提供失败回滚 token 轮换、SQLite Online Backup 一致快照、quick_check、SHA-256 清单和隔离副本恢复演练。备份不包含 API token，恢复演练只绑定 127.0.0.1:18745、不覆盖在线数据库并在验证后删除临时容器和副本
status: VECTOR_READY
owner: Codex 实现
reviewer: 待独立评审
decision_date: 2026-08-15
evidence: ../watchtower-v3_6/src/bin/monitor_aggregate.rs; ../deploy/mainnet/docker-single-vps/rotate-monitor-aggregate-token.sh; sqlite-online-backup.py; backup-monitor-aggregate-alerts.sh; verify-monitor-aggregate-alert-backup.sh; verify-alert-lifecycle.sh; test-single-vps-docker.ps1; ../mainnet-experiment/three-watchtower-canary/closing-state-1/xhub-v36-token-rotation-evidence.json; xhub-v36-alert-backup-manifest-v2.json; xhub-v36-alert-restore-drill-evidence-v2.json; readonly-monitor-alert-backup-rejection-evidence.json; SINGLE_VPS_DOCKER_TESTS_OK; remote_operations_validation=true
```

第五十四阶段只建立单 VPS 测试环境的告警运维闭环，不提供跨故障域副本、异地备份、KMS/HSM、外部通知或公网管理接口。`physical_failure_domain_count=1`、`production_ready=false`，所有 SpendBundle 创建与链上广播能力继续禁用。

### 7.54 单 VPS 告警定时备份与巡检第五十五阶段决策记录
```text
decision: 聚合告警数据库的 SQLite Online Backup 在执行前强制检查至少 1 GiB 可用空间；systemd oneshot/timer 每日调度并启用 Persistent 与随机延迟；巡检报告验证四个监控容器、聚合 quorum、最新备份哈希与年龄、磁盘门禁和 timer 状态。保留策略只计算超过最小年龄且不属于最新 keep_latest 的人工复核候选，永不自动删除文件
status: VECTOR_READY
owner: Codex 实现
reviewer: 待独立评审
decision_date: 2026-08-15
evidence: ../deploy/mainnet/docker-single-vps/backup-monitor-aggregate-alerts.sh; list-monitor-aggregate-backup-retention-candidates.sh; inspect-monitor-aggregate-operations.sh; xhub-v36-monitor-alert-backup.service; xhub-v36-monitor-alert-backup.timer; install-monitor-aggregate-operations.sh; OPERATIONS.md; test-single-vps-docker.ps1; ../mainnet-experiment/three-watchtower-canary/closing-state-1/xhub-v36-alert-scheduled-backup-manifest.json; xhub-v36-alert-retention-candidates.json; xhub-v36-alert-operations-inspection.json; SINGLE_VPS_DOCKER_TESTS_OK; remote_systemd_timer_validation=true
```

第五十五阶段不执行备份删除，不发送邮件，不开放管理端口，也不将单 VPS 计时任务视为异地灾备。备份仍位于同一物理故障域，`production_ready=false`，所有交易创建和广播能力继续禁用。

### 7.55 单 VPS 周期巡检与原子状态发布第五十六阶段决策记录
```text
decision: systemd oneshot/timer 每 15 分钟执行聚合运维巡检并带最多 2 分钟随机延迟；成功报告通过同目录临时文件原子替换 latest.json，失败保留上一份成功报告、写入不含错误详情的 latest-failure.json 并使 service 失败。调度验证要求报告不超过 1200 秒、timer enabled/active、无失败标记且所有广播字段为 false
status: VECTOR_READY
owner: Codex 实现
reviewer: 待独立评审
decision_date: 2026-08-15
evidence: ../deploy/mainnet/docker-single-vps/publish-monitor-aggregate-operations-inspection.sh; verify-monitor-aggregate-inspection-scheduler.sh; xhub-v36-monitor-operations-inspection.service; xhub-v36-monitor-operations-inspection.timer; install-monitor-aggregate-operations.sh; OPERATIONS.md; test-single-vps-docker.ps1; ../mainnet-experiment/three-watchtower-canary/closing-state-1/xhub-v36-alert-periodic-inspection-latest.json; xhub-v36-alert-inspection-publication.json; xhub-v36-alert-inspection-scheduler-evidence.json; xhub-v36-alert-inspection-controlled-failure.json; SINGLE_VPS_DOCKER_TESTS_OK; remote_periodic_inspection_validation=true
```

第五十六阶段只持久化本地巡检状态，不自动重启容器、不确认告警、不修复数据库、不发送外部通知。报告仍位于同一 VPS，不能替代独立监控系统或跨故障域值守；`production_ready=false`，交易创建与广播能力继续禁用。

### 7.56 最新告警备份周期恢复演练第五十七阶段决策记录
```text
decision: systemd oneshot/timer 每周选择有效备份目录中的最新 SQLite Online Backup，在 127.0.0.1:18745 启动隔离临时聚合器并核对全部历史事件 ID；成功或失败报告原子发布，失败保留上一份成功报告并要求人工关注。演练结束必须删除临时容器和副本，在线数据库、token 和源备份保持不变
status: VECTOR_READY
owner: Codex 实现
reviewer: 待独立评审
decision_date: 2026-08-15
evidence: ../deploy/mainnet/docker-single-vps/verify-monitor-aggregate-alert-backup.sh; publish-latest-monitor-aggregate-restore-drill.sh; verify-monitor-aggregate-restore-scheduler.sh; xhub-v36-monitor-alert-restore-drill.service; xhub-v36-monitor-alert-restore-drill.timer; install-monitor-aggregate-operations.sh; OPERATIONS.md; test-single-vps-docker.ps1; ../mainnet-experiment/three-watchtower-canary/closing-state-1/xhub-v36-alert-scheduled-restore-latest.json; xhub-v36-alert-scheduled-restore-publication.json; xhub-v36-alert-restore-scheduler-evidence.json; SINGLE_VPS_DOCKER_TESTS_OK; remote_weekly_restore_validation=true
```

第五十七阶段不自动恢复在线数据库、不删除或移动源备份、不重启业务容器，也不发送外部通知。恢复演练和备份仍位于同一物理故障域，不能证明异地灾备或生产准备度；所有交易创建和广播能力继续禁用。

### 7.57 单 VPS 运维文件完整性与漂移检测第五十八阶段决策记录
```text
decision: 固定 11 个已安装运维程序和 6 个 systemd unit 的源文件名、安装绝对路径与 SHA-256；安装器在写入前验证 staging 清单、写入后验证实际路径，15 分钟巡检再次执行完整性校验。任一缺失、路径越界、重复条目或哈希不一致均 fail-closed，只写本地巡检失败标记，不自动修复文件
status: VECTOR_READY
owner: Codex 实现
reviewer: 待独立评审
decision_date: 2026-08-15
evidence: ../deploy/mainnet/docker-single-vps/operations-integrity-manifest.json; verify-monitor-aggregate-operations-integrity.sh; verify-monitor-aggregate-operations-integrity-fail-closed.sh; inspect-monitor-aggregate-operations.sh; install-monitor-aggregate-operations.sh; OPERATIONS.md; test-single-vps-docker.ps1; ../mainnet-experiment/three-watchtower-canary/closing-state-1/xhub-v36-alert-operations-integrity-evidence.json; xhub-v36-alert-operations-integrity-rejection-evidence.json; xhub-v36-alert-integrity-inspection-latest.json; xhub-v36-alert-integrity-inspection-publication.json; SINGLE_VPS_DOCKER_TESTS_OK; remote_operations_integrity_validation=true
```

第五十八阶段的本地清单可以检测单文件漂移，但不能抵御攻击者同时替换清单、验证器和巡检报告，也不构成外部可信锚。自动修复、远程证明和跨故障域审计仍未启用；`production_ready=false`，所有交易创建和广播能力继续禁用。

### 7.58 源码控制准入与秘密排除第五十九阶段决策记录
```text
decision: V3.6 Git 候选集必须递归排除 local-secrets、任意主网实验 private 目录、*.local.json、实验 SQLite/WAL/SHM、target 和私钥文件扩展；审计额外扫描文本候选中的 PEM 私钥标记。四个真实敏感/生成路径必须通过 git check-ignore，审计只输出 READY_FOR_MANUAL_GIT_REVIEW，不执行 git add 或 commit
status: VECTOR_READY
owner: Codex 实现
reviewer: 待独立评审
decision_date: 2026-08-15
evidence: ../.gitignore; ../audit-source-control-readiness.ps1; ../test-source-control-readiness.ps1; source-control-readiness-evidence.json; SOURCE_CONTROL_READINESS_TESTS_OK; commit_created=false
```

第五十九阶段只证明当前候选路径和高置信私钥标记通过本地审计，不替代人工代码审查、历史 Git 对象秘密扫描或第三方 secret scanner。文件仍未暂存和提交，`production_ready=false`，所有交易创建和广播能力继续禁用。

### 7.59 确定性源码候选清单第六十阶段决策记录
```text
decision: 对通过源码控制准入审计的 V3.6 候选文件按规范路径排序，记录每个文件的字节数与 SHA-256，并以 path\0sha256\0size\n 材料计算整棵候选树 SHA-256；清单与验证证据自身排除在树外以避免自引用。验证器在临时目录重生成完整清单并逐项比较，不执行暂存或提交
status: VECTOR_READY
owner: Codex 实现
reviewer: 待独立评审
decision_date: 2026-08-15
evidence: ../generate-source-control-candidate-manifest.ps1; ../verify-source-control-candidate-manifest.ps1; ../test-source-control-candidate-manifest.ps1; source-control-candidate-manifest.json; source-control-candidate-manifest-verification-evidence.json; SOURCE_CONTROL_CANDIDATE_MANIFEST_TESTS_OK; commit_created=false
```

第六十阶段的树哈希只绑定当前工作区候选集合，尚未由 Git commit、签名标签或外部透明日志锚定。人工审查、历史对象扫描和明确提交授权仍是后续步骤；`production_ready=false`，交易创建和广播能力继续禁用。

### 7.60 V3.6 本地 Git 基线提交第六十一阶段决策记录
```text
decision: 仅将通过源码控制准入审计与确定性候选清单验证的 V3.6 文件加入 Git；private、local-secrets、*.local.json、实验 SQLite/WAL/SHM 和 target 保持忽略。提交前要求暂存路径全部位于 V3.6、禁止路径与 PEM 私钥标记为零、候选树重新验证通过；创建本地 commit 但不推送远程仓库
status: VECTOR_READY
owner: Codex 实现
reviewer: 待独立评审
decision_date: 2026-08-15
evidence: ../audit-source-control-readiness.ps1; ../source-control-candidate-manifest.json; ../verify-source-control-candidate-manifest.ps1; 本地 Git commit 与最终 commit ID；remote_push_performed=false
```

第六十一阶段的本地 commit 只提供版本基线，不代表外部安全评审、生产批准、签名发布或主网广播授权。远程 push、签名 tag 和发布分支仍需独立授权；`production_ready=false`，所有交易创建和广播能力继续禁用。
