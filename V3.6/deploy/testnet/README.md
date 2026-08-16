# XHUB V3.6 测试网部署

本目录启动钱包、HUB 和瞭望塔的测试网服务边界。协议及 CLVM 仍处于 `VECTOR_READY`，不得用于主网。

## 网络边界

- Wallet 默认监听 `127.0.0.1:8736`。
- HUB 默认监听 `127.0.0.1:8737`，所有 `/api/v3.6` 请求必须携带 Bearer token。
- Watchtower 默认监听 `127.0.0.1:8738`，所有 `/api/v3.6` 请求必须携带 Bearer token。
- 只有 Caddy 监听外部 TLS 端口；Rust 服务拒绝绑定非 loopback 地址。
- 测试环境可运行 `start-local.ps1`，生产式测试网必须使用 `Caddyfile` 或等效反向代理。

## 准备私密文件

创建不进入 Git 的 `secrets` 目录：

```text
secrets/hub-api-token.txt          至少 32 个随机字符
secrets/watchtower-api-token.txt   至少 32 个随机字符
secrets/hub-bls-secret.hex         HUB A 的 32 字节 BLS 私钥 hex
```

不要把私钥或 token 写入 JSON、PowerShell 脚本、命令行参数或日志。`config.example.json` 仅包含秘密文件路径。

## Full Node RPC 预检

复制 `config.example.json` 为 `config.local.json`，填入 RPC 地址、客户端证书、私钥路径和预期 Network ID。然后运行：

```powershell
./validate-config.ps1 -ConfigPath ./config.local.json
./rpc-preflight.ps1 -ConfigPath ./config.local.json -FundingCoinId <coin-id>
```

静态校验不联网、不启动进程。它会拒绝缺失字段、非 loopback 服务监听、重复端口、无效或占位 Network ID、非 HTTP(S) RPC、非 loopback 明文 HTTP RPC、未成对配置的 RPC 客户端证书/私钥、与监听地址不一致的瞭望塔 URL、共用数据库路径和共用秘密文件。`start-local.ps1`、`rpc-preflight.ps1` 与 `smoke-test.ps1` 都会自动执行同一校验。

预检会读取 `get_network_info`、`get_blockchain_state` 和 `get_coin_record_by_name`，输出 genesis challenge、同步状态、峰值、CoinRecord 和确认深度。

## 启动与检查

```powershell
./start-local.ps1 -ConfigPath ./config.local.json
./smoke-test.ps1 -ConfigPath ./config.local.json
```

`smoke-test.ps1` 默认只检查三个进程和认证边界。提供 `-FundingRegistrationPath` 后，它会调用 HUB 的链上 Funding 注册接口；文件格式参见 `funding-registration.example.json`。

TLS 入口使用：

```powershell
caddy run --config ./Caddyfile
```

限流由部署入口实施。示例 Caddy 配置只负责 TLS 和请求体上限；上线前仍需在云负载均衡/WAF 配置每 token/IP 的速率限制并保存压测证据。
