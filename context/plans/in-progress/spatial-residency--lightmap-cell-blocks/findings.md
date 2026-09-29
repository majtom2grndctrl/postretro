# Findings: lightmap cell blocks

Task 12 of `plan.md`. Measured values for AC 22–25 against research §1, the handoffs for AC 26–27, and the recommendation (AC 28). Findings are measure-and-report, not gates (`context/lib/experimental_spikes.md`). Measured at `9f315e7a9` plus the measurement harnesses listed under Reproduction.

## Recommendation

**An allocator and pool-policy change is warranted before Low.** Cell blocks fit Low: the hallway's worst mandatory set is 136.5 MiB at L = 16 m, and nothing drawn went missing after spawn at the default levers on either map. The pool around them does not fit:

- The first generation is sized at the cap. It spends 224 MiB at install on the hallway. With the animated atlas and the weight maps, that is about 295 MiB, over Low's 256 MiB before any growth.
- The repack never moves a block upward, so it cannot reach the from-scratch packing. The runtime pool occupied 16 layers where the shelf worst is 12, and it grew past cap 15. Growth then holds both generations (462 MiB for at least two frames) and never shrinks.

Neither half-res nor a smaller lead fixes this. L = 0 m still filled 15 layers and added misses. The next spec should:

- make repack able to compact to the from-scratch layout;
- set the default cap from Low's remainder: 12 layers plus the spare, 182 MiB;
- treat growth as an emergency path that shrinks back after a repack;
- move the payload split off the frame thread;
- warm the staging pool before the spawn preload.

## Pinned conditions

| Pin | Value |
|---|---|
| Machine | i9-9980HK (8C/16T), Radeon Pro 5300M 4 GB (Metal), 32 GB RAM, macOS |
| Build | `target/debug` (dev profile: workspace opt-level 1, deps opt-level 2), unless marked release. The dry run ran in release. |
| Fixtures | `campaign-test.prl` 149,045,701 B (198 blocks, 464 cells). `stress-warren-hallway-inspection.prl` 1,674,702,102 B (2,080 blocks, 5,671 cells). Both carry id 51 with a 32 m max lead. |
| Cache mode | Warm page cache: each PRL was read whole (`cat > /dev/null`) before its walks. First-repeat load and preload times are cold-ish and listed separately. |
| Load | A `stress-warren-mini` bake held about 2 cores until the first sweeps started. Timing figures come from the quiet reruns only. Captures ran beside the hallway sweep (one core). |
| SH | Not streamed in the walks, so the shared drain budget carried lightmap pairs alone. Captures ran SH `sync-proof`. |
| Walk model | Dry-run seeded walks (seed `0x5eedb10c`), the first 2,000 steps. Eye points are jittered inside each cell, 10–90% of its AABB, off the bake's lattice. Four headings 90° apart per frame. HFOV 100° at 16:9. Dwell is the frames to reach the next cell centre at 8 m/s and 60 Hz, capped at 60. Frames with reads in flight sleep to 16.7 ms, so reads race real time. |
| Renderer stand-in | The real `LightmapPoolModel` plans every drain. A grown-out generation retires two drains later. No GPU work runs in the walk; install time comes from captures and the renderer drain test. |

## AC 22: walks (runtime path)

The walk runs the real controller, `LevelStreaming` and level-scope issuer, reading real pairs through the manifest. Visibility is the runtime portal walk. All sizes are MiB and all byte figures are ids 22 + 42.

### Default levers (L = 16 m, cap 15)

| Figure | Hallway random | Hallway tour | Campaign random | Campaign tour |
|---|---|---|---|---|
| Play time (frames) | 1,564 s (93,837) | 1,590 s (95,392) | 1,261 s (75,684) | 1,438 s (86,255) |
| Resident max / p95 | 136.8 / 116.4 | 168.6 / 156.2 | 46.2 / 37.2 | 49.3 / 47.7 |
| – id 22 max / p95 | 58.6 / 49.9 | 72.3 / 66.9 | 19.8 / 16.0 | 21.1 / 20.5 |
| – id 42 max / p95 | 78.2 / 66.5 | 96.4 / 89.2 | 26.4 / 21.3 | 28.2 / 27.3 |
| Mandatory max / p95 | 92.5 / 84.8 | 136.5 / 126.6 | 44.1 / 36.4 | 44.1 / 44.1 |
| – id 22 max / p95 | 39.6 / 36.3 | 58.5 / 54.3 | 18.9 / 15.6 | 18.9 / 18.9 |
| – id 42 max / p95 | 52.8 / 48.5 | 78.0 / 72.4 | 25.2 / 20.8 | 25.2 / 25.2 |
| Pool layers: first → peak (occupied) | 15 → 15 (15) | 15 → 16 (16) | 7 → 7 (5) | 7 → 7 (7) |
| Peak held, spare and retiring included | 224 | 462 | 112 | 112 |
| Growths; transient peak | 0; — | 1; 462 | 0; — | 0; — |
| Repacks (% of steps; block copies) | 0 | 30 (1.50%; 6,039) | 0 | 0 |
| Refusals / deferrals | 1,376 / 0 | 1,186 / 0 | 0 / 0 | 2 / 0 |
| Read per step: mean / p95 / max | 6.19 / 25.2 / 52.4 | 12.34 / 35.7 / 99.6 | 0.66 / 4.1 / 21.6 | 1.30 / 10.4 / 21.6 |
| Total read, id 22 / id 42 | 5,369 / 7,010 | 10,653 / 14,017 | 566 / 754 | 1,122 / 1,487 |
| Per-drain upload p95 / max | 7.76 / 8.00 | 7.86 / 7.99 | 7.70 / 7.76 | 7.79 / 7.83 |
| Hit rate (blocks joining M(c, L) already resident) | 90.3% | 95.5% | 96.5% | 99.8% |
| Spawn preload | 56 pairs, 29.5 MiB, 28 ms | 56 pairs, 29.5 MiB, 14 ms | 120 pairs, 34.5 MiB, 86 ms | 120 pairs, 34.5 MiB, 26 ms |

Visible misses (block-frames). "Drawn missing" is the harness's own count of drawn blocks still non-resident after the frame's drain, which is what renders SH-only:

| Bucket | Hallway random | Hallway tour | Campaign random | Campaign tour |
|---|---|---|---|---|
| Outside the baked set | 415 in 236 frames | 1,056 in 621 frames | 0 | 7 in 3 frames |
| – of which drawn missing | 0 | 0 | 0 | 0 |
| Not resident (in set, stream lagged) | 0 | 0 | 0 | 0 |
| Teleport step (random walk stall) | — | — | 28 in 3 frames | — |

- **No visible misses after spawn at the default levers.** The only non-resident draws followed campaign's one stall teleport, which no lead can cover.
- **Dilation undercounts, and the lead covers it.** Off-lattice eye points see cells that the dilated PVS lacks: hallway cells 3945, 3062, 785 and 920, and campaign cells 172 and 179. At most two blocks per frame, all already resident through the band or retention.

### Cap sweep at L = 16 m

Hallway (campaign's pool is identical at every cap: the first generation is `min(cap, 7)` and its whole level fits in 7 layers):

| Walk | Cap | Layers first → peak | Peak held | Growths | Repacks (% steps; copies) | Deferrals | Refusals | Read/step | Hit rate | Drawn missing / not resident |
|---|---|---|---|---|---|---|---|---|---|---|
| Random | 7 | 7 → 13 | 378 | 6 | 4 (0.20%; 940) | 0 | 13,674 | 10.21 | 80.2% | 1 / 5 in 3 frames |
| Random | 12 | 12 → 13 | 378 | 1 | 1 (0.05%; 288) | 0 | 2,865 | 6.82 | 89.4% | 0 / 0 |
| Random | 15 | 15 → 15 | 224 | 0 | 0 | 0 | 1,376 | 6.19 | 90.3% | 0 / 0 |
| Random | 25 | 25 → 25 (19 occupied) | 364 | 0 | 0 | 0 | 0 | 5.72 | 90.7% | 0 / 0 |
| Tour | 7 | 7 → 19 | 546 | 12 | 2 (0.10%; 658) | 8 | 16,776 | 20.89 | 81.1% | 8 / 27 in 23 frames |
| Tour | 12 | 12 → 16 | 462 | 4 | 30 (1.50%; 6,234) | 2 | 4,080 | 14.00 | 93.0% | 2 / 0 |
| Tour | 15 | 15 → 16 | 462 | 1 | 28–30 (1.4–1.5%; 5,606–6,039) | 0 | 1,186–1,261 | 12.34–12.41 | 95.4–95.5% | 0 / 0 |
| Tour | 25 | 25 → 25 (22 occupied) | 364 | 0 | 0 | 0 | 0 | 10.98 | 96.7% | 0 / 0 |

### Lead sweep at cap 15

| Map, walk | L | Mandatory max / p95 | Layers first → peak | Growths | Repacks | Read/step | Outside the set (block-frames) | Drawn missing / not resident |
|---|---|---|---|---|---|---|---|---|
| Hallway random | 0 m | 56.6 / 45.8 | 15 → 15 | 0 | 0 | 5.92 | 2,666 in 924 frames | 0 / 0 |
| Hallway random | 32 m | 148.0 / 121.0 | 15 → 16 | 1 | 1 (405 copies) | 5.70 | 0 | 0 / 0 |
| Hallway tour | 0 m | 115.2 / 84.5 | 15 → 15 | 0 | 6 (0.30%; 931) | 12.73 | 6,677 in 2,093 frames (max 51 per frame) | 5 / 9 in 5 frames |
| Hallway tour | 32 m | 170.0 / 166.8 | 15 → 20 | 5 | 4 (0.20%; 976) | 10.96 | 0 | 0 / 0 |
| Campaign random | 0 m | 37.0 / 22.6 | 7 → 7 | 0 | 0 | 0.66 | 73 in 73 frames | 0 / 0 |
| Campaign tour | 0 m | 36.8 / 36.7 | 7 → 7 | 0 | 0 | 1.30 | 135 in 98 frames | 0 / 0 |
| Campaign tour | 32 m | 49.3 / 47.7 | 7 → 7 | 0 | 1 (189 copies) | 1.30 | 0 | 0 / 0 |

- On campaign, L only moves blocks between mandatory and band. The pool holds `M(c, 32 m)` either way, so reads per step do not change with L.
- At L = 32 m there is no band, so the hit rate is 0%: every block joining the set is a demand read, 32 m ahead of need. None missed.

### Against research §1 (hallway, L = 16 m, cap 15, band retain)

| Hypothesis | Research §1 | Dry run, real pool model, 20,000 steps | Runtime walk, 2,000 steps |
|---|---|---|---|
| Worst mandatory, shelf layers | 136.5 MiB, 12 layers | 136.5 MiB, 12 layers | 136.5 MiB at the worst cell visited; 16 layers occupied |
| Growth at cap 15, random / tour | 0 / 0 | 0 / 2 generations, peak 15 / 21 | 0 / 1 generation, peak 15 / 16 |
| Repack %, random / tour | 0.01% / 2.54% | 0.01% / 0.06% | 0% / 1.5% |
| Hit rate, random / tour | 97.3% / 95.8% | 97.3% / 96.3% | 90.3% / 95.5% |
| Read MiB per step, random / tour | 5.62 / 10.12 | 5.64 / 10.17 | 6.19 / 12.34 |
| Fits Low (256 MiB) | yes, 170.0 MiB worst at 32 m | same | content yes; pool no (see Low budget) |

- Research's "a cap at or above the shelf worst never grows" assumed a from-scratch repack. The shipped repack never moves a block upward, so a cap of 12–15 grows once fragmentation outruns it. Both the model-driven dry run and the runtime walk show this.
- The runtime repacks far more often than the dry run on the tour (1.5% against 0.06%). It adds never-refused visible demand, per-frame heading changes and the shared drain budget. The dry run has none of these. Each runtime repack moves about 200 blocks through the spare layer: a GPU copy of most of the pool.

### Install time per drain (renderer, GPU)

| Source | Drain | CPU time (plan, stage, record, submit) | Rate |
|---|---|---|---|
| Capture preload, hallway spawn, debug | 58 pairs, 36.9 MiB, cold staging | 86–109 ms | 2.3–3.0 ms/MiB |
| Same, release | same | 86–102 ms | 2.3–2.8 ms/MiB |
| Capture preload, hallway worst cell 5254, release | 163 pairs, 136.6 MiB | 274–292 ms | 2.0–2.1 ms/MiB |
| Capture preload, campaign spawn, debug / release | 104 pairs, 35.9 MiB | 109–123 / 89–104 ms | 2.5–3.4 ms/MiB |
| Renderer drain test, first drain, debug | 13 pairs, 8.53 MiB, cold staging | 20.8 ms | 2.44 ms/MiB |
| Renderer drain test, 119 warm drains, debug | 13 pairs, 8.53 MiB each | mean 2.15, p50 2.13, p95 2.43, max 2.81 ms | 0.25 ms/MiB |

- **In-play drains cost about 2 ms at the 8 MiB budget, once the staging pool is warm.** The capture preload's rate is the cold staging pool's, not the steady state's. Release matches debug, so the cost is in the wgpu staging path, not in our code.
- The pool model's planning is small: 18–22 µs mean per submitted drain on the hallway, p99 63–101 µs, max 0.7 ms on repack drains.

### Capture JSON (`lightmap_streaming.counters`, `POSTRETRO_LIGHTMAP_STREAMING=stream`)

Each capture preloads the view's mandatory and visible set in one drain. Every pose reported zero misses in both buckets, zero repacks, growths, refusals and deferrals, and the default cap.

| Map, pose | Resident blocks | Read id 22 / id 42 | Pool layers | Install (debug) |
|---|---|---|---|---|
| Campaign spawn (−65.84, 2.6, −45.92) yaw 270 | 104 | 16.1 / 21.5 MB | 7 | 106 ms |
| Campaign cell 405 (−55, 6, −11) | 66 | 11.4 / 15.2 MB | 7 | — |
| Campaign cell 268 (−53, 2, 17) | 95 | 17.7 / 23.6 MB | 7 | — |
| Hallway spawn (42.27, 3.2, 63.40) yaw 0 | 58 | 16.6 / 22.1 MB | 15 | 86 ms |
| Hallway cell 5254 (10, 8, 96), worst at 16 m | 163 | 61.4 / 81.8 MB | 15 | 291 ms |
| Hallway cell 5237 (0, 16, 106), worst at 32 m | 143 | 35.5 / 47.3 MB | 15 | 221 ms |
| Hallway cell 2590 (21, 6, −85) | 134 | 53.1 / 70.8 MB | 15 | 290 ms |

The campaign pose at (−30, 3, −46), meant for cell 106, located to a cell with an empty set and preloaded nothing. That is a pose error, not a runtime one. All-resident captures report no counters (mode `all-resident`, every block resident).

## AC 23: CPU time of lightmap residency

Source: the walk harness, not `[CpuTiming]`. The `lightmap_residency` stage is scoped exactly as the game scopes it: demand, completions, batch and reads inside `prepare_drains`, then `apply_outcome` and `finish_frame`. The windowed game could not be used. Launched from this session, it stalled at "Window ready": its window was occluded while the owner was using the machine, so no redraw was delivered and no level loaded. The quiet default-lever runs, in µs:

| Frames | Hallway random | Hallway tour | Campaign random | Campaign tour |
|---|---|---|---|---|
| Camera-cell change: mean / p95 / p99 / max | 43.6 / 84.7 / 123 / 247 | 52.6 / 80.9 / 119 / 262 | 18.4 / 35.8 / 63 / 208 | 24.8 / 40.4 / 55 / 92 |
| Steady: mean / p95 / p99 / max | 14.2 / 5.5 / 497 / 36,852 | 27.2 / 19.0 / 940 / 11,490 | 3.4 / 2.8 / 10.6 / 3,991 | 4.2 / 2.3 / 16.4 / 1,996 |
| Steady frames admitting completions: count; p50 / p95 / p99 / max | 1,776; 509 / 1,529 / 2,432 / 36,852 | 3,229; 531 / 1,939 / 3,154 / 11,490 | 251; 406 / 2,007 / 2,820 / 3,990 | 684; 152 / 1,239 / 1,578 / 1,996 |
| Steady frames admitting none: p99 / max | 16 / 431 | 23 / 304 | 6 / 100 | 3.6 / 2,400 |
| Most read bytes admitted in one frame | 26.9 MiB | 31.8 MiB | 15.1 MiB | 15.1 MiB |

- Demand work is cheap. A camera-cell change costs 40–50 µs on the hallway. A steady frame costs about 1 µs.
- **The cost is admitting completions on the frame thread.** Median 0.5 ms, p99 2.4–3.2 ms, spikes to 11–37 ms on the hallway. `payload_from_pair_bytes` splits each pair with `Vec::split_off`, which allocates and copies the direction plane and shadowmask group B: about 40% of a pair. Up to 32 MiB of pairs arrive in one frame (the in-hand bound). The spikes are page faults on those fresh allocations under memory pressure. The machine had 1–3 GB free with the compressor active.
- `apply_outcome` and `finish_frame` stay under 4 µs at p99.

## AC 24: PRL size and time to first frame

| Map | Baseline PRL | Cell-block PRL | Delta | Of which id 51 |
|---|---|---|---|---|
| Campaign | 142,552,247 B | 149,045,701 B | +6,493,454 B (+4.56%) | about 170 KB |
| Hallway | 1,581,022,836 B | 1,674,702,102 B | +93,679,266 B (+5.93%) | about 2.6 MB |

The rest of the delta is block padding: block texels over chart texels are 1.234× on the hallway and 1.381× on campaign.

Time to first frame, stream against all-resident. The windowed `[Startup]` line (`streaming_preload`, `first_level_frame`) could not be read (see AC 23), so two headless proxies stand in:

| Figure | Campaign stream | Campaign all-resident | Hallway stream | Hallway all-resident |
|---|---|---|---|---|
| `load_prl`, debug, 3 repeats | 181–241 ms | 234–293 ms | 2.94–3.09 s | 4.15–4.51 s |
| Whole lightmap payload held for upload | none (index only) | 62.0 MiB | none (index only) | 1,012.3 MiB |
| Spawn preload: reads and plan, warm (cold) | 27–34 ms (94 ms) | — | 21–24 ms (70 ms) | — |
| Headless capture wall time, spawn, debug | 1.83–2.02 s | 1.89–2.06 s | 10.12–10.38 s | 13.83–15.09 s |
| Same, release | 1.49–1.60 s (3.67 s cold) | 1.55–1.62 s | 5.99–6.01 s | 9.92–10.08 s |

- **The hallway reaches its first frame about 4 s sooner when it streams**, in debug and in release. It also never holds the 1 GiB whole payload.
- On campaign the difference is noise.

## AC 25: dry-run dilation cost

The ignored yardstick was rerun in release on both new PRLs. Baked id 51 equals direct evaluation at every breakpoint: 0 mismatches in 58,540 (camera, lead) checks on the hallway and 0 in 7,570 on campaign. MiB, worst / p95:

| Map | L | Dilated | Undilated | Dilation cost, worst / p95 |
|---|---|---|---|---|
| Hallway | 0 m | 121.4 / 48.9 | 97.0 / 34.6 | +25.2% / +41.1% |
| Hallway | 16 m | 136.5 / 84.7 | 111.7 / 63.5 | +22.2% / +33.5% |
| Hallway | 32 m | 170.0 / 120.2 | 125.7 / 94.0 | +35.3% / +27.8% |
| Campaign | 0 m | 37.0 / 35.0 | 27.7 / 25.0 | +33.4% / +39.7% |
| Campaign | 16 m | 44.1 / 44.1 | 37.1 / 37.1 | +18.7% / +18.7% |
| Campaign | 32 m | 49.3 / 46.2 | 43.4 / 43.4 | +13.6% / +6.5% |

- Every figure equals research §1, as do the shelf layer counts (hallway dilated at 0, 16, 32 m: 10 / 5, 12 / 7, 14 / 10).
- The runtime check on dilation: off-lattice eyes draw blocks outside the dilated set on both maps at every lead. At L ≥ 16 m, the lead and retention always had them resident. At L = 0 m on the hallway tour, 5 such block-frames rendered SH-only. **Dilation alone does not suffice; dilation plus a 16 m lead did, on these walks.**

## Low budget (256 MiB for lightmap-shaped data)

The brief streams only ids 22 and 42. Low must also hold the animated atlas and the id-25 weight maps (research §Other pins), which stay resident.

| Hallway, default levers, streamed | MiB |
|---|---|
| Static pool, first generation: 15 layers plus the spare at 14 MiB | 224 |
| Animated atlas (renderer meter: 3 pages of 1024², Rgba16Float + Rgba8) | 36 |
| Id-25 weight maps (not in the meter; research §Other pins, unmeasured here) | 35.4 |
| **At install** | **about 295** |
| After the tour's one growth (17 array layers) | about 309 |
| During that growth (both generations for two or more frames) | about 533 |

- Campaign streamed: 112 + 48 (meter) + weight maps. Campaign all-resident is 98 + 48 = 146 MiB (meter). **Streaming costs campaign 14 MiB more than all-resident**, the spare layer, because its whole level fits the first generation.
- Research §Other pins gives the hallway's animated atlas as 144 MiB. The current meter reports 36 MiB (the compact atlas). Update the pin.
- Low's remainder after the animated atlas and weight maps is about 185 MiB: 12 usable layers plus the spare (182 MiB). Worst resident content, 170 MiB, fits that only with a from-scratch packing (14 shelf layers at 32 m, 12 at 16 m) and no growth.

## Anomalies and follow-ups

No bug was found: every counter balanced, no read or install failed, and no batch broke its contract. These design findings feed the recommendation:

1. **Growth never shrinks, and each growth holds two generations.** Cap 15 on the hallway tour grows once and keeps 17 array layers. Below the cap the pool ratchets (cap 7 reaches 19 layers after 12 growths).
2. **Repack cannot compact.** "A repack never moves a block upward" keeps one spare layer sufficient, but it also blocks the from-scratch layout. The result is growth at caps above the shelf worst, and 30 repacks of about 200 copies each per 2,000 tour steps.
3. **The first generation is sized at the cap** (Task 8a note), even when the camera needs less: 224 MiB on the hallway at install.
4. **The payload split copies on the frame thread** (AC 23). The issuer thread could split, or read each plane into its own buffer.
5. **The first drain pays for a cold staging pool**: 2.4 ms/MiB against 0.25 warm. The spawn preload (30–140 MiB) is exactly that drain.
6. **Churn is high.** The hallway tour read 24.7 GiB for about 1 GiB of unique pairs over 26 minutes of play: 15.5 MiB/s. The band's outer edge thrashes, as research §1 predicted.
7. **Visibility fallback.** 652 frames of the hallway random walk took the step-limit fallback (baked set only, no visible demand). None missed.

## Reproduction

Harnesses (test-only, ignored):

- `crates/postretro/src/session/lightmap_residency/walk_measurement/` holds `mod.rs`, `paths.rs` (a port of the dry run's `camera_walks.rs`) and `pool_mirror.rs`. It carries two ignored tests: `lightmap_residency_walks_from_prl` and `lightmap_install_timing_from_prl`.
- `crates/renderer/src/lighting/lightmap/stream/install_timing_measurement.rs` carries the ignored `lightmap_drain_install_timing`.

```text
# AC 22 / 23 walks (default plus the cap and lead sweeps; RUNS=default for one config)
POSTRETRO_LIGHTMAP_WALK_PRL=$PWD/content/dev/maps/stress-warren-hallway-inspection.prl \
POSTRETRO_LIGHTMAP_WALK_STEPS=2000 \
  cargo test -p postretro --bin postretro lightmap_residency_walks_from_prl -- --ignored --nocapture

# AC 24 load and preload, once per mode
POSTRETRO_LIGHTMAP_STREAMING=all-resident \
POSTRETRO_LIGHTMAP_WALK_PRL=$PWD/content/dev/maps/campaign-test.prl \
POSTRETRO_LIGHTMAP_WALK_SPAWN=-65.84,2.6,-45.92 \
  cargo test -p postretro --bin postretro lightmap_install_timing_from_prl -- --ignored --nocapture

# Install time per drain, warm and cold
cargo test -p postretro-renderer --lib lightmap_drain_install_timing -- --ignored --nocapture

# Capture JSON: scene with "measurement": {"report": ..., "warmup_frames": 1, "sample_frames": 1}
cargo build -p postretro --features capture
POSTRETRO_SH_STREAMING=sync-proof POSTRETRO_LIGHTMAP_STREAMING=stream \
  target/debug/postretro --capture <scene.json>

# AC 25
cargo test --release -p postretro-level-compiler --bin prl-build --no-run
POSTRETRO_LIGHTMAP_RESIDENCY_DRY_RUN_PRL=$PWD/content/dev/maps/<map>.prl \
  target/release/deps/prl_build-<hash> lightmap_residency_dry_run_from_prl --ignored --nocapture
```

The raw run outputs lived in the measuring session's scratchpad and were not retained; the tables above are their content. Rerun the commands to regenerate them.

## Manual: owner visual read (AC 26)

Pending. Walk the hallway and campaign-test at the default lead with the Streaming-tab pool-cap and lead sliders live. Check for pop-in, seams at block edges and animated-light correctness. While there, read the `[Startup]` line (`RUST_LOG=info`) for `streaming_preload` and `first_level_frame` in both `POSTRETRO_LIGHTMAP_STREAMING` modes, which this session could not reach.

- Result: _pending_

## Handoff: Windows GPU timing (AC 27)

This Mac lacks `TIMESTAMP_QUERY`, so the timing ran on the owner's Windows box. On Windows, walk both maps with `POSTRETRO_GPU_TIMING=1`, streamed and all-resident. Record the forward pass delta from the vertex block-table fetch and the per-lookup offset. Also record repack drain GPU time: a repack copies about 200 blocks through the spare layer.

- **Partial result (owner, 2026-09-29, Windows, hallway, streamed, branch as of `e9cb411e2`):**
  - The owner reports the hallway back above 60 fps throughout. It ran below that before, a slowdown that predates this branch.
  - The lightmap meter reads 224 MiB static: irradiance 64, direction 32 and shadowmask 128 MiB, which is 15 layers plus the spare. The animated pair adds 36 MiB (24 + 12), for 260 MiB in total. That matches the walk harness's first generation at cap 15. All-resident, the same map holds about 1.38 GiB of static pool.
  - GPU timing at one pose: cull 0.00 ms, depth prepass 0.02 ms, forward 2.75 ms, SH compose 0.74 ms, animated direct SH compose 0.97 ms, billboard direct scatter compose 0.15 ms, bloom 0.25 ms, promoted depth cache 0.04 ms, resolve 0.04 ms.
- **Still pending:** the same pose with `POSTRETRO_LIGHTMAP_STREAMING=all-resident`, for the forward-pass delta (the shader is identical, so the delta isolates residency effects); campaign-test; and repack drain GPU time.
