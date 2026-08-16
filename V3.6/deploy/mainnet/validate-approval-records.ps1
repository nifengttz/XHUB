param(
    [Parameter(Mandatory = $true)][string]$ApprovalPath,
    [Parameter(Mandatory = $true)][string]$EvidencePath
)

$ErrorActionPreference = "Stop"
$approvalText = Get-Content -LiteralPath (Resolve-Path $ApprovalPath) -Raw
if ($approvalText -match '(?i)private.?key|mnemonic|spend.?bundle|push_tx') { throw "Approval record contains prohibited material" }
$approval = $approvalText | ConvertFrom-Json
if ($approval.schema -ne "xhub-v3-6-mainnet-canary-approvals-1" -or $approval.protocol_version -ne "0x0360") { throw "Unsupported approval record" }
if ($approval.broadcast_authorized -ne $false) { throw "Approval record cannot authorize broadcast" }
$evidenceHash = (Get-FileHash -LiteralPath (Resolve-Path $EvidencePath) -Algorithm SHA256).Hash.ToLowerInvariant()
if ([string]$approval.evidence_sha256 -ne $evidenceHash) { throw "Approval set is not bound to evidence" }
$records = @($approval.records)
if ($records.Count -ne 2) { throw "Exactly two approval records are required" }
if (($records | Select-Object -ExpandProperty reviewer_id -Unique).Count -ne 2) { throw "Reviewers must be distinct" }
if (($records | Select-Object -ExpandProperty failure_domain -Unique).Count -ne 2) { throw "Reviewer failure domains must be distinct" }
foreach ($record in $records) {
    if ([string]$record.reviewer_id -match '^REPLACE_WITH_' -or [string]$record.failure_domain -match '^REPLACE_WITH_') { throw "Approval identity placeholders are forbidden" }
    if ($record.decision -ne "APPROVED" -or [string]$record.reviewed_evidence_sha256 -ne $evidenceHash) { throw "Reviewer did not approve the exact evidence" }
}
[pscustomobject]@{
    schema = $approval.schema
    protocol_version = "0x0360"
    evidence_sha256 = $evidenceHash
    reviewer_count = 2
    failure_domain_count = 2
    broadcast_authorized = $false
    status = "TWO_PERSON_REVIEW_VERIFIED"
} | ConvertTo-Json -Depth 4
