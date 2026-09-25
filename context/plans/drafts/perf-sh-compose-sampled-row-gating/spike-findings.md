# Spike findings: streamed SH compose cost at 1 m (perf-sh-compose-sampled-row-gating)

Worktree: `C:\Users\danhi\Projects\Personal\postretro\.claude\worktrees\agent-aa6de6e4217c81417`, base 0218faacd, spike diff uncommitted.
Brief under test: `postretro-main-brief/context/plans/drafts/perf-sh-compose-sampled-row-gating/`.

## TL;DR

- **Q1: the direction holds.** Suspending animated SH compose at 1 m cuts frame time by 94–95% in capture (47.9 → 2.5 ms at spawn) and 90% live (41.9 → 4.1 ms). The gate was ≥15%.
- **Q2: the share shrinks at 3 m but stays material.** 42–44% in capture (3.7 → 2.1 ms), 19% live (5.0 → 4.1 ms).
- **The brief ranks its levers wrong.** Most of the 1 m cost is dispatch count, not row work. Collapsing each pass to one range (same rows) takes 1 m from 47.9 to 14.8 ms (capture) and from 41.9 to 12.9 ms (live). That is about 18–21 µs per range dispatch across 1,590 dispatches a frame. Row scoping alone reaches only 30.3 ms (capture) and 26.0 ms (live). At 3 m, row scoping is a net loss (3.67 → 4.72 ms) because it splits 8 ranges per pass into 35.
- **The cluster-grain gate barely helps; the cell-grain gate does.** The owner closure of 3 visible clusters is 19 of 22 installed clusters. Only 22 of 482 clusters are installed, yet they cover 91% of all affinity bricks. Rows near the visible cells, with a one-probe halo, come to 213, which is 5% of the row-scoped set. Emulating it gives 5.9 ms live and 7.2 ms in capture, against 4.1 and 2.5 ms suspended.
- Recommendation: keep the brief, but reframe it. Dispatch collapse (a gather or indirect dispatch, or tolerant merging) goes first or alongside, and comes out of Non-goals. The region-grain gate is the second lever. Row scoping is the smallest, and it must not ship without merging.

## Setup (pinned)

| Item | Value |
|---|---|
| Machine | Windows 11 Home 10.0.26200, 6 logical cores |
| GPU / backend | NVIDIA GeForce GTX 1660 SUPER, Vulkan, driver NVIDIA 617.14 (from logs; not 616.92 as recorded in memory) |
| GPU timing | off (`POSTRETRO_GPU_TIMING` unset; capture report `gpu_timing: not-requested`) |
| Build | release, `postretro --features capture`, commit 0218faacd + spike diff |
| Fixture | `stress-warren-mini.map`: 3 `brightness_curve` lights + 3 `warren_script_pulse` lights |
| Bakes | `prl-build` release, warm/default mode (approximate indirect), `--lightmap-density 0.16`, `--sh-probe-spacing 1.0` / `3.0` → `content/dev/maps/.spike-warren-mini-{1m,3m}.prl` (gitignored) |
| Bake time | 3 m cold (all stages, at density 0.08): 320 s. 1 m with the lightmap cached: 1,478 s (SH 778 s, delta 219 s, direct-delta 382 s). The final 0.16 rebakes reused the SH cache: 71 s each |
| Output size | 1 m: 347,137,098 B · 3 m: 79,799,436 B |
| Ids 49 / 50 | 1 m: 1.55 MB / 166.1 MB · 3 m: 0.29 MB / 15.1 MB |
| Instrument A | E20 capture measurement, 1280×720 offscreen, 120 warm-up + 600 sample frames, `cpu_completion` (device-poll wait after each submit), `POSTRETRO_SH_STREAMING=sync-proof` (required), pulse lights `force_active` at radiance 1.0 |
| Instrument B | live windowed release run, spawn pose only (no input possible), 1280×720 logical window, vsync forced off by the spike, default async streaming, 45 s runs, last 3 × 240-frame windows of wall-clock frame interval |
| Streaming budget / cache | defaults (renderer-snapshot budget). At 1 m: 484 MB active capacity, 22 target clusters, 0 evictions |
| Poses | spawn (16.26, 3.23, 65.02) yaw 0 · animroom (41.71, 3.23, −20.50) yaw 0 · floor = spawn position at pitch −89 (1 visible cell; animated rooms resident but not visible) |
| Repeats | 3 per spacing × condition × pose; ON/SUSP order alternated between reps |

Why capture is valid here: today's gate is flag-based (`descriptor_indices_have_active`). A frozen instant still dispatches every resident row every frame, so time 0 does not reduce the ON workload. The brief is right that capture cannot measure its own end state, because skip-when-unchanged composes nothing at a frozen time. Live runs confirm the direction; capture overstates the 3 m share because live frames carry more non-compose work.

## Honesty gates

| Gate | Result | Evidence |
|---|---|---|
| G1 streamed path | **pass** | Ids 49/50 present in both PRLs (section table). Capture report `streaming_lifecycle`: 22 sampleable, 460 absent. Legacy compose sits on its 1×1×1 dummy |
| G2 ON dispatches every frame; SUSP none after warm-up | **pass** | ON: every 120-frame window in the samples shows `frames_dispatched ind=120 A=120 B=120`. SUSP: `0 0 0` in every sample window (live too). The only SUSP dispatches are install-dirty rows during preload |
| G3 no vsync cap | **pass** | Capture has no present step. Every live log shows `[SPIKE live] vsync forced off`, and the splash reached 0.3 ms frames |
| G4 same pose, resolution, build | **pass, with note** | Same scene JSON and binary per compared pair. The binary was rebuilt twice to add lever modes; ON/SUSP controls on the final binary (reps 7–10) match the earlier runs (1 m ON 48.5–49.6, SUSP 2.51; 3 m ON 3.92–4.00, SUSP 2.14) |
| G5 delta vs spread | **pass** | 1 m: delta 45.3 ms vs spread ≤3.5 ms. 3 m: delta 1.55 ms vs spread ≤0.26 ms. Live 1 m: delta 37.8 vs 0.6. Live 3 m: delta 0.96 vs 0.20. Worst-case gap (min ON − max SUSP) stays positive in every cell |

## Q1 / Q2: suspend A/B

Capture `cpu_completion` median (ms). Each cell is the median of 3 run medians; spread = max − min of those run medians.

| Spacing | Pose | ON | SUSP | Delta | Compose share | ON p95 range |
|---|---|---|---|---|---|---|
| 1 m | spawn | 47.88 (3.20) | 2.55 (0.25) | 45.33 | **94.7%** | 49.8–62.4 |
| 1 m | animroom | 48.38 (3.18) | 2.22 (0.04) | 46.16 | **95.4%** | 51.7–62.3 |
| 1 m | floor | 49.51 (3.50) | 2.29 (0.21) | 47.22 | **95.4%** | 49.5–62.4 |
| 3 m | spawn | 3.67 (0.26) | 2.12 (0.05) | 1.55 | **42.2%** | 4.05–4.31 |
| 3 m | animroom | 3.47 (0.21) | 2.00 (0.02) | 1.47 | **42.4%** | 3.78–4.09 |
| 3 m | floor | 3.60 (0.23) | 2.01 (0.04) | 1.59 | **44.2%** | 3.94–4.36 |

Live spawn, frame interval median (ms):

| Spacing | ON | SUSP | Delta | Share |
|---|---|---|---|---|
| 1 m | 41.93 (0.58) | 4.11 (0.06) | 37.83 | **90.2%** |
| 3 m | 5.02 (0.04) | 4.06 (0.20) | 0.96 | **19.2%** |

Scaling: 1 m keeps 20.7× the rows of 3 m (8,127 vs 392 per pass) and costs 29× the compose time in capture. Compose cost is flat across poses because residency, not the view, sets the rows.

## Q3: lever sizing (rows / dispatches per frame, steady state)

"Current" is what 0218faacd encodes. "Scoped" means resident rows with ≥1 CSR entry for an active light: id 27 for indirect, id 45 plus Pass A rows for Pass B, and for Pass A only dirty rows (promotion weights never changed in these runs). "Cluster-gated" is scoped ∩ rows owned by the owner closure (`owner_dependencies`) of clusters holding visible cells. "Cell-grain" is scoped ∩ bricks overlapping visible-cell AABBs dilated by one probe spacing; it omits the node-origin writer closure, so treat it as a lower bound. Adding fog-reachable cells changed nothing: this map's fog-reachable set equals its visible set. Mover and mesh regions are omitted. Dispatches come after contiguous coalescing.

**1 m** (affinity grid 32×5×56 = 8,960 bricks; 482 clusters)

| Pose | Visible cells / clusters → closure | Resident rows (ind = A = B) | Current ind / A / B | Scoped ind / A / B | Cluster-gated ind / B | Cell-grain ind / B |
|---|---|---|---|---|---|---|
| spawn | 13 / 3 → 19 of 22 installed | 8,127 | 8,127·530 / 8,127·530 / 8,127·530 | 4,214·419 / 0·0 / 3,586·320 | 3,733·421 / 3,105·322 | 213·27 / 213·27 |
| animroom | 8 / 1 → 5 of 18 | 7,067 | 7,067·554 each | 3,648·433 / 0·0 / 3,020·334 | 546·75 / 141·21 | 112·18 / 15·5 |
| floor | 1 / 1 → 19 of 22 | 8,127 | 8,127·530 each | 4,214·419 / 0·0 / 3,586·320 | 3,733·421 / 3,105·322 | 80·10 / 80·10 |

**3 m** (affinity grid about 11×2×19 = 418 bricks)

| Pose | Closure | Resident | Current (each pass) | Scoped ind / A / B | Cluster-gated ind / B | Cell-grain ind / B |
|---|---|---|---|---|---|---|
| spawn | 3 → 18 of 20 | 392 | 392·8 | 241·35 / 0·0 / 197·34 | 241·35 / 197·34 | 28·9 / 28·9 |
| animroom | 1 → 5 of 17 | 392 | 392·8 | 241·35 / 0·0 / 197·34 | 41·10 / 14·4 | 14·4 / 1·1 |
| floor | 1 → 18 of 20 | 392 | 392·8 | 241·35 / 0·0 / 197·34 | 241·35 / 197·34 | 12·3 / 12·3 |

The live spawn run with lever counters on matches capture spawn exactly (8,127 / 4,214 / 3,733 / 213).

### Lever timing (emulation, timing only)

The spike swapped each lever's row set in for the forced whole-resident set after one full compose. These runs measure cost, not correctness. Frame PNGs were byte-identical to ON at this frozen instant.

| Condition (spawn) | Capture 1 m | Live 1 m | Capture 3 m | Live 3 m |
|---|---|---|---|---|
| ON (today) | 47.9 | 41.9 | 3.67 | 5.02 |
| Row-scoped | 30.3 | 26.0 | **4.72 (worse)** | **5.72 (worse)** |
| Scoped ∩ cell-grain | 7.2 | 5.9 | 2.85 | 4.37 |
| ON, one merged range per pass | 14.8 | 12.9 (1 run) | 3.33 | 4.65 (1 run) |
| Scoped, one merged range | 12.6 | — | 3.05 | — |
| SUSP (floor) | 2.55 | 4.11 | 2.12 | 4.06 |

The capture animroom and floor poses give scoped 30.3 / 30.0 and cell 5.8 / 6.5 at 1 m. Scoped-merged spans resident rows between selected rows, so it overcomposes. Cell-merged is omitted because its span covers most resident rows. The emulation's CPU cost is included: compose recording takes 2.7 ms per frame at 1 m for both ON and cell mode (spike `cpu_us.*` counters). The cluster-gated set was not timed, because building it per frame costs about 8 ms of CPU in the spike. Its row counts sit near the scoped counts at spawn and floor.

## Surprises and contradictions with the brief

1. **Dispatch count dominates, and the brief rules out the fix.**
   - *What was measured:* ON issues 530–554 range dispatches per pass at 1 m (`coalesce_rows`), because resident rows are not contiguous. Each dispatch is one `set_bind_group` + `dispatch_workgroups` inside a single compute pass (`StreamingIndirectCompose::dispatch`). Merging to one range per pass leaves the rows unchanged and removes about 70% of compose time.
   - *Likely cause (inferred, not verified):* wgpu barriers between dispatches that write the same atlas, plus GPU idle time on tiny workgroup counts.
   - *Conflict with the brief:* its Non-goal "No gather or indirect dispatch, and no pass merging … these counters are that profile" is now answered. research.md calls "many small dispatches" a smaller contributor; it is the largest.
2. **Row scoping without merging regresses 3 m.** Scoping splits ranges from 8 to about 35 per pass (1 m: 530 → 419 indirect, 320 for Pass B). The brief's first slice, "counters plus row scoping without the gate", would slow 3 m by about 1 ms unless range merging lands with it. A merge that spans only resident rows, keeping AC10 as the brief requires, still leaves hundreds of dispatches at 1 m. The real lever is a row-list gather or indirect dispatch.
3. **Residency is almost the whole map.** 22 of 482 clusters are installed, yet `indirect_resident_rows` holds 8,127 of 8,960 bricks (91%) at 1 m and 392 of about 418 at 3 m. The owner closure of 1–3 visible clusters reaches 18–19 clusters at spawn and floor (`owner_dependencies`, `InstalledCluster::patches` → `affinity_row_for_dense`). The likely cause is coarsened L1/L2 owner nodes whose patch sets span large volumes; this was not verified. So "resident but not sampled" is most of the map, and a cluster-grain gate cannot find it. The brief's region → row gate is the right grain. Its node-origin writer closure must stay tight, or it will re-inflate the way cluster ownership does.
4. **Pass A is pure waste on this fixture.** It composes 8,127 rows a frame. Scoped, it drops to 0 on every steady frame, since promotion weights never changed. This confirms the brief's Pass A decision. Pass A accounts for about a third of dispatches.
5. **Animated share of rows.** Rows with an active contributor: indirect 52%, Pass B 44% at 1 m; 61% / 50% at 3 m. The brief's premise of about 43% holds for Pass B only.
6. **CPU recording cost is real at 1 m.** It takes 2.7 ms per frame (BTreeSet clone and union, coalescing, and 1,590 range descriptors across both functions). A gather design removes most of it.
7. **The fixture does not bake at the default lightmap density on 0218faacd.**
   - At 0.04 m, `prl-build` fails: "animated lightmap atlas is over budget: budget 1073741824 bytes, found 1258291200".
   - At 0.08 m, it bakes, but the engine disables animated lightmaps: "dispatch tile count 242074 exceeds … 65535; 2D-dispatch fallback is not implemented".
   - At 0.16 m, it fits with 65,069 tiles. The timed runs used 0.16. This is a separate defect worth its own task.
8. Minor: the live runs lacked the model `.prm` files under `baked/materials` in the worktree, so enemies drew with fallback textures. Frame-cost effect is negligible.

## Spike files touched (uncommitted)

- `crates/renderer/src/render/sh_streaming/spike.rs` (new): suspend knob, lever counters, lever emulation, merge knob, CPU guard.
- `crates/renderer/src/render/sh_streaming.rs`: `mod spike`.
- `crates/renderer/src/render/sh_streaming/frame.rs`: force override, counter hooks, lever and merge emulation.
- `crates/renderer/src/render/renderer_pre_scene.rs`: passes animation buffers, weight-change probe, `end_frame`.
- `crates/postretro/src/capture/prepared.rs`: gate inputs (visible clusters and cell boxes).
- `crates/postretro/src/main.rs`: live vsync-off plus frame-interval log, live gate inputs.
- `crates/postretro/src/session/sh_residency.rs`, `crates/postretro/src/sh_streaming/targeting.rs`: visible-cluster accessor.

Env knobs:
- `POSTRETRO_SPIKE_SUSPEND_SH_COMPOSE=1`
- `POSTRETRO_SPIKE_SH_COMPOSE_STATS=1`
- `POSTRETRO_SPIKE_SH_COMPOSE_LEVERS=1` (+ `POSTRETRO_SPIKE_GATE_FOG=1`)
- `POSTRETRO_SPIKE_COMPOSE_LEVER=scoped|gated|cell`
- `POSTRETRO_SPIKE_COMPOSE_MERGE=1`
- `POSTRETRO_SPIKE_LIVE_FRAME_LOG=1`

Ignored artifacts in the worktree:
- `content/dev/maps/.spike-warren-mini-{1m,3m}.prl` (plus `..prl.pack.lock`)
- `content/dev/start-script.js` (compiled for release runs)
- `baked/`, `.build-caches/`

The worktree branch was reset from 804351717 to 0218faacd before starting; it had been created behind the requested commit.

Raw data: `scratchpad/runs/*.log`, `*.report.json`; drivers `scratchpad/run_*.sh`; aggregation `scratchpad/analyze*.py`.
