@echo off
cd /d "%~dp0"
powershell.exe -NoProfile -ExecutionPolicy Bypass -Command "$walletScript = Join-Path '%~dp0' 'XHUB-Wallet-V3.6.ps1'; & ([ScriptBlock]::Create((Get-Content -LiteralPath $walletScript -Raw -Encoding UTF8)))"
