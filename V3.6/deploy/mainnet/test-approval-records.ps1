$ErrorActionPreference = "Stop"
$tempRoot = Join-Path $env:TEMP ("xhub-approvals-" + [guid]::NewGuid())
New-Item -ItemType Directory $tempRoot | Out-Null
try {
    $evidencePath = Join-Path $tempRoot "evidence.json"
    $approvalPath = Join-Path $tempRoot "approvals.json"
    '{"status":"VALID_EVIDENCE_ONLY"}' | Set-Content $evidencePath -Encoding utf8
    $hash = (Get-FileHash $evidencePath -Algorithm SHA256).Hash.ToLowerInvariant()
    $approval = [ordered]@{
        schema="xhub-v3-6-mainnet-canary-approvals-1";protocol_version="0x0360";evidence_sha256=$hash
        records=@(
            [ordered]@{reviewer_id="reviewer-a";failure_domain="domain-a";decision="APPROVED";reviewed_evidence_sha256=$hash},
            [ordered]@{reviewer_id="reviewer-b";failure_domain="domain-b";decision="APPROVED";reviewed_evidence_sha256=$hash}
        );broadcast_authorized=$false
    }
    $approval | ConvertTo-Json -Depth 8 | Set-Content $approvalPath -Encoding utf8
    $result = & powershell.exe -NoProfile -ExecutionPolicy Bypass -File (Join-Path $PSScriptRoot "validate-approval-records.ps1") -ApprovalPath $approvalPath -EvidencePath $evidencePath | ConvertFrom-Json
    if ($LASTEXITCODE -ne 0 -or $result.status -ne "TWO_PERSON_REVIEW_VERIFIED") { throw "Valid approvals were rejected" }
    $approval.records[1].failure_domain = "domain-a"
    $approval | ConvertTo-Json -Depth 8 | Set-Content $approvalPath -Encoding utf8
    $previous=$ErrorActionPreference;$ErrorActionPreference="Continue"
    & powershell.exe -NoProfile -ExecutionPolicy Bypass -File (Join-Path $PSScriptRoot "validate-approval-records.ps1") -ApprovalPath $approvalPath -EvidencePath $evidencePath *> $null
    $exitCode=$LASTEXITCODE;$ErrorActionPreference=$previous
    if($exitCode -eq 0){throw "Same-domain approvals were accepted"}
} finally { Remove-Item -LiteralPath $tempRoot -Recurse -Force -ErrorAction SilentlyContinue }
Write-Output "APPROVAL_RECORD_TESTS_OK"
