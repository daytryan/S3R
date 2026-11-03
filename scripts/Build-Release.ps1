# -Build N: a local release's number (1, 2, 3 ...; each must beat the last one published). 0 = not a release.
# -ReleaseKey: the owner's ed25519 release key (PEM), outside the repo; default $env:SKATE_RELEASE_KEY. A release (-Build N)
# is refused without it: the launcher takes no release whose release.json.sig is missing (N8, release review 10-18).
# -StoryMode: ship the Skate 3 story mode and its Skate School (the game's story-mode feature and the launcher's
# skate3-career stage). Without it (user 10-22) the release starts up as before story mode and builds no story export.
param([string]$TargetDirectory = (Join-Path (Split-Path $PSScriptRoot -Parent) 'target'), [long]$Build = 0,
      [string]$ReleaseKey = $env:SKATE_RELEASE_KEY, [switch]$StoryMode)
$ProjectRoot = Split-Path $PSScriptRoot -Parent
$ErrorActionPreference = 'Stop'
if ($Build -gt 0) {
    # Checked before anything is built: package.py also checks the key is the one update.rs names.
    if (-not $ReleaseKey -or -not (Test-Path -LiteralPath $ReleaseKey -PathType Leaf)) {
        throw 'A release needs -ReleaseKey <release-key.pem> (or SKATE_RELEASE_KEY): see RELEASE_KEY in crates/skate-launcher/src/update.rs'
    }
    if ((Get-Content -Raw -LiteralPath (Join-Path $ProjectRoot 'crates/skate-launcher/src/update.rs')) -match 'pub const RELEASE_KEY: &str = "";') {
        throw 'RELEASE_KEY in crates/skate-launcher/src/update.rs is empty: paste the release key''s public half there first'
    }
}
$callerRustFlags = $env:RUSTFLAGS
$callerStoryMode = $env:SKATE_STORY_MODE
# tools/pipeline/stages.py story_mode(): the stage table, versions and release.json this script builds follow it.
$env:SKATE_STORY_MODE = if ($StoryMode) { '1' } else { '0' }
$storyFeatures = if ($StoryMode) { @('--features', 'story-mode') } else { @() }
Push-Location $ProjectRoot
try {
    # Cargo cache restoration can prune Python package files under target.
    # Hosted builds get a fresh packaging environment outside that cache.
    $packageEnvironment = if ($env:GITHUB_ACTIONS -eq 'true') {
        if (-not $env:RUNNER_TEMP) { throw 'RUNNER_TEMP is required in GitHub Actions' }
        Join-Path $env:RUNNER_TEMP ('skate-package-' + $env:GITHUB_RUN_ID + '-' + $env:GITHUB_RUN_ATTEMPT)
    } else { Join-Path $ProjectRoot 'target/package-venv' }
    $packagePython = Join-Path $packageEnvironment 'Scripts/python.exe'
    if (-not (Test-Path -LiteralPath $packagePython)) {
        # Build audit 10-28: `python` can be only the Microsoft Store stub (python.org's installer adds `py`, not
        # `python`, by default), which failed a fresh clone here; the py launcher then.
        $python = Get-Command python -CommandType Application -ErrorAction SilentlyContinue | Where-Object { $_.Source -notlike '*\WindowsApps\*' } | Select-Object -First 1
        if ($python) { & $python.Source -m venv $packageEnvironment } else { & py -3 -m venv $packageEnvironment }
        if ($LASTEXITCODE -ne 0) { throw 'Could not create packaging environment' }
    }
    & $packagePython -m pip install -r tools/requirements-setup.txt
    if ($LASTEXITCODE -ne 0) { throw 'Could not install packaging dependencies' }
    & $packagePython -m unittest tools.test_setup_assets tools.asset_pipeline.test_marquee_assets tools.asset_pipeline.test_customiser_setup tools.asset_pipeline.test_setup_recovery tools.asset_pipeline.test_versions tools.asset_pipeline.test_optional_content
    if ($LASTEXITCODE -ne 0) { throw 'Setup extraction regression checks failed' }
    # Cargo reads RUSTFLAGS first and ignores CARGO_TARGET_<triple>_RUSTFLAGS when it is set, so crt-static lives here too.
    # The remaps keep the project and home folders out of panic messages and debug info in the shipped exe. rustc applies
    # the LAST matching remap and the project is under the home folder, so the home one comes first. /PDBALTPATH stops the
    # linker writing the full symbols path into the exe (link.exe expands %_PDB% itself; PowerShell leaves % alone).
    $env:RUSTFLAGS = "-C target-feature=+crt-static --remap-path-prefix=$env:USERPROFILE=~ --remap-path-prefix=$ProjectRoot=. -C link-arg=/PDBALTPATH:%_PDB%"
    $stamp = Get-Date -Format 'yyyyMMdd-HHmmss-ffff'
    $stage = Join-Path $ProjectRoot "target/release-packages/$stamp/skate3rust-windows-x64"
    $symbols = Join-Path $ProjectRoot "target/release-packages/$stamp/symbols"
    New-Item -ItemType Directory -Path "$stage/support",$symbols -Force | Out-Null
    # NVIDIA DLSS (docs/dlss.md): only when DLSS_SDK names NVIDIA's SDK, which never enters this repo. The feature build
    # also needs VULKAN_SDK and LIBCLANG_PATH; nvngx_dlss.dll and NVIDIA's notice ship beside the exe.
    $dlssFeatures = @()
    if ($env:DLSS_SDK) {
        $dlssDll = Join-Path $env:DLSS_SDK 'lib/Windows_x86_64/rel/nvngx_dlss.dll'
        $dlssNotice = Join-Path $env:DLSS_SDK 'NVIDIA-DLSS-NOTICE.txt'
        foreach ($required in $dlssDll, $dlssNotice) { if (-not (Test-Path -LiteralPath $required)) { throw "DLSS build: $required missing (docs/dlss.md)" } }
        if (-not $env:VULKAN_SDK) { throw 'DLSS build: VULKAN_SDK is not set (docs/dlss.md)' }
        $dlssFeatures = @('--features', 'dlss')
        Copy-Item -LiteralPath $dlssDll,$dlssNotice -Destination $stage
    }
    # Link this invocation directly into private staging; never copy a generic cache EXE.
    # The game alone builds with Cargo.toml [profile.dist] (thin LTO, one codegen unit): 7-19% less main-thread time per
    # frame than release in A/B runs (perf 10-17), for a slower build; the relay and launcher stay on release.
    # The game's crash reports name the release (crates/skate-game/build.rs SKATE_BUILD, as the launcher reads it).
    if ($Build -gt 0) { $env:SKATE_BUILD = "$Build" }
    & cargo rustc --profile dist --locked --target x86_64-pc-windows-msvc --target-dir $TargetDirectory -p skate-game --bin skate3rust --no-default-features @dlssFeatures @storyFeatures -- -C extra-filename= -o "$stage/skate3rust.exe" -C "link-arg=/PDB:$symbols/skate3rust.pdb"
    if ($LASTEXITCODE -ne 0) { throw 'Release compilation failed' }
    Remove-Item Env:SKATE_BUILD -ErrorAction SilentlyContinue
    & cargo build --release --locked --target x86_64-pc-windows-msvc --target-dir $TargetDirectory -p skate-steam-relay
    if ($LASTEXITCODE -ne 0) { throw 'Steam relay compilation failed' }
    & (Join-Path $PSScriptRoot 'Stage-SteamRelay.ps1') `
        -TargetDirectory $TargetDirectory `
        -BinDirectory $stage `
        -RelayExecutable (Join-Path $TargetDirectory 'x86_64-pc-windows-msvc/release/skate-steam-relay.exe')
    # rustc also emits a dep-info file beside -o; it contains local source paths.
    $depInfo = Join-Path $stage 'skate3rust.d'
    if (Test-Path -LiteralPath $depInfo) { Remove-Item -LiteralPath $depInfo }
    & (Join-Path $PSScriptRoot 'Build-Native.ps1')
    # Each native library is bundled in the folder of its source, where its Python wrapper looks first.
    $nativeBinaries = @(Get-ChildItem -LiteralPath "$ProjectRoot/tools" -Filter '*_native.rs' -Recurse | ForEach-Object {
        $folder = [IO.Path]::GetRelativePath($ProjectRoot, $_.DirectoryName).Replace('\', '/')
        '--add-binary'; "$ProjectRoot/target/native/$($_.BaseName -replace '_native$', '').dll;$folder"
    })
    if (-not (Test-Path -LiteralPath "$stage/skate3rust.exe") -or -not (Test-Path -LiteralPath "$symbols/skate3rust.pdb")) { throw 'Fresh executable or matching symbols missing.' }
    # F13: signs only when SKATE_SIGN_CERT is set (scripts/Sign-Release.ps1); before the hashes below and in release.json.
    & (Join-Path $PSScriptRoot 'Sign-Release.ps1') (Get-ChildItem -LiteralPath $stage -Filter *.exe -Recurse).FullName  # -Recurse: the relay in steam-relay/ too
    # Preserve the exact PE and PDB pair privately; symbols are not in the player ZIP.
    Copy-Item -LiteralPath "$stage/skate3rust.exe" -Destination $symbols
    @{
        revision = (& git rev-parse HEAD).Trim()
        source_status = @(& git status --porcelain)
        executable_sha256 = (Get-FileHash -LiteralPath "$stage/skate3rust.exe" -Algorithm SHA256).Hash
        pdb_sha256 = (Get-FileHash -LiteralPath "$symbols/skate3rust.pdb" -Algorithm SHA256).Hash
        compiler = (& rustc --version).Trim()
    } | ConvertTo-Json | Set-Content -LiteralPath "$symbols/build.json" -Encoding UTF8
    New-Item -ItemType Directory -Path "$stage/mods" -Force | Out-Null
    Copy-Item -LiteralPath mods/native-trainer.zip,mods/README.md -Destination "$stage/mods"
    $sourceStage = Join-Path $stage '../setup-source'
    $toolsRoot = Join-Path $ProjectRoot 'tools'
    # Of the generators under tools/pipeline/derive only these three chains run at build time (derive.py GENERATORS and
    # what they import); the rest are reference scripts. names-b370/* is excluded with them.
    $deriveBundle = @('ice2/params2.py','ice2/params.py','ice2/xref.py','ice1/params1.py','ice1/enums.py','ice1/x1.py',
                      'names-5372/names.py') | ForEach-Object { "pipeline/derive/$_" }
    foreach ($source in Get-ChildItem -LiteralPath $toolsRoot -File -Recurse) {
        if ($source.FullName -match '[\\/]__pycache__[\\/]') { continue }
        # .ogg is the finished CC0 weather mix (tools/weather_audio), the one audio the release ships; .npz the clothing
        # fit's recorded changes (skate2port/fit_corrections.npz: offsets and weights, no model of the game's); .class the
        # unluac batch driver (UnluacBatch.class: the downloaded Java runtime has no compiler for UnluacBatch.java).
        if ($source.Extension -notin '.py','.json','.txt','.md','.toml','.rs','.ogg','.npz','.class','.tsv' -and $source.Name -ne 'LICENSE') { continue }
        $relative = [IO.Path]::GetRelativePath($toolsRoot, $source.FullName)
        $portableName = $relative.Replace('\','/')
        # Calibration is derived from private models, never a release resource.
        if ($portableName -like 'mixamo_to_skate/*.json') { continue }
        if ($portableName -match '(^|/)blender[^/]*(/|$)' -or
            $portableName -in @('asset_pipeline/build_map.py','asset_pipeline/finish_character.py',
                'add_onboard_ik_targets.py','apply_default_skater_materials.py','export_bevy_glb.py')) { continue }
        # Rescue tooling, tests and one-off dev tools are not part of the player's build.
        if ($portableName -in @('pipeline/rescue.py','pipeline/make_reference.py','pipeline/ORIGINS.md',
                'pipeline/audit.py','pipeline/package.py') -or
            $source.Name -like 'test_*.py' -or $portableName -like 'pipeline/scratch/*' -or $portableName -like '*/work/*') { continue }
        if ($portableName -like 'pipeline/derive/*' -and $portableName -notin $deriveBundle) { continue }
        $destination = Join-Path "$sourceStage/tools" $relative
        New-Item -ItemType Directory -Path (Split-Path -Parent $destination) -Force | Out-Null
        Copy-Item -LiteralPath $source.FullName -Destination $destination
    }
    # The player's setup tool has no SKATE_STORY_MODE: its stage table reads this (stages.py story_mode()).
    @{ story_mode = [bool]$StoryMode } | ConvertTo-Json | Set-Content -LiteralPath "$sourceStage/tools/pipeline/build_flags.json" -Encoding ascii
    $importerBinaries = Join-Path $ProjectRoot 'target/importer-runtime'
    & "$PSScriptRoot/Prepare-CharacterImporter.ps1" -Destination $importerBinaries
    # Every module the bundled scripts import (frozen_check.modules), as hidden imports: PyInstaller can't see them.
    $scriptImports = & $packagePython -m tools.pipeline.frozen_check --list "$sourceStage/tools"
    if ($LASTEXITCODE -ne 0) { throw 'Could not list the build scripts imports' }
    $hiddenImports = @($scriptImports | ForEach-Object { '--hidden-import'; $_ })
    # Engine sources the build scripts read (the trick catalog; the online HUD's string IDs), at their repo paths.
    $engineSources = @('crates/skate-core/src/scoring/catalog.rs', 'crates/skate-game/src/skate3_career/speech_tables.json') +
        @('online_roster.rs','online_moves.rs','online_boundary.rs','online_menu.rs' | ForEach-Object { "crates/skate-game/src/$_" })
    $engineData = @($engineSources | ForEach-Object { '--add-data'; "$ProjectRoot/$_;$(Split-Path $_ -Parent)" })
    # hash_seed=0: the build scripts walk sets of names; a fixed seed makes the same discs give the same files every build
    # --onedir (B1): support/skate3setup/ is unpacked once, at install. --onefile unpacked ~113 MB into %TEMP% on every one
    # of the ~150-200 starts a build makes (2.6-3.3 s each) and left it there after every Cancel.
    # Pillow's AVIF and WebP codecs (~9 MB of the ~113 MB) are left out: no build script reads or writes either format.
    & $packagePython -m PyInstaller --noconfirm --clean --onedir --contents-directory _internal --name skate3setup @hiddenImports @engineData `
        --python-option hash_seed=0 `
        --icon "$ProjectRoot/docs/images/skating-crab.ico" --paths $ProjectRoot `
        --hidden-import numpy --collect-submodules PIL --hidden-import tkinter `
        --hidden-import cryptography --collect-submodules cryptography.hazmat.primitives.ciphers --hidden-import tools.pipeline.runner `
        @nativeBinaries `
        --add-binary "$importerBinaries/FBX2glTF.exe;tools/mixamo_to_skate/tools" `
        --add-binary "$importerBinaries/*.dll;tools/mixamo_to_skate/tools" `
        --exclude-module bpy --exclude-module mathutils `
        --exclude-module vendor --exclude-module build_audio_image `
        --exclude-module PIL.AvifImagePlugin --exclude-module PIL._avif --exclude-module PIL.WebPImagePlugin --exclude-module PIL._webp `
        --copy-metadata numpy --copy-metadata Pillow --copy-metadata PyInstaller `
        --add-data "$sourceStage/tools;tools" --add-data "$ProjectRoot/docs/images/skating-crab.ico;docs/images" `
        --distpath "$stage/support" --workpath target/setup-build/work --specpath target/setup-build tools/setup.py
    if ($LASTEXITCODE -ne 0) { throw 'Setup packaging failed' }
    # The build scripts run from data, so PyInstaller misses their imports; the frozen exe must load them all. vendor and
    # build_audio_image are excluded above: a frozen copy beats the scripts' own files on every import (build 2 froze half
    # of vendor.skate3_ui and hid project.py); frozen_check fails on any other such name.
    $setupExe = "$stage/support/skate3setup/skate3setup.exe"
    & (Join-Path $PSScriptRoot 'Sign-Release.ps1') $setupExe
    & $setupExe --task tools/pipeline/frozen_check.py
    if ($LASTEXITCODE -ne 0) { throw 'The setup bundle is missing a module the build scripts import' }
    & $packagePython tools/mixamo_to_skate/check_package.py --setup $setupExe
    if ($LASTEXITCODE -ne 0) { throw 'Packaged character importer verification failed' }
    # GitHub run number is monotonic across releases, including prereleases. Re-runs
    # deliberately retain identity; publish a new run/tag for a new eligible build.
    $build = if ($env:GITHUB_RUN_NUMBER) { [long]$env:GITHUB_RUN_NUMBER } else { $Build }
    $tag = if ($env:RELEASE_TAG) { $env:RELEASE_TAG } else { 'development' }
    # The launcher reads its own build number at compile time (SKATE_BUILD) to tell a newer release apart.
    $env:SKATE_BUILD = "$build"
    try {
        & cargo build --release --locked --target x86_64-pc-windows-msvc --target-dir $TargetDirectory -p skate-launcher
        if ($LASTEXITCODE -ne 0) { throw 'Launcher compilation failed' }
    } finally { Remove-Item Env:SKATE_BUILD -ErrorAction SilentlyContinue }
    $launcherExe = Join-Path $TargetDirectory 'x86_64-pc-windows-msvc/release/skate3-remaster.exe'
    & (Join-Path $PSScriptRoot 'Sign-Release.ps1') $launcherExe
    # As a module, so `tools.pipeline.stages` imports from the project; the digests read the trimmed copy that ships.
    $assetPipelinesJson = & $packagePython -m tools.asset_pipeline.versions --tools "$sourceStage/tools"
    if ($LASTEXITCODE -ne 0) { throw 'Could not identify asset extractors' }
    $assetPipelines = $assetPipelinesJson | ConvertFrom-Json
    # U3: each stage's fingerprint, the stages it follows and its seconds, so the launcher can say what an update rebuilds.
    $stageTable = & $packagePython -m tools.pipeline.runner --release-stages "$sourceStage/tools"
    if ($LASTEXITCODE -ne 0) { throw 'Could not list the build stages' }
    $stageTable = $stageTable | ConvertFrom-Json
    $characterCustomiser = (& $packagePython -m tools.asset_pipeline.customiser_setup --fingerprint).Trim()
    if ($LASTEXITCODE -ne 0) { throw 'Could not identify character customiser resources' }
    Copy-Item -LiteralPath README.md,docs/THIRD_PARTY_NOTICES.md -Destination $stage
    New-Item -ItemType Directory -Path "$stage/docs/images" -Force | Out-Null
    Copy-Item -LiteralPath docs/images/skating-crab.png -Destination "$stage/docs/images/skating-crab.png"
    Copy-Item -LiteralPath docs/installation.md -Destination "$stage/docs/installation.md"
    Copy-Item -LiteralPath docs/retail-renderer.md -Destination "$stage/docs/retail-renderer.md"
    Copy-Item -LiteralPath docs/crash-reports.md -Destination "$stage/docs/crash-reports.md"
    Copy-Item -LiteralPath docs/performance-tracing.md -Destination "$stage/docs/performance-tracing.md"
    Copy-Item -LiteralPath docs/updates.md -Destination "$stage/docs/updates.md"
    Copy-Item -LiteralPath docs/custom-models.md,docs/mixamo-to-skate.md -Destination "$stage/docs"
    Copy-Item -LiteralPath docs/character-customisation.md -Destination "$stage/docs"
    New-Item -ItemType Directory -Path "$stage/licenses" -Force | Out-Null
    Copy-Item -LiteralPath tools/mixamo_to_skate/licenses/FBX2glTF.txt -Destination "$stage/licenses/FBX2glTF.txt"
    Copy-Item -LiteralPath tools/vendor/utt/LICENSE -Destination "$stage/licenses/UTT.txt"
    Copy-Item -LiteralPath tools/vendor/university/LICENSE-PROJECT.md -Destination "$stage/licenses/CustomEngineLayer.txt"
    # The Skate 2 port linked into skate3rust.exe (Amendment A): its MIT notice.
    Copy-Item -LiteralPath vendor/skate2-rust-port/LICENSE -Destination "$stage/licenses/skate2-rust-port.txt"
    # The Rust crates the exes are built from: each one's license and notice files (release review 10-18).
    & $packagePython -m tools.pipeline.crate_licenses "$stage/licenses/RustCrates.txt"
    if ($LASTEXITCODE -ne 0) { throw 'Could not list the Rust crate licenses' }
    Copy-Item -LiteralPath vendor/bevy_pbr/LICENSE-MIT -Destination "$stage/licenses/Bevy-MIT.txt"
    Copy-Item -LiteralPath vendor/bevy_pbr/LICENSE-APACHE -Destination "$stage/licenses/Bevy-APACHE.txt"
    $pythonBase = (& $packagePython -c 'import sys; print(sys.base_prefix)').Trim()
    Copy-Item -LiteralPath "$pythonBase/LICENSE.txt" -Destination "$stage/licenses/Python.txt"
    foreach ($license in Get-ChildItem -LiteralPath "$pythonBase/tcl" -Filter license.terms -Recurse -ErrorAction SilentlyContinue) {
        Copy-Item -LiteralPath $license.FullName -Destination "$stage/licenses/$($license.Directory.Name).txt"
    }
    # Own every shipped program component, including future tools and libraries.
    # Mods are user-editable content and remain outside program replacement.
    $files = @{}
    foreach ($file in Get-ChildItem -LiteralPath $stage -File -Recurse) {
        $name = [IO.Path]::GetRelativePath($stage, $file.FullName).Replace('\', '/')
        if ($name -eq 'release.json' -or $name.StartsWith('mods/')) { continue }
        $files[$name] = (Get-FileHash -LiteralPath $file.FullName -Algorithm SHA256).Hash.ToLower()
    }
    @{
        schema = 1; repository = 'daytryan/S3R'; target = 'windows-x64'
        build = $build; tag = $tag; revision = (& git rev-parse HEAD).Trim(); files = $files; asset_pipelines = $assetPipelines; character_customiser = $characterCustomiser
        stages = $stageTable
    } | ConvertTo-Json -Depth 5 | Set-Content -LiteralPath "$stage/release.json" -Encoding utf8
    # The launcher's packages (tools/pipeline/package.py): game.zip, the launcher zip and the standalone release.json.
    $packages = Join-Path $ProjectRoot 'target/release-out'
    if (Test-Path -LiteralPath $packages) { Remove-Item -LiteralPath $packages -Recurse -Force }
    if ($Build -gt 0) { & $packagePython -m tools.pipeline.package $stage $launcherExe $packages $ReleaseKey }
    else { & $packagePython -m tools.pipeline.package $stage $launcherExe $packages }  # not a release: no signature, so no launcher takes it
    if ($LASTEXITCODE -ne 0) { throw 'Packaging failed' }
    # The release audit: no file from the player's discs, DLC or mods, no game file type, no personal C:/Users path.
    # SKATE_SOURCES names the source files folder (adds the DLC packages' contents); without it, as on GitHub's
    # machines, the discs and mods are checked from reference.json. The hashes are cached in TEMP, outside the repo.
    $sources = if ($env:SKATE_SOURCES) { $env:SKATE_SOURCES } else { '-' }
    & $packagePython -m tools.pipeline.audit (Join-Path $env:TEMP 's3r-known.json') $sources `
        "$packages/game.zip" "$packages/Skate3Remaster-launcher.zip"
    if ($LASTEXITCODE -ne 0) { throw 'Release audit failed: see the problems above' }
    Write-Host "Release packages: $packages (nothing is uploaded; publishing is a separate step)"
} finally {
    $env:RUSTFLAGS = $callerRustFlags  # the script runs in the caller's shell: later plain cargo builds must not inherit ours
    $env:SKATE_STORY_MODE = $callerStoryMode
    Remove-Item Env:SKATE_BUILD -ErrorAction SilentlyContinue  # a failed game compile leaves it set
    Pop-Location
}
