param(
    [string]$OutputPath = (Join-Path $PSScriptRoot "testnet-release-v3_6.json")
)

$ErrorActionPreference = "Stop"
$root = Split-Path $PSScriptRoot -Parent

function Relative-Hash([string]$RelativePath) {
    $path = Join-Path $root $RelativePath
    if (-not (Test-Path -LiteralPath $path -PathType Leaf)) {
        throw "Release input is missing: $RelativePath"
    }
    (Get-FileHash -LiteralPath $path -Algorithm SHA256).Hash.ToLowerInvariant()
}

function Component-Commit([string]$RelativePath) {
    $repoRoot = Split-Path $root -Parent
    $tracked = & git -C $repoRoot ls-files -- "V3.6/$RelativePath"
    if (-not $tracked) {
        return "UNCOMMITTED"
    }
    $dirty = & git -C $repoRoot status --porcelain -- "V3.6/$RelativePath"
    if ($dirty) {
        return "UNCOMMITTED"
    }
    (& git -C $repoRoot rev-parse HEAD).Trim()
}

$moduleManifest = Get-Content -LiteralPath (Join-Path $root "puzzles-v3_6/module-hashes.json") -Raw | ConvertFrom-Json
$modules = [ordered]@{}
foreach ($property in $moduleManifest.modules.PSObject.Properties) {
    $module = $property.Value
    $sourcePath = "puzzles-v3_6/$($module.source)"
    $hexPath = "puzzles-v3_6/$($module.hex)"
    $modules[$property.Name] = [ordered]@{
        source = $module.source
        source_sha256 = Relative-Hash $sourcePath
        hex = $module.hex
        hex_sha256 = Relative-Hash $hexPath
        byte_length = $module.byte_length
        module_hash = $module.module_hash
    }
}

$manifest = [ordered]@{
    schema = "xhub-v3-6-testnet-release-1"
    release_id = "v3.6-testnet-vector-1"
    protocol_version = "0x0360"
    release_status = "VECTOR_READY"
    mainnet_approved = $false
    sources = [ordered]@{
        protocol_document = [ordered]@{
            path = "protocol-v3_6/protocol-v3_6.md"
            sha256 = Relative-Hash "protocol-v3_6/protocol-v3_6.md"
        }
        protocol_vectors = [ordered]@{
            path = "protocol-v3_6/test-vectors/protocol-v3_6.json"
            sha256 = Relative-Hash "protocol-v3_6/test-vectors/protocol-v3_6.json"
        }
        testnet_profile = [ordered]@{
            path = "wallet-v3_6/config/testnet-vector-profile-v1.json"
            sha256 = Relative-Hash "wallet-v3_6/config/testnet-vector-profile-v1.json"
        }
    }
    source_commits = [ordered]@{
        protocol = Component-Commit "protocol-v3_6"
        puzzles = Component-Commit "puzzles-v3_6"
        hub = Component-Commit "hub-v3_6"
        watchtower = Component-Commit "watchtower-v3_6"
        wallet = Component-Commit "wallet-v3_6"
    }
    clvm = [ordered]@{
        compiler = $moduleManifest.compiler
        build_script = "puzzles-v3_6/compile-puzzles.ps1"
        build_script_sha256 = Relative-Hash "puzzles-v3_6/compile-puzzles.ps1"
        modules = $modules
    }
    http_api = [ordered]@{
        prefix = "/api/v3.6"
        protocol_version = "0x0360"
        wallet_funding_drafts = "/api/v3.6/funding-drafts"
    }
    defaults = [ordered]@{
        acceptance_blocks = 12288
        freeze_blocks = 200
        close_delay_blocks = 12488
        challenge_blocks = 6000
        funding_confirmation_blocks = 32
        merchant_delivery_confirmations_required = 1
        custody_attestation_threshold = "1-of-3"
        max_ledger_entries = 64
    }
    test_public_keys = [ordered]@{
        wallet_user = "89d0608036649d3484b7cfe71cfbd7f13015081d6206aede1aed0a4c1ad1521233123c08f0870e9d9f605ed429d24419"
        hub_a = "b61c4ee5d1cdd57ea615e6f3003e89afeee153d666562d0abec363d8b88c21c35e55f5622668b113e966564d04eb9fa1"
        merchant_receipt = "97b418875a833bded791b93b592d6cbff91d9a51cc1fe375e9eb44e379751a48f974ead8948542d31b066229aca6fca5"
    }
    compatibility_test = [ordered]@{
        path = "wallet-v3_6/tests/cross_repo.rs"
        sha256 = Relative-Hash "wallet-v3_6/tests/cross_repo.rs"
        covers = @(
            "channel_terms_hash",
            "checkpoint_hash",
            "recovery_package_content_hash",
            "authorization_hash",
            "delivery_confirmation_1_of_3"
        )
    }
    testnet_deployment = [ordered]@{
        status = "VECTOR_READY"
        directory = "deploy/testnet"
        config_schema = "xhub-v3-6-testnet-deployment-1"
        hub_binary = "hub-v3-6"
        watchtower_binary = "watchtower-v3-6"
        wallet_binary = "wallet-v3-6"
        rpc_preflight_binary = "xhub-rpc-preflight"
        authenticated_http_delivery_test = [ordered]@{
            path = "hub-v3_6/tests/watchtower_transport.rs"
            sha256 = Relative-Hash "hub-v3_6/tests/watchtower_transport.rs"
        }
        local_smoke = [ordered]@{
            script = "deploy/testnet/smoke-test.ps1"
            script_sha256 = Relative-Hash "deploy/testnet/smoke-test.ps1"
            result = "wallet/hub/watchtower READY; unauthenticated HUB/watchtower requests rejected"
        }
        config_validation = [ordered]@{
            script = "deploy/testnet/validate-config.ps1"
            script_sha256 = Relative-Hash "deploy/testnet/validate-config.ps1"
            tests = "deploy/testnet/test-config-validation.ps1"
            tests_sha256 = Relative-Hash "deploy/testnet/test-config-validation.ps1"
            result = "fail-closed static deployment configuration gate"
        }
        real_full_node_preflight = "PENDING_EXTERNAL_TESTNET_INPUT"
        real_funding_coin_registration = "PENDING_EXTERNAL_TESTNET_INPUT"
    }
    mainnet_canary_precheck = [ordered]@{
        status = "VECTOR_READY_READ_ONLY"
        directory = "deploy/mainnet"
        config_schema = "xhub-v3-6-mainnet-canary-deployment-1"
        config_example = "deploy/mainnet/config.mainnet.example.json"
        validation_script = "deploy/mainnet/validate-config.ps1"
        validation_script_sha256 = Relative-Hash "deploy/mainnet/validate-config.ps1"
        rpc_preflight_script = "deploy/mainnet/rpc-preflight.ps1"
        rpc_preflight_script_sha256 = Relative-Hash "deploy/mainnet/rpc-preflight.ps1"
        canary_plan_schema = "xhub-v3-6-mainnet-canary-plan-1"
        canary_plan_example = "deploy/mainnet/canary-plan.example.json"
        canary_plan_validator = "deploy/mainnet/validate-canary-plan.ps1"
        canary_plan_validator_sha256 = Relative-Hash "deploy/mainnet/validate-canary-plan.ps1"
        canary_plan_tests = "deploy/mainnet/test-canary-plan.ps1"
        canary_plan_tests_sha256 = Relative-Hash "deploy/mainnet/test-canary-plan.ps1"
        canary_max_total_mojo = 1
        evidence_template = "deploy/mainnet/canary-evidence.example.json"
        evidence_validator = "deploy/mainnet/validate-canary-evidence.ps1"
        evidence_validator_sha256 = Relative-Hash "deploy/mainnet/validate-canary-evidence.ps1"
        evidence_tests = "deploy/mainnet/test-canary-evidence.ps1"
        evidence_tests_sha256 = Relative-Hash "deploy/mainnet/test-canary-evidence.ps1"
        parameter_profile = "deploy/mainnet/mainnet-parameters.candidate.json"
        parameter_profile_sha256 = Relative-Hash "deploy/mainnet/mainnet-parameters.candidate.json"
        artifact_manifest = "deploy/mainnet/mainnet-canary-artifacts.json"
        artifact_manifest_sha256 = Relative-Hash "deploy/mainnet/mainnet-canary-artifacts.json"
        readiness_evaluator = "deploy/mainnet/evaluate-readiness.ps1"
        readiness_evaluator_sha256 = Relative-Hash "deploy/mainnet/evaluate-readiness.ps1"
        candidate_release_generator = "deploy/mainnet/generate-candidate-release.ps1"
        candidate_release_generator_sha256 = Relative-Hash "deploy/mainnet/generate-candidate-release.ps1"
        current_readiness = "BLOCKED_EXTERNAL_INPUT"
        rpc_evidence_validator = "deploy/mainnet/validate-rpc-preflight-output.ps1"
        rpc_evidence_validator_sha256 = Relative-Hash "deploy/mainnet/validate-rpc-preflight-output.ps1"
        funding_binding_validator = "deploy/mainnet/validate-funding-binding.ps1"
        funding_binding_validator_sha256 = Relative-Hash "deploy/mainnet/validate-funding-binding.ps1"
        recovery_simulation_validator = "deploy/mainnet/validate-recovery-simulation.ps1"
        recovery_simulation_validator_sha256 = Relative-Hash "deploy/mainnet/validate-recovery-simulation.ps1"
        approval_validator = "deploy/mainnet/validate-approval-records.ps1"
        approval_validator_sha256 = Relative-Hash "deploy/mainnet/validate-approval-records.ps1"
        dry_run_orchestrator = "deploy/mainnet/run-canary-dry-run.ps1"
        dry_run_orchestrator_sha256 = Relative-Hash "deploy/mainnet/run-canary-dry-run.ps1"
        production_broadcast = $false
    }
    known_limitations = @(
        "This release is a testnet vector baseline and is prohibited for mainnet use.",
        "Mainnet timing safety ranges and challenge_blocks remain OPEN.",
        "The production DeliveryConfirmation threshold remains OPEN; 1-of-3 is testnet-only.",
        "CLVM module hashes are VECTOR_READY candidates, not final mainnet hashes.",
        "Independent security review and two-machine reproducible-build evidence are incomplete.",
        "Authentication, TLS termination, rate limiting, and production key custody are outside this test release."
        "Bearer authentication and a TLS reverse-proxy template are implemented, but external TLS and rate-limit load tests are not yet evidenced."
    )
}

$json = $manifest | ConvertTo-Json -Depth 12
[System.IO.File]::WriteAllText($OutputPath, $json + [Environment]::NewLine, [System.Text.UTF8Encoding]::new($false))
Write-Host "Generated $OutputPath"
