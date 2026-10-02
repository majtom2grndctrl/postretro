#Requires -Version 5.1
<#
.SYNOPSIS
One warm hallway bake under the pinned conditions of bake-parallelism-large-maps
(research.md, Measurement conditions). Windows PowerShell 5.1 or pwsh 7.
.DESCRIPTION
Builds prl-build in cargo's release profile, bakes the hallway on an empty cache,
and samples the process every -SampleSeconds into cpu-samples.tsv. Columns 1-5
match evidence/stats.sh; run it on the TSV from Git Bash or WSL.
Run: powershell -ExecutionPolicy Bypass -File measure-hallway.ps1 [-SkipBuild]
-Cold runs prl-build --release instead: the exact, uncached ship bake (the cold
Lightmap Bake row). -ReuseCacheDir <dir> runs a second warm build on an existing,
non-empty cache (the second-build budget row); pass the first run's prl-cache.
Keep the console open and sleep disabled for the whole run (about 9 h): closing
the console ends prl-build, and sleep pauses the wall clock's meaning.
Written without a PowerShell host to test on: untested until its first run.
#>
[CmdletBinding()]
param(
    [string]$RepoRoot,
    [string]$OutDir,
    [ValidateRange(1, 3600)][int]$SampleSeconds = 10,
    [switch]$SkipBuild,
    [switch]$Cold,
    [string]$ReuseCacheDir
)
$ErrorActionPreference = 'Stop'
Set-StrictMode -Version 2
$inv = [Globalization.CultureInfo]::InvariantCulture

if (-not $RepoRoot) {
    $RepoRoot = "$(& git -C $PSScriptRoot rev-parse --show-toplevel)".Trim()
    if ($LASTEXITCODE -ne 0) { throw 'not inside a git checkout; pass -RepoRoot' }
}
$RepoRoot = (Resolve-Path $RepoRoot).Path
$targetDir = if ($env:CARGO_TARGET_DIR) { $env:CARGO_TARGET_DIR } else { Join-Path $RepoRoot 'target' }
if (-not $OutDir) { $OutDir = Join-Path $targetDir ('bake-measure\hallway-' + (Get-Date -Format 'yyyyMMdd-HHmmss')) }
$null = New-Item -ItemType Directory -Force -Path $OutDir
$OutDir = (Resolve-Path $OutDir).Path
if ($Cold -and $ReuseCacheDir) { throw '-Cold bakes uncached; it cannot reuse a cache dir' }
if ($Cold) {
    $cacheDir = $null
} elseif ($ReuseCacheDir) {
    # A second build reads the cache an earlier run filled.
    if (-not (Test-Path $ReuseCacheDir) -or -not @(Get-ChildItem -Force $ReuseCacheDir).Count) { throw "reuse cache dir missing or empty: $ReuseCacheDir" }
    $cacheDir = (Resolve-Path $ReuseCacheDir).Path
} else {
    # An all-miss first build needs a cache no earlier run has touched.
    $cacheDir = Join-Path $OutDir 'prl-cache'
    if ((Test-Path $cacheDir) -and @(Get-ChildItem -Force $cacheDir).Count) { throw "cache dir not empty: $cacheDir" }
}

if (-not $SkipBuild) {
    Push-Location $RepoRoot
    try { & cargo build --release -p postretro-level-compiler --bin prl-build } finally { Pop-Location }
    if ($LASTEXITCODE -ne 0) { throw "cargo build failed with exit code $LASTEXITCODE" }
}
$exe = Join-Path $targetDir 'release\prl-build.exe'
if (-not (Test-Path $exe)) { throw "missing $exe; build it or drop -SkipBuild" }
# Default -j: the binary's own help prints the value it will use; the rule
# mirrors cli.rs default_jobs_for as a cross-check.
$logical = [Environment]::ProcessorCount
$ruleJobs = if ($logical -le 1) { 1 } elseif ($logical -le 8) { $logical - 1 } else { $logical - 2 }
$helpJobs = 'unknown'
foreach ($l in @(& $exe --help)) { if ($l -match '--jobs <N>.*\(default: (\d+)\)') { $helpJobs = $Matches[1] } }
if ("$helpJobs" -ne "$ruleJobs") { Write-Warning "default -j: binary says $helpJobs, rule says $ruleJobs" }

$map = Join-Path $RepoRoot 'content\dev\maps\stress-warren-hallway-inspection.map'
$prl = Join-Path $OutDir 'stress-warren-hallway-inspection.prl'
if (-not (Test-Path $map)) { throw "missing $map" }
# Warm mode: no prl-build --release (that is the cold, uncached ship bake).
# --no-tui is belt and braces: redirected stdout/stderr already select plain mode.
$argLine = if ($Cold) { "`"$map`" -o `"$prl`" --release --no-tui" } else { "`"$map`" -o `"$prl`" --cache-dir `"$cacheDir`" --no-tui" }
$mode = if ($Cold) { 'cold (--release, no cache)' } elseif ($ReuseCacheDir) { 'warm, reused cache' } else { 'warm, empty cache' }

$cpus = @(Get-CimInstance Win32_Processor)
$os = Get-CimInstance Win32_OperatingSystem
$cs = Get-CimInstance Win32_ComputerSystem
$drive = New-Object System.IO.DriveInfo ([IO.Path]::GetPathRoot($OutDir))
$head = "$(& git -C $RepoRoot rev-parse HEAD)".Trim()
$dirty = @(& git -C $RepoRoot status --porcelain)
if ($dirty.Count) { Write-Warning "working tree is dirty; the numbers may not describe $head" }
$machine = @(
    "cpu: $($cpus[0].Name.Trim())"
    "physical_cores: $(($cpus | Measure-Object NumberOfCores -Sum).Sum)"
    "logical_processors: $(($cpus | Measure-Object NumberOfLogicalProcessors -Sum).Sum)"
    "ram_gib: $([math]::Round($cs.TotalPhysicalMemory / 1GB, 1).ToString($inv))"
    "os: $($os.Caption) $($os.Version) build $($os.BuildNumber)"
    "out_dir_volume: $($drive.Name) $($drive.DriveFormat), $([math]::Round($drive.AvailableFreeSpace / 1GB)) GiB free"
    "git_head: $head"
    "git_status_porcelain: $(if ($dirty.Count) { 'DIRTY' } else { 'clean' })"
) + @($dirty | ForEach-Object { "  $_" }) + @(
    "rustc: $(& rustc -V)"
    "default_jobs: $helpJobs (binary help); $ruleJobs (default_jobs_for rule, $logical logical)"
    "command: `"$exe`" $argLine"
    "sample_seconds: $SampleSeconds"
    "mode: $mode"
)
Set-Content -Path (Join-Path $OutDir 'machine.txt') -Value $machine
# Peak working set read after exit, so a peak in the last interval still counts.
if (-not ('PostretroPeakWs' -as [type])) {
    Add-Type -IgnoreWarnings -TypeDefinition @'
using System; using System.Runtime.InteropServices;
public static class PostretroPeakWs {
    [StructLayout(LayoutKind.Sequential)]
    public struct Counters { public uint cb, faults; public UIntPtr peakWs, ws, a, b, c, d, pf, peakPf; }
    [DllImport("psapi.dll")] static extern bool GetProcessMemoryInfo(IntPtr h, out Counters c, uint cb);
    public static long Peak(IntPtr h) {
        Counters c;
        if (!GetProcessMemoryInfo(h, out c, (uint)Marshal.SizeOf(typeof(Counters)))) return -1;
        return (long)c.peakWs.ToUInt64();
    }
}
'@
}

$errLog = Join-Path $OutDir 'prl-build.stderr.log'   # stage-begin lines, progress, log records
$outLog = Join-Path $OutDir 'prl-build.stdout.log'   # Build Summary and warning tally
$tsv = Join-Path $OutDir 'cpu-samples.tsv'
# LF endings: stats.sh's awk would keep a CR on the stage field.
function Add-Row([string]$row) { [IO.File]::AppendAllText($tsv, $row + "`n") }
Add-Row "epoch`tcpu`trss_kb`tthreads`tstage`telapsed_s`tpeak_ws_kb"

$script:stage = ''; $script:logPos = 0L; $script:partial = ''
function Update-Stage {
    if (-not (Test-Path $errLog)) { return }
    try { $fs = [IO.File]::Open($errLog, 'Open', 'Read', 'ReadWrite') } catch { return }   # keep the last stage
    try { $null = $fs.Seek($script:logPos, 'Begin'); $text = (New-Object IO.StreamReader($fs)).ReadToEnd(); $script:logPos = $fs.Position }
    finally { $fs.Dispose() }
    $lines = ($script:partial + $text) -split "`r?`n"
    $script:partial = $lines[-1]
    if ($lines.Count -lt 2) { return }
    # Stage-begin line: "<elapsed>s  <progress label>..."; overlapping stages keep the latest.
    foreach ($l in $lines[0..($lines.Count - 2)]) { if ($l -match '^\s*(\d+\.\d+s\s+.+\.\.\.)\s*$') { $script:stage = $Matches[1] } }
}

$proc = Start-Process -FilePath $exe -ArgumentList $argLine -WorkingDirectory $RepoRoot -NoNewWindow -PassThru `
    -RedirectStandardError $errLog -RedirectStandardOutput $outLog
$null = $proc.Handle   # cache the handle, or ExitCode and exit-time counters are lost
$sw = [Diagnostics.Stopwatch]::StartNew()
$lastCpu = 0.0; $lastWall = 0.0; $peakSampled = 0L
while (-not $proc.WaitForExit($SampleSeconds * 1000)) {
    try { $proc.Refresh(); $cpu = $proc.TotalProcessorTime.TotalSeconds; $ws = $proc.WorkingSet64; $pws = $proc.PeakWorkingSet64; $thr = $proc.Threads.Count }
    catch { continue }   # exited between WaitForExit and Refresh
    $wall = $sw.Elapsed.TotalSeconds
    # 100 = one busy core, the convention of the Mac sampler's ps %CPU.
    $busy = 100.0 * ($cpu - $lastCpu) / [math]::Max($wall - $lastWall, 0.001)
    $lastCpu = $cpu; $lastWall = $wall; $peakSampled = [math]::Max($peakSampled, $pws)
    Update-Stage
    Add-Row ("{0}`t{1}`t{2}`t{3}`t{4}`t{5}`t{6}" -f [DateTimeOffset]::UtcNow.ToUnixTimeSeconds(), $busy.ToString('F1', $inv),
        [long]($ws / 1KB), $thr, $script:stage, [math]::Round($wall).ToString($inv), [long]($pws / 1KB))
    Write-Host ("{0,8:N0}s {1,7:N1}% {2}" -f $wall, $busy, $script:stage)
}
$proc.WaitForExit()
$wallTotal = $sw.Elapsed.TotalSeconds
Add-Row ("{0}`tEXITED" -f [DateTimeOffset]::UtcNow.ToUnixTimeSeconds())

$exitCode = $proc.ExitCode; $cpuTotal = -1.0
try { $cpuTotal = $proc.TotalProcessorTime.TotalSeconds }
catch { $cpuTotal = $lastCpu; Write-Warning 'total CPU time unreadable after exit; using the last sample, which misses the final interval' }
$peak = [PostretroPeakWs]::Peak($proc.Handle); $peakSource = 'GetProcessMemoryInfo after exit'
if ($peak -lt 0) { $peak = $peakSampled; $peakSource = 'max sampled PeakWorkingSet64 (exit read failed)' }
$cacheLine = 'cache_dir: none (cold)'
if ($cacheDir) {
    $entries = @(Get-ChildItem -File -Force $cacheDir -ErrorAction SilentlyContinue)
    $cacheBytes = ($entries | Measure-Object Length -Sum).Sum
    $cacheLine = "cache_dir: $cacheDir ($($entries.Count) files, $([math]::Round($cacheBytes / 1GB, 2).ToString($inv)) GiB)"
}
$summary = @(
    "exit_code: $exitCode"
    "wall_s: $([math]::Round($wallTotal, 1).ToString($inv)) ($([TimeSpan]::FromSeconds([math]::Round($wallTotal)).ToString()))"
    "cpu_s: $([math]::Round($cpuTotal, 1).ToString($inv)) (user + kernel)"
    "mean_busy_cores: $(if ($cpuTotal -ge 0) { [math]::Round($cpuTotal / $wallTotal, 2).ToString($inv) } else { 'unknown' })"
    "peak_working_set_kb: $([long]($peak / 1KB)) ($([math]::Round($peak / 1GB, 2).ToString($inv)) GiB; $peakSource)"
    $cacheLine
    ''
)
$outLines = @(Get-Content $outLog)
$start = -1
for ($i = 0; $i -lt $outLines.Count; $i++) { if ($outLines[$i] -match '^\s*Build Summary:') { $start = $i; break } }
if ($start -ge 0) { $summary += $outLines[$start..($outLines.Count - 1)] } else { $summary += 'No Build Summary on stdout; see prl-build.stderr.log.' }
Set-Content -Path (Join-Path $OutDir 'summary.txt') -Value $summary
$summary | Write-Host
Write-Host "Results in $OutDir"
exit $exitCode
