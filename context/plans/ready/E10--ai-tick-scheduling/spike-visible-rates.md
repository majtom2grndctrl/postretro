# Spike: visible NPC rates (E10--ai-tick-scheduling rival check)

Findings, measured. The question: is "visible NPCs act at 60 Hz, decide in 15 Hz pools, dormant tier for targetless unperceived NPCs" (the rival) almost as cheap as the brief's 30 Hz cap with 15/30 Hz steering? Throwaway patch, not built on (2026-11-17, against 635edd536); dropped from the tree; recover it from git history (`git log --diff-filter=D -- '*spike-visible-rates.diff'`).

**Machine:** cloud container, 4 vCPU Intel Xeon @ 2.10 GHz, no perf. Each timing run was single-threaded. Some runs shared the box with a callgrind process on another core. Baseline chase AI medians were 1.00 / 2.92 / 8.27 ms at 32/64/128, against the owner's 1.02 / 2.94 / 8.24 ms.
**Build:** `CARGO_PROFILE_RELEASE_STRIP=none CARGO_PROFILE_RELEASE_DEBUG=line-tables-only`, target and TMPDIR on /dev/shm because the root volume was full.
**Commands:**
- Packed row: `RS_ONLY=chase:128,… RS_NAVIDX=0|1 RS_STEER_STRIDE=1|2|4 <bin> --ignored replan_stress_row --nocapture --test-threads=1`
- Arenas: `RS_MODE=today|rival|brief30|brief15 RS_WAVES=64 RS_OFF=436 RS_SPLIT=1 RS_NAVIDX=1 <bin> --ignored spike_arena_row …`
- Profiles: `valgrind --tool=callgrind`, read as inclusive Ir with `callgrind_annotate --inclusive=yes`.

**Throwaway patches:**
- A load-time cell→candidate-regions index (CSR) for `region_at` and `resolve_region_at`. On 20k random points (stacked Y, off-grid, off-mesh) it gave 0 mismatches against the linear scans.
- A per-agent steering schedule: stagger by `(slot + tick) % stride`, scale dt by the stride. Separation still sees every agent.
- An AI include filter. The harness calls the AI once per stride group, with dt scaled.
- A sibling ignored test with 4 arenas, each 64×64 m, joined by 40 m corridors 4 m wide, with 0.8 m walls. Bodies collide with the walls and sight passes over them. The nav mesh is 2×2 m tiles: **4,216 regions** and about 8.3k portals. env-volumes has 4,326 regions.

## 1. Baseline, packed stress row (ms per tick, p50 / p95)
The packed row's nav graph is **one region** (`OpenFloor`). That is not representative: its steering cost has no nav-scan term.

| wave | chase AI | chase steer | strafe AI | strafe steer |
|---|---|---|---|---|
| 32 | 1.00 / 1.11 | 0.14 / 0.16 | 0.73 / 1.04 | 0.15 / 0.19 |
| 64 | 2.97 / 3.31 | 0.27 / 0.32 | 1.45 / 1.58 | 0.19 / 0.22 |
| 128 | 8.53 / 11.18 | 0.57 / 0.77 | 3.91 / 4.80 | 0.34 / 0.41 |

## 2. Steering split, packed chase (callgrind inclusive, share of the steering tick)
| wave | collide_and_slide | separation (n², inlined) | region lookups | replans + rest |
|---|---|---|---|---|
| 32 | 83% (0.11 ms) | ~5% | 5% | ~7% |
| 64 | 81% (0.22 ms) | ~7% | 6% | ~6% |
| 128 | 76% (0.43 ms) | ~13% (0.07 ms) | 6% | ~6% |

On the 4,216-region arena map, the nav index cut the spread wave's steering from 0.57 → 0.36 ms (64) and 1.02 → 0.79 ms (128), means. On the multi-arena map it cut steering from 4.3 → 2.9 ms. The "~80% of steering is `region_at`" claim matches only a large graph with crowded replan gates. On these maps `collide_and_slide` dominates once the index is in.

## 3–4. Steering after the nav index, at 60 / 30 / 15 Hz (staggered; mean, then p95 / max)
| row | 60 Hz | 30 Hz | 15 Hz |
|---|---|---|---|
| packed chase 64 | 0.29 (0.40 / 0.61) | 0.15 (0.18 / 0.31) | 0.08 (0.12 / 0.26) |
| packed chase 128 | 0.56 (0.67 / 1.16) | 0.31 (0.40 / 0.89) | 0.17 (0.24 / 1.21*) |
| packed strafe 128 | 0.36 (0.45 / 0.77) | 0.19 (0.24 / 0.36) | 0.11 (0.15 / 0.30) |
| spread arena 64 | 0.36 (0.50 / 0.77) | 0.28 (0.43 / 0.55) | 0.16 (0.28 / 0.56) |
| spread arena 128 | 0.79 (1.08 / 2.62) | 0.40 (0.57 / 1.14) | 0.31 (0.43 / 2.25*) |

\* A single-tick outlier; p95 is the stable worst tick. Staggering keeps the per-tick cost flat: the worst tick is p95 ≈ 1.4–1.8× the mean, at every rate.

Correctness: no tunnelling or off-mesh agent at any stride, 2 Hz included (0.5 s steps at 3.5 m/s against 0.8 m walls). Replans per wall-second drop with the stride because the staleness and cooldown windows count ticks.

## 5. AI split, packed chase (callgrind inclusive, share of AI tick)
| wave | combat positioning | targeting (select + candidate + scope) | rest of evaluate (LOS, reachability, graph, clip) | outside evaluate (snapshot, sync, arbitration, apply) |
|---|---|---|---|---|
| 64 | 55.6% | 26% identified (≤41%) | ≤15% | 3.2% |
| 128 | 47.9% | 32% identified (≤49%) | ≤17% | 3.0% |

Decide work (positioning, targeting, LOS, reachability, guards) is about 96% of the AI tick. The act-every-tick work (clip clock, heading, arbitration, apply, plus the per-tick snapshot and sync overhead) is about 4%: 0.12 ms at 64 and 0.33 ms at 128.

AI tick estimates (**estimates**, not measured):
- **Decide in 15 Hz pools, act at 60 Hz, today's code:** 0.82 ms (64), 2.32 ms (128). With act at 30 Hz: 0.76 / 2.15 ms.
- **Targeting n² term removed** (pawn index, about ⅓ of AI at 64 and 45% at 128): AI at 60 Hz ≈ 1.95 / 4.6 ms. With 15 Hz pools: **0.58 / 1.40 ms**. With act also at 30 Hz: 0.52 / 1.23 ms.
- If `perf-ai-tick-cost` also meets its own goal (≤ 1.0 / 2.4 ms at 60 Hz), pooled AI is 0.34 / 0.85 ms.

## Large arenas and off-arena NPCs (measured, 4,216-region map, means in ms)
- **Spread wave alone** (arena 0, converging). AI 37.3 / 83.9 ms at 64 / 128 today, falling to 20.6 / 45.3 with the nav index. Early, while spread: 50 / 115 ms. Late, converged: 32 / 77 ms.
  - Callgrind (64): positioning is 89% of the AI tick, and `find_path_from` is 88% of the whole run.
  - 70% of all instructions are `calloc` + `memset`. Each route query allocates and zeroes `g_score` and `came_from` sized to the graph's nodes (`path.rs:600–605`), so every query costs O(portals) before the search starts.
  - On the 1-region packed row this is invisible.
- **Multi-arena, Quake/Doom class** (64 wave + 436 off-arena = 500 NPCs; nav index on). Per role, AI / steering at 60 Hz:

  | role | AI at 60 Hz | steering at 60 Hz |
  |---|---|---|
  | wave | 29.3 | 0.47 |
  | idle (218) | 12.0 | 0.65 |
  | patrol (109) | 6.1 | 0.65 |
  | far-targeted (109, ~250 m away) | **485** | 1.1 |

  - Idle and patrol AI is the n² candidate scan: every brain walks all 500 pawns.
  - Far targets are combat positioning running a long A* per candidate slot per tick: 96% `find_path_from`, a third of it zeroing.
  - The dormant tier (whole tick every 30th tick, staggered) measured idle 12.0 → 0.74 and patrol 6.1 → 0.29 AI, and 1.30 → 0.13 steering. That saves **17.0 ms AI + 1.2 ms steering**.
- **Serious Sam class** (128 + 872 = 1,000 NPCs, 120 ticks). Per role, AI at 60 Hz:

  | role | AI at 60 Hz |
  |---|---|
  | wave | 80 |
  | idle (436) | 50 |
  | patrol (218) | 25 |
  | far (218) | **1,117** |

  - Steering totals 6.1 ms at 60 Hz.
  - The dormant tier takes idle + patrol AI from 75 → 3.2 ms and their steering from 3.0 → 0.3 ms.
  - The brief's 15 Hz steering takes total steering from 3.3 → 1.9 ms.

## Totals: AI + steering per tick (ms; (b) and (c) estimate AI as in §5, steering is measured)
- (a) today, 60 Hz, no index
- (b) rival: nav index, pawn-index targeting, 15 Hz decide pools, dormant tier, visible NPCs acting at 60 Hz
- (c) the brief: as (b), plus act at 30 Hz and 15 Hz steering

| scenario | (a) today | (b) rival | (c) brief | (b)−(c) |
|---|---|---|---|---|
| packed 64 | 3.24 | 0.86 | 0.60 | 0.26 |
| packed 128 | 9.10 | 1.95 | 1.39 | 0.56 |
| spread arena 64 | 37.8 | 5.9 | 5.5 | 0.4 |
| spread arena 128 | 84.9 | 12.8 | 11.9 | 0.9 |
| multi-arena 500 (with far chasers) | 617 | ~140 | ~138 | ~2.7 |
| multi-arena 500, excluding far role | 69.6 | 9.6 | 9.3 | 0.3 |
| multi-arena 1,000 (with far; index on) | 1,278 | ~300 | ~298 | ~2.4 |
| multi-arena 1,000, excluding far (index on) | 159 | 24.5 | 24.0 | 0.5 |

Against a 16.7 ms frame:
- The rival fits the packed rows easily: 0.9 / 2.0 ms is 5% / 12% of the frame.
- The visible-rate cut saves 0.26–0.56 ms there (packed) and 0.4–0.9 ms in the spread arena.
- In the arena and map rows, the dormant tier is worth 17–80 ms, and route-query cost in combat positioning dominates whatever is left.
- With 15 Hz decide pools, the spread arena is still 6–13 ms and the far-chaser map is still 140–300 ms. Neither tick-rate plan touches that.

## Caveats and confidence
- AI columns (b) and (c) are callgrind-share estimates. The pawn index and positioning cache are not built, so "n² removed" is an assumption. Steering columns and dormant-tier savings are measured.
- Act share (~4%) is inferred from what the decide functions leave over. Inlining blurs function boundaries, so take ±2 points.
- Split-mode role numbers include one snapshot pass per call, ~0.05 ms each. Running the AI twice per tick in the spike advances attack-turn clocks twice. Neither changes the conclusions.
- The arena map is a flat tiled grid with few triangles. Real `collide_and_slide` on dense geometry costs more, so the brief's steering saving would be somewhat larger. The route-query cost scales with portal count, which is realistic here.
- The wave graph has no candidate filter, so far chasers acquire across the map. Content with candidate-range filters would have fewer far chasers. Confidence: high on direction, medium on the estimated (b)/(c) AI magnitudes.

## Recommendation
The rival suffices on cost.
- After the nav index, 60 Hz visible steering is 0.3 ms at 64 engaged and 0.6–0.8 ms at 128. The brief's 30 Hz cap with 15/30 Hz steering saves 0.3–0.9 ms per tick at Quake/Doom (~64) and Serious Sam (~128) class in the visible arena. That is about 2–5% of a 16.7 ms frame.
- Buying that saving takes host interpolation, a tick-pose hit contract, a raised client delay, clip quantization and a snapshot wire change.
- The savings that matter come from elsewhere:
  - the **dormant tier**: 17 ms at 500 placed, 75 ms at 1,000;
  - **15 Hz decide pools**: about ¾ of the AI tick;
  - the pawn index;
  - something neither brief names: **route-query cost in combat positioning**. It zeroes graph-sized A* arrays on every query, and runs one long query per candidate slot for far chasers. It costs 20–45 ms for one spread wave and 0.5–1.1 s for ~100–200 far chasers on a 4k-region map.

Recommend: keep the dormant tier and decide pools, drop the visible-rate cut (keep visible NPCs acting at 60 Hz), and add route-query work (reused, generation-stamped A* scratch; a cap or distance gate on per-candidate routes) to `perf-ai-tick-cost` or a sibling brief, since it outweighs every rate cut measured here.
