$ErrorActionPreference = "Stop"
$scriptText = Get-Content -LiteralPath (Join-Path $PSScriptRoot "check-secret-isolation.ps1") -Raw
foreach ($forbidden in @("Write-Output `$secret", "ConvertTo-Json `$secret", "content = `$secret")) {
    if ($scriptText.Contains($forbidden)) { throw "Secret checker may disclose secret content" }
}
if ($scriptText -notmatch 'Secret files must be distinct' -or
    $scriptText -notmatch 'Secret file grants broad access' -or
    $scriptText -notmatch 'secret_values_disclosed = \$false') {
    throw "Secret isolation checker is missing a required fail-closed boundary"
}
Write-Output "SECRET_ISOLATION_STATIC_TESTS_OK"
