$ErrorActionPreference = 'Stop'
Add-Type -AssemblyName System.Windows.Forms
Add-Type -AssemblyName System.Drawing

$signer = Join-Path $PSScriptRoot 'xhub-local-signer-v3-6.exe'
if (-not (Test-Path -LiteralPath $signer)) {
    [System.Windows.Forms.MessageBox]::Show(
        "找不到本地签名器：$signer`n请保持启动器与 xhub-local-signer-v3-6.exe 在同一文件夹。",
        'XHUB V3.6 本地签名器',
        [System.Windows.Forms.MessageBoxButtons]::OK,
        [System.Windows.Forms.MessageBoxIcon]::Error
    ) | Out-Null
    exit 1
}

function Select-OpenFile([string]$Title, [string]$Filter) {
    $dialog = New-Object System.Windows.Forms.OpenFileDialog
    $dialog.Title = $Title
    $dialog.Filter = $Filter
    $dialog.Multiselect = $false
    if ($dialog.ShowDialog() -ne [System.Windows.Forms.DialogResult]::OK) {
        return $null
    }
    return $dialog.FileName
}

function Select-SaveFile([string]$Title) {
    $dialog = New-Object System.Windows.Forms.SaveFileDialog
    $dialog.Title = $Title
    $dialog.Filter = 'JSON 文件 (*.json)|*.json'
    $dialog.DefaultExt = 'json'
    $dialog.AddExtension = $true
    $dialog.OverwritePrompt = $false
    if ($dialog.ShowDialog() -ne [System.Windows.Forms.DialogResult]::OK) {
        return $null
    }
    return $dialog.FileName
}

function Show-Message([string]$Message, [string]$Title, [System.Windows.Forms.MessageBoxIcon]$Icon) {
    [System.Windows.Forms.MessageBox]::Show(
        $Message,
        $Title,
        [System.Windows.Forms.MessageBoxButtons]::OK,
        $Icon
    ) | Out-Null
}

try {
    $request = Select-OpenFile '选择 V3.6 链下预扣请求 JSON' 'JSON 文件 (*.json)|*.json'
    if ($null -eq $request) { exit 0 }

    $rawReview = & $signer inspect $request 2>&1
    if ($LASTEXITCODE -ne 0) {
        Show-Message ($rawReview -join [Environment]::NewLine) '请求校验失败' ([System.Windows.Forms.MessageBoxIcon]::Error)
        exit 1
    }
    $review = ($rawReview -join [Environment]::NewLine) | ConvertFrom-Json
    $summary = @"
Funding Coin: $($review.funding_coin_id)
Funding 金额: $($review.funding_amount_mojo) mojo
链下预扣: $($review.offchain_reservation_amount_mojo) mojo
用户找零: $($review.user_remainder_amount_mojo) mojo
Reservation nonce: $($review.reservation_nonce)
签名状态: $($review.signature_status)

此工具仅生成链下授权签名：
SpendBundle: false
push_tx: false
广播: false
"@
    $answer = [System.Windows.Forms.MessageBox]::Show(
        $summary + "`n请逐项核对。是否继续选择本地 BLS 私钥文件并签名？",
        'XHUB V3.6 — 链下授权审核',
        [System.Windows.Forms.MessageBoxButtons]::YesNo,
        [System.Windows.Forms.MessageBoxIcon]::Warning
    )
    if ($answer -ne [System.Windows.Forms.DialogResult]::Yes) { exit 0 }

    $secret = Select-OpenFile '选择本机保存的用户 BLS 私钥文件（不会上传）' '密钥文件 (*.hex;*.txt)|*.hex;*.txt|所有文件 (*.*)|*.*'
    if ($null -eq $secret) { exit 0 }
    $output = Select-SaveFile '保存已签名的 V3.6 请求'
    if ($null -eq $output) { exit 0 }
    if (Test-Path -LiteralPath $output) {
        Show-Message '为防止覆盖签名证据，输出文件不能已存在。' '拒绝覆盖' ([System.Windows.Forms.MessageBoxIcon]::Error)
        exit 1
    }

    $result = & $signer sign $request $secret $output --confirm-offchain-1-mojo 2>&1
    if ($LASTEXITCODE -ne 0) {
        Show-Message ($result -join [Environment]::NewLine) '本地签名失败' ([System.Windows.Forms.MessageBoxIcon]::Error)
        exit 1
    }
    Show-Message "签名成功。`n已生成：$output`n`n没有创建 SpendBundle，也没有广播。" 'XHUB V3.6 本地签名器' ([System.Windows.Forms.MessageBoxIcon]::Information)
} catch {
    Show-Message $_.Exception.Message 'XHUB V3.6 本地签名器错误' ([System.Windows.Forms.MessageBoxIcon]::Error)
    exit 1
}
