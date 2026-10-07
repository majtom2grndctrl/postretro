# Signed surface height — Windows GPU check (GTX 1660 Super)

> **Read this when:** measuring `feat/signed-surface-height` against `main` on the Windows perf-floor GPU.
> **Key question:** does the branch cost more than `main` with Surface Depth **Off**? Off is the fallback for weak machines and must not regress. On cost is secondary.
> **Related:** `signed-surface-height-contract.md` (Track B GPU result table), `context/lib/rendering_pipeline.md` §12.

The 1660 Super supports timestamp queries, so the engine's own `POSTRETRO_GPU_TIMING` works here. No Instruments equivalent needed.

## Setup

Two worktrees side by side, so switching builds never triggers a rebuild. Run in PowerShell from the main checkout:

```powershell
git fetch origin
git worktree add ..\pr-main origin/main
git worktree add ..\pr-branch origin/feat/signed-surface-height
```

In **each** worktree, build once and bake the map:

```powershell
cargo build --release -p postretro --bin postretro --features capture
cargo build --release -p postretro-script-compiler --bin scripts-build
cargo run --release -p postretro-level-compiler -- content/dev/maps/campaign-test.map -o content/dev/maps/campaign-test.prl
```

- Every run below reuses this one release + `capture` artifact, so no run triggers a rebuild.
- The map bake is the slow step (minutes).
- Every command below runs from inside the worktree being measured.

Before each timing run:
- close other GPU work (browser video, games, overlays);
- plug in AC power and use the High performance power plan;
- disable vsync in the GPU driver for `postretro.exe` if the frame rate is capped. A capped, mostly idle GPU drops its clocks and adds noise; this was the main noise source on the Mac.

## Part 1 — spawn view, Off and On (primary)

Settings file: `%APPDATA%\postretro\config\settings.toml`. Create it if missing. Set one line:

```toml
surface_depth_quality = "off"   # or "on"
```

For each of the four combinations — {main, branch} × {off, on}:

```powershell
$env:RUST_LOG = "info"; $env:POSTRETRO_GPU_TIMING = "1"
cargo run -p xtask -- run --release --features capture -- content/dev/maps/campaign-test.prl --windowed
```

- Don't touch the mouse or keys after the level loads. The spawn view is identical between builds only while the camera stays put.
- GPU timing logs one line per 120-readback window. Wait for at least 5 windows, then quit.
- Record the `forward` average from windows 3–5, ignoring the first two as warm-up.
- Run each combination 3 times, interleaving the builds: main-off, branch-off, main-on, branch-on, and repeat.

## Part 2 — concrete walkway, On only (secondary)

This is the pose the Mac measurements used. Capture mode doesn't read `settings.toml`, so it always renders Surface Depth On. Save as `e1.scene.json` in each worktree's root:

```json
{
  "map": "content/dev/maps/campaign-test.prl",
  "camera": { "position": [-3.048, 1.524, -46.3296], "yaw_deg": 90.0, "pitch_deg": -18.0, "fov_deg": 90 },
  "resolution": [1920, 1080],
  "output": "e1.png",
  "measurement": { "report": "e1.json", "warmup_frames": 120, "sample_frames": 1200 }
}
```

```powershell
$env:POSTRETRO_GPU_TIMING = "1"
cargo run --release -p postretro --bin postretro --features capture -- --capture e1.scene.json
```

Run it 3 times per worktree, alternating between them.

Read `forward` from `gpu_timing.windows` in `e1.json` (average over windows), and keep `e1.png` from each build. In the branch image the stone tops should shift toward the camera compared with `main` (the concrete's stones now rise), and the step from mortar to stone top should read taller.

## What to report back

| Build | Part 1 Off `forward` ms (3 runs) | Part 1 On `forward` ms (3 runs) | Part 2 On `forward` ms (3 runs) |
|---|---|---|---|
| main | | | |
| branch | | | |

Also report:
- GPU driver version;
- whether vsync was off;
- anything visibly wrong in the branch's `e1.png` or at spawn.

Mac reference, one pose and noisy: Off ≈ 4.7 ms, `main` On ≈ 5.0 ms, branch On ≈ 5.3 ms at 1280×720 (Track B table in the contract).
