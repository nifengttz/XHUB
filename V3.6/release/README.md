# XHUB V3.6 测试网发布清单

本目录描述 V3.6 测试网向量基线，不是主网发布包。清单状态为 `VECTOR_READY`，`mainnet_approved` 必须保持 `false`，直到主网参数、生产绿灯门槛、最终 CLVM 哈希和独立安全评审全部完成。

生成清单：

```powershell
./generate-release.ps1
```

脚本从协议文档、golden vectors、测试网配置、CLVM 源码及 hex 产物直接计算 SHA-256，并从 `module-hashes.json` 读取模块哈希。不得手工覆盖这些值。

发布清单同时承诺 `deploy/testnet/validate-config.ps1` 和对应负面测试脚本的 SHA-256，测试网部署入口必须先通过统一的静态配置门禁。

主网部分目前只承诺 `deploy/mainnet` 的只读金丝雀预检脚本，`production_broadcast` 固定为 `false`，不构成主网生产批准。

如果任一 V3.6 组件未被 Git 跟踪或有未提交修改，对应 `source_commits` 必须显示 `UNCOMMITTED`。这能防止发布清单引用一个并未包含当前源码的旧提交。

发布前复核：

```powershell
./generate-release.ps1
git diff --exit-code -- testnet-release-v3_6.json
```

当前测试网固定默认值为：`acceptance=12288`、`freeze=200`、`close_delay=12488`、`challenge=6000`、Funding 确认深度 `32`、一份有效商户回执、托管证明 `1-of-3`。
