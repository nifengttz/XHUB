# XHUB Watchtower V3.6

RecoveryPackage 接收、完整验证、隔离持久化、商户 DeliveryConfirmation 校验，以及独立 Watchtower 托管证明聚合已实现到 `VECTOR_READY`。

实现入口：[src/lib.rs](src/lib.rs)、[src/api.rs](src/api.rs)。HTTP 接口固定使用 `/api/v3.6` 和 `x-xhub-protocol-version: 0x0360`：

```text
POST /api/v3.6/recovery-packages
GET  /api/v3.6/funding-coins/{funding_coin_id}/recovery-packages/latest
GET  /api/v3.6/funding-coins/{funding_coin_id}/recovery-packages/{state_sequence}
POST /api/v3.6/delivery-confirmations
GET  /api/v3.6/funding-coins/{funding_coin_id}/states/{state_sequence}/entries/{entry_index}/greenlight
GET  /api/v3.6/funding-coins/{funding_coin_id}/states/{state_sequence}/entries/{entry_index}/custody-attestation
POST /api/v3.6/custody-attestations
GET  /api/v3.6/funding-coins/{funding_coin_id}/states/{state_sequence}/entries/{entry_index}/production-greenlight
GET  /api/v3.6/funding-coins/{funding_coin_id}/states/{state_sequence}/entries/{entry_index}/single-vps-test-greenlight
```

瞭望塔只接受完整且可恢复的包；当前完整包验证包括规范解码、Funding Puzzle reveal 的 CLVM 解析、HUB A 签名、全部用户授权签名、账本 Root、金额、找零和 append-only 前缀。截断、篡改、同序号冲突、旧账本修改和降序重放都会隔离或拒绝。

DeliveryConfirmation 必须匹配目标 `LedgerEntry.merchant_receipt_public_key`，它只证明商户确认交付。相同公钥即使登记为多个 signer ID 或多个故障域，也只贡献一份商户回执。生产绿灯另外要求 `CustodyAttestation`：每个证明由独立登记的 Watchtower 公钥签署，绑定同一 DeliveryConfirmation、checkpoint 和 RecoveryPackage 内容哈希。聚合同时按不同公钥和不同故障域计数；重复公钥被拒绝，同一 VPS 的多个实例只能贡献一个故障域。推荐生产条件为一份有效商户回执加跨故障域 `2-of-3` 托管证明。

`CustodyAttestation` 是链下运营证明，不进入 `ChannelTerms`，不改变 Funding/Closing CLVM 或任何已经创建的 Funding Coin。服务启动时通过 `XHUB_WATCHTOWER_CUSTODY_ATTESTERS_FILE` 加载托管证明身份；文件格式与 confirmer 文件一致，但必须使用不同的 Watchtower 密钥。

短期单 VPS Docker 模式使用独立测试接口，只按不同 `attester_public_key` 计算门槛，不要求不同 `failure_domain`。该响应固定 `failure_domain_enforced=false`、`test_only=true` 和 `production_ready=false`。生产接口及其跨故障域规则不受影响。

## 独立链状态监控

只读 Full Node RPC 监控与 CHALLENGE 本地预演已实现到 `VECTOR_READY`。实现入口：

- `src/rpc.rs`：严格读取网络、同步状态、峰值、CoinRecord 和 CoinSpend。
- `src/monitor.rs`：从 Funding spend solution 开始推导 Initial Closing Coin，并沿已确认的 Subsequent Closing Coin 谱系追踪。
- `src/bin/monitor.rs`：执行一次只读轮询并输出 JSON 决策。
- `tests/chain_monitor.rs`：覆盖旧状态挑战、最新状态、截止高度、重组、RPC UNKNOWN、终态、伪造 Coin、幂等计划和有界重试。

监控器不信任调用方声明的候选序号或 puzzle hash。它把 Funding puzzle reveal 与 CoinRecord 绑定，解析规范 CLVM solution，再使用已持久化的完整 RecoveryPackage 重建预期 Closing puzzle hash 和 Coin ID。只有链上当前状态序号低于本地最新完整状态、截止高度尚未到达且真实 CHALLENGE CLVM 本地执行通过时，才持久化挑战计划。

一次只读轮询：

```powershell
cargo run --offline --bin watchtower-monitor-v3-6 -- `
  WATCHTOWER_DB RPC_URL FUNDING_COIN_ID
```

链监控循环硬性保持 `spend_bundle_created=false`、`broadcast_ready=false` 和 `chain_broadcast=false`。它不接收私钥、fee Coin 或广播端点，也不会自动创建或发送 SpendBundle。TLS 1.3/mTLS 配置契约与三运营者身份清单门禁位于 `../deploy/mainnet`；真实证书、在线端点、跨运营者部署、fee sponsor、实际 CHALLENGE 广播和主网运营批准仍为 `OPEN`。

## 离线 SpendBundle 向量

不可广播的 CHALLENGE SpendBundle 构造与完整验证已实现到 `VECTOR_READY`：

- `src/bundle.rs` 从 `puzzles-v3_6` 取得唯一的 Closing puzzle reveal、solution 和 RecoveryPackage 签名材料，不复制 CLVM solution 布局。
- 使用真实 `chia_protocol::CoinSpend`/`SpendBundle`，并由 `chia_consensus` 验证 puzzle hash、条件、cost、重复 removal/addition、金额守恒和聚合 BLS 签名。
- 外部费用只使用 `P2DelegatedConditions` 测试向量 Coin，验证 change、实际 fee 和 `RESERVE_FEE` 一致；不读取真实钱包 Coin 或私钥。
- 覆盖 State 0、Initial Closing、Subsequent Closing、`D-1`/`D` 边界、错误 Coin/金额/fee，以及构造后重组或 Coin 已花费。

候选 bundle 只保存在内存，不提供规范二进制导出、RPC 广播端点或 `push_tx`。报告固定 `broadcast_enabled=false`、`broadcast_ready=false`、`chain_broadcast=false`。测试 fee sponsor 使用 Chia 测试共识常量，因此不构成主网可广播材料；生产 fee sponsor、真实私钥隔离、广播审批和确认跟踪仍为 `OPEN`。

## 监控计划与离线准备

显式离线准备状态机已实现到 `VECTOR_READY`：

- 只有已经持久化为 `SIMULATED_ONLY` 的挑战计划，才能调用 `prepare_offline_challenge`。
- 准备过程重新绑定当前 RecoveryPackage、Closing Coin、初始出生高度、固定截止高度、链峰值和测试 fee Coin，再运行完整 consensus/BLS 验证。
- SQLite 只保存验证报告、Coin ID 和链快照，不保存 SpendBundle bytes、测试私钥或签名材料。
- 验证成功后的状态为 `OFFLINE_VERIFIED_AWAITING_APPROVAL`，这不等于广播就绪。
- 新峰值、重组、Closing Coin 已花费、截止高度或其他链变化会转为 `INVALIDATED_CHAIN_CHANGE`；RPC UNKNOWN 会转为 `CHAIN_RECHECK_REQUIRED`。
- 失效记录不能恢复原绿灯，必须基于新的完整链快照重新构造和验证。

## 人工审批与双人复核

离线准备之后的审批凭证状态机已实现到 `VECTOR_READY`：

- `ApprovalStatement` 使用规范二进制编码和独立 `XHUB_CHALLENGE_APPROVAL_V3_6` 域签名，绑定协议版本、preparation ID、Closing/Funding/fee Coin ID、报告哈希、链峰值、截止高度、审批者、故障域、时间窗和 nonce。
- 每次完整离线重建都会产生新 preparation epoch；旧审批保留审计记录但标记撤销，不能重放到新准备。
- 一个有效审批进入 `PARTIALLY_APPROVED`；只有两个不同审批者且来自两个不同故障域，才进入 `DUAL_APPROVED_RECHECK_REQUIRED`。
- 重复审批者、公钥或 nonce、同故障域第二票、签名/字段篡改及过期凭证均被拒绝或不计入门槛。
- RPC UNKNOWN、新峰值、同高度重组、Closing Coin 变化或 `peak >= D` 会把审批标记为 `APPROVAL_REVOKED_CHAIN_CHANGE`；恢复后必须重新准备并重新签名。

双人批准仍不是广播绿灯。状态固定 `broadcast_enabled=false`、`broadcast_ready=false`、`chain_broadcast=false`，SQLite 不保存 SpendBundle bytes 或任何私钥，也没有 bundle 导出、`push_tx` 或广播客户端。生产审批身份基础设施、真实 fee sponsor、最终链上重检、广播和确认跟踪继续保持 `OPEN`。

## 最终链上重检

双人批准后的最终只读链上重检已实现到 `VECTOR_READY`：

- 生产入口 `poll_final_chain_recheck` 通过 `WatchtowerChainProvider` 重新读取 Full Node RPC，并从 Funding spend/Closing Coin 谱系自行推导当前状态，不接受调用者声明的 Coin 状态。
- 重检要求两个仍在有效期内、来自不同故障域的审批；重新解码规范审批声明、验证 BLS 签名，并核对 SQLite 索引列与签名字段一致后计算 `approval_set_hash`。
- 当前峰值、header hash、Closing Coin、出生高度、金额和 `D` 必须与已批准准备完全一致，Closing Coin 必须未花费且 `peak < D`。
- 通过后只持久化 `FINAL_RECHECK_VERIFIED_NO_BROADCAST`，有效期为 30 秒与两张审批最早过期时间的较小值；相同输入幂等，更新重检会使旧记录失效。
- RPC UNKNOWN、节点未同步、重组、链快照变化、到达 `D` 或重建准备都会撤销审批和重检记录，必须从新快照重新开始。

重检表仍只保存哈希、Coin ID、链快照、时间窗和状态，不保存 SpendBundle bytes 或私钥。即使重检通过，三个广播字段仍固定为 `false`，没有 bundle 导出、`push_tx`、mempool 提交或确认跟踪。

## SpendBundle 承诺绑定

离线构造、审批和最终重检之间的精确执行材料承诺已实现到 `VECTOR_READY`：

- `XHUB_SPEND_BUNDLE_COMMITMENT_V3_6` 按原始 CoinSpend 顺序承诺数量，以及每项的 parent Coin ID、puzzle hash、8 字节 amount、8 字节长度前缀的完整 puzzle reveal 和 solution，最后承诺 96 字节聚合签名。
- 承诺只在真实 bundle 完成 consensus/BLS 验证后计算；相同材料产生相同哈希，CoinSpend 顺序、任一程序、Coin、fee sponsor 或签名变化都会产生不同哈希。
- SQLite 只保存 32 字节 `bundle_commitment`；审批声明、preparation ID 和最终重检均绑定该值，不提供读取或导出底层 SpendBundle 的接口。
- 从第十三阶段数据库迁移时只增加可空列；旧准备因没有承诺而明确不可用，必须重新构造并重新取得双人审批，禁止根据旧报告猜测或补造承诺。

该承诺证明审批针对哪一份已验证执行材料，但仍不是广播授权。`broadcast_enabled`、`broadcast_ready` 和 `chain_broadcast` 继续固定为 `false`。

## 广播前执行清单

最终重检后的短期 Execution Manifest 已实现到 `VECTOR_READY`：

- 只有状态为 `FINAL_RECHECK_VERIFIED_NO_BROADCAST` 且仍在有效期内的重检可以签发清单；签发时再次核对当前 preparation、双审批状态和重新计算的 `approval_set_hash`。
- `XHUB_EXECUTION_MANIFEST_V3_6` 绑定 recheck/preparation ID、Closing/Funding/fee Coin ID、报告哈希、bundle commitment、审批集合、峰值、`D` 和清单时间窗。
- 清单有效期为 10 秒与最终重检剩余有效期的较小值；相同时间与输入幂等，新重检会把旧清单标为 `MANIFEST_SUPERSEDED`。
- RPC UNKNOWN、重组、链快照变化、重建准备或到达截止条件会转为 `MANIFEST_INVALIDATED_CHAIN_CHANGE`；时间到达则转为 `MANIFEST_EXPIRED`。
- 清单只保存哈希、Coin ID、链快照和时间，不包含 SpendBundle bytes、CLVM 程序、签名材料、私钥、RPC 端点或广播命令。

`MANIFEST_VERIFIED_NO_BROADCAST` 是审计状态，不是执行授权。三个广播字段继续由数据库约束固定为 `false`。

## 最终执行授权闸门

第十六阶段增加了独立的 `ExecutionAuthorization`。它只能从当前有效的
`MANIFEST_VERIFIED_NO_BROADCAST` 签发，并在签发时重新核对最终链上重检、离线准备、双故障域审批集合、所有 Coin ID、报告哈希、Bundle commitment、链峰值和挑战截止高度。授权有效期最多 5 秒，新的授权会替换同一 Manifest 的旧授权；Manifest 过期、被替换、RPC UNKNOWN、重组或准备重建会使授权失效。

`simulate_execution_submission` 只记录模拟提交次数和时间，不接收或导出 SpendBundle，不生成广播材料，不调用 `push_tx`，并始终保持 `broadcast_enabled=false`、`broadcast_ready=false`、`chain_broadcast=false`。因此该闸门是可审计的执行前检查，不是主网广播许可。

第十七阶段将授权闸门接入已认证的版本化 HTTP API：

```text
POST /api/v3.6/execution-manifests/{manifest_id}/authorization
GET  /api/v3.6/execution-authorizations/{authorization_id}
POST /api/v3.6/execution-authorizations/{authorization_id}/simulate
GET  /api/v3.6/execution-authorizations/{authorization_id}/simulated-receipt
```

所有请求同时要求 `x-xhub-protocol-version: 0x0360` 与请求体或查询参数中的 `protocol_version=0x0360`。响应只包含授权绑定哈希、Coin ID、链快照、状态、时间窗和模拟次数；请求 DTO 使用 `deny_unknown_fields`，明确拒绝 `spend_bundle_canonical_hex` 等执行材料。不存在或当前不可用分别返回 `AUTHORIZATION_NOT_FOUND` 与 `AUTHORIZATION_NOT_AVAILABLE`，且均为 fail-closed。

第十八阶段把模拟提交收紧为单次消费。请求必须提供 32 字节 `submission_nonce`；第一次调用在同一 SQLite 事务内把授权转为 `EXECUTION_AUTHORIZATION_CONSUMED_SIMULATED_ONLY` 并写入 `SIMULATED_SUBMISSION_RECORDED` 收据。同一授权和 nonce 的重试幂等返回原收据，不同 nonce、全局 nonce 重用，以及通过同一 Manifest 新签发授权后的重复消费都会被拒绝。`XHUB_SIMULATED_SUBMISSION_RECEIPT_V3_6` 只承诺授权 ID、Manifest ID、bundle commitment、nonce 和消费时间，收据仍不包含 SpendBundle 或广播材料。

第十九阶段加入了执行审计哈希链。`XHUB_EXECUTION_AUDIT_V3_6` 追加承诺 Manifest 签发、Authorization 签发和模拟收据消费事件，链头可通过以下接口只读核验：

```text
GET /api/v3.6/execution-audit?protocol_version=0x0360
```

每个事件绑定前一事件哈希、序号、事件类型、主体 ID、绑定哈希、状态和时间；接口返回事件数、链头和 `valid`，不导出事件材料、SpendBundle 或私钥。审计链可检测事件篡改、删改和链头不一致；为了抵御整个本地数据库被回滚或替换，链头仍必须由独立系统定期锚定，该外部锚定机制保持 `OPEN`。

第二十阶段将新 Manifest、Authorization 和模拟收据的业务写入与对应审计事件、审计链头放入同一个 SQLite 事务。若审计事件追加失败，SQLite 会回滚该次业务状态变更；故障注入测试分别覆盖三种事件类型。该原子性保证不覆盖其他无关状态更新，也不改变任何广播限制。

第二十一阶段补齐 SQLite 并发与 WAL 恢复证据：8 个独立 Watchtower 连接同时提交同一后继 RecoveryPackage 时只保留一个 append-only head，幂等请求不产生隔离记录；关闭后重新打开数据库仍保留已提交包、head 和隔离记录，并恢复 `journal_mode=WAL`、`synchronous=FULL`；子进程在大事务未提交时强制异常退出后，重启会丢弃全部未提交记录、保留之前提交的 RecoveryPackage，且 `PRAGMA integrity_check` 返回 `ok`。这些测试证明当前本地 SQLite 边界，不等同于跨主机复制、磁盘损坏恢复或整库回滚防护。

第二十二阶段加入执行审计链头锚点。`create_execution_audit_anchor` 只允许对当前有效链创建幂等锚点，调用方应把 `anchor_id`、事件数和链头哈希保存到 Watchtower 数据库之外；`verify_execution_audit_anchor` 支持验证该锚点的前缀是否仍存在，并在当前事件数倒退时报告 `rollback_detected=true`。本地表只用于留痕，不能替代外部保存；锚点接口不联网、不广播、不导出 SpendBundle 或私钥。

第二十三阶段加入数据库备份清单和恢复前校验：`create_database_backup` 使用 SQLite `VACUUM INTO` 生成新文件，并对文件大小、文件哈希、审计链头及可选外部锚点生成 `DatabaseBackupManifest`；`verify_database_backup_state` 在恢复副本上重新打开数据库，验证文件校验、审计链和锚点前缀。备份文件被篡改时会在打开前失败关闭。此阶段没有实现加密、密钥托管、远程复制或自动备份编排，这些仍属于部署与安全评审范围。

第二十四阶段加入 `XHUB_WATCHTOWER_ENCRYPTED_BACKUP_V1` 封装。备份使用 XChaCha20-Poly1305、32 字节调用方密钥、24 字节操作系统随机 nonce、32 字节 key ID 和绑定协议版本/key ID 的 AAD；错误密钥、错误 key ID、密文或认证标签篡改均拒绝且不会写出明文。密钥轮换通过解密旧封装并用新 key ID/密钥重新加密完成，密钥本身不写入数据库、备份清单、文件头或日志。跨故障域副本一致性比较解密后备份清单中的文件哈希、大小、审计链头和锚点，而不是比较因随机 nonce 必然不同的密文。KMS/HSM、密钥销毁和远程复制仍由部署层负责。

第二十五阶段将加密备份收紧为原子工作流。`BackupKeyProvider` 是唯一的密钥获取边界，返回的 32 字节 key 使用 `Zeroizing` 管理且不由 Watchtower 持久化；`create_encrypted_database_backup` 只在随机临时路径生成 SQLite 明文和加密 envelope，成功后重命名发布，任何失败都会清理临时文件；`restore_encrypted_database_backup` 只有在 envelope 哈希、AEAD 解密、文件清单、审计链和可选锚点全部验证通过后才发布恢复数据库。目标路径预先存在时 fail-closed，避免覆盖现有副本。

第二十六阶段增加版本化加密备份清单交接。`EncryptedBackupArtifact` 使用 `XHUBAM01` magic、V3.6 协议版本和 `XHUB_WATCHTOWER_ENCRYPTED_BACKUP_V1` 域进行 canonical 编码，固定包含 DatabaseBackupManifest、envelope hash 与 key ID；`encode_encrypted_backup_artifact` 和 `decode_encrypted_backup_artifact` 可跨进程、跨副本传递清单。解析严格拒绝错误 magic/domain、错误版本、截断数据、错误字段长度和 trailing bytes；清单 `backup_id` 会重新由字段派生校验，任何篡改在恢复前 fail-closed。清单不包含 key bytes，密钥仍只通过 `BackupKeyProvider` 获取。KMS/HSM、远程密钥分发和真实跨 VPS 复制保持 `OPEN`，所有广播开关继续为 `false`。

第二十七阶段增加备份清单交接审计。`record_backup_artifact_handoff` 只持久化 artifact hash、backup_id、envelope hash、key ID、清单哈希、时间和 `RECEIVED/VERIFIED/REJECTED` 状态；重复接收同一清单幂等，内容冲突 fail-closed。`verify_backup_artifact_handoff` 重新检查 envelope、AEAD、数据库清单、审计链和可选 anchor，验证成功与对应 `BACKUP_ARTIFACT_VERIFIED` 审计事件在同一 SQLite 事务中提交；任何失败写入 `REJECTED` 审计记录并向客户端返回错误，已拒绝清单不可恢复为成功。密钥不落盘，广播能力仍关闭。

第二十八阶段增加跨副本一致性比较。`encrypted_backup_replicas_are_consistent` 比较解密后的 DatabaseBackupManifest，故意忽略随机 nonce、envelope hash 和独立 key ID，允许合法的密钥轮换副本一致；`verified_backup_handoffs_are_consistent` 要求所有交接均为 `VERIFIED`，并匹配 backup_id、envelope hash 和清单字节哈希。恢复演练测试覆盖两个独立加密副本、篡改清单和未验证/分歧交接，仍不启用远程复制、自动调度或任何广播能力。

第二十九阶段增加恢复演练报告与保留候选计算。`run_backup_restore_drill` 只接受 `VERIFIED` 交接，使用临时明文路径重新执行 AEAD、数据库清单、审计链和 anchor 验证，记录耗时、结果和失败原因并在完成后清理明文；报告与 `BACKUP_RESTORE_DRILL_COMPLETED` 审计事件原子提交。`backup_retention_candidates` 只返回已经通过演练、超过最小保留年龄且不属于最新 N 份的 backup_id，永不自动删除文件；未来的人工删除必须再次核对清单和审计状态。

第三十阶段将恢复运维结果接入版本化 HTTP API：

```text
GET  /api/v3.6/backup-restore-drills/{drill_id}?protocol_version=0x0360
POST /api/v3.6/backup-retention-candidates
```

请求和响应继续要求 `x-xhub-protocol-version: 0x0360`；演练查询只返回已持久化报告，保留候选接口只根据调用方提交的清单计算 `backup_id`，响应固定 `deletion_performed=false`。HTTP 层不获取密钥、不解密文件、不删除文件、不接收 SpendBundle 或广播材料，未知报告、错误清单、错误版本均 fail-closed。

## 第三十一阶段评审状态

V3.6 五个 Rust 库已完成跨模块回归与部署前接口复核，当前代码主线状态为 `REVIEWED`：

- `cargo test --offline --all-targets`：protocol、puzzles、hub、watchtower、wallet 全部通过。
- `cargo clippy --offline --all-targets -- -D warnings`：五库通过；watchtower 依赖仍报告 3 条 `GenericArray::from_slice` 弃用提示，不影响构建。
- `cargo fmt -- --check`、协议版本/API 一致性扫描、Rust 源码广播安全扫描和 `git diff --check`：通过。

`REVIEWED` 不等于生产上线。主网参数、KMS/HSM、跨 VPS 复制、真实 TLS 端点、真实测试网生命周期、独立外部安全评审和广播审批仍为 `OPEN`；`broadcast_enabled`、`broadcast_ready`、`chain_broadcast` 始终为 `false`。
