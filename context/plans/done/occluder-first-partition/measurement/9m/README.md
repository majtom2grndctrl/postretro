# occluder-first-partition: Task 9m, street render-submit diagnosis

Recorded 2026-10-29, 10:04–10:48 PDT, on `claude/occluder-first-partition` at HEAD `69568540f`. No source changed, and `git status --short` shows only this folder. Machine: MacBookPro16,1, Radeon Pro 5300M, Metal, freshly rebooted, screen unlocked. One `caffeinate -dimsu` ran for the whole session. Every sentinel (12 of them) read 16.51–16.54 ms, so no run was in the slowdown regime.

## Answer

**The street's render-submit rise is CPU encoding of more indirect draws, one Metal draw per drawn BVH leaf per pass. Almost all of its size comes from a dev-build-only cost: objc2's `debug_assertions` check on every Objective-C message send.**

- **Partition side.** The branch's partition draws 2.5–2.7× more BVH leaves ("slots") at these poses. That comes from fewer, larger drawn cells holding more faces each, not from more cells. At `rd:elevated` the drawn cells go 261 → 374 and the faces 1,270 → 3,103. At `rd:promenade` the cells go 206 → 287 and the faces 1,019 → 2,770. Faces per drawn cell go from 4.9 to 8.3–9.7.
- **Renderer side.** These paths predate this branch:
  - `render_submit` holds `encoder.finish()`, and wgpu 30 defers all command encoding to `finish()`.
  - wgpu-hal Metal expands each `multi_draw_indexed_indirect` into one `drawIndexedPrimitives` per slot (`wgpu-hal-30.0.1/src/metal/command.rs:1608`).
  - The camera span list is drawn twice, in the depth pre-pass and in forward.
  - In the `dev` profile, objc2 0.6's `msg_send_check` (`#[cfg(debug_assertions)]`, `objc2-0.6.4/src/runtime/message_receiver.rs:294`) looks up the method and parses its type encoding on every message.
- **The draw cost itself is small.** The AMD driver's own draw call is about 6% of the per-draw cost.

The candidates the handoff named are ruled out live:
- **SH compose stall.** At both poses, on both sides, compose fires 0 rows. The SH log reads `compose frame: indirect 0 rows/0 dispatches … animated-direct 0/0`.
- **Uploads.** Both sides make 32 staged writes per frame. Bytes rise 76,912 → 84,504 B, and that +7,592 B is the candidate leaf list (4 B × ≈1,900 slots).
- **Upload copies on the GPU.** The live batch's traces show 0.11 → 0.13 ms per frame, with no stall.
- **Lightmap residency.** `lightmap_residency` holds at 0.011–0.014 ms.
- **Walk.** The walk is flat.

## Evidence

**1. The rise scales with slots (`records/submit-vs-slots.txt`).** Each of the live batch's 45 runs was paired with its pose's drawn leaves, using `world_poses_ray_sweep` at the live eye (the eye whose `considered` count matches the live run). Across 2 maps and 3 sides:

`render_submit` = 4.56 ms + 1.80 µs × slots, R² 0.977.

1.80 µs per slot is two draws, at about 0.9 µs each. `rd:upperdeck` is flat because its slots are flat (1,051 → 1,315). `rd:plaza` rises by its +556 slots.

**2. Time Profiler, live, attached for 6 s (`records/time-profile.txt`).** Values are ms per frame on the main thread, before → cap26.

| Bucket | `rd:elevated` | `rd:promenade` |
|---|---|---|
| encoder `finish` (in `render_submit`) | 5.65 → 8.26 | 5.37 → 7.60 |
| hal Metal `draw_indexed_indirect` | 2.05 → 4.78 | 1.81 → 3.98 |
| objc2 `msg_send_check` inside that draw | 1.89 → 4.48 | 1.69 → 3.65 |
| AMD driver `drawIndexedPrimitives` | 0.15 → 0.27 | 0.11 → 0.31 |
| wgpu indirect validation | 0.37 → 0.45 | 0.32 → 0.48 |
| `queue_submit` | 0.58 → 0.78 | 0.70 → 0.73 |

**3. Cheap A/Bs on the same poses (`records/stages.txt`).** Values are `render_submit` in ms, before → cap26, with the delta in brackets.

| Build or switch | `rd:elevated` | `rd:promenade` |
|---|---|---|
| frozen H3 binary, default | 6.93 → 9.65 (+2.72) | 6.55 → 8.81 (+2.26) |
| `WGPU_VALIDATION_INDIRECT_CALL=0` | 6.44 → 8.88 (+2.44) | 6.18 → 8.20 (+2.02) |
| HEAD dev build (control) | 6.84 → 9.62 (+2.79) | 6.56 → 8.97 (+2.41) |
| HEAD with `--config 'profile.dev.package.objc2.debug-assertions=false'` | 4.09 → 4.34 (**+0.25**) | 4.02 → 4.34 (**+0.32**) |

With objc2's checks off, work CPU rises +1.27 ms (elevated) and +1.17 ms (promenade) instead of +3.6 and +3.1. The remainder splits as follows:
- `render_submit` +0.25–0.32 ms: residual encoding, about 0.13 µs per slot.
- `render_prep` +0.6 ms. About half of it is dev-tools `refresh_camera_cull_diagnostics` (0.55 → 0.83 ms in the profile). About +0.3 ms is SH streaming `prepare_drains`/`update_targets`, whose SH clusters rise 77 → 111.
- `rec_cull_diagnostics` +0.25–0.4 ms, dev-tools only.

The dev-tools costs scale with drawn leaves too.

## Fix candidates (smallest first)

1. **Workspace `Cargo.toml` (neither the compiler nor the renderer; outside F3).** Add `[profile.dev.package.objc2] debug-assertions = false`. It removes about 2.5 ms of the rise and about 2.7 ms of base submit cost on every map in the standard dev build. Release never had either cost. Blast radius: Mac dev builds lose objc2's message-signature checks, a debugging aid for hand-written `msg_send!`, which this workspace does not write. Measured above.
2. **Compiler only: fewer slots.** Options:
   - merge faces of the same cell and material bucket into one BVH primitive (`bvh_build.rs`: one leaf per face today; the street cap26 PRL has 10,534 faces and 10,534 leaves);
   - `face-fragment-remerge`;
   - bounding the facade slab cells (`../h3/street-gpu.md`).

   Each cuts CPU draws and GPU per-draw cost alike. Not measured; merging leaves coarsens GPU culling and touches the animated-light chunk ranges per leaf.
3. **Renderer (F3).** Fewer draw calls per slot on Metal, through compacting visible slots, an indirect command buffer, or skipping the depth pre-pass span. This is a follow-up.

## Not determined

- **Release-build cost.** No release binary was built. The objc2 checks and indirect validation are absent there, so the residual is expected near the nd build's +0.25–0.32 ms, but that was not measured.
- **The 4.0 ms base `render_submit`** in the objc2-off build was not broken down.
- **Repeats.** Each A/B cell is one run of 6–13 windows. The frozen H3 binary and the HEAD control agree within 0.1–0.16 ms on each side.

## Method and files

- **Gate and sentinel.** The thermal gate ran at 67 °C, not 57 °C: idle after the reboot sat at 62–65 °C (GPU utilization 4–13%), so a 57 °C gate never opens. The sentinel ran at the start, every 6 runs and at the end.
- **Runs.** Each was 2 settle windows plus 6 kept, no GPU trace. Time Profiler runs attached for 6 s after the CPU windows.
- **PRLs and runner.** The live batch's PRLs and `run_live.sh`.
- **Binaries.** Frozen H3 `9164ab4a6432`. HEAD `3cbb569665d2` and HEAD with objc2 checks off `60caea979bc0` are in `target/occluder-9m/`. Building them dirtied no source, and the tree was rebuilt back to the default profile.
- **`tools/`:**
  - `drive.sh`: the driver, with modes `def`, `val0`, `tp`, `tpval0`, `head`, `nd`;
  - `run_tp.sh`: Time Profiler variant of `run_live.sh`;
  - `run_bin.sh`: `run_live.sh` with a selectable binary;
  - `fit.py`: the slot fit (reads the sweep records in the session scratchpad; the per-pose rows are copied in `records/sweep.*.txt`);
  - `tpcmp.py` and `tpparse.py`: the time-profile reducers.
- **`records/`:**
  - `9m-{pilot,main,objc2}/`: status, `cpu.txt` and `meta` per run;
  - `stages.txt`, `submit-vs-slots.txt`, `time-profile.txt`.
- **Not kept.** Engine logs (in `target/occluder-h3live/runs/9m-*`) and the time-profile XML exports (session scratchpad).
