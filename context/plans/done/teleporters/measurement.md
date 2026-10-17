# Teleporters AC52: arrival-origin mandatory demand (bake half + runtime runbook)

Teleporters worktree (branch `codex/teleporters`), HEAD `7a538a989` plus the
uncommitted `reach_measurement.rs` extension. Machine: MacBook Pro, Intel i9-9980HK,
AMD Radeon Pro 5300M 4 GB + Intel UHD 630.

`$M` below is a scratch output directory outside the repo; the 2026-10-15 run's
CSVs, logs and scripts lived in session scratch and are not retained in git.
Set `M` to any empty directory to reproduce.

These are bake figures: what id 51 + id 49 make mandatory for each camera cell. They are
not runtime VRAM. The runtime half (§3) is a runbook for the owner.

## 1. Inputs

### Fixture verification

| Check | Result |
|---|---|
| `teleporters-baseline.map` vs `teleporters.map` | Identical non-teleporter entity sequence (order, key/values, brushes). Variant adds 9 entities: 5 `trigger_teleport`, 4 `teleport_destination`. |
| `teleporters-stress-baseline.map` vs `teleporters-stress.map` | Identical non-teleporter entity sequence. Variant adds 20 entities: 8 `trigger_teleport`, 12 `teleport_destination`. |
| Compiled cell parity | All four pairs of PRLs have identical cell counts, camera-cell ids and camera-cell centres; whole-map cluster counts/bytes and lightmap block counts/bytes match. Id-51 entries 534 → 1418 (dev) and 225 → 425 (stress). Byte-level section diffs were not taken; ids 29/55 also differ by construction. |
| Stress: 8 sources in one 32 m lead radius | Yes. All 8 source AABBs sit inside a 9.7 m span (max gap 7.3 m) in one 12.2 m room. |
| Stress: destinations in distinct distant regions | Yes. 8 sealed rooms, 104–147 m away, 13 m apart, each with its own light and 5 camera cells. |
| Stress: classic + 90° seamless | Yes. Sources 0/2/4/6 classic; 1/3/5/7 seamless with landmark yaw 0 → target yaw 90. |
| Stress: disabled sources | Yes. Sources 4–7 have `enabled_on_spawn 0`. |
| Stress: occupied arming | **Co-op only.** `stress_button` is a use-activated pad at quake (-208..-144, -208..-144), outside every source. A solo player cannot stand in a disarmed source and press it. A second peer is needed (§3.4). The dev map covers solo occupied arming: the `era_plate` touch pads sit inside the seamless regions. |

Fixture gaps (no edits were made to `content/dev`):

- **Stress occupied arming needs two peers.** A clearly mechanical solo fix isn't available. A touch plate inside a 0.8 m-wide source region still overlaps the capsule before the body point enters, so it would arm before entry, not while the source is occupied. The runbook uses a loopback host and client instead.
- **The destination rooms are tiny.** Each one is 5 cells, about 15 KiB of lightmap and up to 17 KiB of SH. The fixture shows that 8 arrivals become mandatory together from every source-room cell. It cannot show byte scale: the whole stress map is 0.32 MiB SH and 0.16 MiB lightmap. §2.3 extrapolates from a real map.
- Not a gap, just noise: every script-bound `trigger_volume` (`stress_button`, `vault_button`, `era_*`, `closet_plate`) logs `has neither target_tag, on_fire, nor on_exit; it will be inert` at compile, though the data script binds them with `t.on("enter")`.

### Bakes

Same flags for all four, from the worktree root:

```bash
M=<scratch output dir>   # outside the repo
CARGO_INCREMENTAL=0 cargo build -p postretro-level-compiler --bin prl-build
for m in teleporters-baseline teleporters teleporters-stress-baseline teleporters-stress; do
  ./target/debug/prl-build content/dev/maps/$m.map -o $M/prl/$m.prl --no-tui -j 8 --release --baked-root $M/baked
done
```

`--release` means exact lighting and no cache. Each bake took about 57 s. Logs are in `$M/logs/`.

After these bakes (22:54), another worker removed `"on_teleport" "gate.shut"` from `vault_gate` in `teleporters.map` (23:04). Arrival routing does not read that key, so the id-51 demand here is unaffected. The baseline parity check still passes on the edited map.

### Measurement

```bash
POSTRETRO_SH_REACH_PRL=$M/prl/<map>.prl POSTRETRO_SH_REACH_LEADS=0,16,32 POSTRETRO_SH_REACH_CSV=$M/<map>.csv \
  CARGO_INCREMENTAL=0 cargo test -p postretro --features dev-tools --bin postretro \
  sh_reach_mandatory_bytes_from_prl -- --ignored --nocapture
```

What each camera cell gets, at each lead L:

- **Mandatory cells:** id-51 entries with lead ≤ L. Lead 0 is the cell's own dilated visible set, plus teleporter arrival sets where a source region touches the cell.
- **SH:** clusters of those cells, plus id-49 pins, closed over transitive owners. Bytes are `requested_resident_bytes`.
- **Lightmap:** blocks of those cells plus pinned blocks.
  - Pairs are blocks that also carry an id-42 half. Every block in every PRL here is a pair.
  - Bytes are id-22 + id-42 upload bytes.
  - Pool layers are the 2048² layers a fresh shelf placement of only that set needs. That is a floor under the runtime pool.

None of these maps pins a cluster, so pins contribute 0.

## 2. Results

### 2.1 Summary (median / p95 / max over camera cells; Δ is variant − baseline)

**Dev level** (`teleporters` vs `teleporters-baseline`, 40 camera cells):

| L | SH bytes, variant | SH bytes, baseline | Δ SH (med / p95 / max) | Lightmap bytes, variant | Lightmap bytes, baseline | Δ lightmap (med / p95 / max) | Blocks (=pairs), var / base max | Pool layers | Cells changed |
|---|---|---|---|---|---|---|---|---|---|
| 0 m | 0.309 / 0.338 / 0.496 MiB | 0.159 / 0.187 / 0.187 MiB | +0.150 / +0.150 / +0.309 MiB | 0.096 / 0.101 / 0.148 MiB | 0.049 / 0.052 / 0.052 MiB | +0.047 / +0.049 / +0.096 MiB | 40 / 14 | 1 / 1 | 27 of 40 |
| 16 m | 0.496 / 0.496 / 0.496 MiB | 0.159 / 0.187 / 0.187 MiB | +0.338 / +0.309 / +0.309 MiB | 0.148 / 0.148 / 0.148 MiB | 0.049 / 0.052 / 0.052 MiB | +0.099 / +0.096 / +0.096 MiB | 40 / 14 | 1 / 1 | 40 of 40 |
| 32 m | same as 16 m | same | same | same | same | same | 40 / 14 | 1 / 1 | 40 of 40 |

**Eight-source stress** (`teleporters-stress` vs `teleporters-stress-baseline`, 45 camera cells):

| L | SH bytes, variant | SH bytes, baseline | Δ SH (med / p95 / max) | Lightmap bytes, variant | Lightmap bytes, baseline | Δ lightmap (med / p95 / max) | Blocks (=pairs), var / base max | Pool layers | Cells changed |
|---|---|---|---|---|---|---|---|---|---|
| 0 m | 0.016 / 0.189 / 0.260 MiB | 0.016 / 0.189 / 0.189 MiB | +0 / +0 / +0.071 MiB | 0.014 / 0.042 / 0.099 MiB | 0.014 / 0.042 / 0.042 MiB | +0 / +0 / +0.057 MiB | 25 / 5 | 1 / 1 | 2 of 45 |
| 16 m | 0.016 / 0.323 / 0.323 MiB | 0.016 / 0.189 / 0.189 MiB | +0 / +0.134 / +0.134 MiB | 0.014 / 0.156 / 0.156 MiB | 0.014 / 0.042 / 0.042 MiB | +0 / +0.115 / +0.115 MiB | 45 / 5 | 1 / 1 | 5 of 45 |
| 32 m | same as 16 m | same | same | same | same | same | 45 / 5 | 1 / 1 | 5 of 45 |

Cluster counts: dev max 1 → 3 clusters. Stress max 1 → 5 clusters at L = 0, and 1 → 9 at L ≥ 16.

### 2.2 What the fixtures show

- **Mechanism: confirmed.**
  - Stress: at L ≥ 16 m, all 5 source-room cells carry all 8 arrival rooms. Each one's mandatory set is the whole map (45 cells, 9 clusters): 9× its baseline (5 cells, 1 cluster).
  - At L = 0 only the 2 cells a source region touches carry arrivals, 4 rooms each.
  - Destination-room cells are unchanged. They hold no sources, and arrival routes are not transitive.
- **Dev level.** Rooms A and B each hold sources to both other regions, so at L ≥ 16 m every room-A/B cell's mandatory set is the whole level. Hub cells gain room A (13 → 27 cells).
- **Absolute scale is negligible here.** The worst per-cell increase is +0.35 MiB SH and +0.12 MiB lightmap, and no fixture grows a pool layer. At fixture scale, teleporters do not materially raise mandatory demand against the 2 GB target. But the fixtures are too small to answer the owner's question by themselves (see §1 gaps).

### 2.3 Real-map extrapolation (reference only; not a teleporter measurement)

Each arrival adds roughly its arrival cell's lead-0 set, at the lead of the region's distance. Lead 0 when the region touches the camera cell, so it is mandatory at every L. For a realistic price, the same harness ran on `campaign-test.map`. This was a warm, non-release bake (`prl-build ... -j 8 --cache-dir $M/cache`), so its SH bytes are approximate-lighting bytes.

| campaign-test, 207 camera cells | L = 0 (≈ cost of one arrival) | L = 16 m | L = 32 m |
|---|---|---|---|
| SH mandatory (med / p95 / max) | 10.1 / 12.0 / 12.0 MiB (whole map 17.3 MiB) | 11.2 / 12.0 / 12.6 MiB | 11.5 / 15.7 / 16.1 MiB |
| Lightmap mandatory (med / p95 / max) | 21.7 / 40.5 / 44.9 MiB | 31.6 / 44.9 / 45.9 MiB | 43.4 / 53.6 / 54.8 MiB |
| Fresh-placement pool layers (med / p95 / max) | 3 / 6 / 7 | 5 / 7 / 7 | 6 / 8 / 9 |

What eight arrivals into distinct, campaign-dense regions would cost a source-room cell. This assumes disjoint arrival sets, an upper bound:

- **Lightmap:** median about 8 × 21.7 ≈ 174 MiB, worst about 8 × 44.9 ≈ 359 MiB of mandatory pair payload. That is on top of the source cell's own 43–55 MiB at L = 32.
  - In pool layers, roughly +24 to +56 at 3–7 layers per arrival.
  - The owner's 2026-10-08 sample was 15 layers = 224 MiB, about 15 MiB per physical layer. At that ratio this is roughly **+360 to +830 MiB** of non-evictable lightmap pool. That is above the default 15-layer cap; mandatory demand grows past the cap by design.
  - During growth, an old and a new generation are both alive, so the transient peak is higher still.
- **SH:** on a campaign-sized map, arrival sets mostly overlap the whole 17 MiB map, so SH is bounded by the map.
  - On the dense hallway stress map, the done plan (`sh-streaming--reveal-gate-and-warm-horizon` §Measurement) already put the worst cell at 200.8 MiB of the 256 MiB SH floor, with fixed metadata, at L = 16. There is little headroom for even one dense arrival.
  - SH overshoot does not fail. It shows up as the `[SH streaming] non-evictable logical demand exceeds the effective floor` warning and pool growth.

**Verdict for the owner's question.**

- **At fixture scale: no material effect.** Under 0.4 MiB per cell.
- **In principle, yes for dense maps.** Mandatory demand scales linearly with the number of distinct arrival regions within lead of a cell, and every arrival is mandatory from lead 0.
  - Eight dense arrivals near one spot could plausibly add several hundred MiB of non-evictable lightmap pool. That is a material fraction of the 2 GB target.
  - It is an extrapolation, not a measurement: no authored dense multi-teleporter map exists.
- The runtime runbook (§3) measures the real pool peak and growth transient on the fixtures.

**Policy lever under discussion, not applied:** move arrival origins beyond the touching cells to the evictable band. On the stress fixture that would drop the L ≥ 16 increase for the 3 non-touching source-room cells. The 2 touching cells keep their 4 arrivals each.

### 2.4 Retained data (all under `$M`; session scratch, not in git)

- Per-cell rows (every camera cell × L ∈ {0, 16, 32}):
  - `teleporters.csv`, `teleporters-baseline.csv`, `teleporters-stress.csv`, `teleporters-stress-baseline.csv`, `campaign-test-reference.csv`
  - Columns: `lead_m, camera_cell, center_xyz (engine m), mandatory_cells, sh_clusters, sh_bytes, lightmap_blocks, lightmap_pairs, lightmap_half_bytes, shadowmask_half_bytes, lightmap_pair_bytes, lightmap_pool_layers`
- Joined variant/baseline/delta per cell: `teleporters-delta.csv`, `teleporters-stress-delta.csv`.
- Full summary tables for every metric: `summary-tables.md`. Harness stdout: `logs/measure-*.log`. Bake logs: `logs/<map>.log`.
- Scripts: `scripts/mapdiff.py` (entity diff), `scripts/tpsummary.py` (source/destination geometry), `scripts/summarize.py` (tables).

## 3. Runtime runbook (Mac/Metal, owner-run; not executed here)

Goal (AC52): compare runtime lightmap pool peak and SH working set beside a teleporter with the matched no-teleporter map, plus the growth transient on cross. Report overruns of the 2 GB target plainly.

### 3.1 Build and bake (repository root)

```bash
# from the repository root
cargo build -p postretro --features dev-tools        # authoring engine (Streaming tab needs dev-tools)
cargo build -p postretro-script-compiler --bin scripts-build
cargo build -p postretro-level-compiler --bin prl-build
cargo build -p postretro-tool
for m in teleporters-baseline teleporters teleporters-stress-baseline teleporters-stress; do
  ./target/debug/prl-build content/dev/maps/$m.map -o content/dev/maps/$m.prl --no-tui -j 8 --release --baked-root baked
done
```

The baked PRLs are git-ignored. If a §1 run's `$M/prl/` still exists, those PRLs are identical bakes and may be copied in instead.

### 3.2 Launch (one process per run; relaunch for each map so pool peaks don't carry over)

```bash
RUST_LOG=info POSTRETRO_CPU_TIMING=1 \
  cargo run -p postretro-tool -- run --install-root . maps/<map>.prl [--start-pose x,y,z,yaw_deg,0] \
  2>&1 | tee $M/runtime-<map>-<step>.log
```

- Leave `POSTRETRO_SH_STREAMING` and `POSTRETRO_LIGHTMAP_STREAMING` unset. That gives async SH and streamed lightmaps.
- Optionally add `POSTRETRO_GPU_TIMING=1`. It reports unsupported on adapters without timestamps; then use the Metal System Trace path in `rendering_pipeline.md` §12.
- Keep the window in front, and run `caffeinate -d` in another terminal. A screen saver or background window stops frames.
- Before each run, record idle VRAM: `ioreg -c IOAccelerator | grep -E 'inUseVidMemoryBytes|vramFreeBytes|Device Utilization'`.

### 3.3 What to read

| Quantity | Where |
|---|---|
| Lightmap pool layers / peak / cap, pool bytes, retiring bytes, **growth transient peak**, mandatory blocks and bytes, resident blocks and bytes | Dev panel (Alt+Shift+\`) → Streaming tab → Lightmap: "Layers (peak / cap)", "Pool bytes", "Retiring bytes", "Growth transient peak", "Mandatory blocks/bytes", "Resident blocks/bytes". Also the `[Lightmap streaming] last 5.0 s: … \| now: … mandatory N blocks …, pool L layers (peak P, cap C) …, growth peak G` log line. It prints only when counters changed, so read the tab while dwelling. |
| Lightmap texture bytes actually bound | `[Renderer] Lightmap residency: …; total … B` load log, and Performance tab → "Lightmap memory" (rebuilt on growth/retire) |
| SH mandatory / resident | Streaming tab → SH Residency: "Target clusters", "Lead clusters", "Sampleable clusters", "Logical occupancy", "Pool capacity" (= requested physical GPU bytes for the SH pool); Pool growth: "Growth events", "Capacity added". `[SH streaming] … \| now: T targets (N lead), S sampleable, … pool O of C` log line. An overshoot logs `[SH streaming] non-evictable logical demand exceeds the effective floor by … bytes`. |
| Visible misses | Lightmap: "Visible misses (outside set / not resident)". SH: "Misses: reach / pressure / in flight …". |
| Lead L and lightmap cap in effect | Streaming tab sliders: Lead L (default 16 m, max 32 m) and Pool cap (default 15 layers). Record both, and repeat the stress run at L = 32. |
| Frame timing | `[CpuTiming]` lines (120-frame windows; `wait_acquire`-dominated = GPU-bound) and Performance tab. Note the frame window covering each cross. |
| Machine | `sysctl -n machdep.cpu.brand_string`, `system_profiler SPDisplaysDataType` (which GPU is active), surface/scene extents from the renderer's init log |

### 3.4 Routes

Positions are `--start-pose` engine metres: engine = (−quake_y, quake_z, −quake_x) × 0.0254, yaw 0 faces quake +X. Run every step on the variant **and** its baseline with the same pose and dwell. On the baseline the source is simply absent.

**Dev level** (`maps/teleporters.prl` vs `maps/teleporters-baseline.prl`)

1. **Spawn dwell.** No pose; spawn is in the hub. Dwell 15 s and record all §3.3 quantities.
2. **Approach and dwell beside the source.** Walk to the purple vault gate (quake -64..-16, -208..-160), or launch with `--start-pose 3.454,1.016,1.016,0,0`, just north of it. Dwell 15 s and record. This is the AC52 "beside a teleporter" sample: the variant should show room-A blocks/clusters mandatory and resident before any cross.
3. **Cross (classic).** Variant only: press use on the yellow pad (`--start-pose 3.251,1.016,3.251,0,0`), then walk into the gate.
   - On the arrival frame, record any lightmap growth event, the growth transient peak, visible misses and the CPU window.
   - Baseline equivalent: launch at the arrival pose `--start-pose -2.845,1.016,-99.568,0,0`. Cold arrival, for the steady-state comparison.
4. **Dwell in room A** for 15 s and record. Then walk to the centre touch plate (quake 4144..4176, -16..16). It is inside the seamless region, so it arms both directions while you are standing in it: an **occupied-arm shift** to room B on the next tick. Record the same transient quantities.
   - Baseline: `--start-pose 0,1.016,-105.664,0,0`, dwell 15 s; then `--start-pose 0,1.016,-208.077,0,0` for room B.
5. **Return.** Walk out of the room-B centre square (quake 8064..8320, -128..128) and into the south-west return gate (quake 7984..8032, -208..-160; classic, armed) back to the hub. Record the transient, then dwell 15 s in the hub and record the pool peak; it should stay at the run's maximum.

**Eight-source stress** (`maps/teleporters-stress.prl` vs `maps/teleporters-stress-baseline.prl`)

1. **Spawn dwell** (spawn at the south-west corner of the source room), 15 s. Record. In the variant, every source-room cell mandates all 8 arrival rooms at L ≥ 16 m.
2. **Beside the sources.** `--start-pose 1.016,1.016,0,0,0`, between the source rows next to the centre pillar. Dwell 15 s at L = 16, then set the Lead slider to 32 and dwell 15 s. Record both.
3. **Cross a classic route.** Walk into source 0 (quake -160..-128, -96..-64; `--start-pose 2.032,1.016,3.658,0,0` stands in it). Record the transient. Baseline: launch at `--start-pose 0,1.016,-104.038,0,0`.
4. **Cross a seamless route.** Relaunch, walk into source 1 (quake -64..-32, -96..-64). The view yaw turns 90°. Record. The stress fixture has no return routes, so reload between crosses.
5. **Occupied arm (co-op loopback, two processes):**
   ```bash
   # Terminal 1 (host)
   RUST_LOG=info cargo run -p postretro-tool -- run --install-root . maps/teleporters-stress.prl --host
   # Terminal 2 (client)
   RUST_LOG=info cargo run -p postretro-tool -- run --install-root . maps/teleporters-stress.prl --connect 127.0.0.1:<port>   # the host logs the port it bound
   ```
   - The client stands in disarmed source 4 (quake -160..-128, 64..96, north-west of the pillar; nothing happens while it is disarmed).
   - The host presses use on the yellow pad (quake -208..-144, -208..-144). On the next tick the client shifts to `stress_out_4`.
   - Record the client's transient and pool peak.
   - Baseline: no arm exists, so record the client dwelling at the same spot.

### 3.5 Report back (per map, variant vs baseline, per step)

- Machine, GPU in use, surface/scene extents, Lead L, pool cap.
- Lightmap: layers, peak layers, cap, pool bytes, retiring bytes, growth transient peak, mandatory blocks/bytes, resident blocks/bytes, visible misses.
- SH: targets/lead/sampleable clusters, logical occupancy, pool capacity, growth events/bytes, misses, any floor-overshoot warning.
- `[Renderer] Lightmap residency` total, and idle `inUseVidMemoryBytes` before the run.
- CPU timing windows around each cross (avg/max frame, `wait_acquire`).
- Any visible pop or lighting change on arrival (AC49 shares the seamless step).
- Overruns: any pool or transient that would breach 2 GB, stated plainly.

The owner then decides the brief's open question: keep full mandatory arrival origins, or move origins beyond the touching cells to the evictable band.

## 4. Runtime results (2026-10-16, unattended)

Run by an agent with the owner away, on worktree HEAD `fb8e0dfca`. Raw logs, per-run power/VRAM checks and the driver scripts lived in session scratch and are not retained in git.

### 4.1 Setup

| Item | Value |
|---|---|
| Machine | MacBook Pro, Intel i9-9980HK, AMD Radeon Pro 5300M 4 GB + Intel UHD 630 |
| GPU the engine selected | `AMD Radeon Pro 5300M (backend=Metal, type=DiscreteGpu)` in every run |
| Power | AC power before and after every run; no `ScreenSaverEngine` before or after any run; `caffeinate -dimsu` for the whole session |
| Build | §3.1: `dev` profile, `postretro --features dev-tools`; fixtures re-baked with `prl-build ... --release` into `content/dev/maps/` |
| Launch | `RUST_LOG=info POSTRETRO_CPU_TIMING=1 target/debug/postretro-tool run --install-root . maps/<map>.prl [--start-pose ...]`. One fresh process per row; 25 s wall dwell (about 22 s after reveal), then SIGINT. No stray `postretro` process after any run. |
| Extents, lead, cap | surface 3584×2240, scene 1195×747 (divisor 3); lead 16.0 of 32.0 m; lightmap pool cap 15 layers |
| Frame regime | Every run vsync-bound at 60 Hz: 120-frame window averages 16.2–16.6 ms, `wait_acquire` ≈ 9–10.5 ms. No slow-regime run occurred, none discarded. |

Readings come from the last `[Lightmap streaming]` and `[SH streaming]` log lines of each run (§3.3). Each fixture installs everything in the first 5 s window and stays idle after it, so one window holds the whole run, any cross included.

### 4.2 What was automated, substituted or skipped

`xtask observe` / `--headless` has no renderer, so it cannot measure GPU residency. `--observe-live` reads state but injects no input. Neither was used; no engine code or flags were added. The only per-run control was `--start-pose`.

| Step | How it ran |
|---|---|
| Dev 1, 2; stress 1, 2 (L = 16) | Automated as written. |
| Stress 2 at L = 32 | **Skipped.** Lead is set only by the dev-panel slider. §2.1 shows L = 32 equals L = 16 on both fixtures. |
| Dev 3, classic cross | **Substituted.** `vault_gate` starts disarmed and needs a use press. Variant and baseline both launch cold at the `vault_in` arrival pose. That row also serves as the variant's step-4 room-A dwell. |
| Dev 4, occupied-arm shift | **Substituted, unverified.** The variant launches standing on the room-A `era_plate` (`0,1.016,-105.664`). The trigger system emits Enter on the first tick for a capsule already inside, which arms the shift. No log line confirms the teleport, and A↔B are rotated copies, so portal-walk stats cannot tell the rooms apart. Residency cannot differ either way: both room cells mandate the whole level. |
| Dev 5, return | **Substituted.** The variant launches inside `return_b` (`4.674,1.016,-203.403`), armed classic, and crosses to the hub on the first tick: 21 evictions confirm it. Baseline: the `hub_return` pose and the same `return_b` pose. |
| Stress 3, 4, crosses | **Substituted.** The variant launches inside source 0 (classic) or source 1 (seamless, standing on landmark `stress_in_1`) and crosses on the first tick: 40 evictions confirm it. The source cell's full mandatory set (all 8 arrivals) is resident at reveal, so the pre-cross peak is captured. Baseline: the same source pose (no teleporter), plus the arrival pose. |
| Stress 5, co-op occupied arm | **Skipped.** The host must press use on `stress_button`; no input path exists without code. |

### 4.3 Dev level (`teleporters` vs `teleporters-baseline`, 56 blocks)

Lightmap: mandatory = resident in every row. The pool is 1 layer (peak 1, cap 15), 28.0 MiB, 0 retiring, growth peak 0 B, 0 growths, 0 visible misses. `[Renderer] Lightmap residency` total is 28.19 MiB for both maps (1 streamed layer + spare, shadowmask resident). SH pool capacity is 3.0 MiB for both maps, with 0 growths and 0 misses.

| Step | Variant: LM mandatory | Variant: SH targets / occupancy | Baseline: LM mandatory | Baseline: SH targets / occupancy | Cross activity (variant, first window) |
|---|---|---|---|---|---|
| 1 Spawn (hub) | 35 blocks, 110.4 KiB | 3, 415.6 KiB | 14 blocks, 53.0 KiB | 1, 262.2 KiB | — |
| 2 Beside vault gate | 35, 110.4 KiB | 3, 415.6 KiB | 14, 53.0 KiB | 1, 262.2 KiB | — |
| 3/4 Room A at `vault_in` (cold) | 56, 166.4 KiB (whole level) | 6, 577.4 KiB | 21, 57.3 KiB | 2, 153.4 KiB | — |
| 4 Room A centre (baseline) / on plate (variant shift) | 56, 166.4 KiB | 6, 577.4 KiB | 21, 57.3 KiB | 2, 153.4 KiB | 56 installs, 0 evictions, 0 growths |
| 4 Room B (baseline) | — | — | 21, 56.0 KiB | 3, 161.9 KiB | — |
| 5 Return gate → hub | 35, 110.4 KiB after cross | 3, 415.6 KiB | hub: 14, 53.0 KiB; at gate pose: 21, 56.0 KiB | hub: 1, 262.2 KiB; gate: 3, 161.9 KiB | 56 requested/installed, 21 evicted, 0 growths, peak 1 layer |

### 4.4 Eight-source stress (`teleporters-stress` vs `teleporters-stress-baseline`, 45 blocks)

Same pool state in every row: 1 layer (peak 1, cap 15), 28.0 MiB, growth peak 0 B, 0 misses. Lightmap residency total is 28.00 MiB. SH pool capacity is 1.3 MiB, with 0 growths.

| Step | Variant: LM mandatory | Variant: SH targets / occupancy | Baseline: LM mandatory | Baseline: SH targets / occupancy | Cross activity (variant, first window) |
|---|---|---|---|---|---|
| 1 Spawn (source room) | 45 blocks, 160.2 KiB (whole map) | 9, 331.2 KiB | 5 blocks, 42.9 KiB | 1, 193.7 KiB | — |
| 2 Beside sources, L = 16 | 45, 160.2 KiB | 9, 331.2 KiB | 5, 42.9 KiB | 1, 193.7 KiB | — |
| 3 Classic cross, source 0 | 5, 14.7 KiB after cross | 1, 16.2 KiB | at source pose: 5, 42.9 KiB; at arrival: 5, 14.7 KiB | 1, 193.7 KiB; arrival 1, 16.2 KiB | 45 installed, 40 evicted, 0 growths, peak 1 layer |
| 4 Seamless cross, source 1 | 5, 14.7 KiB after cross | 1, 16.2 KiB | at source pose: 5, 42.9 KiB; at arrival: 5, 14.7 KiB | 1, 193.7 KiB; arrival 1, 16.2 KiB | 45 installed, 40 evicted, 0 growths, peak 1 layer |

Stress runtime matches §2.1 L = 16 exactly: 45 blocks and 9 clusters from every source-room cell. The dev level has since been edited (floor markers `5ce836af0`, hub lamp `3962349a0`) and now has 56 blocks, so its counts are not comparable one-to-one with §2.1.

### 4.5 Device memory and warnings

- **Device VRAM.** AMD `inUseVidMemoryBytes` (device-wide, other apps included):
  - Idle: about 1.16–1.18 GiB.
  - Mid-run: about 1.85–1.86 GiB.
  - Engine-attributable: 688–712 MiB in every run, variant and baseline alike. The spread is noise, not a teleporter effect. It is dominated by fixed allocations, such as the 384 MiB spot shadow pool and the 3584×2240 surface.
- **SH floor.** No `[SH streaming] non-evictable logical demand exceeds the effective floor` warning in any run. No ERROR lines.
- **2 GB target.** No overrun. Peak device-wide use was about 1.86 GiB of 4 GiB, including the desktop. The teleporter-attributable increase is ≤ 0.12 MiB lightmap and ≤ 0.42 MiB SH (dev room A: +109 KiB, +424 KiB).

### 4.6 Verdict against AC52

AC52 asks for the runtime lightmap pool peak and SH working set beside a teleporter, against the same map with teleporters removed.

- **Mechanism.** Arrival origins are mandatory and resident before any cross. Beside the stress sources the variant holds the whole map: 45 vs 5 lightmap blocks and 9 vs 1 SH clusters. Beside the dev vault gate it holds 35 vs 14 blocks and 3 vs 1 clusters.
- **Bytes.** The absolute cost is negligible at fixture scale: +117 KiB lightmap and +138 KiB SH (stress), +57 KiB lightmap and +153 KiB SH (dev beside the gate).
- **Pool.** The lightmap pool stays at 1 layer with peak 1 in every run. No growth event, no retiring generation, and a 0 B growth transient on every cross, classic or seamless. SH pools never grow and never warn.
- **Verdict.** Measured, not exceeded. No runtime pool peak or transient approaches the 2 GB target. These fixtures cannot show byte scale. The dense-map exposure in §2.3 (several hundred MiB of non-evictable lightmap from eight dense arrivals) remains an extrapolation that this runtime run neither confirms nor refutes.
- **Gaps.** The L = 32 dwell and the co-op occupied arm were not run, and the dev crosses used start-pose substitutions. These are listed in §4.2.
