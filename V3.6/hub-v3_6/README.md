# XHUB Hub V3.6

本目录实现 X-Hub V3.6 的有状态签名器、append-only 账本、reservation 幂等核心、SQLite 持久化和故障恢复测试。

当前范围为 `VECTOR_READY`，尚不是生产服务。HTTP/CBOR API、HUB A 密钥托管/轮换、RecoveryPackage 外部投递和瞭望塔确认不在本阶段范围内。

## 核心不变量

- 持久化指针固定为 `(funding_coin_id, latest_sequence, latest_checkpoint_hash)`。
- 第一份 OfficialState 必须满足 `sequence = 1` 且 `previous_checkpoint_hash = state_zero_hash`。
- 后续状态只能满足 `new_sequence = latest_sequence + 1` 和 `new_previous_checkpoint_hash = latest_checkpoint_hash`。
- 新账本必须完整包含旧账本前缀；旧 LedgerEntry 及其用户授权签名不得修改、删除或重排。
- 每条用户授权在签名前逐条验证；金额、Merkle root、nonce 唯一性和找零由 `protocol-v3_6` 重新计算。
- HUB 私钥不写入数据库；调用方每次提供密钥，存储层校验公钥必须等于通道条款中的 HUB A 公钥。

## 持久化顺序

SQLite 固定启用：

```text
PRAGMA journal_mode = WAL
PRAGMA synchronous = FULL
BEGIN IMMEDIATE
```

成功预扣使用两个持久化阶段：

```text
事务 1：验证请求和完整候选账本
       -> 写入不可删除的 PREPARED state intent
       -> 写入 PREPARED reservation 幂等记录
       -> COMMIT，确认 WAL 落盘

事务外：生成 HUB OfficialState BLS 签名和 ReservationResult BLS 签名

事务 2：写入 OfficialState、RecoveryPackage 和 LedgerEntry
       -> CAS 更新 latest_sequence/latest_checkpoint_hash
       -> intent 与 reservation 更新为 SIGNED
       -> COMMIT
```

在两次事务之间崩溃时，`recover_pending` 从持久化的 `PREPARED` 记录重建并完成同一个状态。BLS 签名和规范结果保持确定性，不会分配第二个序号或重复扣款。

## Reservation 幂等

唯一键为 `(funding_coin_id, reservation_nonce)`。内容指纹覆盖完整 LedgerEntry 和用户授权签名，不包含传输层重试信息。

- 相同 nonce、相同授权内容：返回数据库中原始 SignedReservationResult 和 RecoveryPackage。
- 相同 nonce、不同商户/回执公钥/金额/授权签名：返回 `HubError::NonceConflict`，不创建新状态。
- `observed_peak_height >= acceptance_cutoff_height`：持久化并返回已签名 `RejectedFreezing`，`ledger_written = false`。
- 用户签名编码合法但验证失败：持久化并返回已签名 `InvalidAuthorization`，`ledger_written = false`。
- 余额不足或账本已满：返回对应的确定性签名拒绝，不增加序号。

`NonceConflict` 是内部服务错误，后续 API 层必须映射为协议状态码 `104`，且不得签出一份与原成功结果字段冲突的第二份 ReservationResult。

## 可信链状态门控

生产调用使用 `reserve_with_chain`，不得把客户端提供的高度传给测试兼容入口 `reserve`。`ChainStateProvider` 每次读取：

```text
network_id / genesis challenge
node synced
peak height + header hash
Funding Coin birth height / puzzle hash / amount / spent state
```

`ChiaFullNodeRpcProvider` 支持公开 HTTPS 或 Chia Full Node 双向 TLS，调用 `get_network_info`、`get_blockchain_state` 和 `get_coin_record_by_name`。Funding Coin 的 puzzle hash 从保存的 CLVM reveal 重新计算，不接受调用方声明值。

预扣链状态顺序固定为：

```text
事务前读取一次节点快照
BEGIN IMMEDIATE，锁定 Funding Coin 账本
事务内再次读取节点快照
验证网络、同步状态、peak 和 Funding Coin
持久化提交高度、A、S 和签名意图
COMMIT 后才生成签名
```

最终判断只使用事务内第二次快照。因此请求到达时为 `A-1`、提交时已经为 `A` 的请求必须返回 `REJECTED_FREEZING`，不会分配 entry index 或状态序号。

测试激活策略等待 32 个确认高度。激活后 Funding Coin 消失进入 `REORG_PENDING`；重新出现时：

```text
new_A      = new_F + acceptance_blocks
effective_A = min(previous_effective_A, new_A)
new_S      = new_F + close_delay_blocks
```

重组后的首次观测只解除链不确定性，不立即接受预扣；下一次稳定双快照才允许继续。接受截止绝不因 Funding Coin 出生高度增大而延后。

`RedundantChainStateProvider` 支持主备来源一致性检查：网络或 Funding Coin 不同、同高度 header hash 不同、peak 高度差超过配置阈值时返回 `CHAIN_STATE_UNCERTAIN`；一致且高度差在阈值内时采用较高峰值。

## 冲突证据

`protocol-v3_6` 现已定义：

```text
SignedReservationResult(result, hub_result_signature)
DoubleSignEvidence(first_official_state, second_official_state)
ConflictingResultEvidence(first_signed_result, second_signed_result)
```

两份证据中的对象按各自消息哈希升序编码。验证时要求相同协议上下文、相同 Funding Coin、相同序号或 nonce、不同消息哈希，并逐份验证 HUB A 签名。

## Golden vectors

机器可读向量位于 [test-vectors/hub-v3_6.json](test-vectors/hub-v3_6.json)，固定覆盖：

- State 0 和 sequence 1/2/3 的连续推进；
- OfficialState、RecoveryPackage 和 SignedReservationResult 的哈希与规范二进制；
- 相同请求字节级幂等；
- nonce 内容冲突；
- A 高度冻结拒绝；
- A-1/A/A+1 和提交时跨越 A 的边界；
- RPC 断开、节点未同步、错误网络、缺失 peak 和错误 Funding Coin；
- 测试网 32 个确认激活、Funding Coin 消失/花费和主备源冲突；
- 激活后重组及 `effective_A = min(old_A, new_A)`；
- PREPARED 落盘后故障及恢复。

重新生成与验证：

```powershell
cargo run --offline --manifest-path .\Cargo.toml --bin generate-hub-vectors
cargo test --offline --manifest-path .\Cargo.toml
cargo clippy --offline --all-targets --manifest-path .\Cargo.toml -- -D warnings
```

## HTTP API V3.6

版本化 HTTP API 已达到 `VECTOR_READY`，规范见 [HTTP-API.md](HTTP-API.md)。当前实现包含可信链状态预扣、原 nonce 查询、按序号或 latest 获取 RecoveryPackage，以及带接收方幂等键的投递/重试状态持久化。

API 固定使用 `/api/v3.6` 和 `x-xhub-protocol-version: 0x0360`。`UNKNOWN`、`RPC_UNAVAILABLE` 与 `INTERNAL_ERROR` 的 `ledger_written` 为 `null`，客户端必须继续查询原 nonce；只有确定性拒绝才返回 `ledger_written = false`。

测试还会在真实文件数据库上验证 `WAL + synchronous=FULL`、两个故障注入点、6 个并发写入者最终只能生成唯一的 `1..6` 相邻序号，以及链状态异常时账本序号和 entry index 均不变化。
