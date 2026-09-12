# Windows Lightmap Bake Measurement Runbook

Use this runbook on the Windows measurement computer to collect the remaining
acceptance evidence for the incremental lightmap bake plan. It uses **GitHub
Desktop** for branch switching; PowerShell runs only the Rust build and bake.

## What this measures

The test bakes `stress-warren-hallway-inspection.map` at the default `0.04`
texel density through the cold, shippable compiler path. It records:

- Peak working set for the pre-change and post-change compiler processes.
- SHA-256 hashes of their emitted `.prl` files, which must match.
- A second post-change output hash, which must match the first post-change
  output and proves repeat determinism.

The `cargo build` steps happen before measurement. The measurements invoke
`prl-build.exe` directly, so Cargo compilation memory is not included.

### Host-memory prerequisite

This map's SH-delta stage estimates a 36.5 GiB dense working set before it
reaches the lightmap bake. The compiler's normal 16 GiB SH-delta safety gate
therefore refuses the map before any useful lightmap measurement. The commands
below explicitly raise that *unrelated* gate to 48 GiB. Run this test only on a
machine with at least 64 GiB installed RAM and substantial free memory; the
flag permits the stage, it does not allocate memory or make an undersized
machine safe. If the bake fails or the system pages heavily during the SH-delta
stage, stop and report that result rather than changing density or other bake
settings.

## 1. Fetch the two branches in GitHub Desktop

1. Open the PostRetro repository in GitHub Desktop and choose **Fetch origin**.
2. In the **Current Branch** menu, select
   `test/lightmap-bake-before-incremental-flush`.
3. Use **Repository → Open in Terminal** to open PowerShell in the checkout,
   then build the pre-change executable:

```powershell
cargo build --release -p postretro-level-compiler
```

4. Return to GitHub Desktop and select
   `feature/lighting-scale--lightmap-bake-incremental-flush`.
5. Use **Repository → Open in Terminal** again, then build the post-change
   executable:

```powershell
cargo build --release -p postretro-level-compiler
```

Leave GitHub Desktop on the feature branch before continuing.

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
        -ArgumentList "$map -o `"$out`" --release --sh-delta-working-set-max-size 48GiB" `
        -NoNewWindow -PassThru -Wait
    $stopwatch.Stop()

    $file = Get-Item $out
    [pscustomobject]@{
        Label = $Label
        ExitCode = $process.ExitCode
        Seconds = [math]::Round($stopwatch.Elapsed.TotalSeconds, 1)
        PeakWorkingSetGiB = [math]::Round($process.PeakWorkingSet64 / 1GB, 2)
        OutputGiB = [math]::Round($file.Length / 1GB, 3)
        Sha256 = (Get-FileHash $out -Algorithm SHA256).Hash
        Output = $out
    }
}
```

## 3. Measure the pre-change baseline

In GitHub Desktop, switch back to
`test/lightmap-bake-before-incremental-flush`, then choose **Repository → Open
in Terminal**. Paste the helper from step 2 if this is a new terminal, then run:

```powershell
$before = Invoke-LightmapBake "before"
$before
```

Record the result row. If this bake cannot complete because of memory pressure,
record that fact and any displayed error; do not lower density.

## 4. Measure the post-change branch twice

In GitHub Desktop, switch to
`feature/lighting-scale--lightmap-bake-incremental-flush`, then choose
**Repository → Open in Terminal**. Paste the helper again if needed, then run:

```powershell
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
