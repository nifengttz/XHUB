# X-Hub V3.6 实现规范附录

状态：`VECTOR_READY`（覆盖哈希、BLS、规范编码、Merkle、冲突证据和本附录列出的核心数据结构；CLVM 与 HUB 状态机证据分别位于相邻代码库目录）。  
适用实现：HUB、钱包、瞭望塔及所有离线验证器。  
主网状态：未批准；CLVM 模块哈希和主网参数安全范围仍需后续冻结。

## 1. 规范标识

```text
protocol_version = 0x0360
vector_schema    = xhub-protocol-v3-6-vectors-1
hash             = SHA-256
```

所有哈希均为 `SHA256(part_0 || part_1 || ...)`，域字符串是 ASCII 原始字节，不带结尾零字节。固定宽度字段不添加长度前缀。可变字节串和数组使用本附录规定的长度编码。

## 2. BLS 签名

使用 Chia `chia-bls 0.36.1` 的 BLS12-381 Augmented Scheme：

```text
ciphersuite = BLS_SIG_BLS12381G2_XMD:SHA-256_SSWU_RO_AUG_
public_key  = 48-byte compressed G1
signature   = 96-byte compressed G2
```

`Sign(sk, message_hash)` 使用库的 augmented `sign`，即签名前将签名者的公钥前置到消息。V3.6 的消息参数为 32 字节协议哈希，不直接签可变长度结构。

解析必须使用库的规范压缩点解析，并拒绝：

- 非规范压缩编码；
- 不在曲线上的点；
- 不在正确子群中的点；
- 无穷远点；
- 长度不是 48/96 字节的输入。

用户授权、HUB 状态、商户送达确认和 ReservationResult 各自使用调用方约定的签名公钥验证，不得以 HUB 签名替代用户授权签名。V3.6 核心签名对象逐份验证，不允许用聚合签名替代协议要求的独立签名字段。

## 3. 规范编码

### 3.1 基本类型

```text
u16/u32/u64     big-endian fixed width
bool            0x00 = false, 0x01 = true
option<T>       0x00 = none, 0x01 + T = some
bytes           u32_be(byte_length) + raw bytes
array<T>        u32_be(element_count) + encoded elements
```

所有整数必须满足 `0..=2^63-1`；任何加法和总额计算使用 checked arithmetic。规范解码器拒绝未知枚举、非法 bool/option tag、超出长度限制的 blob、截断输入和尾随字节。

可变 blob 的实现上限为 1 MiB。账本数组上限为 64 条，签名数组必须与账本条数完全相等。

`network_id` 固定编码为 bytes32。部署时取目标 Chia 网络的 Genesis Challenge；钱包、HUB 和瞭望塔必须对照其连接节点的网络配置验证该值。测试向量使用 `aa` 重复 32 次的专用虚拟值。

### 3.2 共识与 API 的边界

JSON、URL 和其他 API 表示不是共识编码。API 层必须先解析为类型，再通过本附录的 canonical encoding 计算哈希。展示用 `0x` 前缀、大小写和地址格式不得进入协议哈希。

### 3.3 Funding 条款参数

创建 Funding Coin 时用户输入或确认：

```text
acceptance_blocks  = default 12288
freeze_blocks      = default 200
challenge_blocks   = default 6000
close_delay_blocks = acceptance_blocks + freeze_blocks
```

三个输入值必须为正且不超过 `2^63-1`；`close_delay_blocks` 也必须不溢出。`close_delay_blocks` 不提供独立编辑入口。实际值进入 `ChannelTerms`、`channel_terms_hash` 和 Puzzle 参数，Funding Coin 创建后不可修改。

## 4. 核心哈希输入

| 名称 | 规范输入 |
|---|---|
| `channel_terms_hash` | `CHANNEL_TERMS_DOMAIN || ChannelTerms.canonical_bytes()` |
| `state_zero_hash` | `STATE_ZERO_DOMAIN || protocol_version || network_id || funding_coin_id || channel_terms_hash || funding_amount || user_remainder_puzzle_hash` |
| `authorization_hash` | `USER_AUTH_DOMAIN || protocol_version || network_id || funding_coin_id || channel_terms_hash || merchant_puzzle_hash || merchant_receipt_public_key || amount || reservation_nonce` |
| `entry_hash` | `LEDGER_ENTRY_DOMAIN || entry_index || merchant_puzzle_hash || merchant_receipt_public_key || amount || reservation_nonce || authorization_hash` |
| `leaf_hash` | `LEDGER_LEAF_DOMAIN || entry_hash` |
| `node_hash` | `LEDGER_NODE_DOMAIN || left || right` |
| `checkpoint_hash` | `CHECKPOINT_DOMAIN || protocol_version || network_id || LedgerCheckpoint.canonical_bytes()` |
| `hub_state_hash` | `HUB_STATE_DOMAIN || checkpoint_hash` |
| `recovery_package_content_hash` | `RECOVERY_PACKAGE_DOMAIN || RecoveryPackage.canonical_bytes()` |
| `delivery_confirmation_hash` | `DELIVERY_CONFIRMATION_DOMAIN || DeliveryConfirmation.canonical_bytes()` |
| `reservation_result_hash` | `RESERVATION_RESULT_DOMAIN || ReservationResult.canonical_bytes()` |

## 5. Merkle Tree

叶子按不可变 `entry_index` 顺序排列。空树根为：

```text
H("XHUB_LEDGER_EMPTY_V3_6")
```

每层节点数为奇数时复制最后一个节点，再按相邻的 `(left, right)` 计算父节点。证明步骤使用一个字节表示方向：

```text
0 = sibling is on the right
1 = sibling is on the left
```

proof 编码为 `leaf_index:u32_be || leaf_count:u32_be || step_count:u32_be || steps[]`，每个 step 为 `direction:u8 || sibling:bytes32`。证明验证必须检查索引、叶子数量、方向序列、完整路径和最终根。

## 6. 已覆盖的核心结构

以下类型已经有严格编码、严格解码、哈希或验证实现：

```text
ChannelTerms
LedgerEntry
StateZero
LedgerCheckpoint
OfficialState
RecoveryPackage
DeliveryConfirmation
ReservationResult
SignedReservationResult
DoubleSignEvidence
ConflictingResultEvidence
MerkleProof
```

Watchtower 另有链下运营结构 `CustodyAttestation`。它使用 `XHUB_WATCHTOWER_CUSTODY_ATTESTATION_V3_6` 域，绑定 Funding Coin、状态序号、checkpoint、RecoveryPackage 内容哈希、账本项、用户授权哈希和商户 DeliveryConfirmation 哈希。该结构不进入 ChannelTerms 或 CLVM；生产绿灯要求商户回执存在，并按不同 Watchtower 公钥和不同故障域聚合托管证明。

单 VPS Docker 测试配置可以只按不同 Watchtower 公钥计算测试门槛，但必须通过独立 API/状态类型返回，并固定 `test_only=true`、`failure_domain_enforced=false`、`production_ready=false`。生产绿灯继续同时要求不同公钥和不同故障域。

冲突证据的规范结构为：

```text
SignedReservationResult = ReservationResult || hub_result_signature:bytes96

DoubleSignEvidence = first:OfficialState || second:OfficialState
  要求相同 funding_coin_id、channel_terms_hash、state_sequence
  要求 first_checkpoint_hash < second_checkpoint_hash
  要求两个 checkpoint hash 不同且两份 HUB A 签名均有效

ConflictingResultEvidence = first:SignedReservationResult || second:SignedReservationResult
  要求相同 network_id、funding_coin_id、reservation_nonce
  要求 first_result_hash < second_result_hash
  要求两个 result hash 不同且两份 HUB A 签名均有效
```

构造函数必须按消息哈希升序排列两个对象；严格解码后的验证拒绝上下文不同、内容相同、顺序颠倒或任一签名无效的证据。

测试向量文件为 [test-vectors/protocol-v3_6.json](test-vectors/protocol-v3_6.json)，生成命令为：

```powershell
cargo run --manifest-path .\Cargo.toml --bin generate-vectors
```

向量使用的私钥种子仅用于测试，不能用于真实资金。向量覆盖正常值、round-trip、空树、奇数叶、64 条记录、非法 BLS 点、close delay 不一致、非法 option/bool、重复 nonce、金额不足、篡改 proof、双签证据和冲突结果证据。

## 7. 分阶段实现状态

Funding、Closing、CHALLENGE、FINALIZE 和 Merchant Payment 的 CLVM 候选接口及模块哈希已在 `../puzzles-v3_6` 达到 `VECTOR_READY`。HUB 状态签名器、SQLite WAL、append-only 校验、reservation 幂等核心以及可信链状态门控已在 `../hub-v3_6` 达到 `VECTOR_READY`。

以下范围仍未进入 `VECTOR_READY`：

- 主网 `challenge_blocks` 最小安全值及三类周期的生产允许范围；
- 主网 RPC 来源数量、允许的 peak 高度差及 Funding 确认深度策略；
- HUB A 私钥托管、冷备、轮换和紧急停用；
- CBOR API、生产 TLS/身份认证和跨运营者 RecoveryPackage 投递；
- 瞭望塔真实故障域核验、RPC、广播和 fee Coin 策略；
- 跨代码库 API 版本兼容矩阵及全部上线测试证据。
