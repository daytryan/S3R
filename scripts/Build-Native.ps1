# Every tools/**/<name>_native.rs becomes target/native/<name>.dll (refpack_native.rs -> refpack.dll): plain rustc, no
# crates. Build-Release.ps1 runs this and bundles each DLL beside its source; tools/native.py loads them in a checkout.
param([string]$Only = '')
$ProjectRoot = Split-Path $PSScriptRoot -Parent
$ErrorActionPreference = 'Stop'
New-Item -ItemType Directory -Path "$ProjectRoot/target/native" -Force | Out-Null
foreach ($source in Get-ChildItem -LiteralPath "$ProjectRoot/tools" -Filter '*_native.rs' -Recurse) {
    $name = $source.BaseName -replace '_native$', ''
    if ($Only -and $name -ne $Only) { continue }
    & rustc --edition 2024 --crate-type cdylib -C opt-level=3 -C panic=abort -C target-feature=+crt-static `
        "--remap-path-prefix=$env:USERPROFILE=~" "--remap-path-prefix=$ProjectRoot=." $source.FullName `
        -o "$ProjectRoot/target/native/$name.dll"
    if ($LASTEXITCODE -ne 0) { throw "Native converter compilation failed: $($source.Name)" }
}
