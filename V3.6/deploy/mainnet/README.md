# XHUB V3.6 主网金丝雀预检

本目录只提供主网只读预检，不提供主网服务启动脚本，不导出 SpendBundle，不调用 `push_tx`，不广播交易。

当前阶段不是主网生产批准。必须先完成本目录的只读预检、主网参数冻结、生产瞭望塔与密钥托管评审，随后才允许另行设计人工审批的金丝雀交易流程。

## 使用顺序

1. 复制 `config.mainnet.example.json` 为本地未跟踪文件，例如 `config.mainnet.local.json`。
2. 默认使用 `rpc_mode=trusted_public_https` 和 `https://api.coinset.org`，无需客户端证书；如改用自建节点，则设置 `rpc_mode=self_hosted_mtls` 并填写 mTLS 证书/私钥。两种模式都必须填写主网 genesis challenge、真实 Funding Coin ID 和瞭望塔接收者 ID。
3. 将 API Token、HUB BLS 私钥、商户回执确认者配置和独立 Watchtower 托管证明身份配置放入本地 `secrets`/配置文件路径，不写入 Git。`watchtower_custody_attesters_file` 中不得重复使用商户回执公钥。
4. 复制并填写 `watchtower-identities.mainnet.example.json`、`custody-attesters.mainnet.example.json` 和 `watchtower-tls-profile.mainnet.example.json`。三个 Watchtower 必须来自不同运营者、基础设施供应商、区域和故障域，并使用不同 BLS 公钥及 TLS 证书。
5. 执行静态门禁：

```powershell
powershell.exe -NoProfile -ExecutionPolicy Bypass `
  -File .\validate-config.ps1 `
  -ConfigPath .\config.mainnet.local.json
```

6. 通过静态门禁后执行身份和 TLS 配置门禁：

```powershell
powershell.exe -NoProfile -ExecutionPolicy Bypass `
  -File .\validate-watchtower-identities.ps1 `
  -IdentityManifestPath .\watchtower-identities.mainnet.local.json `
  -CustodyAttestersPath .\custody-attesters.mainnet.local.json

powershell.exe -NoProfile -ExecutionPolicy Bypass `
  -File .\validate-watchtower-tls-profile.ps1 `
  -TlsProfilePath .\watchtower-tls-profile.mainnet.local.json `
  -IdentityManifestPath .\watchtower-identities.mainnet.local.json
```

身份门禁核对三套身份与应用实际加载配置完全一致，并拒绝重复公钥、运营者、供应商、区域、故障域、主机或证书指纹。TLS 门禁要求 TLS 1.3、强制验证客户端证书、证书指纹绑定、请求大小/速率限制和回环 HTTP 上游。它们只验证配置，不读取私钥，不探测在线端点，也不授予生产批准。

7. 通过以上门禁后执行只读 RPC 预检：

```powershell
powershell.exe -NoProfile -ExecutionPolicy Bypass `
  -File .\rpc-preflight.ps1 `
  -ConfigPath .\config.mainnet.local.json
```

预检只调用 `get_network_info`、`get_blockchain_state` 和 `get_coin_record_by_name`，验证 Network ID、同步状态、峰值、Funding Coin 和确认深度。任何错误都 fail-closed。

## 金丝雀审批计划

只读预检通过后，先复制 `canary-plan.example.json` 到本地未跟踪文件，填入真实 Funding Coin ID 和 Puzzle Hash，并运行：

```powershell
powershell.exe -NoProfile -ExecutionPolicy Bypass `
  -File .\validate-canary-plan.ps1 `
  -PlanPath .\canary-plan.local.json
```

计划固定 `max_total_mojo=1`、`manual_approval_required=true`、`broadcast_enabled=false`，并要求 RPC、Coin、Puzzle/模块哈希、RecoveryPackage 模拟投递和双人复核全部有证据。该计划只生成审批前审计对象，不生成 SpendBundle，不提供广播命令。

证据包使用 `canary-evidence.example.json` 模板，并通过 `validate-canary-evidence.ps1` 校验。校验会绑定计划 SHA-256，核对四个 CLVM 模块哈希、主网 RPC 快照摘要、Funding Coin/Puzzle Hash、RecoveryPackage 模拟结果和两个不同故障域的批准记录。证据包不能包含私钥、助记词、SpendBundle bytes 或广播授权。

主网参数、CLVM 最终哈希、KMS/HSM、真实三运营者部署、在线 TLS 端点验证和人工广播审批仍未被本目录批准。代码中的生产候选绿灯要求一份商户回执加跨故障域 `2-of-3` 独立 CustodyAttestation，但配置校验不等于实际运营身份已经通过评审；三个广播字段固定为 `false`。

## 第 36 至 40 阶段门禁

- `validate-mainnet-parameters.ps1` 校验候选参数算术、不可变性、一份商户回执与 Watchtower 托管证明 `2-of-3` 策略；候选参数仍要求外部安全评审，且 `mainnet_approved=false`。
- `generate-artifact-manifest.ps1` 和 `verify-artifact-manifest.ps1` 对协议、CLVM、配置、计划和证据校验器生成并验证 SHA-256 清单。
- `check-secret-isolation.ps1` 检查真实秘密文件的分离、大小和 Windows ACL，不输出秘密内容。
- `evaluate-readiness.ps1` 聚合全部门禁；当前模板因缺少真实配置、RPC、证据、秘密 ACL 和外部安全评审而返回 `BLOCKED_EXTERNAL_INPUT`。
- `generate-candidate-release.ps1` 只生成 `CANDIDATE_NOT_APPROVED` 候选包，排除本地配置、秘密、数据库和交易材料，且固定 `production_broadcast=false`。

## 第 41 至 45 阶段证据链

- `validate-rpc-preflight-output.ps1` 验证现有 Rust Full Node preflight 的结构化输出、主网 Network ID、同步状态和 Funding 确认深度。
- `validate-funding-binding.ps1` 将主网配置、1-mojo 计划、RPC Coin/Puzzle 和候选参数绑定为同一个摘要。
- `validate-recovery-simulation.ps1` 验证钱包 Closing/Recovery 模拟报告，要求 RecoveryPackage 与所有 CLVM 条件通过且没有 SpendBundle 或广播。
- `validate-approval-records.ps1` 要求两名不同 reviewer、不同故障域对同一个证据 SHA-256 明确批准；审批记录本身不能授权广播。
- `run-canary-dry-run.ps1` 串行执行七个校验器并输出输入哈希。最高状态为 `DRY_RUN_COMPLETE_MANUAL_REVIEW_REQUIRED`，不会启动服务、签名或广播。

## 第 46 至 47 阶段身份与 TLS 门禁

- `validate-watchtower-identities.ps1` 要求三套身份分别使用不同 BLS 公钥、运营者、供应商、区域、故障域、API 主机和证书指纹，并与 Watchtower 实际加载的 attester 文件逐项一致。
- `validate-watchtower-tls-profile.ps1` 要求三个公开 HTTPS 入口固定 TLS 1.3 和强制客户端证书验证，证书指纹必须与身份清单一致，后端只能连接回环 HTTP Watchtower。
- 两个校验器都固定 `production_approved=false` 和 `production_broadcast=false`。真实跨运营者部署与在线 TLS 握手验证继续是外部门禁。

## 第 48 至 49 阶段在线端点与部署证据

先复制并填写 `watchtower-endpoint-probe.mainnet.example.json` 为 `watchtower-endpoint-probe.mainnet.local.json`。计划模式只核对三节点身份、URL、证书指纹和独立凭据路径，不读取秘密或联网：

```powershell
powershell.exe -NoProfile -ExecutionPolicy Bypass `
  -File .\verify-watchtower-tls-endpoints.ps1 `
  -ProbeProfilePath .\watchtower-endpoint-probe.mainnet.local.json `
  -IdentityManifestPath .\watchtower-identities.mainnet.local.json `
  -TlsProfilePath .\watchtower-tls-profile.mainnet.local.json `
  -PlanOnly
```

实际探针执行前，使用 `check-secret-isolation.ps1 -ProbeProfilePath ...` 检查三个独立 API Token、客户端 PFX 和密码文件的 ACL。去掉 `-PlanOnly` 后，探针强制 TLS 1.3、mTLS、系统证书链和叶证书 SHA-256 pin，并分别查询健康接口与最新 RecoveryPackage。三个节点必须返回完全相同的 Funding Coin、状态序号、checkpoint 和 RecoveryPackage 内容哈希，才输出 `TLS_ENDPOINTS_VERIFIED`。

随后复制并填写 `watchtower-deployment-evidence.mainnet.example.json` 为 `watchtower-deployment-evidence.mainnet.local.json`，将身份清单和 TLS 报告的文件 SHA-256、三套运营者部署记录及两名不同故障域复核人绑定，并运行：

```powershell
powershell.exe -NoProfile -ExecutionPolicy Bypass `
  -File .\validate-watchtower-deployment-evidence.ps1 `
  -EvidencePath .\watchtower-deployment-evidence.mainnet.local.json `
  -IdentityManifestPath .\watchtower-identities.mainnet.local.json `
  -TlsEndpointReportPath .\watchtower-tls-endpoint-report.local.json
```

只有 24 小时内的三端点报告、三套唯一部署和双故障域复核全部一致，才输出 `THREE_OPERATORS_VERIFIED`。两个状态都只供准备度聚合使用，仍固定禁止生产批准和广播。

## 第 50 至 51 阶段运行时与反向代理

复制 `watchtower-runtime.mainnet.example.json` 为 `watchtower-runtime.mainnet.local.json`，填写最终 Watchtower 二进制 SHA-256 和三个节点的 Linux 路径。运行时门禁要求非管理员账户、回环监听、固定数据库/配置/备份路径、资源上限和完整 systemd 沙箱：

```powershell
powershell.exe -NoProfile -ExecutionPolicy Bypass `
  -File .\validate-watchtower-runtime.ps1 `
  -RuntimePath .\watchtower-runtime.mainnet.local.json `
  -IdentityManifestPath .\watchtower-identities.mainnet.local.json `
  -TlsProfilePath .\watchtower-tls-profile.mainnet.local.json
```

通过后可生成未安装配置：

```powershell
powershell.exe -NoProfile -ExecutionPolicy Bypass `
  -File .\generate-watchtower-systemd-units.ps1 `
  -RuntimePath .\watchtower-runtime.mainnet.local.json `
  -IdentityManifestPath .\watchtower-identities.mainnet.local.json `
  -TlsProfilePath .\watchtower-tls-profile.mainnet.local.json `
  -OutputDirectory .\generated-systemd.local

powershell.exe -NoProfile -ExecutionPolicy Bypass `
  -File .\generate-watchtower-nginx-configs.ps1 `
  -TlsProfilePath .\watchtower-tls-profile.mainnet.local.json `
  -IdentityManifestPath .\watchtower-identities.mainnet.local.json `
  -OutputDirectory .\generated-nginx.local
```

`verify-watchtower-generated-configs.ps1` 会重新核对输入哈希、全部生成文件哈希、systemd 沙箱项和 Nginx TLS/mTLS/限流项。生成器不执行 `systemctl`、不重载 Nginx、不读取秘密，也不启动服务；最终状态只能是 `DEPLOYMENT_CONFIGS_VERIFIED_NOT_INSTALLED`。

## 第 52 至 53 阶段单 VPS Docker 测试

资源有限时可使用 `docker-single-vps` 目录，在同一 Linux VPS 上运行三个使用不同 BLS 公钥、数据库和 API Token 的 Watchtower 容器。测试接口取消故障域门槛，只按不同公钥计算 `2-of-3`；生产接口不变。Docker 计划和 Compose 生成物始终标记 `test_only=true`、`production_ready=false`、`production_broadcast=false`。

当前开发机没有 Docker CLI，因此本阶段只完成 Dockerfile、Compose 生成和静态安全测试。真实 VPS 必须人工执行 `docker build`、`docker compose config` 和 `docker compose up -d`，其输出不能用来满足生产三运营者门禁。
