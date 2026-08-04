# Chia 主网 10 Mojo 测试报告

日期：2026-08-03  
网络：Chia 主网  
钱包 fingerprint：`3895043750`  
钱包 ID：`1`

## 一、测试范围

本报告记录两次真实主网测试：

1. 一次 Claim 测试，使用 10 mojo funding coin，验证输出为 Merchant 1 mojo 和 User 9 mojo。
2. 一次 Refund 测试，使用另一枚 10 mojo funding coin，验证 User 取回完整 10 mojo。

两次测试的 funding coin 均未用于支付手续费，测试 bundle 均以 0 mojo fee 广播并被本地 full node 接受。

## 二、测试前钱包状态

- 钱包高度：`9097387`
- 同步状态：`Synced`
- 可花费余额：`11240399880 mojo`
- 本地 full node：`https://127.0.0.1:8555`
- 主网 Genesis Challenge：`ccd5bb71183532bff220ba46c268991a3ff07eb358e8255a65c30a2dce0e5fbb`

## 三、Claim 测试

Channel puzzle hash：

`03a15cff7496e045aab67b8ad3d9e866232bc4b8b84e04f52621fa2afeb744eb`

- Funding coin：`d69a24145fe33299f42e119362007629224419e272d71878fcfe96ff6ed7c8b5`
- Funding 金额：10 mojo
- Funding 确认高度：`9097414`
- Claim 截止高度：`9097451`
- Refund 高度：`9097452`
- Transaction id：`e32541cdb188d9300db8c5c0756afd30d883103c0407e9fdcad118ce36a535a6`
- Claim 确认高度：`9097428`
- Channel id：`238f62d4f43e907f2415ead8e73768d01bf5a1668a2940e91e928640341af4cb`
- 手续费：0 mojo
- Mempool cost：`26812617`
- Mempool 条件：移除 10 mojo，新增 1 mojo + 9 mojo，无 CLVM 错误

### Claim 最终 children

| Child coin | 金额 | 确认高度 |
|---|---:|---:|
| `aa9f6bb455d32bc581967ee0d5afa33240f05875fb514fabcaf3d941b7d86c69` | 1 mojo | 9097428 |
| `260c31602285b22e0f43211c2df91410981352a3a964090be1c31fa06242afd5` | 9 mojo | 9097428 |

结果：**PASS**。Funding coin 只产生 1 mojo + 9 mojo 两个最终输出，并且只被花费一次。

## 四、Refund 测试

Channel puzzle hash：

`531e23ae642982b37f08c2a91a4d61012b41981ab500a83272a4c6ad2da53ac3`

- Funding coin：`1e5a96a3dbdbf86aad80371b84ca9a345b6a74f59e9cbb45c642f2d1639edfb7`
- Funding 金额：10 mojo
- Funding 确认高度：`9097431`
- Refund 激活高度：`9097413`
- Transaction id：`3f741a941809b4ea44b18cc47baa13ae6b05c671900c7f3203b6790011a9e9c1`
- Refund 确认高度：`9097434`
- 手续费：0 mojo
- Mempool 条件：移除 10 mojo，向 User puzzle hash 新增 10 mojo

### Refund 最终 child

| Child coin | 金额 | 确认高度 |
|---|---:|---:|
| `682fad3c0fa83735f37c754bd1630e07c6dea8c3fcd1f21e68a6ca0c2da72f99` | 10 mojo | 9097434 |

结果：**PASS**。Refund 分支将完整的 10 mojo 返回 User。

## 五、自动化验证

- Rust 测试：38 通过，0 失败
- `cargo clippy --all-targets -- -D warnings`：PASS
- Day 1 协议向量验证：PASS
- 真实主网 Claim：PASS
- 真实主网 Refund：PASS
- Funding 金额守恒：PASS
- 重组回滚演练：未执行
- 连续 20 次 Claim：未执行
- 连续 20 次 Refund：未执行
- 主网独立 fee coin 选择：未执行；本次两笔 bundle 均使用 0 fee

## 六、问题与限制

本次确认轮询发现 Chia 2.7.3 的 RPC 返回兼容问题：当排除已花费 coin 时，部分 coin record 的 `spent_block_index` 返回 `null`，而当前固定版本 SDK 结构要求整数。

最终确认高度和 children 已通过 Chia 原生 RPC 直接核验。代码中的请求路径已调整为请求完整 coin record 后本地过滤；但针对本节点完整钱包地址的批量查询仍需进一步验证返回格式。

本报告未读取、打印或保存 seed phrase/private key。

## 七、结论

10 mojo funding coin 的真实主网 Claim 和 Refund 闭环均已成功：

- Claim：10 mojo -> Merchant 1 mojo + User 9 mojo
- Refund：10 mojo -> User 10 mojo

本次结果证明协议的金额约束、Claim 输出结构、Refund 输出结构、主网签名验证和链上确认行为均可工作，但尚未满足原阶段 A 的 20 次连续测试及重组退出标准。
