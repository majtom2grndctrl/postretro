# fluid-surface-motion — plan of record

mode: compact
status: active
read at: 06237f700

## Corrections
- Source moved since `e8f607b94` (vault landed: `physics::movement` dispatch, substrate, `collision/mod.rs`). `movement/push.rs`, `collision::resolve`, `acceleration.rs`, `environment_install.rs`, the fluid shader, the compiler surface stage and the format are unchanged, so every Decision read still holds.
- "Each face takes the nominal push winning `resolve_environment`" → the nominal push is the point rule's winner, which `resolve_particle_acceleration` already returns (`resolve_environment` calls the same reduction). Drift samples that function; `resolve_environment`'s `push` becomes the felt push (`push × force`), so every movement read (lift-off, drift accrual, terminal clamp, fluid-jump check in `swim_transition`) sees force without per-site edits. Particles keep the nominal push. Same meaning: two outputs, nominal and felt.
- "Hook drift [in `refresh_environment_fluids`] before its early return" → the refresh is restructured so drift re-resolves on every call, after any volume rebuild, reading viscosity from the table by fluid name. Same meaning, and the rebuilt volumes are never stale at that point.
- Drift precision (R9): the uniform clock is `script_time` as f32, shared with forward and billboard, and no binding may be added. Drift therefore rides the vertex as a rate in texture periods per second; the vertex shader wraps `time × rate` with an exact split-mantissa product and passes the per-face scroll flat, and the fragment floors it to whole texels. No uniform or binding changes.
- R6 (sliver UV) → a face's world-to-texel map is taken from the largest face of its volume sharing its texture, plane and projection (a split part fits its parent's map within 1 mm and 0.01 texel), so every part of one split face converts drift with one map.
- R14 → drift lives in a second vertex buffer the re-resolve rewrites in place; install, slots and parameter rows are untouched by a re-resolve.
- Terminal velocity source → a connected client reads its installed tuning payload's movement descriptor; host and offline read the local player's descriptor (the pawn's class, else `player`) from the data registry. No movement descriptor means no swimmer: drift takes no cap.
- Fixed tick → `frame_timing::TICK_DURATION` (16 667 µs), the dt movement integrates with.
- Fixture → the e24 map gains an east wing through a doorway from the north hall: still, current and scale-0.25 pools with grain-marker walls, a two-brush pool under a partial push, a force-0 bending river and a force-3 trap, textured from a new procedural set (`content/dev/textures/fluids/`, generator `tools/gen_fluid_textures.py`). Existing areas are unchanged.

- Fade crossings (review round 1) → the shader decides fade per fragment from that fragment's texels per pixel, with no hysteresis (a stateless shader has none), so camera motion can flatten or restore a cell off its step boundary. Clarification, same meaning: the 3 Hz bound governs the animation clock at a fixed view; a camera-driven fade crossing is a view change, like a mip or LOD transition, and happens only where texels are about a pixel. Tests pin the bound at a fixed view. Flagged to the owner in the report.
- Face split (review round 1) → the compiler cuts each fluid face against push planes shifted to the runtime's 2 mm drift sample depth, so a part's split and its runtime sample always agree (near-flush and oblique planes included), and closes the T-junctions splitting and burial leave between a volume's pieces, since the surface is translucent.

## Delegated answers
- Shimmer step rate, ring density and lifetime, shimmer amplitude, fade threshold — recorded in *Tuning* after the first capture.

## Tuning
First tuning by capture on the 1660 Super; the owner tunes by eye in-engine (M1–M4). Every value lives in `render-cpu::fluid_surface::motion`, mirrored by `fluid_surface.wgsl` and pinned by `fluid_motion_shader_constants_match_the_cpu_reference`.
- Step rate 2.5 Hz (a change at most every 0.4 s, under the 3 Hz threshold with margin).
- Shimmer: ±1 texel along a fixed per-cell direction (one of nine, zero included), cycle `0, +d, 0, −d`. A per-cycle direction was tried first and failed the 0.05-texel mean row: a 10 s window could catch an unmatched `−d` at one end and an unrelated `+d` at the other (worst mean 0.08). Fixed direction bounds any window to one step (0.04).
- Rings: one drop per 32-texel block per 5-step lifetime (2 s), radius growing 2 texels per step to 8, one texel thick, pushing cells one texel outward. Drops land ≥ 9 texels inside their block, so a ring never leaves it and a cell evaluates one block. Every block drips each lifetime, which makes "steps n and n + 1 differ in a 64×64 grid" hold by construction; a 50 % drop chance left four-block grids with unchanged steps.
- Fade: per-cell threshold hashed across 0.75–1.5 texels per pixel.

## GPU timing (M6)
Capture measurement (`postretro.capture.measurement.v3`), e24 pool room, camera 2.0 m engine-Y over the pool looking down (pitch −89°), 1920×1080, 240 warmup + 1200 samples, `POSTRETRO_SH_STREAMING=sync-proof`, Vulkan, GTX 1660 SUPER, dev profile with `capture,dev-tools`. `fluid_surface` pass, mean of 10 windows:
- Before (sine warp): 3.73 ms (3.68–3.80).
- First build, 3×3 ring neighbourhood: 4.68 ms. Reach-culled neighbourhood: 4.20 ms. Diagnosis: cell offset off 3.63 ms; shimmer only 3.67 ms; rings were the cost.
- After (rings confined to their own block): 3.77 ms (3.77–3.84), +0.04 ms.
The owner's in-engine `POSTRETRO_GPU_TIMING=1` read remains manual.

## AC-to-proof

| AC | Proof | Status |
|---|---|---|
| A1 shader mirrors CPU reference; `fs_main` samples at warped UV | `fluid_motion_shader_constants_match_the_cpu_reference`, `fluid_fs_main_samples_albedo_and_emissive_at_the_warped_uv` (renderer) | achievable as stated |
| A2 textual: no `sin(`/`cos(`; cells floored from unwarped UV; offsets floored before moving the sample | `fluid_surface_wgsl_*` textual tests on comment-stripped source (renderer) | achievable as stated |
| A3 shimmer/rings vs GPU-free reference (whole texels, step boundaries ≤ 3 Hz, ≤ 10 % cells per frame incl. negative coords, zero-mean, amplitude caps, fade, fade sweep, ring determinism/variation/no tile repeat) | `render-cpu::fluid_surface::motion` tests | achievable as stated |
| A4 drift scroll direction/speed at scale 1, 0.25, 45°; reversal; 24 h f32; wrap leaves cells unchanged | `render-cpu::fluid_surface` drift tests over the CPU mirror of the vertex wrap | achievable as stated |
| A5 install drift resolve per face (cover, outside touch, two pushes, vertical faces, e24 channel 3 m/s +Y) | `postretro::fluid_surface_drift` tests; e24 row compiles the fixture map | achievable as stated |
| A6 face splitting (half cover, flush-from-below vs above, whole cover, overlap, touches/slivers/area sum, thin parts) | `level-compiler` surface split tests (+ drift identity in `render-cpu`) | achievable as stated |
| A7 drift speed (water/sludge swimmer within 2 %, viscosity 120 steady state, terminal cap, two strengths, zero viscosity) | `physics::movement::push` steady-state tests with a swimmer harness | achievable as stated |
| A8 re-resolve (viscosity-only, capture zero drift at capture opacity, client before tuning / after demotion, mid-level re-resolve uploads no rows) | `postretro` environment-install/drift tests; render-cpu slot test | achievable as stated |
| A9 `force` (default 1, force 0 drift/feel/particle, 0.5 and 3, force-0 updraft and fluid jump, host/prediction/replay under 0.5, bad values fail naming entity) | compiler parse tests, collision/physics/particle tests, netcode predict-reconcile harness | achievable as stated |
| A10 digest | `runtime_movers` digest tests | achievable as stated |
| A11 force section (absent, pre-change map, round trip, validation, env bytes identical) | level-format + level-loader + compiler tests | achievable as stated |
| A12 no reduce-motion/accessibility/render-profile input (grep gate) | textual test on `fluid_surface.wgsl` and the drift module | achievable as stated |
| A13 fluid/mover Post Retro parity test unchanged | `fluid_post_retro_sampling_matches_the_mover_shader` | achievable as stated |
| A14 pipeline budget tests; no binding added | existing budget tests | achievable as stated |
| A15 DX12 FXC shader test on the 1660 Super, no skip line | existing on-demand FXC test, run here | achievable as stated |
| M1 still pool vs current; carried at surface speed | owner, in-engine | manual |
| M2 force-0 river, force-3 pool, bending river | owner, in-engine | manual |
| M3 grain matches walls at 1 and 0.25; no border smear; distant calm | owner, in-engine | manual |
| M4 view-filling still pool reads gentle | owner, in-engine | manual |
| M5 side face through glass drifts without tearing | owner, in-engine | manual |
| M6 GPU timing fluid pass before/after | executor runs `POSTRETRO_GPU_TIMING=1` on the 1660 Super and records here; owner confirms view | manual |

## Tasks

| # | Task | Owner | Depends on | Status |
|---|---|---|---|---|
| 1 | Shimmer + rings + fade: CPU reference (`render-cpu`), shader warp, parity and textual tests; first capture at scale 1 and 0.25 to tune constants and check cell borders | integrating executor | — | done: render-cpu motion tests (30 with drift), renderer fluid tests (18, incl. GPU frame test) |
| 2 | `force`: compiler parse, format section 53, loader validation, collision force + felt push, physics adapter, digest, FGD + docs | integrating executor | — | done: compiler, format, loader, collision, physics, netcode, digest tests; FGD + docs |
| 3 | Steady-state drift function in `physics::movement::push` + swimmer settle tests | integrating executor | 2 | done: swim_tests steady-state rows |
| 4 | Compiler face split along push brushes | integrating executor | — | done: surface_tests split rows + e24 fixture test |
| 5 | Drift: second vertex buffer, face map clusters, app resolve at install and every refresh, shader scroll; tests | integrating executor | 1, 3, 4 | done: render-cpu drift tests, postretro drift + install tests |
| 6 | Fixture map: still/current pools, partial push, force-0 bending river, force-3 trap, scale 1/0.25 beside walls | integrating executor | 2, 4 | done: e24 east wing (six pools) with fluids/ textures |
| 7 | GPU timing before/after, preflight, review loop | integrating executor | 1–6 | in progress: see Review loop |

## Review loop
- Round 1 (10 lenses, 3 slices + seam): 2 🔴 — drift write batched at install/commit points (debug panic on any water map load or reload), missing `load-prl` cfg on a loader initializer. 🟡: compiler/runtime classification band, absolute face-map fit tolerance far from the origin, split T-junctions, fade per fragment (clarified above), test gaps (adapter force, terminal-velocity source, section 53 through a real compile, netcode half force), doc and comment drift. All fixed; focused gate green (14 runs).
