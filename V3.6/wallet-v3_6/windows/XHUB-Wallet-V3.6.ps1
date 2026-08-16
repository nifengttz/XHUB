$ErrorActionPreference = 'Stop'
Add-Type -AssemblyName System.Windows.Forms
Add-Type -AssemblyName System.Drawing

$walletRoot = if ([string]::IsNullOrWhiteSpace($PSScriptRoot)) {
    [Environment]::CurrentDirectory
} else {
    $PSScriptRoot
}
$signer = Join-Path $walletRoot 'xhub-local-signer-v3-6.exe'
if (-not (Test-Path -LiteralPath $signer)) {
    [System.Windows.Forms.MessageBox]::Show('缺少 xhub-local-signer-v3-6.exe。请保持它与钱包启动器位于同一文件夹。', 'XHUB Wallet V3.6') | Out-Null
    exit 1
}

$state = @{ Request = $null; Secret = $null; Review = $null }

function Add-Label([string]$Text, [int]$X, [int]$Y, [int]$Width = 170, [bool]$Bold = $false) {
    $label = New-Object System.Windows.Forms.Label
    $label.Text = $Text
    $label.Location = New-Object System.Drawing.Point($X, $Y)
    $label.Size = New-Object System.Drawing.Size($Width, 24)
    if ($Bold) { $label.Font = New-Object System.Drawing.Font('Microsoft YaHei UI', 10, [System.Drawing.FontStyle]::Bold) }
    $form.Controls.Add($label)
    return $label
}

function Pick-OpenFile([string]$Title, [string]$Filter) {
    $dialog = New-Object System.Windows.Forms.OpenFileDialog
    $dialog.Title = $Title
    $dialog.Filter = $Filter
    if ($dialog.ShowDialog() -eq [System.Windows.Forms.DialogResult]::OK) { return $dialog.FileName }
    return $null
}

function Pick-SaveFile() {
    $dialog = New-Object System.Windows.Forms.SaveFileDialog
    $dialog.Title = '保存已签名 V3.6 链下授权'
    $dialog.Filter = 'JSON 文件 (*.json)|*.json'
    $dialog.DefaultExt = 'json'
    $dialog.AddExtension = $true
    $dialog.OverwritePrompt = $false
    if ($dialog.ShowDialog() -eq [System.Windows.Forms.DialogResult]::OK) { return $dialog.FileName }
    return $null
}

function Set-Status([string]$Message, [bool]$Error = $false) {
    $status.Text = $Message
    $status.ForeColor = if ($Error) { [System.Drawing.Color]::Firebrick } else { [System.Drawing.Color]::FromArgb(20, 90, 60) }
}

function Update-Controls() {
    $sign.Enabled = ($null -ne $state.Review -and $null -ne $state.Secret -and $state.Review.signature_status -eq 'UNSIGNED')
}

$form = New-Object System.Windows.Forms.Form
$form.Text = 'XHUB Wallet V3.6 — 本地链下授权钱包'
$form.StartPosition = 'CenterScreen'
$form.ClientSize = New-Object System.Drawing.Size(900, 620)
$form.Font = New-Object System.Drawing.Font('Microsoft YaHei UI', 9)
$form.BackColor = [System.Drawing.Color]::White

$title = Add-Label 'XHUB Wallet V3.6' 28 22 420 $true
$title.Font = New-Object System.Drawing.Font('Microsoft YaHei UI', 20, [System.Drawing.FontStyle]::Bold)
$subtitle = Add-Label '本地、非托管 · 链下预扣签名 · 默认不可广播' 30 60 600
$subtitle.ForeColor = [System.Drawing.Color]::DimGray

$guard = New-Object System.Windows.Forms.Label
$guard.Text = '安全状态：本应用不连接 RPC、不创建 SpendBundle、不调用 push_tx、不广播。'
$guard.Location = New-Object System.Drawing.Point(30, 95)
$guard.Size = New-Object System.Drawing.Size(830, 32)
$guard.ForeColor = [System.Drawing.Color]::FromArgb(20, 90, 60)
$guard.Font = New-Object System.Drawing.Font('Microsoft YaHei UI', 10, [System.Drawing.FontStyle]::Bold)
$form.Controls.Add($guard)

$line = New-Object System.Windows.Forms.Label
$line.BorderStyle = [System.Windows.Forms.BorderStyle]::Fixed3D
$line.Location = New-Object System.Drawing.Point(30, 136)
$line.Size = New-Object System.Drawing.Size(840, 2)
$form.Controls.Add($line)

$requestTitle = Add-Label '1. 加载链下授权请求' 30 158 360 $true
$requestPath = New-Object System.Windows.Forms.TextBox
$requestPath.Location = New-Object System.Drawing.Point(30, 190)
$requestPath.Size = New-Object System.Drawing.Size(650, 28)
$requestPath.ReadOnly = $true
$form.Controls.Add($requestPath)
$chooseRequest = New-Object System.Windows.Forms.Button
$chooseRequest.Text = '选择请求 JSON'
$chooseRequest.Location = New-Object System.Drawing.Point(695, 188)
$chooseRequest.Size = New-Object System.Drawing.Size(155, 32)
$form.Controls.Add($chooseRequest)

$summary = New-Object System.Windows.Forms.TextBox
$summary.Location = New-Object System.Drawing.Point(30, 235)
$summary.Size = New-Object System.Drawing.Size(820, 180)
$summary.Multiline = $true
$summary.ReadOnly = $true
$summary.ScrollBars = 'Vertical'
$summary.BackColor = [System.Drawing.Color]::FromArgb(248, 250, 252)
$summary.Text = '尚未加载请求。选择新的、未签名的 V3.6 链下授权 JSON。'
$form.Controls.Add($summary)

$secretTitle = Add-Label '2. 选择本机用户 BLS 私钥文件' 30 440 450 $true
$secretPath = New-Object System.Windows.Forms.TextBox
$secretPath.Location = New-Object System.Drawing.Point(30, 472)
$secretPath.Size = New-Object System.Drawing.Size(650, 28)
$secretPath.ReadOnly = $true
$form.Controls.Add($secretPath)
$chooseSecret = New-Object System.Windows.Forms.Button
$chooseSecret.Text = '选择本机密钥'
$chooseSecret.Location = New-Object System.Drawing.Point(695, 470)
$chooseSecret.Size = New-Object System.Drawing.Size(155, 32)
$form.Controls.Add($chooseSecret)

$privacy = Add-Label '私钥仅在点击“签署链下授权”时由本机读取，绝不上传或保存在此钱包中。' 30 510 760
$privacy.ForeColor = [System.Drawing.Color]::DimGray

$sign = New-Object System.Windows.Forms.Button
$sign.Text = '签署链下授权（不广播）'
$sign.Location = New-Object System.Drawing.Point(30, 550)
$sign.Size = New-Object System.Drawing.Size(270, 42)
$sign.Enabled = $false
$sign.BackColor = [System.Drawing.Color]::FromArgb(23, 110, 82)
$sign.ForeColor = [System.Drawing.Color]::White
$sign.FlatStyle = 'Flat'
$form.Controls.Add($sign)

$status = Add-Label '等待加载请求。' 320 560 530

$chooseRequest.Add_Click({
    $path = Pick-OpenFile '选择 V3.6 链下预扣请求 JSON' 'JSON 文件 (*.json)|*.json'
    if ($null -eq $path) { return }
    try {
        $result = & $signer inspect $path 2>&1
        if ($LASTEXITCODE -ne 0) { throw ($result -join [Environment]::NewLine) }
        $review = ($result -join [Environment]::NewLine) | ConvertFrom-Json
        $state.Request = $path
        $state.Review = $review
        $requestPath.Text = $path
        $summary.Text = @"
Funding Coin: $($review.funding_coin_id)
Funding 金额: $($review.funding_amount_mojo) mojo
本次链下预扣: $($review.offchain_reservation_amount_mojo) mojo
用户找零: $($review.user_remainder_amount_mojo) mojo
Merchant Puzzle Hash: $($review.merchant_puzzle_hash)
Reservation nonce: $($review.reservation_nonce)
Request ID: $($review.request_id)
Authorization hash: $($review.authorization_hash)
签名状态: $($review.signature_status)

安全：local_only=$($review.local_only) / SpendBundle=$($review.spend_bundle_created) / push_tx=$($review.push_tx_called) / broadcast=$($review.chain_broadcast)
"@
        Set-Status '请求校验通过。请核对上述信息后选择本机密钥。'
    } catch {
        $state.Request = $null; $state.Review = $null; $requestPath.Text = ''; $summary.Text = '请求校验失败：' + $_.Exception.Message
        Set-Status '请求无效，未执行任何签名。' $true
    }
    Update-Controls
})

$chooseSecret.Add_Click({
    $path = Pick-OpenFile '选择仅保存在本机的用户 BLS 私钥文件' '密钥文件 (*.hex;*.txt)|*.hex;*.txt|所有文件 (*.*)|*.*'
    if ($null -eq $path) { return }
    $state.Secret = $path
    $secretPath.Text = $path
    Set-Status '已选择本机密钥文件；尚未读取或签名。'
    Update-Controls
})

$sign.Add_Click({
    $answer = [System.Windows.Forms.MessageBox]::Show(
        "请再次确认：`nFunding Coin: $($state.Review.funding_coin_id)`n链下预扣: $($state.Review.offchain_reservation_amount_mojo) mojo`n找零: $($state.Review.user_remainder_amount_mojo) mojo`n`n这是链下签名，不创建或广播交易。是否继续？",
        '确认本地链下授权',
        [System.Windows.Forms.MessageBoxButtons]::YesNo,
        [System.Windows.Forms.MessageBoxIcon]::Warning
    )
    if ($answer -ne [System.Windows.Forms.DialogResult]::Yes) { return }
    $output = Pick-SaveFile
    if ($null -eq $output) { return }
    if (Test-Path -LiteralPath $output) {
        [System.Windows.Forms.MessageBox]::Show('输出文件已存在。为保护签名证据，钱包拒绝覆盖。', '拒绝覆盖') | Out-Null
        return
    }
    try {
        $result = & $signer sign $state.Request $state.Secret $output --confirm-offchain-1-mojo 2>&1
        if ($LASTEXITCODE -ne 0) { throw ($result -join [Environment]::NewLine) }
        Set-Status "签名成功：$output"
        [System.Windows.Forms.MessageBox]::Show("链下授权已签名：`n$output`n`n未创建 SpendBundle，未广播。", 'XHUB Wallet V3.6') | Out-Null
    } catch {
        Set-Status '签名失败；未广播。' $true
        [System.Windows.Forms.MessageBox]::Show($_.Exception.Message, '签名失败') | Out-Null
    }
})

[void]$form.ShowDialog()
