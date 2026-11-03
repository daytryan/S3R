# F13: Authenticode-sign the shipped programs, only when a certificate is configured. Without SKATE_SIGN_CERT this does
# nothing and the release stays unsigned (Windows SmartScreen then warns players on first run). SKATE_SIGN_CERT is a
# .pfx file (SKATE_SIGN_PASSWORD its password) or the thumbprint of a certificate in the Windows store. Optional:
# SKATE_SIGNTOOL (signtool.exe; default PATH, then the newest Windows SDK) and SKATE_SIGN_TIMESTAMP (an RFC 3161 server).
param([Parameter(Mandatory, ValueFromRemainingArguments)][string[]]$Path)
$ErrorActionPreference = 'Stop'
if (-not $env:SKATE_SIGN_CERT) { return }
$signtool = $env:SKATE_SIGNTOOL
if (-not $signtool) { $signtool = (Get-Command signtool.exe -ErrorAction SilentlyContinue).Source }
if (-not $signtool) {
    $signtool = Get-ChildItem "${env:ProgramFiles(x86)}/Windows Kits/10/bin/*/x64/signtool.exe" -ErrorAction SilentlyContinue |
        Sort-Object FullName | Select-Object -Last 1 -ExpandProperty FullName
}
if (-not $signtool) { throw 'SKATE_SIGN_CERT is set but signtool.exe was not found: install the Windows SDK or set SKATE_SIGNTOOL' }
$certificate = if (Test-Path -LiteralPath $env:SKATE_SIGN_CERT -PathType Leaf) {
    @('/f', $env:SKATE_SIGN_CERT) + $(if ($env:SKATE_SIGN_PASSWORD) { @('/p', $env:SKATE_SIGN_PASSWORD) } else { @() })
} else { @('/sha1', $env:SKATE_SIGN_CERT) }
$timestamp = if ($env:SKATE_SIGN_TIMESTAMP) { $env:SKATE_SIGN_TIMESTAMP } else { 'http://timestamp.digicert.com' }
& $signtool sign /fd sha256 /tr $timestamp /td sha256 @certificate @Path
if ($LASTEXITCODE -ne 0) { throw "Signing failed: $($Path -join ', ')" }
