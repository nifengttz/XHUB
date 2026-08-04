# 第 7 天演示与最终报告

日期：2026-08-03

最终结论：**PASS**

## 一键复现

在仓库根目录运行：

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File .\scripts\demo-day7.ps1
```

脚本从干净 simulator 状态开始，依次完成 CLVM 编译、协议向量验证、两条业务演示、全量攻击与回归测试，以及严格 Clippy 检查。任一步失败都会停止并返回非零退出码。

## 场景 A：User/Hub 离线后 Merchant 独立索赔

演示生成三方密钥、10 mojo funding coin、Invoice、Intent 和双签 Voucher。Voucher 验证通过并持久化后释放 User 与 Hub 对象，Merchant 只使用 funding coin、公开通道参数和 Voucher 在高度 25 构造及提交 SpendBundle。

可复现证据：

```text
funding_coin_id=9946de8b9287ac681a4e0cdfe0e82a46b4d6c21f6ec99d9ed7f6002660a8a55d
commitment_hash=25d4feffa91f406bdde386552d0dac08d40b97a11929dc3a13290fe46bba9afb
voucher_signature=VERIFIED
user_service=OFFLINE hub_service=OFFLINE
spend_bundle_id=68a7663d0907221b7ff505b5192f13172981c3fbd9f57bb53fe09e2e7855e8bb
merchant_output_mojos=1
user_output_mojos=9
final_state=Settled
```

结果：Merchant 得到 1 mojo，User 得到 9 mojos，总额严格等于 funding 10 mojos。

## 场景 B：无 Voucher 退款

第二个干净 simulator 创建独立 funding coin，不签发 Voucher。到高度 26 后 User 提交 Refund。

可复现证据：

```text
funding_coin_id=a7dcad9d0b607951dc9c92b52b22d5a81c57957c6ff4e47431fb28e083b54287
voucher=NONE
spend_bundle_id=413fb0e631058436a02b22060ea2988af376f6ac7d933839d141da0cd393df65
user_refund_mojos=10
final_state=Refunded
```

结果：User 取回完整 10 mojos。

## 七日验收总览

| 天数 | 交付结果 | 结论 |
|---:|---|---|
| 1 | 协议、编码、签名承诺和测试向量冻结 | PASS |
| 2 | CLVM Claim/Refund 分支与 simulator 验证 | PASS |
| 3 | Invoice、Intent、Voucher 双签生命周期 | PASS |
| 4 | SQLite 原子状态机、去重与重启恢复 | PASS |
| 5 | Claim/Refund/fee coin 集成与链上确认跟踪 | PASS |
| 6 | 篡改、重放、竞争、恢复和边界攻击矩阵 | PASS |
| 7 | 干净环境一键演示和最终判定 | PASS |

最终自动化结果：

```text
Protocol vectors: PASS
Scenario A: PASS
Scenario B: PASS
34 tests passed; 0 failed
Clippy -D warnings: PASS
WALL-HUB MVP FINAL RESULT: PASS
```

## 结论边界

本次 PASS 证明的是：单笔、单商户、限时 Voucher 可以在 User 和 Hub 离线后，由 Merchant 从预锁定资金中非托管兑现；无 Voucher 时 User 可按时退款。

它不证明主网生产可用，也不覆盖多次状态更新、多商户聚合、watcher、主网手续费策略、容量、隐私、运维或生产安全审计。真实网络部署前必须重新核对共识高度边界并完成独立安全审计。
