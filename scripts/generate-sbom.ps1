$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

$repoRoot = Split-Path -Parent $PSScriptRoot
$outputDirectory = Join-Path $repoRoot 'target\security'
$outputPath = Join-Path $outputDirectory 'sbom.cdx.json'
[IO.Directory]::CreateDirectory($outputDirectory) | Out-Null

$metadata = cargo metadata --locked --format-version 1 | ConvertFrom-Json
$components = @(
    $metadata.packages |
        Sort-Object name, version |
        ForEach-Object {
            $component = [ordered]@{
                type = 'library'
                name = $_.name
                version = $_.version
                purl = "pkg:cargo/$($_.name)@$($_.version)"
            }
            if ($_.license) {
                $component.licenses = @(
                    [ordered]@{ license = [ordered]@{ id = $_.license } }
                )
            }
            [PSCustomObject]$component
        }
)

$bom = [ordered]@{
    bomFormat = 'CycloneDX'
    specVersion = '1.5'
    version = 1
    metadata = [ordered]@{
        tools = @(
            [ordered]@{ vendor = 'Rust'; name = 'cargo metadata'; version = '1' }
        )
    }
    components = $components
}
$bom | ConvertTo-Json -Depth 12 | Set-Content -Path $outputPath -Encoding utf8
Write-Host "Wrote $outputPath"
