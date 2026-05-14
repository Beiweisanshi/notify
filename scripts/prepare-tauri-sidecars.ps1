$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'

$repoRoot = (Resolve-Path (Join-Path $PSScriptRoot '..')).Path
$hostLine = (& rustc -vV | Where-Object { $_ -like 'host:*' } | Select-Object -First 1)
if ([string]::IsNullOrWhiteSpace($hostLine)) {
    throw 'failed to resolve Rust host target'
}
$targetTriple = $hostLine.Substring(5).Trim()

Push-Location $repoRoot
try {
    & cargo build --release --ignore-rust-version -p agent-notify -p agent-notify-tray --bins
    if ($LASTEXITCODE -ne 0) {
        throw "cargo build failed with exit code $LASTEXITCODE"
    }
}
finally {
    Pop-Location
}

$targetDir = $env:CARGO_TARGET_DIR
if ([string]::IsNullOrWhiteSpace($targetDir)) {
    $targetDir = Join-Path $repoRoot 'target'
}
elseif (-not [System.IO.Path]::IsPathRooted($targetDir)) {
    $targetDir = Join-Path $repoRoot $targetDir
}

$releaseDir = Join-Path $targetDir 'release'
$sidecarDir = Join-Path $repoRoot 'src-tauri\binaries'
New-Item -ItemType Directory -Path $sidecarDir -Force | Out-Null

$isWindows = $targetTriple -match 'windows'
$bins = @('agent-notify', 'agent-notify-activate', 'agent-notify-tray')
foreach ($bin in $bins) {
    $sourceName = if ($isWindows) { "$bin.exe" } else { $bin }
    $destName = if ($isWindows) { "$bin-$targetTriple.exe" } else { "$bin-$targetTriple" }
    $source = Join-Path $releaseDir $sourceName
    if (-not (Test-Path -LiteralPath $source -PathType Leaf)) {
        throw "missing sidecar source: $source"
    }
    Copy-Item -LiteralPath $source -Destination (Join-Path $sidecarDir $destName) -Force
}
