$ErrorActionPreference = 'Stop'
Add-Type -AssemblyName System.Windows.Forms
Add-Type -AssemblyName System.Drawing

$walletRoot = if ([string]::IsNullOrWhiteSpace($PSScriptRoot)) { [Environment]::CurrentDirectory } else { $PSScriptRoot }
$core = Join-Path $walletRoot 'xhub-wallet-core-v3-6.exe'
$chain = Join-Path $walletRoot 'xhub-wallet-chain-v3-6.exe'
$vaultPath = Join-Path $walletRoot 'wallet-v3_6.plaintext.json'
$fundingStatePath = Join-Path $walletRoot 'funding-v3_6.json'
$script:wallet = $null
$script:secretsVisible = $false
$script:preparedSend = $null
$script:lastSync = $null
$script:rpcUrl = 'https://api.coinset.org'
$script:walletServiceUrl = 'https://wallet.chiagame.top'
$script:fundingDraft = $null
$script:preparedFunding = $null
$script:fundingSession = $null
$script:lastFundingStatus = $null

if (-not (Test-Path -LiteralPath $core) -or -not (Test-Path -LiteralPath $chain)) {
    [System.Windows.Forms.MessageBox]::Show('缺少 xhub-wallet-core-v3-6.exe 或 xhub-wallet-chain-v3-6.exe。请保持它们与钱包启动器位于同一文件夹。', 'XHUB Wallet V3.6') | Out-Null
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

function Invoke-WalletChain([string]$Command, [string]$InputText) {
    $start = New-Object System.Diagnostics.ProcessStartInfo
    $start.FileName = $chain
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
    $process.StandardInput.Write($InputText)
    $process.StandardInput.Close()
    $stdout = $process.StandardOutput.ReadToEnd()
    $stderr = $process.StandardError.ReadToEnd()
    $process.WaitForExit()
    if ($process.ExitCode -ne 0) { throw $stderr.Trim() }
    return ($stdout | ConvertFrom-Json)
}

function Clear-PreparedSend {
    $script:preparedSend = $null
    if ($null -ne $broadcastButton) { $broadcastButton.Enabled = $false }
}

function Clear-PreparedFunding {
    $script:preparedFunding = $null
    if ($null -ne $fundingBroadcastButton) { $fundingBroadcastButton.Enabled = $false }
}

function Refresh-Chain {
    if ($null -eq $script:wallet) { return }
    try {
        $refreshBalance.Enabled = $false
        $refreshHistory.Enabled = $false
        Set-Status '正在通过主网 RPC 同步余额与 Coin 历史…'
        [System.Windows.Forms.Application]::DoEvents()
        $request = [ordered]@{
            rpc_url = $script:rpcUrl
            puzzle_hash = $script:wallet.puzzle_hash
        }
        $result = Invoke-WalletChain 'sync' ($request | ConvertTo-Json -Compress)
        $script:lastSync = $result
        $balanceHeader.Text = "余额：$($result.confirmed_balance_mojo) mojo"
        $chainDetail.Text = "主网高度 $($result.peak_height) · 未花费 Coin $($result.unspent_coin_count) · RPC $($result.rpc_url)"
        $historyGrid.Rows.Clear()
        foreach ($entry in @($result.history)) {
            $time = if ([UInt64]$entry.timestamp -gt 0) {
                [DateTimeOffset]::FromUnixTimeSeconds([Int64]$entry.timestamp).LocalDateTime.ToString('yyyy-MM-dd HH:mm:ss')
            } else { '-' }
            $stateText = if ($entry.status -eq 'UNSPENT') { '收到/可用' } else { '已花费' }
            $spentHeight = if ($null -eq $entry.spent_height) { '-' } else { [string]$entry.spent_height }
            [void]$historyGrid.Rows.Add($time, $stateText, $entry.amount_mojo, $entry.confirmed_height, $spentHeight, $entry.coin_id)
        }
        Set-Status "同步完成：余额 $($result.confirmed_balance_mojo) mojo，共 $(@($result.history).Count) 条 Coin 历史。"
    } catch {
        $chainDetail.Text = '同步失败：' + $_.Exception.Message
        Set-Status '余额和历史同步失败。' $true
    } finally {
        $refreshBalance.Enabled = $true
        $refreshHistory.Enabled = $true
    }
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

function Save-FundingSession($Session) {
    $json = $Session | ConvertTo-Json -Depth 20
    $utf8NoBom = New-Object System.Text.UTF8Encoding($false)
    [System.IO.File]::WriteAllText($fundingStatePath, $json, $utf8NoBom)
}

function Load-FundingSession {
    if (-not (Test-Path -LiteralPath $fundingStatePath)) { return $null }
    try {
        $loaded = Get-Content -LiteralPath $fundingStatePath -Raw -Encoding UTF8 | ConvertFrom-Json
        if ($loaded.schema -ne 'xhub.wallet.v3_6.funding_session.v1') { throw 'Funding 状态文件格式错误。' }
        return $loaded
    } catch {
        Set-Status "Funding 状态读取失败：$($_.Exception.Message)" $true
        return $null
    }
}

function Test-FundingSessionWallet {
    return ($null -ne $script:wallet -and $null -ne $script:fundingSession -and
        $script:fundingSession.wallet_public_key_index0 -eq $script:wallet.wallet_public_key_index0 -and
        $script:fundingSession.expected_remainder_puzzle_hash -eq $script:wallet.puzzle_hash)
}

function Update-FundingView {
    if ($null -eq $fundingOutput) { return }
    $hasWallet = ($null -ne $script:wallet)
    $fundingTermsButton.Enabled = $hasWallet
    $fundingConfirmButton.Enabled = ($hasWallet -and $null -ne $script:fundingDraft -and $script:fundingDraft.confirmed -ne $true)
    $fundingPrepareButton.Enabled = ($hasWallet -and $null -ne $script:fundingDraft -and $script:fundingDraft.confirmed -eq $true)
    $hasMatchingSession = Test-FundingSessionWallet
    $fundingStatusButton.Enabled = $hasMatchingSession
    $fundingRegisterButton.Enabled = ($hasMatchingSession -and $null -ne $script:lastFundingStatus -and $script:lastFundingStatus.registration_ready -eq $true)
    if ($null -ne $script:fundingSession) {
        if ($hasMatchingSession) {
            $fundingStateLabel.Text = "Funding Coin：$($script:fundingSession.funding_coin_id)"
        } else {
            $fundingStateLabel.Text = '存在旧 Funding 状态，但它不属于当前索引 0 钱包。'
        }
    } elseif ($null -ne $script:fundingDraft) {
        $state = if ($script:fundingDraft.confirmed) { '条款已锁定，尚未广播' } else { '条款待锁定' }
        $fundingStateLabel.Text = $state
    } else {
        $fundingStateLabel.Text = '尚未开始 Funding Coin 流程。'
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
    $refreshBalance.Enabled = $hasWallet
    $refreshHistory.Enabled = $hasWallet
    if (-not $hasWallet) {
        $loginState.Text = '尚未创建钱包'
        $balanceHeader.Text = '余额：-- mojo'
        Clear-PreparedSend
        Clear-PreparedFunding
        Update-FundingView
        return
    }
    $loginState.Text = '已直接登录 · 无密码 · 单地址索引 0'
    $addressBox.Text = $script:wallet.address
    $sourceCoinInput.Text = $script:wallet.address
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
    Update-FundingView
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
$balanceHeader = Add-Label $form '余额：-- mojo' 550 55 455 30 $true
$balanceHeader.TextAlign = 'MiddleRight'
$balanceHeader.Font = New-Object System.Drawing.Font('Microsoft YaHei UI', 14, [System.Drawing.FontStyle]::Bold)
$warning = Add-Label $form '无密码模式：本地钱包文件无保护。发送功能会在你核对并确认后广播真实主网交易。' 24 86 980 28 $true
$warning.ForeColor = [System.Drawing.Color]::Firebrick

$tabs = New-Object System.Windows.Forms.TabControl
$tabs.Location = New-Object System.Drawing.Point(22, 116)
$tabs.Size = New-Object System.Drawing.Size(994, 563)
$form.Controls.Add($tabs)
$walletTab = New-Object System.Windows.Forms.TabPage
$walletTab.Text = '钱包首页'
$walletTab.BackColor = [System.Drawing.Color]::White
$tabs.TabPages.Add($walletTab)
$historyTab = New-Object System.Windows.Forms.TabPage
$historyTab.Text = '交易历史'
$historyTab.BackColor = [System.Drawing.Color]::White
$tabs.TabPages.Add($historyTab)
$previewTab = New-Object System.Windows.Forms.TabPage
$previewTab.Text = '发送 MOJO'
$previewTab.BackColor = [System.Drawing.Color]::White
$tabs.TabPages.Add($previewTab)
$fundingTab = New-Object System.Windows.Forms.TabPage
$fundingTab.Text = 'Funding Coin'
$fundingTab.BackColor = [System.Drawing.Color]::White
$tabs.TabPages.Add($fundingTab)

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
$refreshBalance = New-Object System.Windows.Forms.Button
$refreshBalance.Text = '刷新余额'
$refreshBalance.Location = New-Object System.Drawing.Point(850, 6)
$refreshBalance.Size = New-Object System.Drawing.Size(100, 30)
$walletPanel.Controls.Add($refreshBalance)
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

[void](Add-Label $historyTab '索引 0 地址的主网 Coin 历史（收到、未花费和已花费状态）' 20 16 700 28 $true)
$refreshHistory = New-Object System.Windows.Forms.Button
$refreshHistory.Text = '刷新历史'
$refreshHistory.Location = New-Object System.Drawing.Point(824, 12)
$refreshHistory.Size = New-Object System.Drawing.Size(120, 32)
$historyTab.Controls.Add($refreshHistory)
$chainDetail = Add-Label $historyTab '尚未同步主网。' 20 52 920 28
$chainDetail.ForeColor = [System.Drawing.Color]::DimGray
$historyGrid = New-Object System.Windows.Forms.DataGridView
$historyGrid.Location = New-Object System.Drawing.Point(20, 86)
$historyGrid.Size = New-Object System.Drawing.Size(924, 420)
$historyGrid.ReadOnly = $true
$historyGrid.AllowUserToAddRows = $false
$historyGrid.AllowUserToDeleteRows = $false
$historyGrid.AllowUserToResizeRows = $false
$historyGrid.RowHeadersVisible = $false
$historyGrid.AutoSizeColumnsMode = 'Fill'
$historyGrid.SelectionMode = 'FullRowSelect'
[void]$historyGrid.Columns.Add('time', '时间')
[void]$historyGrid.Columns.Add('state', '状态')
[void]$historyGrid.Columns.Add('amount', 'MOJO')
[void]$historyGrid.Columns.Add('confirmed', '确认高度')
[void]$historyGrid.Columns.Add('spent', '花费高度')
[void]$historyGrid.Columns.Add('coin', 'Coin ID')
$historyGrid.Columns['coin'].FillWeight = 220
$historyTab.Controls.Add($historyGrid)

[void](Add-Label $previewTab '两步发送：先构造并签名预览；只有再次核对确认后才调用 push_tx 广播主网。' 24 18 900 30 $true)
[void](Add-Label $previewTab '来源地址（固定索引 0，自动选择未花费 Coin）' 24 65 500 24 $true)
$sourceCoinInput = New-Object System.Windows.Forms.TextBox
$sourceCoinInput.Location = New-Object System.Drawing.Point(24, 91)
$sourceCoinInput.Size = New-Object System.Drawing.Size(920, 27)
$sourceCoinInput.ReadOnly = $true
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
$previewButton.Text = '1. 构造并签名预览'
$previewButton.Location = New-Object System.Drawing.Point(24, 280)
$previewButton.Size = New-Object System.Drawing.Size(240, 40)
$previewButton.BackColor = [System.Drawing.Color]::FromArgb(23, 110, 82)
$previewButton.ForeColor = [System.Drawing.Color]::White
$previewButton.FlatStyle = 'Flat'
$previewTab.Controls.Add($previewButton)
$broadcastButton = New-Object System.Windows.Forms.Button
$broadcastButton.Text = '2. 确认并广播主网'
$broadcastButton.Location = New-Object System.Drawing.Point(280, 280)
$broadcastButton.Size = New-Object System.Drawing.Size(240, 40)
$broadcastButton.Enabled = $false
$broadcastButton.BackColor = [System.Drawing.Color]::Firebrick
$broadcastButton.ForeColor = [System.Drawing.Color]::White
$broadcastButton.FlatStyle = 'Flat'
$previewTab.Controls.Add($broadcastButton)
$previewOutput = New-ReadOnlyBox $previewTab 24 340 920 170 $true
$previewOutput.Text = '等待输入。第一步会联网读取未花费 Coin 并在本机签名，但不会广播。'

[void](Add-Label $fundingTab 'V3.6 Funding Coin：条款、公钥、Puzzle、金额和 Coin ID 均由本机交叉验证。' 22 14 925 28 $true)
[void](Add-Label $fundingTab 'Funding 金额（mojo）' 22 52 190 24 $true)
$fundingAmountInput = New-Object System.Windows.Forms.TextBox
$fundingAmountInput.Location = New-Object System.Drawing.Point(22, 78)
$fundingAmountInput.Size = New-Object System.Drawing.Size(190, 27)
$fundingAmountInput.Text = '5'
$fundingTab.Controls.Add($fundingAmountInput)
[void](Add-Label $fundingTab '最大费用（mojo）' 232 52 180 24 $true)
$fundingFeeInput = New-Object System.Windows.Forms.TextBox
$fundingFeeInput.Location = New-Object System.Drawing.Point(232, 78)
$fundingFeeInput.Size = New-Object System.Drawing.Size(190, 27)
$fundingFeeInput.Text = '0'
$fundingTab.Controls.Add($fundingFeeInput)
[void](Add-Label $fundingTab 'HUB 钱包服务' 442 52 180 24 $true)
$fundingServiceInput = New-Object System.Windows.Forms.TextBox
$fundingServiceInput.Location = New-Object System.Drawing.Point(442, 78)
$fundingServiceInput.Size = New-Object System.Drawing.Size(502, 27)
$fundingServiceInput.Text = $script:walletServiceUrl
$fundingServiceInput.ReadOnly = $true
$fundingTab.Controls.Add($fundingServiceInput)

$fundingTermsButton = New-Object System.Windows.Forms.Button
$fundingTermsButton.Text = '1. 获取并校验条款'
$fundingTermsButton.Location = New-Object System.Drawing.Point(22, 122)
$fundingTermsButton.Size = New-Object System.Drawing.Size(210, 38)
$fundingTab.Controls.Add($fundingTermsButton)
$fundingConfirmButton = New-Object System.Windows.Forms.Button
$fundingConfirmButton.Text = '2. 确认并锁定条款'
$fundingConfirmButton.Location = New-Object System.Drawing.Point(246, 122)
$fundingConfirmButton.Size = New-Object System.Drawing.Size(210, 38)
$fundingConfirmButton.Enabled = $false
$fundingTab.Controls.Add($fundingConfirmButton)
$fundingPrepareButton = New-Object System.Windows.Forms.Button
$fundingPrepareButton.Text = '3. 构造并签名预览'
$fundingPrepareButton.Location = New-Object System.Drawing.Point(470, 122)
$fundingPrepareButton.Size = New-Object System.Drawing.Size(210, 38)
$fundingPrepareButton.Enabled = $false
$fundingTab.Controls.Add($fundingPrepareButton)
$fundingBroadcastButton = New-Object System.Windows.Forms.Button
$fundingBroadcastButton.Text = '4. 确认并广播主网'
$fundingBroadcastButton.Location = New-Object System.Drawing.Point(694, 122)
$fundingBroadcastButton.Size = New-Object System.Drawing.Size(250, 38)
$fundingBroadcastButton.Enabled = $false
$fundingBroadcastButton.BackColor = [System.Drawing.Color]::Firebrick
$fundingBroadcastButton.ForeColor = [System.Drawing.Color]::White
$fundingTab.Controls.Add($fundingBroadcastButton)

$fundingStateLabel = Add-Label $fundingTab '尚未开始 Funding Coin 流程。' 22 174 922 28 $true
$fundingOutput = New-ReadOnlyBox $fundingTab 22 205 922 220 $true
$fundingOutput.Text = '第一步只发送索引 0 公钥、Puzzle Hash 和金额到 HUB 钱包服务；助记词和私钥永远只留在本机。'
$fundingStatusButton = New-Object System.Windows.Forms.Button
$fundingStatusButton.Text = '刷新链上确认'
$fundingStatusButton.Location = New-Object System.Drawing.Point(22, 445)
$fundingStatusButton.Size = New-Object System.Drawing.Size(210, 38)
$fundingStatusButton.Enabled = $false
$fundingTab.Controls.Add($fundingStatusButton)
$fundingRegisterButton = New-Object System.Windows.Forms.Button
$fundingRegisterButton.Text = '确认达标后注册 HUB'
$fundingRegisterButton.Location = New-Object System.Drawing.Point(246, 445)
$fundingRegisterButton.Size = New-Object System.Drawing.Size(250, 38)
$fundingRegisterButton.Enabled = $false
$fundingTab.Controls.Add($fundingRegisterButton)
$fundingNote = Add-Label $fundingTab '注册门槛：链上未花费且达到 32 个确认；注册不广播新的链上交易。' 520 450 424 34
$fundingNote.ForeColor = [System.Drawing.Color]::DimGray
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
$refreshBalance.Add_Click({ Refresh-Chain })
$refreshHistory.Add_Click({ Refresh-Chain })

$invalidatePrepared = {
    Clear-PreparedSend
    if ($null -ne $previewOutput) { $previewOutput.Text = '输入已变化，请重新执行第一步构造并签名预览。' }
}
$destinationInput.Add_TextChanged($invalidatePrepared)
$amountInput.Add_TextChanged($invalidatePrepared)
$feeInput.Add_TextChanged($invalidatePrepared)
$purposeInput.Add_TextChanged($invalidatePrepared)

$previewButton.Add_Click({
    try {
        Clear-PreparedSend
        [UInt64]$amount = 0
        [UInt64]$fee = 0
        if (-not [UInt64]::TryParse($amountInput.Text.Trim(), [ref]$amount) -or $amount -eq 0) { throw '金额必须是大于 0 的 mojo 整数。' }
        if (-not [UInt64]::TryParse($feeInput.Text.Trim(), [ref]$fee)) { throw '费用必须是非负 mojo 整数。' }
        $request = [ordered]@{
            rpc_url = $script:rpcUrl
            wallet_private_key_index0 = $script:wallet.wallet_private_key_index0
            expected_puzzle_hash = $script:wallet.puzzle_hash
            destination_address = $destinationInput.Text.Trim()
            amount_mojo = $amount
            fee_mojo = $fee
            purpose = $purposeInput.Text.Trim()
        }
        Set-Status '正在读取未花费 Coin、构造并本地签名；此步骤不会广播…'
        [System.Windows.Forms.Application]::DoEvents()
        $result = Invoke-WalletChain 'prepare-send' ($request | ConvertTo-Json -Compress)
        $script:preparedSend = $result
        $coinIds = @($result.selected_coins | ForEach-Object { $_.coin_id }) -join [Environment]::NewLine
        $previewOutput.Text = @"
网络: $($result.network)
来源 Puzzle Hash: $($result.source_puzzle_hash)
收款地址: $($result.destination_address)
金额: $($result.amount_mojo) mojo
费用: $($result.fee_mojo) mojo
输入合计: $($result.input_total_mojo) mojo
找零: $($result.change_mojo) mojo
目的: $($result.purpose)
SpendBundle ID: $($result.spend_bundle_id)
输入 Coin:
$coinIds

离线验证: consensus=$($result.consensus_conditions_verified) / signature=$($result.aggregate_signature_verified)
当前状态: 已签名、尚未广播
"@
        $broadcastButton.Enabled = $true
        Set-Status '签名预览已生成，尚未广播。请逐项核对后再执行第二步。'
    } catch {
        $previewOutput.Text = "预览失败：$($_.Exception.Message)"
        Set-Status '交易构造失败；未广播。' $true
    }
})

$broadcastButton.Add_Click({
    if ($null -eq $script:preparedSend) { return }
    $coinCount = @($script:preparedSend.selected_coins).Count
    $confirmation = @"
这是一次真实 Chia 主网广播，请逐项确认：

收款地址：$($script:preparedSend.destination_address)
发送金额：$($script:preparedSend.amount_mojo) mojo
手续费：$($script:preparedSend.fee_mojo) mojo
找零：$($script:preparedSend.change_mojo) mojo
输入 Coin：$coinCount 个
交易目的：$($script:preparedSend.purpose)
SpendBundle ID：$($script:preparedSend.spend_bundle_id)
RPC：$($script:preparedSend.rpc_url)

点击“是”将立即调用 push_tx，交易广播后无法撤销。是否继续？
"@
    $answer = [System.Windows.Forms.MessageBox]::Show($confirmation, '最终确认：广播真实主网交易', 'YesNo', 'Warning')
    if ($answer -ne [System.Windows.Forms.DialogResult]::Yes) {
        Set-Status '你已取消广播；已签名预览未提交。'
        return
    }
    try {
        $broadcastButton.Enabled = $false
        Set-Status '正在重检输入 Coin 并广播主网…'
        [System.Windows.Forms.Application]::DoEvents()
        $request = [ordered]@{ prepared = $script:preparedSend }
        $result = Invoke-WalletChain 'broadcast' ($request | ConvertTo-Json -Depth 40 -Compress)
        $previewOutput.AppendText("`r`n`r`n广播结果: $($result.status)`r`nSpendBundle ID: $($result.spend_bundle_id)")
        [System.Windows.Forms.MessageBox]::Show("主网节点已接受提交。`n状态：$($result.status)`nSpendBundle ID：$($result.spend_bundle_id)", '发送结果', 'OK', 'Information') | Out-Null
        Set-Status "主网提交完成：$($result.status)。等待链上确认。"
        $script:preparedSend = $null
        Refresh-Chain
    } catch {
        $previewOutput.AppendText("`r`n`r`n广播失败：$($_.Exception.Message)")
        Set-Status '广播失败或被节点拒绝；请刷新余额核对 Coin 状态。' $true
        $broadcastButton.Enabled = $true
    }
})

$fundingAmountInput.Add_TextChanged({
    if ($null -ne $script:fundingDraft) {
        $script:fundingDraft = $null
        Clear-PreparedFunding
        $fundingOutput.Text = 'Funding 金额已变化，请重新获取并锁定条款。'
        Update-FundingView
    }
})
$fundingFeeInput.Add_TextChanged({ Clear-PreparedFunding })

$fundingTermsButton.Add_Click({
    if ($null -eq $script:wallet) { return }
    try {
        [UInt64]$amount = 0
        if (-not [UInt64]::TryParse($fundingAmountInput.Text.Trim(), [ref]$amount) -or $amount -eq 0) { throw 'Funding 金额必须是大于 0 的 mojo 整数。' }
        if ($null -ne $script:fundingSession) {
            $answer = [System.Windows.Forms.MessageBox]::Show('当前已有一个保存的 Funding Coin 状态。继续可以生成新条款；只有新交易成功广播后才会替换当前跟踪状态。是否继续？', '生成新的 Funding 条款', 'YesNo', 'Warning')
            if ($answer -ne [System.Windows.Forms.DialogResult]::Yes) { return }
        }
        Clear-PreparedFunding
        Set-Status '正在从 HUB 获取主网配置，并在本机重算 Funding 条款与 Puzzle…'
        [System.Windows.Forms.Application]::DoEvents()
        $request = [ordered]@{
            wallet_service_url = $script:walletServiceUrl
            wallet_public_key_index0 = $script:wallet.wallet_public_key_index0
            expected_remainder_puzzle_hash = $script:wallet.puzzle_hash
            funding_amount_mojo = $amount
        }
        $script:fundingDraft = Invoke-WalletChain 'prepare-funding-terms' ($request | ConvertTo-Json -Compress)
        $preview = $script:fundingDraft.preview
        $fundingOutput.Text = @"
状态: 条款校验通过，尚未锁定
协议: $($preview.protocol_version) / $($preview.profile_id)
Funding 金额: $amount mojo
索引 0 公钥: $($script:wallet.wallet_public_key_index0)
找零 Puzzle Hash: $($script:wallet.puzzle_hash)
Funding 地址: $($preview.funding_address)
Funding Puzzle Hash: $($preview.funding_puzzle_hash)
Channel Terms Hash: $($preview.channel_terms_hash)
确认门槛: $($preview.funding_confirmation_blocks) blocks

下一步必须由你确认并锁定这些不可变条款。
"@
        Set-Status 'Funding 条款已由本机验证；尚未锁定、签名或广播。'
        Update-FundingView
    } catch {
        $fundingOutput.Text = "Funding 条款获取失败：$($_.Exception.Message)"
        Set-Status 'Funding 条款获取失败；未广播。' $true
    }
})

$fundingConfirmButton.Add_Click({
    if ($null -eq $script:fundingDraft -or $null -eq $script:wallet) { return }
    $preview = $script:fundingDraft.preview
    $message = @"
确认后，下列 Funding 条款将不可修改：

金额：$($fundingAmountInput.Text.Trim()) mojo
Funding 地址：$($preview.funding_address)
Funding Puzzle Hash：$($preview.funding_puzzle_hash)
Channel Terms Hash：$($preview.channel_terms_hash)
找零 Puzzle Hash：$($script:wallet.puzzle_hash)

此步骤不会创建 SpendBundle，也不会广播。是否锁定？
"@
    if ([System.Windows.Forms.MessageBox]::Show($message, '确认并锁定 Funding 条款', 'YesNo', 'Warning') -ne [System.Windows.Forms.DialogResult]::Yes) { return }
    try {
        [UInt64]$amount = [UInt64]$fundingAmountInput.Text.Trim()
        $request = [ordered]@{
            wallet_service_url = $script:walletServiceUrl
            wallet_public_key_index0 = $script:wallet.wallet_public_key_index0
            expected_remainder_puzzle_hash = $script:wallet.puzzle_hash
            funding_amount_mojo = $amount
            draft = $script:fundingDraft
        }
        $script:fundingDraft = Invoke-WalletChain 'confirm-funding-terms' ($request | ConvertTo-Json -Depth 20 -Compress)
        $fundingOutput.AppendText("`r`n`r`n状态: 条款已确认并锁定。现在可以构造专用 Funding SpendBundle 预览。")
        Set-Status 'Funding 条款已锁定；尚未构造或广播交易。'
        Update-FundingView
    } catch {
        $fundingOutput.AppendText("`r`n`r`n锁定失败：$($_.Exception.Message)")
        Set-Status 'Funding 条款锁定失败；未广播。' $true
    }
})

$fundingPrepareButton.Add_Click({
    if ($null -eq $script:fundingDraft -or $script:fundingDraft.confirmed -ne $true) { return }
    try {
        [UInt64]$fee = 0
        if (-not [UInt64]::TryParse($fundingFeeInput.Text.Trim(), [ref]$fee)) { throw '费用必须是非负 mojo 整数。' }
        Clear-PreparedFunding
        $request = [ordered]@{
            rpc_url = $script:rpcUrl
            wallet_service_url = $script:walletServiceUrl
            wallet_private_key_index0 = $script:wallet.wallet_private_key_index0
            wallet_public_key_index0 = $script:wallet.wallet_public_key_index0
            expected_puzzle_hash = $script:wallet.puzzle_hash
            fee_mojo = $fee
            confirmed_draft = $script:fundingDraft
        }
        Set-Status '正在读取索引 0 未花费 Coin，并在本机构造、签名和验证 Funding SpendBundle…'
        [System.Windows.Forms.Application]::DoEvents()
        $script:preparedFunding = Invoke-WalletChain 'prepare-funding' ($request | ConvertTo-Json -Depth 20 -Compress)
        $p = $script:preparedFunding
        $fundingOutput.Text = @"
状态: 已签名、尚未广播
Funding 地址: $($p.destination_address)
Funding Puzzle Hash: $($p.destination_puzzle_hash)
Funding 金额: $($p.amount_mojo) mojo
最大费用: $($p.fee_mojo) mojo
输入合计: $($p.input_total_mojo) mojo
索引 0 找零: $($p.change_mojo) mojo
预测 Funding Coin ID: $($p.funding.predicted_funding_coin_id)
Channel Terms Hash: $($p.funding.draft.preview.channel_terms_hash)
SpendBundle ID: $($p.spend_bundle_id)
确认门槛: $($p.funding.required_confirmations) blocks
本地验证: consensus=$($p.consensus_conditions_verified) / signature=$($p.aggregate_signature_verified)

下一步会再次显示完整主网广播确认框。
"@
        $fundingBroadcastButton.Enabled = $true
        Set-Status 'Funding SpendBundle 已本地签名验证，尚未广播。'
    } catch {
        $fundingOutput.Text = "Funding 交易预览失败：$($_.Exception.Message)"
        Set-Status 'Funding 交易构造失败；未广播。' $true
    }
})

$fundingBroadcastButton.Add_Click({
    if ($null -eq $script:preparedFunding) { return }
    $p = $script:preparedFunding
    $message = @"
这是一次不可撤销的真实 Chia 主网 Funding Coin 广播：

Funding Coin ID：$($p.funding.predicted_funding_coin_id)
Funding 地址：$($p.destination_address)
金额：$($p.amount_mojo) mojo
最大费用：$($p.fee_mojo) mojo
找零：$($p.change_mojo) mojo
Channel Terms Hash：$($p.funding.draft.preview.channel_terms_hash)
SpendBundle ID：$($p.spend_bundle_id)
RPC：$($p.rpc_url)

点击“是”将立即调用 push_tx。是否广播？
"@
    if ([System.Windows.Forms.MessageBox]::Show($message, '最终确认：创建真实主网 Funding Coin', 'YesNo', 'Warning') -ne [System.Windows.Forms.DialogResult]::Yes) {
        Set-Status '你已取消 Funding Coin 广播；签名预览未提交。'
        return
    }
    try {
        $fundingBroadcastButton.Enabled = $false
        Set-Status '正在重检输入 Coin 并广播 Funding SpendBundle…'
        [System.Windows.Forms.Application]::DoEvents()
        $result = Invoke-WalletChain 'broadcast' (([ordered]@{ prepared = $p }) | ConvertTo-Json -Depth 40 -Compress)
        $script:fundingSession = [pscustomobject][ordered]@{
            schema = 'xhub.wallet.v3_6.funding_session.v1'
            wallet_service_url = $p.funding.wallet_service_url
            rpc_url = $p.rpc_url
            wallet_public_key_index0 = $script:wallet.wallet_public_key_index0
            expected_remainder_puzzle_hash = $script:wallet.puzzle_hash
            funding_coin_id = $result.funding_coin_id
            funding_puzzle_hash = $p.destination_puzzle_hash
            funding_amount_mojo = $p.amount_mojo
            required_confirmations = $p.funding.required_confirmations
            confirmed_draft = $p.funding.draft
            spend_bundle_id = $result.spend_bundle_id
            broadcast_status = $result.status
            broadcast_at_unix = $result.submitted_at_unix
        }
        Save-FundingSession $script:fundingSession
        $script:preparedFunding = $null
        $script:lastFundingStatus = $null
        $fundingOutput.AppendText("`r`n`r`n广播结果: $($result.status)`r`nFunding Coin ID: $($result.funding_coin_id)`r`n状态已保存，等待链上确认。")
        Set-Status 'Funding Coin 已提交主网；请使用“刷新链上确认”跟踪。'
        Update-FundingView
        Refresh-Chain
    } catch {
        $fundingOutput.AppendText("`r`n`r`n广播失败：$($_.Exception.Message)")
        Set-Status 'Funding 广播失败或被节点拒绝；请核对 Coin 状态。' $true
        $fundingBroadcastButton.Enabled = $true
    }
})

$fundingStatusButton.Add_Click({
    if (-not (Test-FundingSessionWallet)) { return }
    try {
        $s = $script:fundingSession
        $request = [ordered]@{
            rpc_url = $s.rpc_url
            funding_coin_id = $s.funding_coin_id
            funding_puzzle_hash = $s.funding_puzzle_hash
            funding_amount_mojo = [UInt64]$s.funding_amount_mojo
            required_confirmations = [UInt64]$s.required_confirmations
        }
        Set-Status '正在只读查询 Funding Coin 链上状态…'
        [System.Windows.Forms.Application]::DoEvents()
        $script:lastFundingStatus = Invoke-WalletChain 'funding-status' ($request | ConvertTo-Json -Compress)
        $f = $script:lastFundingStatus
        $fundingOutput.AppendText("`r`n`r`n链上状态: $($f.status)`r`n确认数: $($f.confirmations)/$($f.required_confirmations)`r`n确认高度: $($f.confirmed_height)`r`n可注册 HUB: $($f.registration_ready)")
        Set-Status "Funding Coin 状态：$($f.status)，确认 $($f.confirmations)/$($f.required_confirmations)。"
        Update-FundingView
    } catch {
        $fundingOutput.AppendText("`r`n`r`n状态查询失败：$($_.Exception.Message)")
        Set-Status 'Funding Coin 状态查询失败。' $true
    }
})

$fundingRegisterButton.Add_Click({
    if (-not (Test-FundingSessionWallet) -or $script:lastFundingStatus.registration_ready -ne $true) { return }
    $s = $script:fundingSession
    $message = "将把 Funding Coin $($s.funding_coin_id) 及其公开 Puzzle Reveal、锁定条款注册到 $($s.wallet_service_url) 的 HUB。此操作不广播新的链上交易。是否继续？"
    if ([System.Windows.Forms.MessageBox]::Show($message, '确认注册 Funding Coin 到 HUB', 'YesNo', 'Warning') -ne [System.Windows.Forms.DialogResult]::Yes) { return }
    try {
        $request = [ordered]@{
            rpc_url = $s.rpc_url
            wallet_service_url = $s.wallet_service_url
            wallet_public_key_index0 = $s.wallet_public_key_index0
            expected_remainder_puzzle_hash = $s.expected_remainder_puzzle_hash
            funding_coin_id = $s.funding_coin_id
            confirmed_draft = $s.confirmed_draft
        }
        Set-Status '正在重新核验链上状态并注册 Funding Coin 到 HUB…'
        [System.Windows.Forms.Application]::DoEvents()
        $result = Invoke-WalletChain 'register-funding' ($request | ConvertTo-Json -Depth 20 -Compress)
        $s | Add-Member -NotePropertyName hub_registration -NotePropertyValue $result.hub_response -Force
        $s | Add-Member -NotePropertyName registered_at_utc -NotePropertyValue ([DateTime]::UtcNow.ToString('o')) -Force
        Save-FundingSession $s
        $fundingOutput.AppendText("`r`n`r`nHUB 注册完成：$($result.hub_response.chain_state)`r`n接受截止高度: $($result.hub_response.acceptance_cutoff_height)`r`n计划关闭高度: $($result.hub_response.scheduled_close_height)")
        $fundingRegisterButton.Enabled = $false
        Set-Status 'Funding Coin 已完成链上确认并注册到 HUB。'
    } catch {
        $fundingOutput.AppendText("`r`n`r`nHUB 注册失败：$($_.Exception.Message)")
        Set-Status 'Funding Coin 注册失败；链上 Coin 未受影响。' $true
    }
})

$script:wallet = Load-Wallet
$script:fundingSession = Load-FundingSession
Update-WalletView
if ($null -ne $script:wallet) { Set-Status "已从本地明文钱包文件自动直接登录：$vaultPath" } else { Set-Status '请选择新建或恢复钱包。' }
$form.Add_Shown({
    if ($null -ne $script:wallet) { Refresh-Chain }
})
[void]$form.ShowDialog()
