# XHUB V3.6 单 VPS Docker 测试

本目录只用于短期单 VPS 测试。三个 Watchtower 容器共享一个故障域，但必须使用三个不同 BLS 公钥、三个数据库目录和三个 API Token。测试 API 可以按不同公钥达到 `2-of-3`；任何结果都固定 `test_only=true`、`production_ready=false`。

在 Linux VPS 的 `V3.6` 目录构建固定标签镜像：

```bash
docker build \
  -f deploy/mainnet/docker-single-vps/Dockerfile \
  -t xhub-watchtower-v3-6:test .
```

复制两个 example JSON 为 `.local.json`，填写三个不同的 Watchtower BLS 公钥，并创建 profile 中列出的目录、确认者配置和三个至少 32 字符的 API Token 文件。所有秘密文件应设置为 `chmod 600`。

先验证并生成 Compose：

```powershell
powershell.exe -NoProfile -ExecutionPolicy Bypass `
  -File .\validate-single-vps-docker-profile.ps1 `
  -ProfilePath .\single-vps-docker-profile.local.json `
  -CustodyAttestersPath .\custody-attesters.single-vps.local.json

powershell.exe -NoProfile -ExecutionPolicy Bypass `
  -File .\generate-single-vps-docker-compose.ps1 `
  -ProfilePath .\single-vps-docker-profile.local.json `
  -CustodyAttestersPath .\custody-attesters.single-vps.local.json `
  -OutputDirectory .\generated-compose.local
```

在 VPS 上人工执行：

```bash
docker compose -f generated-compose.local/compose.yaml config
docker compose -f generated-compose.local/compose.yaml up -d
docker compose -f generated-compose.local/compose.yaml ps
```

Compose 使用 Linux host network，三个容器只监听宿主机 `127.0.0.1:18738`、`18739`、`18740`，不声明公网 `ports`。如需公网访问，应由已配置 TLS 1.3/mTLS 的 Nginx 反向代理接入。

RecoveryPackage 必须分别投递并持久化到三个容器。取得两份不同 BLS 公钥签署的 CustodyAttestation 后，可查询：

```text
GET /api/v3.6/funding-coins/{coin_id}/states/{sequence}/entries/{entry}/single-vps-test-greenlight?protocol_version=0x0360&threshold=2
```

同一状态的 `/production-greenlight` 仍会因为 `failure_domain_count=1` 返回 `production_ready=false`。不得把单 VPS 测试报告写入主网生产准备度的 `THREE_OPERATORS_VERIFIED` 或 `TLS_ENDPOINTS_VERIFIED` 字段。

`Dockerfile.custody-signer` builds the one-shot custody signing utility. Run it with only the unsigned API payload, one read-only attester secret, and a writable output mount. The utility validates the canonical payload and domain-separated hash before signing; it does not contain HTTP or broadcast code.
