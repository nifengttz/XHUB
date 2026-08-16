$ErrorActionPreference = 'Stop'
Add-Type -AssemblyName System.Windows.Forms
Add-Type -AssemblyName System.Drawing

$walletRoot = if ([string]::IsNullOrWhiteSpace($PSScriptRoot)) { [Environment]::CurrentDirectory } else { $PSScriptRoot }
$core = Join-Path $walletRoot 'xhub-wallet-core-v3-6.exe'
$vaultPath = Join-Path $walletRoot 'wallet-v3_6.plaintext.json'
$script:wallet = $null
$script:secretsVisible = $false

if (-not (Test-Path -LiteralPath $core)) {
    [System.Windows.Forms.MessageBox]::Show('缺少 xhub-wallet-core-v3-6.exe。请保持它与钱包启动器位于同一文件夹。', 'XHUB Wallet V3.6') | Out-Null
    exit 1
}

function Invoke-WalletCore([string]$Command, [string]$InputText = '') {
    $start = New-Object System.Diagnostics.ProcessStartInfo
    $start.FileName = $core
    $start.Arguments = $Command
    $start.UseShellExecute = $false
    $start.CreateNoWindow = $true
    $start.RedirectStandardInput = $true
    $start.RedirectStandardOutput = $true
    $start.RedirectStandardError = $true
    $start.StandardOutputEncoding = [System.Text.Encoding]::UTF8
    $start.StandardErrorEncoding = [System.Text.Encoding]::UTF8
    $process = New-Object System.Diagnostics.Process
    $process.StartInfo = $start
    [void]$process.Start()
    if (-not [string]::IsNullOrEmpty($InputText)) { $process.StandardInput.Write($InputText) }
    $process.StandardInput.Close()
    $stdout = $process.StandardOutput.ReadToEnd()
    $stderr = $process.StandardError.ReadToEnd()
    $process.WaitForExit()
    if ($process.ExitCode -ne 0) { throw $stderr.Trim() }
    return ($stdout | ConvertFrom-Json)
}

function Save-Wallet($Wallet) {
    $Wallet | Add-Member -NotePropertyName saved_by -NotePropertyValue 'XHUB Wallet V3.6' -Force
    $Wallet | Add-Member -NotePropertyName saved_at_utc -NotePropertyValue ([DateTime]::UtcNow.ToString('o')) -Force
    $json = $Wallet | ConvertTo-Json -Depth 8
    $utf8NoBom = New-Object System.Text.UTF8Encoding($false)
    [System.IO.File]::WriteAllText($vaultPath, $json, $utf8NoBom)
}

function Load-Wallet {
    if (-not (Test-Path -LiteralPath $vaultPath)) { return $null }
    try {
        $loaded = Get-Content -LiteralPath $vaultPath -Raw -Encoding UTF8 | ConvertFrom-Json
        if ($loaded.schema -ne 'xhub.wallet.v3_6.material.v1' -or $loaded.address_index -ne 0 -or $loaded.single_address_mode -ne $true) {
            throw '钱包文件不是 V3.6 单地址索引 0 格式。'
        }
        return $loaded
    } catch {
        [System.Windows.Forms.MessageBox]::Show("钱包文件读取失败：`n$($_.Exception.Message)`n`n文件：$vaultPath", 'XHUB Wallet V3.6', 'OK', 'Error') | Out-Null
        return $null
    }
}

function Copy-Text([string]$Text, [string]$Name) {
    if ([string]::IsNullOrWhiteSpace($Text)) { return }
    [System.Windows.Forms.Clipboard]::SetText($Text)
    Set-Status "$Name 已复制到剪贴板。"
}

function Set-Status([string]$Message, [bool]$Error = $false) {
    $status.Text = $Message
    $status.ForeColor = if ($Error) { [System.Drawing.Color]::Firebrick } else { [System.Drawing.Color]::FromArgb(20, 90, 60) }
}

function Add-Label($Parent, [string]$Text, [int]$X, [int]$Y, [int]$Width, [int]$Height = 24, [bool]$Bold = $false) {
    $label = New-Object System.Windows.Forms.Label
    $label.Text = $Text
    $label.Location = New-Object System.Drawing.Point($X, $Y)
    $label.Size = New-Object System.Drawing.Size($Width, $Height)
    if ($Bold) { $label.Font = New-Object System.Drawing.Font('Microsoft YaHei UI', 10, [System.Drawing.FontStyle]::Bold) }
    $Parent.Controls.Add($label)
    return $label
}

function New-ReadOnlyBox($Parent, [int]$X, [int]$Y, [int]$Width, [int]$Height = 27, [bool]$Multiline = $false) {
    $box = New-Object System.Windows.Forms.TextBox
    $box.Location = New-Object System.Drawing.Point($X, $Y)
    $box.Size = New-Object System.Drawing.Size($Width, $Height)
    $box.ReadOnly = $true
    $box.Multiline = $Multiline
    if ($Multiline) { $box.ScrollBars = 'Vertical' }
    $box.BackColor = [System.Drawing.Color]::FromArgb(248, 250, 252)
    $Parent.Controls.Add($box)
    return $box
}

function Confirm-UnprotectedWallet([string]$Action) {
    $answer = [System.Windows.Forms.MessageBox]::Show("$Action 将创建或替换本地钱包。`n`n此钱包不设置任何密码，也不使用 Windows 账户保护。助记词和私钥会保存在本机明文钱包文件中；任何能复制该文件的人都能控制资产。`n`n是否按此模式继续？", '确认无密码钱包模式', 'YesNo', 'Warning')
    return ($answer -eq [System.Windows.Forms.DialogResult]::Yes)
}

function Show-RestoreDialog {
    $dialog = New-Object System.Windows.Forms.Form
    $dialog.Text = '用 24 个助记词恢复 V3.6 钱包'
    $dialog.StartPosition = 'CenterParent'
    $dialog.ClientSize = New-Object System.Drawing.Size(720, 330)
    $dialog.Font = New-Object System.Drawing.Font('Microsoft YaHei UI', 9)
    [void](Add-Label $dialog '输入 24 个英文助记词（空格或换行分隔）' 20 18 650 28 $true)
    $input = New-Object System.Windows.Forms.TextBox
    $input.Location = New-Object System.Drawing.Point(20, 55)
    $input.Size = New-Object System.Drawing.Size(680, 185)
    $input.Multiline = $true
    $input.ScrollBars = 'Vertical'
    $dialog.Controls.Add($input)
    $note = Add-Label $dialog '仅恢复索引 0：m/12381/8444/2/0。内容不会联网发送。' 20 248 650 24
    $note.ForeColor = [System.Drawing.Color]::DimGray
    $ok = New-Object System.Windows.Forms.Button
    $ok.Text = '恢复钱包'
    $ok.Location = New-Object System.Drawing.Point(480, 282)
    $ok.Size = New-Object System.Drawing.Size(105, 32)
    $dialog.Controls.Add($ok)
    $cancel = New-Object System.Windows.Forms.Button
    $cancel.Text = '取消'
    $cancel.Location = New-Object System.Drawing.Point(595, 282)
    $cancel.Size = New-Object System.Drawing.Size(105, 32)
    $dialog.Controls.Add($cancel)
    $ok.Add_Click({
        if ([string]::IsNullOrWhiteSpace($input.Text)) { [System.Windows.Forms.MessageBox]::Show('请输入 24 个助记词。', '恢复钱包') | Out-Null; return }
        $dialog.Tag = $input.Text
        $dialog.DialogResult = [System.Windows.Forms.DialogResult]::OK
        $dialog.Close()
    })
    $cancel.Add_Click({ $dialog.Close() })
    if ($dialog.ShowDialog($form) -eq [System.Windows.Forms.DialogResult]::OK) { return [string]$dialog.Tag }
    return $null
}

function Update-WalletView {
    $hasWallet = ($null -ne $script:wallet)
    $emptyPanel.Visible = -not $hasWallet
    $walletPanel.Visible = $hasWallet
    $replaceWallet.Enabled = $hasWallet
    $previewButton.Enabled = $hasWallet
    if (-not $hasWallet) { $loginState.Text = '尚未创建钱包'; return }
    $loginState.Text = '已直接登录 · 无密码 · 单地址索引 0'
    $addressBox.Text = $script:wallet.address
    $puzzleHashBox.Text = $script:wallet.puzzle_hash
    $publicKeyBox.Text = $script:wallet.wallet_public_key_index0
    if ($script:secretsVisible) {
        $mnemonicBox.Text = $script:wallet.mnemonic
        $masterPrivateBox.Text = $script:wallet.master_private_key
        $walletPrivateBox.Text = $script:wallet.wallet_private_key_index0
        $syntheticPrivateBox.Text = $script:wallet.synthetic_private_key_index0
        $revealSecrets.Text = '隐藏助记词和私钥'
    } else {
        $mnemonicBox.Text = '••••••••  点击下方按钮后明文显示  ••••••••'
        $masterPrivateBox.Text = '••••••••••••••••••••••••••••••••'
        $walletPrivateBox.Text = '••••••••••••••••••••••••••••••••'
        $syntheticPrivateBox.Text = '••••••••••••••••••••••••••••••••'
        $revealSecrets.Text = '明文显示助记词和私钥'
    }
    $copyMnemonic.Enabled = $script:secretsVisible
    $copyMasterPrivate.Enabled = $script:secretsVisible
    $copyWalletPrivate.Enabled = $script:secretsVisible
    $copySyntheticPrivate.Enabled = $script:secretsVisible
}

$form = New-Object System.Windows.Forms.Form
$form.Text = 'XHUB Wallet V3.6 — 单地址无密码钱包'
$form.StartPosition = 'CenterScreen'
$form.ClientSize = New-Object System.Drawing.Size(1040, 735)
$form.MinimumSize = New-Object System.Drawing.Size(1056, 774)
$form.Font = New-Object System.Drawing.Font('Microsoft YaHei UI', 9)
$form.BackColor = [System.Drawing.Color]::White

$title = Add-Label $form 'XHUB Wallet V3.6' 24 16 400 42 $true
$title.Font = New-Object System.Drawing.Font('Microsoft YaHei UI', 20, [System.Drawing.FontStyle]::Bold)
$loginState = Add-Label $form '正在载入钱包…' 550 25 455 28 $true
$loginState.TextAlign = 'MiddleRight'
$loginState.ForeColor = [System.Drawing.Color]::FromArgb(20, 90, 60)
$warning = Add-Label $form '无密码模式：本地钱包文件不受密码或 Windows 账户保护。请仅存放你愿意承担损失风险的小额 MOJO。' 24 62 980 34 $true
$warning.ForeColor = [System.Drawing.Color]::Firebrick

$tabs = New-Object System.Windows.Forms.TabControl
$tabs.Location = New-Object System.Drawing.Point(22, 104)
$tabs.Size = New-Object System.Drawing.Size(994, 575)
$form.Controls.Add($tabs)
$walletTab = New-Object System.Windows.Forms.TabPage
$walletTab.Text = '钱包首页'
$walletTab.BackColor = [System.Drawing.Color]::White
$tabs.TabPages.Add($walletTab)
$previewTab = New-Object System.Windows.Forms.TabPage
$previewTab.Text = '不可广播交易预览'
$previewTab.BackColor = [System.Drawing.Color]::White
$tabs.TabPages.Add($previewTab)

$emptyPanel = New-Object System.Windows.Forms.Panel
$emptyPanel.Location = New-Object System.Drawing.Point(0, 0)
$emptyPanel.Size = New-Object System.Drawing.Size(980, 535)
$walletTab.Controls.Add($emptyPanel)
$emptyTitle = Add-Label $emptyPanel '还没有本地钱包' 55 85 500 46 $true
$emptyTitle.Font = New-Object System.Drawing.Font('Microsoft YaHei UI', 22, [System.Drawing.FontStyle]::Bold)
[void](Add-Label $emptyPanel '创建全新的 24 词钱包，或用已有 24 词恢复。两种方式都只派生索引 0。' 58 145 820 32)
$newWallet = New-Object System.Windows.Forms.Button
$newWallet.Text = '新建 24 词钱包'
$newWallet.Location = New-Object System.Drawing.Point(58, 205)
$newWallet.Size = New-Object System.Drawing.Size(220, 48)
$newWallet.BackColor = [System.Drawing.Color]::FromArgb(23, 110, 82)
$newWallet.ForeColor = [System.Drawing.Color]::White
$newWallet.FlatStyle = 'Flat'
$emptyPanel.Controls.Add($newWallet)
$restoreWallet = New-Object System.Windows.Forms.Button
$restoreWallet.Text = '使用助记词恢复'
$restoreWallet.Location = New-Object System.Drawing.Point(295, 205)
$restoreWallet.Size = New-Object System.Drawing.Size(220, 48)
$emptyPanel.Controls.Add($restoreWallet)
[void](Add-Label $emptyPanel '钱包启动后会自动直接登录，不出现密码界面。' 58 282 700 28 $true)

$walletPanel = New-Object System.Windows.Forms.Panel
$walletPanel.Location = New-Object System.Drawing.Point(0, 0)
$walletPanel.Size = New-Object System.Drawing.Size(980, 535)
$walletPanel.AutoScroll = $true
$walletTab.Controls.Add($walletPanel)
[void](Add-Label $walletPanel '主网地址（固定索引 0）' 20 14 300 24 $true)
$addressBox = New-ReadOnlyBox $walletPanel 20 42 820
$copyAddress = New-Object System.Windows.Forms.Button
$copyAddress.Text = '复制地址'
$copyAddress.Location = New-Object System.Drawing.Point(850, 40)
$copyAddress.Size = New-Object System.Drawing.Size(100, 30)
$walletPanel.Controls.Add($copyAddress)
[void](Add-Label $walletPanel 'Puzzle Hash' 20 82 180 24 $true)
$puzzleHashBox = New-ReadOnlyBox $walletPanel 20 108 820
$copyPuzzleHash = New-Object System.Windows.Forms.Button
$copyPuzzleHash.Text = '复制'
$copyPuzzleHash.Location = New-Object System.Drawing.Point(850, 106)
$copyPuzzleHash.Size = New-Object System.Drawing.Size(100, 30)
$walletPanel.Controls.Add($copyPuzzleHash)
[void](Add-Label $walletPanel '索引 0 公钥 · m/12381/8444/2/0' 20 148 350 24 $true)
$publicKeyBox = New-ReadOnlyBox $walletPanel 20 174 820
$copyPublicKey = New-Object System.Windows.Forms.Button
$copyPublicKey.Text = '复制公钥'
$copyPublicKey.Location = New-Object System.Drawing.Point(850, 172)
$copyPublicKey.Size = New-Object System.Drawing.Size(100, 30)
$walletPanel.Controls.Add($copyPublicKey)
$revealSecrets = New-Object System.Windows.Forms.Button
$revealSecrets.Text = '明文显示助记词和私钥'
$revealSecrets.Location = New-Object System.Drawing.Point(20, 220)
$revealSecrets.Size = New-Object System.Drawing.Size(240, 38)
$walletPanel.Controls.Add($revealSecrets)
$replaceWallet = New-Object System.Windows.Forms.Button
$replaceWallet.Text = '恢复/替换钱包'
$replaceWallet.Location = New-Object System.Drawing.Point(275, 220)
$replaceWallet.Size = New-Object System.Drawing.Size(180, 38)
$walletPanel.Controls.Add($replaceWallet)
$newReplacement = New-Object System.Windows.Forms.Button
$newReplacement.Text = '新建/替换钱包'
$newReplacement.Location = New-Object System.Drawing.Point(470, 220)
$newReplacement.Size = New-Object System.Drawing.Size(180, 38)
$walletPanel.Controls.Add($newReplacement)
[void](Add-Label $walletPanel '24 词助记词' 20 278 160 24 $true)
$mnemonicBox = New-ReadOnlyBox $walletPanel 20 304 820 65 $true
$copyMnemonic = New-Object System.Windows.Forms.Button
$copyMnemonic.Text = '复制助记词'
$copyMnemonic.Location = New-Object System.Drawing.Point(850, 302)
$copyMnemonic.Size = New-Object System.Drawing.Size(100, 32)
$walletPanel.Controls.Add($copyMnemonic)
[void](Add-Label $walletPanel '主私钥' 20 384 120 24 $true)
$masterPrivateBox = New-ReadOnlyBox $walletPanel 20 410 820
$copyMasterPrivate = New-Object System.Windows.Forms.Button
$copyMasterPrivate.Text = '复制主私钥'
$copyMasterPrivate.Location = New-Object System.Drawing.Point(850, 408)
$copyMasterPrivate.Size = New-Object System.Drawing.Size(100, 30)
$walletPanel.Controls.Add($copyMasterPrivate)
[void](Add-Label $walletPanel '索引 0 钱包私钥' 20 450 180 24 $true)
$walletPrivateBox = New-ReadOnlyBox $walletPanel 20 476 820
$copyWalletPrivate = New-Object System.Windows.Forms.Button
$copyWalletPrivate.Text = '复制私钥'
$copyWalletPrivate.Location = New-Object System.Drawing.Point(850, 474)
$copyWalletPrivate.Size = New-Object System.Drawing.Size(100, 30)
$walletPanel.Controls.Add($copyWalletPrivate)
[void](Add-Label $walletPanel '索引 0 Synthetic 私钥' 20 516 220 24 $true)
$syntheticPrivateBox = New-ReadOnlyBox $walletPanel 20 542 820
$copySyntheticPrivate = New-Object System.Windows.Forms.Button
$copySyntheticPrivate.Text = '复制私钥'
$copySyntheticPrivate.Location = New-Object System.Drawing.Point(850, 540)
$copySyntheticPrivate.Size = New-Object System.Drawing.Size(100, 30)
$walletPanel.Controls.Add($copySyntheticPrivate)

[void](Add-Label $previewTab '该页面只做格式校验和金额预览，不构造 SpendBundle、不连接节点、不广播。' 24 18 900 30 $true)
[void](Add-Label $previewTab '来源 Coin ID（32 字节十六进制）' 24 65 320 24 $true)
$sourceCoinInput = New-Object System.Windows.Forms.TextBox
$sourceCoinInput.Location = New-Object System.Drawing.Point(24, 91)
$sourceCoinInput.Size = New-Object System.Drawing.Size(920, 27)
$previewTab.Controls.Add($sourceCoinInput)
[void](Add-Label $previewTab '收款 XCH 主网地址' 24 132 250 24 $true)
$destinationInput = New-Object System.Windows.Forms.TextBox
$destinationInput.Location = New-Object System.Drawing.Point(24, 158)
$destinationInput.Size = New-Object System.Drawing.Size(920, 27)
$previewTab.Controls.Add($destinationInput)
[void](Add-Label $previewTab '金额（mojo）' 24 202 150 24 $true)
$amountInput = New-Object System.Windows.Forms.TextBox
$amountInput.Location = New-Object System.Drawing.Point(24, 228)
$amountInput.Size = New-Object System.Drawing.Size(220, 27)
$previewTab.Controls.Add($amountInput)
[void](Add-Label $previewTab '最大费用（mojo）' 275 202 180 24 $true)
$feeInput = New-Object System.Windows.Forms.TextBox
$feeInput.Location = New-Object System.Drawing.Point(275, 228)
$feeInput.Size = New-Object System.Drawing.Size(220, 27)
$feeInput.Text = '0'
$previewTab.Controls.Add($feeInput)
[void](Add-Label $previewTab '交易目的' 526 202 150 24 $true)
$purposeInput = New-Object System.Windows.Forms.TextBox
$purposeInput.Location = New-Object System.Drawing.Point(526, 228)
$purposeInput.Size = New-Object System.Drawing.Size(418, 27)
$previewTab.Controls.Add($purposeInput)
$previewButton = New-Object System.Windows.Forms.Button
$previewButton.Text = '生成不可广播预览'
$previewButton.Location = New-Object System.Drawing.Point(24, 280)
$previewButton.Size = New-Object System.Drawing.Size(240, 40)
$previewButton.BackColor = [System.Drawing.Color]::FromArgb(23, 110, 82)
$previewButton.ForeColor = [System.Drawing.Color]::White
$previewButton.FlatStyle = 'Flat'
$previewTab.Controls.Add($previewButton)
$previewOutput = New-ReadOnlyBox $previewTab 24 340 920 170 $true
$previewOutput.Text = '等待输入。'
$status = Add-Label $form '启动中…' 24 690 980 28

$createAction = {
    if (-not (Confirm-UnprotectedWallet '新建钱包')) { return }
    try {
        $created = Invoke-WalletCore 'generate'
        Save-Wallet $created
        $script:wallet = $created
        $script:secretsVisible = $true
        Update-WalletView
        Set-Status '钱包已创建并直接登录。请立即抄写 24 个助记词。'
        [System.Windows.Forms.MessageBox]::Show('钱包已创建。助记词和私钥已按你的要求明文显示；请立即离线抄写备份。', 'XHUB Wallet V3.6', 'OK', 'Warning') | Out-Null
    } catch { Set-Status "创建钱包失败：$($_.Exception.Message)" $true }
}
$newWallet.Add_Click($createAction)
$newReplacement.Add_Click($createAction)

$restoreAction = {
    if (-not (Confirm-UnprotectedWallet '恢复钱包')) { return }
    $phrase = Show-RestoreDialog
    if ($null -eq $phrase) { return }
    try {
        $restored = Invoke-WalletCore 'restore' $phrase
        Save-Wallet $restored
        $script:wallet = $restored
        $script:secretsVisible = $true
        Update-WalletView
        Set-Status '钱包恢复成功并已直接登录；当前只使用索引 0。'
    } catch {
        [System.Windows.Forms.MessageBox]::Show($_.Exception.Message, '恢复失败') | Out-Null
        Set-Status '钱包恢复失败，原钱包未改变。' $true
    }
}
$restoreWallet.Add_Click($restoreAction)
$replaceWallet.Add_Click($restoreAction)

$revealSecrets.Add_Click({
    if (-not $script:secretsVisible) {
        $answer = [System.Windows.Forms.MessageBox]::Show('即将在屏幕上明文显示助记词和全部私钥。请确认周围没有他人或录屏软件。', '显示敏感信息', 'OKCancel', 'Warning')
        if ($answer -ne [System.Windows.Forms.DialogResult]::OK) { return }
    }
    $script:secretsVisible = -not $script:secretsVisible
    Update-WalletView
})

$copyAddress.Add_Click({ Copy-Text $script:wallet.address '地址' })
$copyPuzzleHash.Add_Click({ Copy-Text $script:wallet.puzzle_hash 'Puzzle Hash' })
$copyPublicKey.Add_Click({ Copy-Text $script:wallet.wallet_public_key_index0 '索引 0 公钥' })
$copyMnemonic.Add_Click({ Copy-Text $script:wallet.mnemonic '助记词' })
$copyMasterPrivate.Add_Click({ Copy-Text $script:wallet.master_private_key '主私钥' })
$copyWalletPrivate.Add_Click({ Copy-Text $script:wallet.wallet_private_key_index0 '索引 0 私钥' })
$copySyntheticPrivate.Add_Click({ Copy-Text $script:wallet.synthetic_private_key_index0 '索引 0 Synthetic 私钥' })

$previewButton.Add_Click({
    try {
        [UInt64]$amount = 0
        [UInt64]$fee = 0
        if (-not [UInt64]::TryParse($amountInput.Text.Trim(), [ref]$amount) -or $amount -eq 0) { throw '金额必须是大于 0 的 mojo 整数。' }
        if (-not [UInt64]::TryParse($feeInput.Text.Trim(), [ref]$fee)) { throw '费用必须是非负 mojo 整数。' }
        $request = [ordered]@{
            source_coin_id = $sourceCoinInput.Text.Trim()
            destination_address = $destinationInput.Text.Trim()
            amount_mojo = $amount
            fee_mojo = $fee
            purpose = $purposeInput.Text.Trim()
        }
        $result = Invoke-WalletCore 'preview' ($request | ConvertTo-Json -Compress)
        $previewOutput.Text = @"
网络: $($result.network)
来源 Coin ID: $($result.source_coin_id)
收款地址: $($result.destination_address)
金额: $($result.amount_mojo) mojo
最大费用: $($result.fee_mojo) mojo
最大合计: $($result.total_mojo) mojo
目的: $($result.purpose)

安全闸门: preview_only=$($result.preview_only) / SpendBundle=$($result.spend_bundle_created) / RPC=$($result.rpc_called) / push_tx=$($result.push_tx_called) / broadcast=$($result.chain_broadcast)
"@
        Set-Status '不可广播交易预览已生成；未创建、签名或广播交易。'
    } catch {
        $previewOutput.Text = "预览失败：$($_.Exception.Message)"
        Set-Status '交易预览失败；未执行任何链操作。' $true
    }
})

$script:wallet = Load-Wallet
Update-WalletView
if ($null -ne $script:wallet) { Set-Status "已从本地明文钱包文件自动直接登录：$vaultPath" } else { Set-Status '请选择新建或恢复钱包。' }
[void]$form.ShowDialog()
