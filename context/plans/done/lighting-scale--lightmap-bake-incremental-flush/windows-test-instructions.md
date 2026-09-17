# Windows Lightmap Stage-Reach Diagnostic

Use this runbook on the Windows measurement computer to verify that a cold bake
gets through `LightmapBake`. It uses **GitHub Desktop** for branch switching;
PowerShell runs only the Rust build and bake. It is not a full compiler memory
or `.prl` identity gate.

## What this measures

The test bakes `stress-warren-hallway-inspection.map` at the default `0.04`
lightmap texel density through the cold, cache-free compiler path. It uses a
test-only `4.0 m` SH probe spacing (the normal spacing is `1.0 m`) so the
unrelated direct-SH-delta stage fits on the 16 GiB Windows host. It records
the last named compiler stage and any failure after it.

Each `cargo build` happens immediately before its matching run. The helper
invokes `prl-build.exe` directly, so Cargo compilation memory is not included.

### Host-memory prerequisite

At the normal `1.0 m` spacing, this map's SH-delta stage estimates a 36.5 GiB
dense working set before it reaches the lightmap bake, so the normal 16 GiB
safety gate correctly refuses it. Increasing the spacing to `4.0 m` reduces
the three-dimensional probe grid by roughly 64x while leaving the lightmap
layout and its `0.04` density unchanged. This is deliberately a
lightmap-focused stress profile, not a production-quality SH bake. A failure
after `LightmapBake`, such as `ShadowmaskAtlas`, is outside this plan's scope.

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

Record the result row and the last named compiler stage. If it fails after
`LightmapBake`, record the error; do not lower the lightmap density or raise
the SH working-set limit.

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

"Pre/Post identical: $($before.ExitCode -eq 0 -and $after1.ExitCode -eq 0 -and $before.Sha256 -and $before.Sha256 -eq $after1.Sha256)"
"Post deterministic: $($after1.ExitCode -eq 0 -and $after2.ExitCode -eq 0 -and $after1.Sha256 -and $after1.Sha256 -eq $after2.Sha256)"
```

If `$before` came from another terminal, paste its SHA-256 value into the
comparison instead:

```powershell
$beforeSha256 = "PASTE_THE_BEFORE_SHA256_HERE"
"Pre/Post identical: $([bool]$beforeSha256 -and $after1.ExitCode -eq 0 -and $after1.Sha256 -and $beforeSha256 -eq $after1.Sha256)"
"Post deterministic: $($after1.ExitCode -eq 0 -and $after2.ExitCode -eq 0 -and $after1.Sha256 -and $after1.Sha256 -eq $after2.Sha256)"
```

## 5. Send back these results

Send the three printed result rows and the two Boolean lines. Include any bake
error verbatim and name the last compiler stage displayed. Empty hashes are
not a successful comparison.

Do not delete the output files until the hashes have been reviewed.
