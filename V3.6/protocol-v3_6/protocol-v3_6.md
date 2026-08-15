# X-Hub V3.6 协议书

> 状态：设计草案，尚未用于主网。  
> 目的：定义“用户逐条授权 + HUB 单一有状态协调签名 + 开放瞭望塔保存与挑战 + 任意第三方关闭和广播”的非托管小额支付协议。  
> V3.6 与 V3.5 及更早版本的 Funding Coin、Closing Coin、Puzzle、哈希域和签名域不兼容，不得混用。
> 本修订允许用户在创建 Funding Coin 时选择 `acceptance_blocks`、`freeze_blocks` 和 `challenge_blocks`；三个值属于该 Funding Coin 的不可变通道条款。

## 1. V3.6 设计决定

1. 用户必须逐条签署每笔商户付款授权。
2. HUB 使用受限的 `hub_state_public_key_a` 签署正式累计账本状态。
3. HUB A 只负责状态验证、接受和排序，其签名不能代替用户付款签名。
4. HUB 瞭望塔不参与共识签名，只负责验证、保存、监视、传播、挑战和恢复。
5. 正式账本是 append-only 累计账本。更高状态不得删除、修改或替换旧状态中的正式记录。
6. Funding Coin 出生后经过创建时承诺的 `acceptance_blocks` 个高度的预扣接受期，再经过 `freeze_blocks` 个高度的冻结期；界面默认值分别为 `12288` 和 `200`。
7. Funding Coin 出生至少 `close_delay_blocks = acceptance_blocks + freeze_blocks` 个高度后才允许发起关闭；使用默认值时为 `12488`。
8. 第一枚 Closing Coin 出生后经过该 Funding Coin 创建时承诺的固定 `challenge_blocks` 个高度的挑战期；界面默认值为 `6000`。
9. 挑战生成的新 Closing Coin 必须继承第一枚 Closing Coin 的原始挑战截止高度，不得重新计时。
10. START_CLOSE 和 CHALLENGE 必须提交可供链上完整验证的候选账本数据；仅有 HUB checkpoint 签名不足以替换当前关闭状态。
11. State 0 使用专门的空状态验证分支，不要求 HUB 签名或用户授权签名。
12. 不设置独立退款高度；用户全额退出是 State 0 未被更高正式状态挑战后的结算结果。
13. 所有链上手续费由 Funding Coin 之外的 fee Coin 提供。
14. 正式预扣不可撤销；V3.6 不定义删除、替换、取消或回滚正式 LedgerEntry 的分支。
15. 每条 LedgerEntry 在 FINALIZE 时创建一枚独立 Merchant Payment Coin；相同商户的多条记录不得合并输出。每枚 Payment Coin 的 puzzle hash 必须绑定 `funding_coin_id`、`entry_index` 和 `reservation_nonce`，确保同一商户、相同金额的多笔记录仍产生不同 Coin ID。
16. START_CLOSE、CHALLENGE 和 FINALIZE 均优先采用完整账本验证；未经后续版本明确升级，不得用仅验证 checkpoint 的优化替代。
17. HUB A 签名只产生 `SIGNED` 状态；只有完整 RecoveryPackage 被指定接收方验证并返回确认后，付款承诺才进入 `DELIVERED`，商户才可依赖。
18. `challenge_blocks = 6000` 是界面默认值，不是已经证明安全的主网下限；主网允许范围和最小值必须通过测试和安全评审后冻结。
19. 瞭望塔可以共用一台 VPS 和一个公网 IP，但同一宿主机、同一运营者或同一上游网络中的多个实例只计算为一个故障域，不得冒充多个独立副本。
20. 当 HUB 观测到规范链高度 `height >= A` 时，必须拒绝新预扣，且向用户和商户明确返回“Funding Coin 已进入冻结期，预扣失败，未写入正式账本”；该请求不得取得 `entry_index`、不得增加 `state_sequence`，也不得产生 HUB A 签名。

## 2. 安全目标与明确限制

### 2.1 安全目标

- 未经用户签名的付款不能进入最终输出；
- 商户地址、金额和 nonce 不能被 HUB 或瞭望塔修改；
- 用户找零只能发送到 Funding Coin 创建时固定的地址；
- 已进入正式状态的账目不能被后续更高状态删除；
- HUB 消失后，已签署并已传播完整恢复包的状态仍可结算；
- 任意人均可发起关闭、提交更高状态挑战和完成最终结算；
- 不持有完整有效账本数据的人不能仅凭高序号 checkpoint 锁死 Closing Coin；
- 没有正式预扣时，State 0 经过相同挑战流程后将全部资金返回用户。

### 2.2 不保证事项

V3.6 不保证：

- HUB A 私钥泄露后不会产生冲突状态；
- 同一 Funding Coin 能同时赔付 HUB 双签产生的所有超额冲突分支；
- 尚未取得正式签名的 PENDING 请求一定成功；
- 未传播的恢复包能够在 HUB 消失后恢复；
- 所有持有最新状态的参与者离线时仍能及时挑战；
- 链上拥堵或缺少 fee sponsor 时仍能及时广播。

## 3. 角色与密钥

### 3.1 用户

用户持有 `user_private_key`，逐条签署付款授权。Funding Coin 固定：

```text
user_public_key
user_remainder_puzzle_hash
funding_amount
```

### 3.2 HUB A 状态签名器

HUB A 必须：

- 验证 Funding Coin 条款和用户签名；
- 验证账本排序、nonce 唯一性、金额和容量；
- 验证新状态完整包含上一状态的全部记录；
- 串行分配 `state_sequence`；
- 在持久化签名意图后才返回签名；
- 在接受截止后拒绝新的预扣。

### 3.3 瞭望塔和广播者

HUB、商户、钱包和第三方均可运行瞭望塔。挑战者不需要登记身份。挑战权限来自其提交的完整有效状态，而不是挑战者公钥。

## 4. 协议常量、通道参数、类型与域

```text
protocol_version          = u16_be(0x0360)
default_acceptance_blocks = 12288
default_freeze_blocks     = 200
default_challenge_blocks  = 6000  # candidate_mainnet_default
max_ledger_entries        = 64
```

创建 Funding Coin 时，XHUB 界面必须让用户输入或确认：

```text
acceptance_blocks         # 默认 12288
freeze_blocks             # 默认 200
challenge_blocks          # 默认 6000
```

`close_delay_blocks` 不得作为独立可编辑参数。钱包必须以 checked arithmetic 自动计算并只读展示：

```text
close_delay_blocks = acceptance_blocks + freeze_blocks
```

创建界面必须在用户最终确认前显示这四个值和由其生成的 `channel_terms_hash`。钱包、HUB 和 Funding Puzzle 必须分别重新验证参数，不得信任其他参与者已经执行的界面校验。

必须满足：

```text
1 <= acceptance_blocks <= 2^63-1
1 <= freeze_blocks <= 2^63-1
1 <= challenge_blocks <= 2^63-1
close_delay_blocks == acceptance_blocks + freeze_blocks
close_delay_blocks <= 2^63-1
```

测试网和主网可以使用不同的安全参数配置文件限制输入范围。配置文件不替代 Funding Coin 中承诺的实际值；进入共识验证和哈希的是用户确认后的实际值。主网最小值和允许范围尚未冻结，在完成第 19、20 节的评审前不得用于主网。

Funding Coin 创建后，`acceptance_blocks`、`freeze_blocks`、`close_delay_blocks` 和 `challenge_blocks` 均不可修改。任何修改必须创建具有新 `channel_terms_hash` 的新 Funding Coin。

共识整数采用无符号 `u64_be`，允许范围为 `0..=2^63-1`，所有加法和求和必须检查溢出。Puzzle Hash、Coin ID 和普通哈希为 32 字节；BLS 公钥为 48 字节；BLS 签名为 96 字节；nonce 为 32 字节。

V3.6 的哈希函数、BLS 签名方案、数组编码、`network_id` 编码、非法曲线点检查和测试向量由同目录的 [实现规范附录](IMPLEMENTATION-SPEC.md) 冻结。CLVM 模块哈希和主网参数范围仍未冻结，因此当前版本不得用于主网。

域分离字符串为 ASCII 字节，不带结尾零字节：

```text
XHUB_CHANNEL_TERMS_V3_6
XHUB_USER_AUTH_V3_6
XHUB_LEDGER_ENTRY_V3_6
XHUB_LEDGER_LEAF_V3_6
XHUB_LEDGER_NODE_V3_6
XHUB_LEDGER_EMPTY_V3_6
XHUB_LEDGER_CHECKPOINT_V3_6
XHUB_HUB_STATE_V3_6
XHUB_STATE_ZERO_V3_6
XHUB_MERCHANT_PAYMENT_V3_6
XHUB_SEAL_V3_6
XHUB_RECOVERY_PACKAGE_V3_6
XHUB_DELIVERY_CONFIRMATION_V3_6
XHUB_DOUBLE_SIGN_EVIDENCE_V3_6
XHUB_RESERVATION_RESULT_V3_6
XHUB_CONFLICTING_RESULT_EVIDENCE_V3_6
```

## 5. 高度模型

### 5.1 术语

```text
F  = funding_birth_height
A  = acceptance_cutoff_height = F + acceptance_blocks
S  = scheduled_close_height   = F + close_delay_blocks
C0 = initial_closing_birth_height
D  = challenge_deadline_height = C0 + challenge_blocks
Cn = current_closing_birth_height
```

`F` 和 `C0` 是 Coin 在链上的实际出生高度，不是钱包构造交易时预填的预计确认高度。

### 5.2 Funding 阶段

```text
OPEN:       F <= height < A
FREEZING:   A <= height < S
CLOSEABLE:  height >= S
```

Funding Puzzle 不要求创建交易预先知道 `F`。START_CLOSE 必须返回：

```text
ASSERT_HEIGHT_RELATIVE(close_delay_blocks)
```

钱包和 HUB 可在 Funding Coin 确认后从完整节点取得 `F`，并计算 `A` 和 `S` 用于业务控制。链上最早关闭时间由相对高度条件强制执行。

### 5.2.1 接受截止与失败反馈

HUB 必须以已同步可信全节点返回的规范链峰值高度执行接受判断：

```text
height < A:
  可以进入预扣验证和签名流程

height >= A:
  拒绝预扣
  不分配 entry_index
  不修改累计账本
  不增加 state_sequence
  不生成 hub_state_signature
```

HUB 向用户钱包和商户返回的失败结果至少必须包含：

```text
ReservationResult {
  status = REJECTED_FREEZING,
  funding_coin_id,
  observed_peak_height,
  acceptance_cutoff_height = A,
  scheduled_close_height = S,
  ledger_written = false,
  hub_state_signature = null,
  message = "Funding Coin 已进入冻结期，预扣失败，未写入正式账本"
}
```

用户钱包和商户必须以 HUB 返回的明确结果判断本次业务请求是否成功；不得把超时、断线、排队中或缺少响应解释为成功或失败，此类结果统一为 `UNKNOWN`，必须使用原 `reservation_nonce` 查询。只有收到包含自身授权的有效 OfficialState，且完成第 12.1 节的送达确认后，商户才能显示 `DELIVERED`。

HUB 必须在正式提交签名意图前重新读取并持久化本次判断使用的 `observed_peak_height`。如果全节点未同步、RPC 不可用或无法确认规范链峰值，HUB 必须暂停接受新预扣并返回不可用错误，不得降级为按本地时间接受。

### 5.3 固定挑战截止高度

第一枚 Closing Coin 创建时尚不知道自己的实际出生高度。因此其 Puzzle 不预先承诺一个由发起者声称的 `C0`。

第一枚 Closing Coin 被花费时，solution 提交候选 `C0`，Puzzle 必须返回：

```text
ASSERT_MY_BIRTH_HEIGHT(C0)
```

并以检查溢出的方式计算：

```text
D = C0 + challenge_blocks
```

第一枚 Closing Coin 的 CHALLENGE 分支必须同时返回：

```text
ASSERT_MY_BIRTH_HEIGHT(C0)
ASSERT_BEFORE_HEIGHT_ABSOLUTE(D)
```

并创建一个 puzzle hash 已承诺固定 `D` 的后继 Closing Coin。

第一枚 Closing Coin 的 FINALIZE 分支必须返回：

```text
ASSERT_MY_BIRTH_HEIGHT(C0)
ASSERT_HEIGHT_ABSOLUTE(D)
```

后继 Closing Coin 的 CHALLENGE 和 FINALIZE 分支直接使用其已承诺的 `D`：

```text
CHALLENGE: ASSERT_BEFORE_HEIGHT_ABSOLUTE(D)
FINALIZE:  ASSERT_HEIGHT_ABSOLUTE(D)
```

任何后继 Closing Coin 均不得使用 `Cn + challenge_blocks` 重新计算截止高度。

## 6. Funding Coin 条款

Funding Coin 固定：

```text
network_id
protocol_version
acceptance_blocks
freeze_blocks
close_delay_blocks
challenge_blocks
user_public_key
hub_state_public_key_a
state_rules_hash
funding_amount
user_remainder_puzzle_hash
max_ledger_entries
```

注意：`funding_birth_height`、`acceptance_cutoff_height` 和 `scheduled_close_height` 不作为创建时预知字段写入条款；它们在 Coin 确认后由实际出生高度派生。

```text
channel_terms_hash = H(
  "XHUB_CHANNEL_TERMS_V3_6",
  protocol_version,
  network_id,
  acceptance_blocks,
  freeze_blocks,
  close_delay_blocks,
  challenge_blocks,
  user_public_key,
  hub_state_public_key_a,
  state_rules_hash,
  funding_amount,
  user_remainder_puzzle_hash,
  max_ledger_entries
)
```

Funding Puzzle 必须自行验证协议常量、用户确认的通道参数及其加法关系。

## 7. State 0

```text
StateZero {
  state_sequence = 0,
  manifest_root = H("XHUB_LEDGER_EMPTY_V3_6"),
  entry_count = 0,
  reserved_total = 0,
  user_remainder = funding_amount
}
```

```text
state_zero_hash = H(
  "XHUB_STATE_ZERO_V3_6",
  protocol_version,
  network_id,
  funding_coin_id,
  channel_terms_hash,
  funding_amount,
  user_remainder_puzzle_hash
)
```

State 0 不需要 HUB A 签名。State 1 的 `previous_checkpoint_hash` 必须等于 `state_zero_hash`。

State 0 的 FINALIZE 分支必须验证空根、零记录、零预扣和全额找零，不得要求 `hub_state_signature`、用户授权签名或非空 RecoveryPackage。

## 8. 用户授权

```text
LedgerEntry {
  merchant_puzzle_hash,       # bytes32
  merchant_receipt_public_key, # BLS public key, 48 bytes
  amount,                      # 1..=2^63-1
  reservation_nonce           # bytes32
}
```

```text
authorization_hash = H(
  "XHUB_USER_AUTH_V3_6",
  protocol_version,
  network_id,
  funding_coin_id,
  channel_terms_hash,
  merchant_puzzle_hash,
  merchant_receipt_public_key,
  amount,
  reservation_nonce
)
```

```text
user_authorization_signature = Sign(
  user_private_key,
  authorization_hash
)
```

金额必须大于零。相同 Funding Coin 中 `reservation_nonce` 必须全局唯一。`merchant_receipt_public_key` 由商户在开具付款请求时提供，并被用户授权签名绑定，用于确认完整 RecoveryPackage 已送达。用户签名只是授权；记录进入 HUB A 已签署的 checkpoint 后成为不可撤销的 `SIGNED` 预扣，但商户只有在完成第 12 节的送达确认后才能把它视为可依赖的 `DELIVERED` 付款承诺。

## 9. Append-only 累计账本

记录按 HUB 正式接受顺序追加。每条记录取得不可变的：

```text
entry_index = 0..entry_count-1
```

新状态必须满足：

```text
new.entry_count >= old.entry_count
new.entries[0 : old.entry_count] == old.entries
new.reserved_total >= old.reserved_total
```

已存在记录的商户、回执确认公钥、金额、nonce、授权哈希和用户签名不得修改、删除、重排或替换。V3.6 不支持取消正式预扣。退款只能由商户在收到结算资金后通过协议外的新交易处理。

### 9.1 每条记录唯一的 Merchant Payment Coin

`merchant_puzzle_hash` 是商户最终标准收款地址，但 FINALIZE 不直接向该 puzzle hash 创建每条记录的 Coin。每条记录必须确定性派生唯一的中间支付 puzzle hash：

```text
merchant_payment_puzzle_hash_i = curry_hash(
  MERCHANT_PAYMENT_MOD_HASH_V3_6,
  protocol_version,
  network_id,
  funding_coin_id,
  channel_terms_hash,
  entry_index_i,
  reservation_nonce_i,
  merchant_puzzle_hash_i
)
```

其中 `MERCHANT_PAYMENT_MOD_HASH_V3_6` 必须在主网上线前冻结。`entry_index_i` 和 `reservation_nonce_i` 同时参与派生，任何实现均不得只使用商户地址和金额计算 Payment Puzzle。

Merchant Payment Puzzle 是无托管转发 Puzzle。任何人均可触发其花费，但它必须验证自己的金额，并把全部金额原样创建到记录固定的商户地址：

```text
ASSERT_MY_AMOUNT(payment_coin_amount)
CREATE_COIN(merchant_puzzle_hash_i, payment_coin_amount)
```

Merchant Payment Puzzle 不得提供替换目标地址、扣除内置手续费、减少转发金额或找零到其他地址的分支。所需手续费必须由同一 Spend Bundle 中的外部 fee Coin 提供。

因此，同一商户的十条记录由一次 FINALIZE 创建十枚不同的 Merchant Payment Coin。商户或任意 keeper 随后可以在一个 Spend Bundle 中批量花费这十枚 Coin，将它们分别转发到商户的标准 `merchant_puzzle_hash`。这里的独立结算是十枚独立 Coin，而不是十次独立 FINALIZE。

```text
entry_hash = H(
  "XHUB_LEDGER_ENTRY_V3_6",
  entry_index,
  merchant_puzzle_hash,
  merchant_receipt_public_key,
  amount,
  reservation_nonce,
  authorization_hash
)

leaf_hash = H("XHUB_LEDGER_LEAF_V3_6", entry_hash)
node_hash = H("XHUB_LEDGER_NODE_V3_6", left, right)
empty_root = H("XHUB_LEDGER_EMPTY_V3_6")
```

Merkle Tree 的奇数节点处理、padding、proof 方向位和规范编码必须在实现规范中冻结。所有实现必须使用同一算法和测试向量。

```text
reserved_total = checked_sum(entries.amount)
user_remainder = funding_amount - reserved_total
```

必须满足：

```text
0 <= entry_count <= max_ledger_entries
0 <= reserved_total <= funding_amount
reserved_total + user_remainder == funding_amount
```

## 10. HUB 正式状态

```text
LedgerCheckpoint {
  funding_coin_id,
  channel_terms_hash,
  state_sequence,
  previous_checkpoint_hash,
  manifest_root,
  entry_count,
  reserved_total,
  user_remainder
}
```

```text
checkpoint_hash = H(
  "XHUB_LEDGER_CHECKPOINT_V3_6",
  protocol_version,
  network_id,
  funding_coin_id,
  channel_terms_hash,
  state_sequence,
  previous_checkpoint_hash,
  manifest_root,
  entry_count,
  reserved_total,
  user_remainder
)

hub_state_hash = H(
  "XHUB_HUB_STATE_V3_6",
  checkpoint_hash
)

hub_state_signature = Sign(
  hub_state_private_key_a,
  hub_state_hash
)
```

```text
OfficialState {
  checkpoint,
  hub_state_signature
}
```

HUB A 签名前必须验证完整新状态、全部用户签名、append-only 关系、金额守恒、nonce 唯一性及前序状态。不得接受裸哈希盲签。

## 11. 有状态签名器与双签防护

签名器保存：

```text
(funding_coin_id, latest_sequence, latest_checkpoint_hash)
```

只允许原子转换：

```text
new_sequence == latest_sequence + 1
new_previous_checkpoint_hash == latest_checkpoint_hash
```

第一份正式状态必须满足：

```text
new_sequence == 1
new_previous_checkpoint_hash == state_zero_hash
```

签名器必须先验证、写入不可回滚日志并确认 WAL 落盘，再生成、保存和返回签名。相同请求重试返回原签名；同序号不同 checkpoint 永久拒绝并报警。

初版只允许一个活动 HUB A 签名器。冷备不得自动接管。瞭望塔维护：

```text
(funding_coin_id, state_sequence) -> checkpoint_hash
```

同 Coin、同序号、不同 checkpoint 且两份签名有效时生成公开可验证的 `DoubleSignEvidence`。

## 12. 正式回执和恢复包

```text
MerchantReservationReceipt {
  funding_coin_id,
  entry_index,
  ledger_entry,
  authorization_hash,
  user_authorization_signature,
  official_state,
  inclusion_proof,
  recovery_package_content_hash,
  recovery_locations[]
}
```

```text
RecoveryPackage {
  funding_coin_id,
  funding_puzzle_reveal,
  funding_amount,
  channel_terms,
  official_state,
  entries[],
  user_authorization_signatures[]
}
```

必须满足：

```text
len(entries) == checkpoint.entry_count
len(user_authorization_signatures) == checkpoint.entry_count
recompute_manifest_root(entries) == checkpoint.manifest_root
checked_sum(entries.amount) == checkpoint.reserved_total
funding_amount - checkpoint.reserved_total == checkpoint.user_remainder
```

```text
recovery_package_content_hash = H(
  "XHUB_RECOVERY_PACKAGE_V3_6",
  canonical_encode(RecoveryPackage)
)
```

每次产生正式状态后，HUB 必须把完整恢复包发送给相关商户、HUB 瞭望塔、商户指定瞭望塔及至少一个 HUB 之外的存储点。只保存 Root 或 checkpoint 不足以保证可结算性。


### 12.1 送达状态与确认

状态必须明确区分：

```text
AUTHORIZED:
  用户已签名，但 HUB A 尚未接受

SIGNED:
  记录已进入 HUB A 签署的 OfficialState
  该记录不可撤销
  但尚不能证明商户取得了完整结算数据

DELIVERED:
  商户已验证完整 RecoveryPackage
  并使用 merchant_receipt_private_key 返回送达确认
```

```text
DeliveryConfirmation {
  protocol_version,
  network_id,
  funding_coin_id,
  channel_terms_hash,
  state_sequence,
  checkpoint_hash,
  entry_index,
  authorization_hash,
  recovery_package_content_hash
}
```

```text
delivery_confirmation_hash = H(
  "XHUB_DELIVERY_CONFIRMATION_V3_6",
  protocol_version,
  network_id,
  funding_coin_id,
  channel_terms_hash,
  state_sequence,
  checkpoint_hash,
  entry_index,
  authorization_hash,
  recovery_package_content_hash
)

delivery_confirmation_signature = Sign(
  merchant_receipt_private_key,
  delivery_confirmation_hash
)
```

商户或代管其回执确认密钥的指定瞭望塔只有在完成以下验证后才能签署 DeliveryConfirmation：

- RecoveryPackage 内容哈希匹配；
- HUB A 状态签名有效；
- 用户授权签名有效；
- 自己的 LedgerEntry、entry index 和 Merkle proof 有效；
- 完整账本可重新计算出 checkpoint 中的 Root、总额和找零；
- 本地或指定外部存储已经持久化完整 RecoveryPackage。

HUB 在收到有效 DeliveryConfirmation 前不得向用户报告“付款完成”。DeliveryConfirmation 是交付和责任证明，不参与 Funding Coin 或 Closing Coin 的花费条件；缺少 DeliveryConfirmation 不会使已经签署的 LedgerEntry 可撤销，也不会阻止其最终结算。

### 12.2 瞭望塔守则

本节是 V3.6 的最低运行与安全规范。瞭望塔不取得 HUB A 的签名权，也不因保存状态而获得修改账本、替换收款地址或支配 Funding Coin 的权限。

#### 12.2.1 部署与故障域

1. 同一 VPS 可以运行多个瞭望塔容器；各实例必须使用独立容器名、监听端口、配置目录、数据库和持久化卷。
2. 同一 VPS 上的三个实例在可用性评估中只算一个故障域。不同 Docker 端口、容器网络或进程身份不能消除宿主机宕机、磁盘损坏、账号失陷、封禁和断网造成的共同故障。
3. 对外宣称“三个独立瞭望塔”时，三个实例必须至少分布在两个独立运营者控制的故障域；生产建议分布在三个不同 VPS、不同管理凭据和至少两个上游网络中。
4. HUB 自营瞭望塔不得作为第 12 节要求的“至少一个 HUB 之外的存储点”。
5. 多实例可以共用一个公网 IP，但任何需要来源独立性的审计、限流或告警不得把不同端口误认为不同来源。
6. 测试阶段明确允许三个瞭望塔使用同一公网 IP，亦允许部署在同一 VPS；测试报告必须标注其为单一故障域，不得用该拓扑证明生产环境的容灾能力。

#### 12.2.2 接收、验证与持久化

瞭望塔收到 RecoveryPackage 后必须先执行第 12 节的全部完整验证，再接受该包。验证失败的包必须隔离并记录原因，不得进入“可挑战”集合。

对每个 `funding_coin_id`，瞭望塔必须持久化：

```text
latest_valid_sequence
latest_checkpoint_hash
recovery_package_content_hash
canonical RecoveryPackage
first_seen_time
last_verified_chain_height
storage_integrity_status
```

持久化必须满足：

- 先写临时对象，校验内容哈希，再以原子方式提交；
- 数据确认落盘后才允许返回存储或送达确认；
- 相同序号、相同 checkpoint 的重复包按幂等请求处理；
- 相同 Coin、相同序号、不同 checkpoint 的有效签名不得覆盖，必须同时保留并生成 `DoubleSignEvidence`；
- 更低序号状态不得覆盖或降级本地最新状态；
- 至少定期执行内容哈希复核和恢复演练，损坏副本不得计入可用副本数。

#### 12.2.3 链上监视与挑战

1. 瞭望塔必须监视 Funding Coin、初始 Closing Coin、全部后继 Closing Coin、确认高度和链重组，不得只监听 HUB 推送的事件。
2. 发现候选 Closing State 低于本地最新有效状态时，必须重新验证本地 RecoveryPackage，并在 `D` 前构造和广播 CHALLENGE。
3. 广播前必须检查当前 Coin 状态、挑战截止高度、Spend Bundle、外部 fee Coin 和预期新 Closing Coin；广播后必须跟踪进入内存池、确认、被替换和重组后的状态。
4. 单次广播失败不得视为任务完成；在截止高度前必须按退避和手续费策略重试，并向其他瞭望塔传播可验证的挑战材料。
5. 每个生产故障域必须具有独立的全节点视图或经过认证的节点连接、可用的 fee Coin/手续费预算，以及不依赖 HUB 在线的构造和广播能力。
6. 到达 `D` 后，瞭望塔可以协助 FINALIZE，但不得以 FINALIZE 取代截止高度前应执行的更高状态挑战。

#### 12.2.4 密钥与最小权限

- 普通瞭望塔不得保存 `hub_state_private_key_a`、用户私钥或商户收款私钥。
- 只有商户明确委托的瞭望塔才可代管 `merchant_receipt_private_key`；该密钥必须与监控服务密钥、主机登录凭据和 fee Coin 密钥隔离。
- Docker 实例必须使用最小权限运行。挂载 Docker socket 的管理型容器与保存 RecoveryPackage 的瞭望塔应隔离，因为 Docker socket 等价于宿主机高权限控制面。
- 管理接口默认不得直接暴露公网；确需开放时必须使用认证、加密传输、来源限制和速率限制。

#### 12.2.5 保留、审计与退出

1. RecoveryPackage 不得仅因 HUB 报告“已完成”、商户离线或发现更高状态而删除；至少保留到 Funding Coin 已最终结算、相关 Merchant Payment Coin 可验证地产生，并经过实现规范规定的重组安全余量。
2. 运行日志不得记录私钥、认证令牌或完整敏感凭据，但必须足以审计接收、验证、持久化、冲突检测、挑战构造、广播和确认过程。
3. 停运前必须把仍承担监视责任的完整包安全移交给另一独立故障域并验证接收；未完成移交不得宣称监视责任已经解除。
4. 瞭望塔不得签署其未完整验证、未成功持久化或无法在本地恢复的 DeliveryConfirmation。

### 12.3 预扣处理、结果确认与冻结期规范

本节规定业务层预扣的确定性处理。链上有效性仍由 Funding Puzzle、Closing Puzzle、用户签名和 HUB A 状态签名决定；业务响应不得替代完整 OfficialState 或 RecoveryPackage。

#### 12.3.1 确定结果、未知结果与状态查询

预扣请求只有三类顶层结果：

```text
SUCCESS:   已取得可验证的 SIGNED 或 DELIVERED 结果
REJECTED:  已取得 HUB 签名的确定性拒绝，保证未写入账本
UNKNOWN:   超时、断线、无响应或处理结果不确定
```

`UNKNOWN` 既不等于成功，也不等于失败。钱包不得立即更换 nonce 重付，必须使用原请求查询：

```text
GET_RESERVATION_STATUS(funding_coin_id, reservation_nonce)
```

查询返回 `PENDING`、`SIGNED`、`DELIVERED`、确定性拒绝码或 `UNKNOWN`。HUB 必须允许用户和商户取得同一个持久化结果。

#### 12.3.2 nonce 幂等性

唯一幂等键为：

```text
reservation_key = (funding_coin_id, reservation_nonce)
```

- 同一键且授权内容完全相同的重试必须返回原结果，不得新增记录或重新扣款；
- 同一键但商户、回执公钥、金额、授权哈希或签名不同，必须返回 `NONCE_CONFLICT`；
- HUB 必须在分配 `entry_index` 前以原子唯一约束持久化该键；
- 客户端在原请求为 `UNKNOWN` 时不得使用新 nonce 表示同一笔业务付款。

#### 12.3.3 A 高度的提交边界

HUB 的处理顺序必须是：验证请求、锁定 Funding Coin 账本、重新读取可信全节点的规范链峰值、持久化判断高度，然后才允许提交签名意图。

```text
commit_height < A:
  原子写入幂等键、WAL、LedgerEntry 和新 checkpoint
  生成并保存 HUB A 签名

commit_height >= A:
  返回 REJECTED_FREEZING
  不分配 entry_index
  不增加 state_sequence
  不修改 reserved_total
  不生成 HUB A 签名
```

`commit_height` 是正式提交前最后一次确认并与签名意图一起持久化的规范链峰值高度，而不是请求到达时间或本地墙钟时间。

#### 12.3.4 A 前签名、A 后送达

HUB 在 `commit_height < A` 时已经持久化并签署的 OfficialState，可以在 `A <= height < S` 期间继续重发、传播、验证和取得 DeliveryConfirmation。瞭望塔必须验证原 HUB 签名、完整 RecoveryPackage、相同幂等键和未发生账本变更；不得把补传解释为允许 A 后新增预扣。

到达 `height >= S` 后，瞭望塔不再首次签发新的 DeliveryConfirmation 或业务绿灯，但必须继续保存已有状态、监视关闭、传播更高状态、执行 CHALLENGE 并协助 FINALIZE。缺少绿灯不撤销已经有效签署的 LedgerEntry。

#### 12.3.5 冻结期允许与禁止事项

在 `A <= height < S` 期间，允许查询原请求、返回原结果、重发已有 OfficialState、传播和修复已有 RecoveryPackage、完成送达确认、对账及生成 SealStatement。

冻结期禁止接受新授权、增加或修改 LedgerEntry、增加 `reserved_total`、分配新 `entry_index`、增加账本 `state_sequence`，或对不同内容生成新的 OfficialState。内容完全相同的重试只能返回已经持久化的原状态和原签名。

#### 12.3.6 S 高度的最终候选状态

关闭使用的最终候选账本是 S 前已经由 HUB A 有效签署、且具有完整 RecoveryPackage 的最高序号 OfficialState。`SIGNED` 记录即使尚未取得 `DELIVERED` 绿灯仍不可撤销，并可进入最终结算。

SealStatement 仅用于冻结期对账，不得替代 OfficialState 或 RecoveryPackage，不得删除 SIGNED 记录，也不得阻止任何人用更高的有效完整状态挑战。

#### 12.3.7 统一签名结果与冲突证据

```text
ReservationResult {
  protocol_version,
  network_id,
  request_id,
  funding_coin_id,
  reservation_nonce,
  authorization_hash,
  status,
  state_sequence,              # 拒绝时为 null
  checkpoint_hash,             # 拒绝时为 null
  observed_peak_height,
  acceptance_cutoff_height,
  scheduled_close_height,
  ledger_written
}
```

```text
reservation_result_hash = H(
  "XHUB_RESERVATION_RESULT_V3_6",
  canonical_encode(ReservationResult)
)

hub_result_signature = Sign(
  hub_state_private_key_a,
  reservation_result_hash
)
```

HUB 必须把同一份已签名结果提供给用户和商户。相同 `(funding_coin_id, reservation_nonce)` 存在两份字段冲突且签名有效的结果时，两份结果构成 `ConflictingResultEvidence`；其证据哈希使用 `XHUB_CONFLICTING_RESULT_EVIDENCE_V3_6` 域。返回确定性拒绝后又把同一授权写入账本，或返回成功但不存在对应 OfficialState，均属于 HUB 协议违规。

#### 12.3.8 瞭望塔绿灯权限

瞭望塔绿灯只允许把业务状态从 `SIGNED` 推进为 `DELIVERED`，不参与 HUB 状态共识签名，不能修改、删除或撤销 LedgerEntry，也不能阻止有效 SIGNED 状态最终结算。

DeliveryConfirmation 必须绑定完全相同的 `funding_coin_id`、`state_sequence`、`checkpoint_hash` 和 `recovery_package_content_hash`。它由目标 LedgerEntry 唯一的 `merchant_receipt_private_key` 签署，只证明商户已确认收到完整恢复材料；同一公钥登记为多个 signer ID 或故障域不得增加确认数量。

生产运营门槛使用独立的链下 `CustodyAttestation`，其签名消息为：

```text
CustodyAttestation {
  protocol_version,
  funding_coin_id,
  state_sequence,
  checkpoint_hash,
  recovery_package_content_hash,
  entry_index,
  authorization_hash,
  delivery_confirmation_hash
}

custody_attestation_hash = H(
  "XHUB_WATCHTOWER_CUSTODY_ATTESTATION_V3_6",
  canonical_encode(CustodyAttestation)
)
```

托管证明只能由已经完整验证、持久化 RecoveryPackage 且验证商户 DeliveryConfirmation 的 Watchtower 使用独立运营密钥签署。它不进入 `ChannelTerms`，不参与 Funding Coin、Closing Coin 或 Merchant Payment Coin 的花费条件，也不改变任何已创建 Coin 的 puzzle hash。

#### 12.3.9 测试与生产绿灯门槛

测试阶段一份有效商户 DeliveryConfirmation 即可把业务状态推进为 `DELIVERED`。托管证明聚合必须测试 `1-of-3`、`2-of-3` 和 `3-of-3` 内容一致性。多个 Watchtower 允许共用同一公网 IP 或同一 VPS，但只算一个故障域；同一公钥无论使用多少身份都只算一个证明。

生产建议采用“一份有效商户 DeliveryConfirmation + 跨故障域 `2-of-3` CustodyAttestation”。商户回执密钥不得复制给多个 Watchtower 以模拟独立门槛。在主网参数、Watchtower 运营身份和故障域策略正式冻结前，生产门槛不得被写成已经获得共识安全证明。

资源受限的短期单 VPS 测试可以使用显式 `single-vps-test` 运营模式，仅按不同 Watchtower BLS 公钥验证 `2-of-3` CustodyAttestation，不要求不同故障域。该例外只影响链下测试绿灯，不修改 CustodyAttestation 签名消息、ChannelTerms、CLVM 或任何 Coin。测试响应必须同时声明 `failure_domain_enforced=false`、`test_only=true` 和 `production_ready=false`；不得写入或替代生产绿灯状态。

#### 12.3.10 RPC 与节点健康

出现全节点未同步、RPC 断开、peak 为空、网络 ID 不匹配、无法确认 Funding Coin，或主备节点返回超出实现阈值的冲突时，HUB 必须暂停新预扣，分别返回 `NODE_NOT_SYNCED`、`RPC_UNAVAILABLE` 或 `CHAIN_STATE_UNCERTAIN`，不得按本地时间继续接受。

生产 HUB 建议配置至少两个独立 RPC 来源。瞭望塔应具有不依赖 HUB 在线的链状态来源。

#### 12.3.11 Funding 确认与链重组

测试阶段默认在 Funding Coin 首次出现后等待 `funding_confirmation_blocks_test = 32` 个高度，再从 `UNCONFIRMED` 进入 `ACTIVE`。该参数是业务激活策略，不写入 Funding Puzzle 或 `channel_terms_hash`。

激活前发生重组时，重新取得 `F` 并计算 `A`、`S`。激活后 Funding Coin 消失时，HUB 必须暂停新预扣并进入 `CHANNEL_REORG_PENDING`；若 Coin 在新高度重新进入规范链，则业务接受截止采用：

```text
effective_A = min(old_A, new_A)
```

重组不得重新开放已经冻结的通道或延长预扣接受期。链上最早关闭仍由规范链上的相对高度条件决定。

#### 12.3.12 统一状态码

成功或处理中状态：

```text
SIGNED
DELIVERED
PENDING
UNKNOWN
```

确定性拒绝且保证未写账：

```text
REJECTED_FREEZING
REJECTED_CLOSEABLE
INVALID_AUTHORIZATION
INSUFFICIENT_REMAINDER
NONCE_CONFLICT
LEDGER_FULL
CHANNEL_CLOSING
CHANNEL_FINALIZED
```

节点或系统状态：

```text
NODE_NOT_SYNCED
RPC_UNAVAILABLE
CHAIN_STATE_UNCERTAIN
CHANNEL_REORG_PENDING
INTERNAL_ERROR
```

`UNKNOWN`、`RPC_UNAVAILABLE` 和 `INTERNAL_ERROR` 不得被解释为保证未写账，客户端必须按原 nonce 查询。`NODE_NOT_SYNCED`、`CHAIN_STATE_UNCERTAIN` 和 `CHANNEL_REORG_PENDING` 要求 HUB 暂停新预扣，直到链状态恢复确定。

## 13. 可选封账声明

FREEZING 阶段 HUB A 可以签署：

```text
SealStatement {
  funding_coin_id,
  final_sequence,
  final_checkpoint_hash,
  observed_acceptance_cutoff_height,
  observed_scheduled_close_height,
  seal_signature
}
```

封账声明只用于运营确认，不是 START_CLOSE、CHALLENGE 或 FINALIZE 的必要条件，也不能替代完整 RecoveryPackage。

## 14. 关闭状态机

```text
OPEN -> FREEZING -> CLOSEABLE -> CLOSING -> FINALIZED
```

### 14.1 START_CLOSE

任何人均可提交 State 0 或非空正式状态发起关闭。Funding Coin 必须返回：

```text
ASSERT_MY_COIN_ID(funding_coin_id)
ASSERT_MY_AMOUNT(funding_amount)
ASSERT_HEIGHT_RELATIVE(close_delay_blocks)
CREATE_COIN(initial_closing_puzzle_hash, funding_amount)
```

候选为 State 0 时，执行第 7 节的空状态验证。

候选为非空状态时，START_CLOSE 必须验证：

- HUB A 签名；
- 完整 RecoveryPackage；
- 全部用户签名；
- 记录格式、顺序、nonce 唯一性和数量；
- Merkle Root、总额、找零和金额守恒。

初始 Closing State 承诺：

```text
ClosingState {
  funding_coin_id,
  channel_terms_hash,
  proposed_sequence,
  proposed_checkpoint_hash,
  proposed_manifest_root,
  proposed_entry_count,
  proposed_reserved_total,
  proposed_user_remainder,
  challenge_deadline_height_mode = DERIVE_FROM_OWN_BIRTH
}
```

### 14.2 CHALLENGE

挑战状态必须满足：

```text
new_state_sequence > current_state_sequence
```

挑战者必须提交更高 OfficialState 及其完整 RecoveryPackage。Closing Puzzle 必须完整验证账本，不能只验证 HUB 签名和序号。

第一枚 Closing Coin 挑战时从自身真实出生高度确定 `D`，并创建一个明确承诺 `D` 的后继 Closing Coin。后继挑战必须原样复制 `D`。

```text
new_closing_coin.challenge_deadline_height
  == current_closing_coin.challenge_deadline_height
```

挑战不得延长截止高度。

### 14.3 FINALIZE

达到固定截止高度后，任何人均可结算。

State 0 分支：

```text
CREATE_COIN(user_remainder_puzzle_hash, funding_amount)
```

非空状态分支必须重新验证当前 Closing State 对应的完整 RecoveryPackage。一次 FINALIZE Spend 必须为每条记录分别创建一枚唯一的 Merchant Payment Coin；即使多条记录具有相同 `merchant_puzzle_hash` 和相同金额，也不得合并或直接创建重复的 `(parent_coin_id, puzzle_hash, amount)` 输出。对每条记录创建：

```text
CREATE_COIN(merchant_payment_puzzle_hash_i, amount_i)
```

找零大于零时创建：

```text
CREATE_COIN(user_remainder_puzzle_hash, user_remainder)
```

所有 Merchant Payment Coin 输出和用户找零之和必须严格等于 Closing Coin 金额。Merchant Payment Coin 的后续批量转发是独立 Spend Bundle，不属于 FINALIZE，也不改变相应 LedgerEntry 的结算金额。

## 15. 接受截止的共识边界

`ASSERT_HEIGHT_RELATIVE(close_delay_blocks)` 能在链上强制该 Funding Coin 的最早关闭高度，但“某条预扣是否在 `A` 之前被 HUB 接受”仍属于 HUB A 的有状态签名规则，链上无法仅从普通 checkpoint 证明签名发生的实际时间。

因此 V3.6 明确依赖以下安全假设：

> HUB A 签名器正确取得 Funding Coin 的实际出生高度，并在 `A = F + acceptance_blocks` 后拒绝把新授权加入正式状态。

在业务接口层，HUB 是用户钱包和商户判断“本次预扣是否被接受”的权威响应方。`REJECTED_FREEZING` 必须表示该请求没有写入账本且没有 HUB A 签名；任何已经返回成功但实际未进入对应 OfficialState 的行为，以及任何返回冻结失败后又把该请求写入账本的行为，均属于可审计的 HUB 协议违规。

封账声明和瞭望塔可以检测违规，但不能把 HUB 自报时间变成可信链上时间。若未来需要共识级接受截止证明，必须另行设计链上状态推进机制。

## 16. 费用

Funding Coin 和 Closing Coin 必须保持金额守恒。START_CLOSE、CHALLENGE 和 FINALIZE 所需手续费由同一 Spend Bundle 中的外部 fee Coin 提供。V3.6 不定义内置奖励或固定手续费。

## 17. 主要攻击与处理

### 17.1 伪造商户、金额或 nonce

由用户逐条签名和链上完整账本验证阻止。

### 17.2 更高状态删除旧商户

由 append-only 规则和 HUB A 相邻状态验证阻止。Closing Puzzle 验证候选账本本身，但跨状态单调性仍依赖有状态 HUB A 签名器。

### 17.3 仅凭高序号 checkpoint 锁死资金

START_CLOSE 和 CHALLENGE 均要求完整有效 RecoveryPackage，因此只有 checkpoint 而无结算数据的状态不能成为 Closing State。

### 17.4 提交旧状态关闭

任何持有更高 OfficialState 及完整 RecoveryPackage 的参与者可在固定截止高度前挑战。

### 17.5 连续挑战延长关闭

所有后继 Closing Coin 继承由第一枚 Closing Coin 出生高度确定的同一个 `D`，不得重新计时。

### 17.6 用户使用 State 0 全额退出

更高正式状态可以在挑战期内替换 State 0，因此不存在绕过正式商户预扣的退款旁路。

### 17.7 HUB 双签

由有状态签名器预防，由瞭望塔检测。Funding Coin 不提供超额赔付；抵押、保险和合同责任属于运营层。

## 18. 安全假设

V3.6 的安全性依赖：

1. 用户私钥和 BLS 签名安全；
2. HUB A 签名器不双签并严格执行 append-only 和接受截止规则；
3. 最新正式 RecoveryPackage 至少由一个 HUB 之外的诚实参与者保存；生产部署应存在跨故障域副本；
4. 至少一个持有更高完整状态的独立故障域在挑战期内在线、能看到已确认 Closing Coin、能独立构造并广播挑战且能支付手续费；
5. Chia 共识和高度断言条件按预期运行。

## 19. 参数冻结状态

`acceptance_blocks = 12288`、`freeze_blocks = 200` 和 `challenge_blocks = 6000` 是创建界面的默认值，不是强制所有 Funding Coin 使用的固定值。用户确认的实际值必须写入 `channel_terms_hash` 和 Puzzle；测试网 Funding Coin 一旦创建，该 Coin 的实际值不可修改。

主网参数配置文件的允许范围尚未冻结。冻结主网最小接受期、最小冻结期和最小挑战期前，至少评估区块高度对应的实际时间分布、长重组、网络拥堵、手续费市场、瞭望塔最长故障恢复时间及商户离线模型，并形成书面安全评审结论。仅因某个值写入 `channel_terms_hash`，不能证明该值具有足够的主网安全性。

## 20. 上线门槛

主网上线前至少完成：

1. 冻结 Funding、初始 Closing、后继 Closing、CHALLENGE、FINALIZE 和 Merchant Payment Puzzle 的 CLVM 模块哈希；
2. 验证 `ASSERT_HEIGHT_RELATIVE(close_delay_blocks)` 的参数化边界、默认值 `12488` 的测试向量和重组行为；
3. 验证初始 Closing Coin 的 `ASSERT_MY_BIRTH_HEIGHT(C0)`；
4. 验证 `D = C0 + challenge_blocks` 的开始、最后可挑战高度和最早可结算高度，并包含默认值 `6000` 的测试向量；
5. 验证多次挑战后 `D` 完全不变；
6. 验证 State 0 无 HUB 签名仍可结算且可被更高状态挑战；
7. 验证无 RecoveryPackage 的高序号 checkpoint 不能 START_CLOSE 或 CHALLENGE；
8. 验证后续状态删除、修改、重排旧记录时 HUB A 拒绝签名；
9. 验证 Merkle proof、空树、奇数叶子和 64 条记录；
10. 验证缺签名、错 Coin ID、错条款、错金额、错 nonce、错 Root 和错找零均失败；
11. 验证 WAL 回滚、冷备接管、脑裂和双签检测；
12. 验证 HUB 永久离线后第三方可以独立关闭、挑战和结算；
13. 测量 1、10、64 条记录在 START_CLOSE、CHALLENGE 和 FINALIZE 中的真实 CLVM cost；
14. 验证外部 fee Coin 与三类操作的 Spend Bundle 组合；
15. 验证同一商户存在多笔相同金额记录时，每个 `merchant_payment_puzzle_hash` 和最终 Coin ID 仍然唯一；
16. 验证 1、10、64 枚 Merchant Payment Coin 可以在单一 Spend Bundle 中批量原额转发到商户标准地址，且目标地址和金额不可替换。
17. 验证三个瞭望塔容器位于同一 VPS 时，宿主机断电、磁盘损坏和断网会被正确判定为单一故障域失效；
18. 验证 RecoveryPackage 截断、篡改、降序重放、同序号冲突和重复投递时的拒绝、幂等及双签证据行为；
19. 验证瞭望塔在 HUB 永久离线、一个故障域失效、节点重组和首次广播失败的组合场景下仍能在 `D` 前完成挑战；
20. 验证存储哈希巡检、备份恢复、fee Coin 耗尽告警、挑战重试和安全停运移交流程；
21. 验证未持久化完整 RecoveryPackage 时，瞭望塔不能签署 DeliveryConfirmation。
22. 验证高度恰好等于 `A`、高于 `A`、RPC 断开和节点未同步时，HUB 均不会分配 entry index、增加序号、修改账本或生成状态签名；
23. 验证 `REJECTED_FREEZING` 同时发送给用户和商户，且两方得到相同的 `funding_coin_id`、观测高度、`A`、`S` 和 `ledger_written = false`；
24. 验证测试环境的三个瞭望塔可以共用同一公网 IP 和不同端口，同时在拓扑及故障测试报告中只计为一个故障域。
25. 验证回复丢失时客户端进入 `UNKNOWN`，使用原 nonce 查询且不会重复预扣；
26. 验证相同 nonce 的相同请求幂等返回，相同 nonce 的不同内容返回 `NONCE_CONFLICT`；
27. 验证请求在 A 前到达但提交时已到 A 的竞争条件必定返回 `REJECTED_FREEZING`；
28. 验证 A 前签署的状态可在 A 至 S 之间完成送达，而 S 后不再首次签发绿灯；
29. 验证冻结期不能新增记录、序号或预扣金额，只能重发原状态和原签名；
30. 验证相同 nonce 的冲突 ReservationResult 可生成公开可验证的 `ConflictingResultEvidence`；
31. 验证测试 `1-of-3`、`2-of-3` 和 `3-of-3` 托管证明流程只合并绑定完全一致的 CustodyAttestation，且重复公钥或同故障域不能增加独立门槛；
32. 验证主备 RPC 冲突、错误网络和落后节点触发暂停接受；
33. 验证 Funding Coin 在激活前及激活后重组时重新计算高度，且 `effective_A` 不会延长接受期；
34. 验证所有确定性拒绝码保证未写账，而 `UNKNOWN`、`RPC_UNAVAILABLE` 和 `INTERNAL_ERROR` 必须查询。

## 21. 推荐流程

```text
用户创建 Funding Coin 时输入 acceptance_blocks、freeze_blocks、challenge_blocks
        -> 钱包自动计算 close_delay_blocks = acceptance_blocks + freeze_blocks
        -> 用户确认实际参数和 channel_terms_hash
        -> Funding Coin 在高度 F 出生
        -> OPEN acceptance_blocks 高度（默认 12288）
        -> HUB 接受用户授权并签署 append-only OfficialState
        -> 完整 RecoveryPackage 传播给商户和瞭望塔
        -> A = F + acceptance_blocks，停止新预扣
        -> FREEZING freeze_blocks 高度（默认 200）
        -> A 至 S 期间仅补传、查询、送达确认、对账和封账
        -> S = F + close_delay_blocks，Funding Coin 允许 START_CLOSE（默认 12488）
        -> S 后不再首次签发业务绿灯，但瞭望塔继续监视、挑战和结算
        -> 任意人用 State 0 或完整正式状态发起关闭
        -> 第一枚 Closing Coin 在实际高度 C0 出生
        -> D = C0 + challenge_blocks（默认 6000）
        -> 任意人携带更高完整正式状态挑战
        -> 每次挑战产生的 Closing Coin 均继承同一个 D
        -> height >= D
        -> 任意人提交当前完整状态并 FINALIZE
        -> 一次 FINALIZE 生成逐条唯一的 Merchant Payment Coin 和用户找零
        -> 商户或 keeper 可批量转发 Payment Coin 到商户标准地址
```

## 22. 核心结论

V3.6 使用两套独立的相对出生高度计时：

```text
Funding Coin：出生后 acceptance_blocks 高度停止接受，close_delay_blocks 高度后允许关闭
Closing Coin：第一枚 Closing Coin 出生后 challenge_blocks 高度结束挑战
```

上述三个周期由用户在创建 Funding Coin 时确认，默认分别为 `12288`、`200` 和 `6000`；`close_delay_blocks` 自动计算，使用默认值时为 `12488`。Funding Coin 不需要在创建交易时预知确认高度。第一枚 Closing Coin 也不需要在创建时预知自己的出生高度；它在首次花费时通过 `ASSERT_MY_BIRTH_HEIGHT` 取得并验证 `C0`，随后固定唯一的挑战截止高度 `D`。后续挑战不得重新计时。

正式账本只增不减且不可撤销；一次 FINALIZE 为每条记录创建唯一 Merchant Payment Coin；START_CLOSE、CHALLENGE 和 FINALIZE 均执行完整验证；State 0 使用独立空状态分支。HUB 签名产生 SIGNED 状态，完整 RecoveryPackage 经商户签名确认后产生 DELIVERED 状态。这样既避免更高序号状态删除已确认商户，也避免仅凭不可恢复 checkpoint 将资金永久锁定，并避免同一商户相同金额记录产生重复 Coin ID。
