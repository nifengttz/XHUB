$ErrorActionPreference="Stop"
$test=Join-Path $PSScriptRoot "docker-single-vps\test-single-vps-docker.ps1"
& powershell.exe -NoProfile -ExecutionPolicy Bypass -File $test
if($LASTEXITCODE-ne0){throw "Single VPS Docker tests failed"}
