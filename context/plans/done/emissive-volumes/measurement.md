# emissive-volumes — measurement procedure

Procedure for the brief's Measured rows Ms1, Ms2 and Ms4. Ms3 (bytes uploaded) is test-gated; the
`emissive_uploaded` count below also reads it in-engine. The first frame after a level load uploads
all 257 entries, a full re-send by design, so Ms3's static-frame zero excludes that frame. Every
run is report, not gate: a worse result is the owner's cue to weigh another implementation
(`index.md` §Measured). Nothing here has been run. Each run needs an idle machine and the owner's go-ahead.

## Fixed for every run

- **Machine class.** Mac target: Radeon Pro 5300M/5500M-class discrete GPU, Metal. This class lacks
  the timestamp features, so `POSTRETRO_GPU_TIMING=1` reports unsupported. GPU time comes from an
  Instruments Metal System Trace (`rendering_pipeline.md` §12, Without timestamp support).
- **Machine state** (`rendering_pipeline.md` §12, Machine-state confounders). Nothing else builds,
  bakes or renders. Before each batch, record idle VRAM and GPU utilisation from
  `ioreg -c IOAccelerator`. During each capture, poll `Core Clock(MHz)` and `Temperature(C)`. Run
  `caffeinate -dimsu` across the batch. Keep the engine window in front with the screen unlocked; a
  screen saver or a locked screen stalls or skews a windowed run.
- **Build.** `--profile profiling --features dev-tools`, the same profile for HEAD and baseline.
  Record the build SHA, surface and scene extents, render divisor and window mode, which the renderer
  logs after init. Use a resolution that saturates the GPU (native 1440p or above, divisor 1).
- **Pilot first.** Run one configuration end to end and check the trace reduces before starting a
  batch. Abort the batch at the first invalid run.

## Fixture (Ms1, Ms2)

`tools/gen_emissive_stress.py` writes one sealed room. The player stands on a pedestal between two
16 × 16 panel grids, each 768 × 768 units at 240 units:

| View | Yaw | Contents |
|---|---|---|
| Glow | 90 | 256 panels in `neon/glow_panel` (strength 4.0, `content/dev/materials.toml`), filling the view |
| No-`_e` | −90 | The same 256 panels in `metal/metal_rough_046a`, which has no `_e` map |

Both poses share one point: `--start-pose=-6.30,9.96,-9.75,90,0` (glow) and
`--start-pose=-6.30,9.96,-9.75,-90,0` (no-`_e`). Variants differ only in their trailing volume
entities:

| Map (`content/dev/maps/`) | Volumes | Animating | Data script |
|---|---|---|---|
| `emissive-volumes-stress-0.map` | 0 | 0 | none |
| `emissive-volumes-stress-1.map` | 1 around every glow panel | 0 | none |
| `emissive-volumes-stress-256.map` | 1 per glow panel | 0 | none |
| `emissive-volumes-stress-256-anim16.map` | 1 per glow panel | 16, the grid diagonal, pulsing 0 → 1 over 1.5 s | `scripts/emissive-volumes-stress.ts` |

Regenerate with `python3 tools/gen_emissive_stress.py --volumes {0,1,256} [--animate 16]`. The first
variant compiles cold in about 6 s, the rest warm in about 1 s. The one expected warning is the room's SH cluster singleton:
cell 21 holds 592 primitives against a limit of 64. The room is one convex cell by design. Compile
each variant with:

```bash
cargo run -p postretro-level-compiler -- content/dev/maps/emissive-volumes-stress-<variant>.map \
  -o content/dev/maps/emissive-volumes-stress-<variant>.prl
```

**Baseline.** The map before this brief, compiled and run at `c22e8757a`. That commit has no
`emissive_volume` and no `materials.toml`, and the bright panel is `neon/neon_glow_panel` there,
with prefix strength 4.0. So the baseline holds only the 0-volume map. Its comparable views are the
0-volume glow view and the no-`_e` view; HEAD's 1-volume and 256-volume runs compare against HEAD's
own 0-volume run and against the baseline.

1. Make a worktree at `c22e8757a` and build a lean `prl-build` and engine there, then copy the
   binaries out and delete its target (`development_guide.md` §Worktree builds). Check free disk
   space first.
2. From HEAD, write the baseline map into the worktree's content tree, so its textures resolve:
   `python3 tools/gen_emissive_stress.py --volumes 0 --glow-texture neon/neon_glow_panel -o <worktree>/content/dev/maps/emissive-volumes-stress-baseline.map`.
3. Compile it with the baseline `prl-build`, and run it with the baseline engine at the same poses.

## Ms1 — forward-pass GPU time

Ten configurations: {baseline 0, HEAD 0, HEAD 1, HEAD 256, HEAD 256-anim16} × {glow, no-`_e`}.

1. Launch windowed:
   `RUST_LOG=info cargo run -p xtask -- run --profile profiling --features dev-tools -- content/dev/maps/emissive-volumes-stress-<variant>.prl --start-pose=<pose>`.
   Confirm the log reports GPU timing unsupported.
2. Wait about 10 s for streaming and shader warm-up, then
   `xcrun xctrace record --template 'Metal System Trace' --attach <pid> --time-limit 20s --output <dir>/<config>.trace`,
   with a watchdog on `xctrace`.
3. `xctrace export` the GPU intervals. Take `forward` as the union of its encoder's intervals, with
   nested and coalesced rows counted once, divided by the frame count from a once-per-frame encoder
   of the same pass. Never take the frame count from render-pass encoder counts.
4. Three runs per configuration, in interleaved order. Report the median and spread of per-frame
   `forward` ms.
5. Delete each `.trace` bundle and the `instruments*.ktrace` files in `$TMPDIR` once the export is
   parsed (about 1 GB each).

What each comparison isolates:
- No-`_e`, HEAD 0 vs baseline: the slot-2 volume-id stream every forward draw now binds, on faces
  that never read the volume table.
- Glow, HEAD 0 vs baseline: that stream plus the texture-strength path, still with no volume read.
- Glow, HEAD 1 and HEAD 256 vs HEAD 0: the volume-table read inside the `emissive_strength > 0`
  branch, for one table entry and for 256 different entries.
- Glow, HEAD 256-anim16 vs HEAD 256: 16 changed table entries uploaded each frame.

## Ms2 — CPU per-frame volume update

`POSTRETRO_CPU_TIMING=1` adds two rows under `render_prep` in the `[CpuTiming]` log line, the
telemetry `cpu_timing_window` event and the Performance tab:

- `emissive_update`: the emissive bridge over every volume (brightness curve sampling, the flash
  limiter, table packing), then sending the changed runs to the renderer.
- `emissive_uploaded`: volume-table entries sent this frame, zero included.

Run `emissive-volumes-stress-256-anim16.prl` at the glow pose with
`RUST_LOG=info POSTRETRO_CPU_TIMING=1 POSTRETRO_TELEMETRY=<dir>/ms2-<limiter>.jsonl`, using the
Ms1 launch line. Run once with the flash limiter on (the default) and once with it off. Toggle it in
Options › Accessibility, persisted as `accessibility.flash_limiter` in `settings.toml`. Restore it
afterwards. Collect at least five 120-frame windows after the first, per setting. Report the
`emissive_update` avg and max (ms) and the `emissive_uploaded` avg: expect 16 with the limiter off.
`python3 tools/telemetry_reduce.py <file>` gives the window percentiles. As controls, the static
`emissive-volumes-stress-256.prl` should read `emissive_uploaded` 0 after its first frame (Ms3), and `-0.prl` gives the
update's floor with no volumes. The baseline has no such stage, so Ms2 has no baseline row.

## Ms4 — bake stage time

The Build Summary gains two rows that are not stages. Each runs uncached on every build:
- `  materials.toml`: a sub-line of `Materials`, the read and parse of the mod's `materials.toml`.
- `Emissive Membership`: face-to-volume membership, after `TextureMips`.

The text summary prints whole hundredths of a second, so read milliseconds from the telemetry
`bake_summary` rows (`stage: false`). Fixture: the hallway map,
`content/dev/maps/stress-warren-hallway-inspection.map`, which has no emissive volumes, so
membership times its no-volume walk. Add `emissive-volumes-stress-256.map` for the 256-volume
cost. Inputs: default `-j` (record the worker count), default `--baked-root`, and the dev mod's
`content/dev/materials.toml`.

- **Cold:** `POSTRETRO_TELEMETRY=<dir>/ms4-cold.jsonl prl-build <map> --no-cache -o <out>.prl`.
- **Warm:** build once to fill the cache, then run again with
  `POSTRETRO_TELEMETRY=<dir>/ms4-warm.jsonl` and the default cache.

Run each three times. Report the two rows' ms, and `Materials`, `TextureMips`,
`Reflection Probes` and `Total`, against the baseline `prl-build` at `c22e8757a`. That build has
neither row; compare its `Materials` and `Total`. A cold hallway bake is long and saturates every
core, so run it on an idle machine and nothing beside it. Delete the `--no-cache` outputs and the
telemetry files afterwards.
