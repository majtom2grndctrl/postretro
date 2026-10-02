#Requires -Version 5.1
<#
.SYNOPSIS
Fixture output digests for bake-parallelism-large-maps Acceptance rows A1 and A2.
.DESCRIPTION
Builds prl-build in cargo's release profile at -RepoRoot, then bakes each fixture
map in four modes and prints one "<mode> <map> <sha256>" line per .prl:
  cold-j1       prl-build --release -j 1
  cold-default  prl-build --release (default -j)
  warm-miss     warm build on an empty cache dir
  warm-hit      warm build of the same map on that now-full cache dir
Run once on the pre-lever compiler and commit the output as
fixture-digests-before.txt; after each lever, run again and diff.
Run: powershell -ExecutionPolicy Bypass -File fixture-bytes.ps1 [-RepoRoot <dir>] [-OutFile <file>] [-SkipBuild]
#>
[CmdletBinding()]
param(
    [string]$RepoRoot,
    [string]$OutFile,
    [switch]$SkipBuild
)
$ErrorActionPreference = 'Stop'
Set-StrictMode -Version 2

# Name (content/dev/maps/<name>.map) and extra prl-build args. GATE_FIXTURES
# (fixture_pipeline.rs) plus shadowmask selection, animated delta SH, and a
# map whose density forces the oversize-face cut.
$fixtures = @(
    @{ Map = 'gate-heavily-lit'; Args = @() }
    @{ Map = 'soft_shadow_test'; Args = @() }
    @{ Map = 'test_animated_weight_maps_cap'; Args = @() }
    @{ Map = 'test_animated_weight_maps_mixed'; Args = @() }
    @{ Map = 'test_animated_weight_maps_occluded'; Args = @() }
    @{ Map = 'test_animated_weight_maps_single'; Args = @() }
    @{ Map = 'shadowmask-groups-capture'; Args = @() }
    @{ Map = 'specular-shadowmask-capture'; Args = @() }
    @{ Map = 'animated-layer-spill'; Args = @() }
    @{ Map = 'anim-demo'; Args = @() }
    @{ Map = 'sdf-shadow-test'; Args = @('--lightmap-density', '0.006') }
)

if (-not $RepoRoot) {
    $RepoRoot = "$(& git -C $PSScriptRoot rev-parse --show-toplevel)".Trim()
    if ($LASTEXITCODE -ne 0) { throw 'not inside a git checkout; pass -RepoRoot' }
}
$RepoRoot = (Resolve-Path $RepoRoot).Path
$targetDir = if ($env:CARGO_TARGET_DIR) { $env:CARGO_TARGET_DIR } else { Join-Path $RepoRoot 'target' }
if (-not $SkipBuild) {
    Push-Location $RepoRoot
    try { & cargo build --release -p postretro-level-compiler --bin prl-build } finally { Pop-Location }
    if ($LASTEXITCODE -ne 0) { throw "cargo build failed with exit code $LASTEXITCODE" }
}
$exe = Join-Path $targetDir 'release\prl-build.exe'
if (-not (Test-Path $exe)) { throw "missing $exe; build it or drop -SkipBuild" }

$work = Join-Path $targetDir ('fixture-bytes\' + (Get-Date -Format 'yyyyMMdd-HHmmss'))
$null = New-Item -ItemType Directory -Force -Path $work
# .prm sidecars go here too, so a run never writes the workspace baked/ tree.
$bakedRoot = Join-Path $work 'baked'

function Invoke-Bake([string]$map, [string[]]$extra, [string]$mode, [string[]]$modeArgs) {
    $mapPath = Join-Path $RepoRoot "content\dev\maps\$map.map"
    $prl = Join-Path $work "$mode-$map.prl"
    $log = Join-Path $work "$mode-$map.log"
    $all = @($mapPath, '-o', $prl, '--no-tui', '--baked-root', $bakedRoot) + $modeArgs + $extra
    # Windows PowerShell 5.1 turns native stderr into error records; under
    # 'Stop' the first log line would abort the run.
    $ErrorActionPreference = 'Continue'
    & $exe @all *> $log
    if ($LASTEXITCODE -ne 0) { throw "prl-build $mode $map failed ($LASTEXITCODE); see $log" }
    $hash = (Get-FileHash -Algorithm SHA256 $prl).Hash.ToLowerInvariant()
    "$mode $map $hash"
}

$head = "$(& git -C $RepoRoot rev-parse HEAD)".Trim()
$dirty = @(& git -C $RepoRoot status --porcelain -- crates)
$lines = @("# git_head: $head; crates/ $(if ($dirty.Count) { 'DIRTY' } else { 'clean' })")
foreach ($f in $fixtures) {
    $cache = Join-Path $work "cache-$($f.Map)"
    $lines += Invoke-Bake $f.Map $f.Args 'cold-j1' @('--release', '-j', '1')
    $lines += Invoke-Bake $f.Map $f.Args 'cold-default' @('--release')
    $lines += Invoke-Bake $f.Map $f.Args 'warm-miss' @('--cache-dir', $cache)
    $lines += Invoke-Bake $f.Map $f.Args 'warm-hit' @('--cache-dir', $cache)
    Write-Host "$($f.Map) done"
}
if ($OutFile) { [IO.File]::WriteAllText($OutFile, (($lines -join "`n") + "`n")) }
$lines
