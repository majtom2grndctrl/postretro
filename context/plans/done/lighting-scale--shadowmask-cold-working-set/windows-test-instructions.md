# Windows Shadowmask Cold-Working-Set Gate

Run this once on the 16 GiB Windows measurement machine after fetching
`codex/lighting-scale--shadowmask-cold-working-set`. It exercises the full
stress map at lightmap density 0.04 through the cold, cache-free path and
records the compiler process's peak working set.

The command pins SH probe spacing to 4.0 m so the unrelated direct-SH-delta
working set fits on this host. It does not change the lightmap layout or the
ShadowmaskAtlas stress case. `--release` already bypasses the cache;
`--no-cache` is repeated explicitly to make the disk policy visible.

## 1. Fetch and build

In GitHub Desktop, fetch origin and switch to
`codex/lighting-scale--shadowmask-cold-working-set`. Open the repository in
PowerShell, then build the compiler outside the timed measurement:

```powershell
cargo build --release -p postretro-level-compiler
```

## 2. Run the cold stress bake

Paste and run this block from the repository root:

```powershell
$repo = (Get-Location).Path
$exe = Join-Path $repo "target\release\prl-build.exe"
$map = "content/dev/maps/stress-warren-hallway-inspection.map"
$outDir = Join-Path $repo "baked\shadowmask-cold-working-set"
$out = "baked\shadowmask-cold-working-set\density-004.prl"

New-Item -ItemType Directory -Force $outDir | Out-Null

$arguments = @(
    $map,
    "-o", $out,
    "--release",
    "--no-cache",
    "--no-tui",
    "-v",
    "-j", "4",
    "--lightmap-density", "0.04",
    "--sh-probe-spacing", "4.0"
)

$stopwatch = [System.Diagnostics.Stopwatch]::StartNew()
$process = Start-Process -FilePath $exe -WorkingDirectory $repo `
    -ArgumentList $arguments -NoNewWindow -PassThru
$process.WaitForExit()
$stopwatch.Stop()
$process.Refresh()

$file = if (Test-Path -LiteralPath $out) { Get-Item -LiteralPath $out } else { $null }
[pscustomobject]@{
    ExitCode = $process.ExitCode
    Seconds = [math]::Round($stopwatch.Elapsed.TotalSeconds, 1)
    PeakWorkingSetGiB = [math]::Round($process.PeakWorkingSet64 / 1GB, 3)
    OutputGiB = if ($file) { [math]::Round($file.Length / 1GB, 3) } else { $null }
    Sha256 = if ($file) { (Get-FileHash -LiteralPath $out -Algorithm SHA256).Hash } else { $null }
    Output = $out
}
```

The bake is expected to take roughly forty minutes in the lightmap stage at
this density. Do not switch to a warm or incremental bake if it fails.

## 3. Send back

Send the printed result row. On failure, also send the error and the last named
compiler stage shown. Keep the output until the result and hash have been
reviewed; it can then be deleted to recover disk space.

The supported-density before/after timing row remains a separate follow-up:
density 0.16 on the same machine, release build, and worker count, with no more
than 10% ShadowmaskAtlas-stage regression.
