# Windows Lightmap Bake Measurement Runbook

Use this runbook on the Windows measurement computer to collect the remaining
acceptance evidence for the incremental lightmap bake plan. It uses **GitHub
Desktop** for branch switching; PowerShell runs only the Rust build and bake.

## What this measures

The test bakes `stress-warren-hallway-inspection.map` at the default `0.04`
lightmap texel density through the cold, cache-free compiler path. It uses a
test-only `4.0 m` SH probe spacing (the normal spacing is `1.0 m`) so the
unrelated direct-SH-delta stage fits on the 16 GiB Windows host. It records:

- Peak working set for the pre-change and post-change compiler processes.
- SHA-256 hashes of their emitted `.prl` files, which must match.
- A second post-change output hash, which must match the first post-change
  output and proves repeat determinism.

Each `cargo build` happens immediately before its matching measurement. The
measurements invoke `prl-build.exe` directly, so Cargo compilation memory is
not included.

### Host-memory prerequisite

At the normal `1.0 m` spacing, this map's SH-delta stage estimates a 36.5 GiB
dense working set before it reaches the lightmap bake, so the normal 16 GiB
safety gate correctly refuses it. Increasing the spacing to `4.0 m` reduces
the three-dimensional probe grid by roughly 64x while leaving the lightmap
layout and its `0.04` density unchanged. This is deliberately a
lightmap-focused stress profile, not a production-quality SH bake. Use the
same profile on both branches: their output hashes must still match each
other, but they are not hashes of the normal-precision shipping map.

## 1. Fetch the two branches in GitHub Desktop

1. Open the PostRetro repository in GitHub Desktop and choose **Fetch origin**.
2. Confirm both branches appear in the **Current Branch** menu:
   `test/lightmap-bake-before-incremental-flush` and
   `feature/lighting-scale--lightmap-bake-incremental-flush`.

Build immediately after switching branches in steps 3 and 4 below. A single
checkout has one `target\\release\\prl-build.exe`; building both branches up
front would measure the wrong executable for the first branch.

## 2. Paste this PowerShell helper

Paste the complete function below into the PowerShell terminal opened from
GitHub Desktop. It creates gitignored output files under
`baked/incremental-flush-measurements/` and returns one concise result row for
each bake.

```powershell
function Invoke-LightmapBake($Label) {
    $repo = (Get-Location).Path
    $exe = Join-Path $repo "target\release\prl-build.exe"
    $map = "content/dev/maps/stress-warren-hallway-inspection.map"
    $outDir = Join-Path $repo "baked\incremental-flush-measurements"
    $out = Join-Path $outDir "$Label.prl"

    New-Item -ItemType Directory -Force $outDir | Out-Null

    $stopwatch = [System.Diagnostics.Stopwatch]::StartNew()
    $process = Start-Process `
        -FilePath $exe `
        -WorkingDirectory $repo `
        -ArgumentList "$map -o `"$out`" --release --sh-probe-spacing 4.0" `
        -NoNewWindow -PassThru -Wait
    $stopwatch.Stop()

    $file = if (Test-Path -LiteralPath $out) { Get-Item -LiteralPath $out } else { $null }
    [pscustomobject]@{
        Label = $Label
        ExitCode = $process.ExitCode
        Seconds = [math]::Round($stopwatch.Elapsed.TotalSeconds, 1)
        PeakWorkingSetGiB = [math]::Round($process.PeakWorkingSet64 / 1GB, 2)
        OutputGiB = if ($file) { [math]::Round($file.Length / 1GB, 3) } else { $null }
        Sha256 = if ($file) { (Get-FileHash -LiteralPath $out -Algorithm SHA256).Hash } else { $null }
        Output = $out
    }
}
```

## 3. Measure the pre-change baseline

In GitHub Desktop, switch to `test/lightmap-bake-before-incremental-flush`,
then choose **Repository → Open in Terminal**. Paste the helper from step 2 if
this is a new terminal, then build and run:

```powershell
cargo build --release -p postretro-level-compiler
$before = Invoke-LightmapBake "before"
$before
```

Record the result row. If this bake cannot complete because of memory pressure,
record that fact and any displayed error; do not lower the lightmap density or
raise the SH working-set limit.

## 4. Measure the post-change branch twice

In GitHub Desktop, switch to
`feature/lighting-scale--lightmap-bake-incremental-flush`, then choose
**Repository → Open in Terminal**. Paste the helper again if needed, then
build and run:

```powershell
cargo build --release -p postretro-level-compiler
$after1 = Invoke-LightmapBake "after-1"
$after2 = Invoke-LightmapBake "after-2"

$after1
$after2

"Pre/Post identical: $($before.Sha256 -eq $after1.Sha256)"
"Post deterministic: $($after1.Sha256 -eq $after2.Sha256)"
```

If `$before` came from another terminal, paste its SHA-256 value into the
comparison instead:

```powershell
$beforeSha256 = "PASTE_THE_BEFORE_SHA256_HERE"
"Pre/Post identical: $($beforeSha256 -eq $after1.Sha256)"
"Post deterministic: $($after1.Sha256 -eq $after2.Sha256)"
```

## 5. Send back these results

Send the three printed result rows and the two Boolean lines. Include any bake
error verbatim. With the measured pre- and post-change peak working sets, the
next step is a Windows Job Object address-space-cap run between those two
figures.

Do not delete the output files until the hashes have been reviewed.
