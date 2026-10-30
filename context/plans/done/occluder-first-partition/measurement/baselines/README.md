# occluder-first-partition: before baselines

Recorded 2026-10-27 on branch `claude/occluder-first-partition` at HEAD `b55917964`. The compiler is the frozen binary built at `8a9a34177` (output equals main, per plan.md's Baselines note), copied to `target/occluder-baselines/bin/prl-build-8a9a341` with `scripts-build` beside it. The engine side is the `postretro` bin test harness built from the tree and copied to `target/occluder-baselines/bin/postretro-tests` (run from the workspace root; the after run should build its own and read these records).

Commands are those of `../world-poses/README.md`, with the PRL and records under `target/occluder-baselines/<map>/` (gitignored). The tooling's own tests are not part of this record.

## Machine and bake mode

- Machine: Intel(R) Core(TM) i9-9980HK @ 2.40 GHz (16 threads), macOS 26.6.2 (25G83). Counts are deterministic CPU work, so any machine reproduces them; the wall times are for scale only.
- Every bake: `RUST_LOG=info prl-build-8a9a341 content/dev/maps/<map>.map -o target/occluder-baselines/<map>/<map>.prl --baked-root target/occluder-baselines/baked --no-tui -v <per-map flags>`. The `-v` is needed: without it the frozen binary prints only warnings (no BSP leaf, face, door-sealing, chunk-list or id-51 lines).
- Cache mode: **default stage cache (warm-SH, approximate indirect lighting), pointed at a private, initially empty directory** `--cache-dir target/occluder-baselines/cache`, for every map but the arena, which is `--no-cache` as its flags say. The private directory makes each bake cold, so the log prints every stage's counts (a cache hit prints none). The warm-SH approximation shifts no cell, portal or lightmap block (world-poses README). The after-bakes should use the same flags and a fresh private cache directory. Cache hits also hide those counts, so do not reuse this one. It grew to 27 GB (hallway), see Disk.
- Not `prl-build --release`: the world-poses README's `--release` is the cargo profile, and its commands do not pass the prl-build flag.

## Summary

| Map | Bake flags (besides the common ones) | Wall | PRL | Poses | M1 | M2 deferred texels (random / far / tour) |
|---|---|---|---|---|---|---|
| occlusion-test | none (README) | 10 s | 138 MB | 1,217 (3 m grid, 302 floors, +9 `fc:` poses) | PASS: 0 misses, 0 trips | 0 / 0 / 0 |
| stress-warren-hallway-inspection | `--sh-probe-spacing 4.0` | 1,232 s (20.5 min) | 2.8 GB | 4,596 (9 m grid, 1,149 floors) | PASS: 0 trips, 0 drawn misses; reported: 2 faceless poses, 1 culled pose | 0 / 0 / 0 |
| campaign-test | none (README) | 29 s | 330 MB | 1,448 (3.75 m grid, 362 floors) | PASS: 0 misses, 0 trips | 0 / 0 / 0 |
| arena: stress-warren-showcase | `--sh-probe-spacing 10.0 --lightmap-density 0.25 --no-cache` | 12 s | 33 MB | 5,116 (7.25 m grid, 1,279 floors) | PASS: 0 drawn misses, 0 trips; reported: 13 culled-opening poses (max 5 rays) | 0 / 0 / 0 |
| street: stress-street | `--sh-probe-spacing 4.0 --lightmap-density 0.25` | 26 s | 130 MB | 2,960 (4.5 m grid, 740 floors) | **FAIL as recorded: 518 step-cap trips** (see below) | 0 / 0 / 0, but 19,554 / 25,858 / 1,036 trip frames |

Wall time is the prl-build wall clock. Hallway at 4 m probe spacing and the default 0.025 m lightmap density: the earlier 11 h was at 1 m spacing.

### Street: the before build trips the step cap

On stress-street, 518 of 2,960 poses (17.5%) hit the walk's 5,000-test step cap in the before build (mean portals tested 2,043, p95 and max 5,000; drawn cells mean 272, max 1,894). The ray-grid guard fails on those trips by definition and the sweep test panics, but its before record was still written (`stress-street.before.sweep`). Three consequences for the after comparison:

- **M1 is not a clean before.** Rays run only on portal-path poses, so the 518 tripped poses have no rays. The paired check reads "no new trips" against a before that already has 518; the after build clears the step cap only if the change cuts the walk, which is the brief's aim. Decide whether the street's before trips are exempt or a target.
- **M2 on the street is degenerate.** Tripped frames walk a truncated set, so deferral reads 0 beside 2.02 G, 2.69 G and 82 M left-behind texels (random walk, far-point tour, pose tour). After the change the walk may complete and defer real area. "A walk that deferred nothing before defers nothing after" will bind on a before that never saw the full draw.
- The id-51 bake also step-limited 43,457 of 321,372 walks on this map (`CellResidencySet sampling`), and the id-51 table holds 3,606,142 entries (28.9 MB), max 1,985 per camera, against 24,201 on campaign-test.

## M1 per-pose headline (160 x 90 rays, 100 deg HFOV, near 0.1 m)

Portals tested per pose: mean / p95 / max. Drawn cells, mean / max.

| Map | Portals tested | Accepted mean | Drawn cells | Seen mean | Misses (drawn / faceless / culled) | hit_solid max | id51_outside mean / max |
|---|---|---|---|---|---|---|---|
| occlusion-test | 464.3 / 1,749 / 2,084 | 190.2 | 66.6 / 256 | 43.8 | 0 / 0 / 0 | 0 | 0 / 0 |
| hallway | 65.3 / 196 / 998 | 22.1 | 11.9 / 97 | 9.5 | 0 / 3 rays max / 1 ray max | 60 | 0 / 21 |
| campaign-test | 108.1 / 296 / 524 | 34.4 | 15.9 / 62 | 12.3 | 0 / 0 / 0 | 0 | 0 / 0 |
| arena | 58.8 / 202 / 686 | 20.1 | 9.6 / 61 | 8.2 | 0 / 0 / 5 rays max | 68 | 0 / 0 |
| street | 2,042.6 / 5,000 / 5,000 | 664.9 | 272.1 / 1,894 | 41.7 | 0 / 0 / 0 (portal-path poses only) | 7 | 15.5 / 543 |

Reported (not failed) misses: hallway has 2 poses with hits in unreached faceless cells and 1 in a cell entered across a culled opening; the arena has 13 poses with hits across a culled opening, all new in the sense that there is no earlier record. `hit_solid` counts rays whose pulled-back hit locates in a solid cell; it is not a miss class. The full per-pose columns (faces, triangles, lightmap texels, SH clusters, fog volumes) are in each `sweep.out`.

## M2 bake gap (completed reads, L 16 m, cap 15, 2 frames per pose on the tour)

Deferred texels are 0 on every map and sequence, so after must also be 0 everywhere. Left-behind (unadmitted) texel area, reported beside it:

| Map | Random walk frames / left-behind texels | Far-point tour | Pose tour | Trip frames |
|---|---|---|---|---|
| occlusion-test | 54,370 / 0 | 70,432 / 0 | 2,434 / 0 | 0 |
| hallway | 93,837 / 28,869,408 | 95,392 / 40,551,792 | 9,192 / 23,232,976,320 | 0 |
| campaign-test | 77,575 / 0 | 88,704 / 0 | 2,896 / 157,395,616 | 0 |
| arena | 98,360 / 0 | 116,745 / 0 | 10,232 / 9,264,208 | 0 |
| street | 75,374 / 2,020,796,272 | 106,267 / 2,686,359,120 | 5,920 / 82,482,400 | 19,554 / 25,858 / 1,036 |

Outside-baked-set blocks (random / far / tour): occlusion-test 0 / 0 / 0; hallway 129 / 112 / 136; campaign-test 0 / 0 / 0; arena 370 / 1,614 / 8; street 3,588,615 / 4,832,123 / 135,912. The occlusion-test and hallway frame counts match the world-poses README's validation (hallway there was an older bake).

## M5 structure (from the PRL)

| Map | Cells (interior / solid / exterior) | Portals (walkable) | Faces / tris | Lightmap blocks | SH clusters (oversize) | Vis components / coupled pairs | id-51 entries (camera cells) | Lights / fog |
|---|---|---|---|---|---|---|---|---|
| occlusion-test | 555 (287 / 171 / 97) | 960 (693) | 1,018 / 2,036 | 287 | 197 (0) | 173 / 6,601 | 81,199 (555) | 9 / 7 |
| hallway | 5,671 (2,082 / 3,178 / 411) | 5,800 (4,722) | 8,457 / 16,910 | 2,083 over 2,080 cells | 3,386 (0) | 3,181 / 47,886 | 356,354 (5,671) | 345 / 0 |
| campaign-test | 487 (207 / 166 / 114) | 837 (524) | 808 / 1,612 | 207 | 188 (0) | 168 / 4,545 | 24,201 (487) | 34 / 5 |
| arena | 1,479 (560 / 691 / 228) | 1,894 (1,333) | 2,239 / 4,478 | 560 | 752 (0) | 693 / 13,235 | 36,968 (1,479) | 94 / 0 |
| street | 3,709 (1,985 / 1,547 / 177) | 6,706 (6,288) | 7,657 / 15,315 | 1,985 | 1,915 (0) | 1,549 / 40,096 | 3,606,142 (3,709) | 25 / 0 |

Interior cell size distribution, largest-extent p50 / p95 / max (m): occlusion-test 13.0 / 21.5 / 26.0; hallway 10.2 / 26.0 / 52.0; campaign-test 9.75 / 22.8 / 35.8; arena 16.3 / 29.3 / 94.3; street 5.84 / 24.4 / 88.0. Volume, smallest-extent, portal area, aspect and normal histograms are in each `structure.out`. Hallway's bake log prints 8,445 faces and 16,886 triangles against the PRL's 8,457 and 16,910; the other four match their logs exactly. The 12-face gap is unexplained (not chased); the PRL numbers are the M5 ones.

### M7: arena regions (`stress-warren-showcase`, `region-arena.out`, `region-stairs.out`)

- Arena interior box `-45.5168,1.6256,-61.7728,13.0048,17.8816,61.7728`: 58 interior cells, 117 walkable portals on 40 planes.
- Staircase box `6.5024,1.6256,35.7632,13.0048,11.3792,58.5216`: 32 interior cells, 58 walkable portals on 25 planes. The cell and plane listing (riser at 0.7112 m, tread at 0.3048 m spacing) is in `region-stairs.out`. The first lines show cells already cut at stair height (e.g. cells 965, 967, 970 ... at 0.71 m steps on y 7.11-8.13), so the before build slices the staircase on tread planes.

## M3 door and mover sealing (log lines `kinematic mover N seals generated portal P`)

Portal ids are leaf-era and will change; compare per mover id and count.

| Map | Movers that seal | Lines (movers: portals sealed) |
|---|---|---|
| occlusion-test | none | 0 (the map has movers, none seals) |
| hallway | 9 (movers 0-8) | 23: m0 x5, m1 x4, m2 x2, m3 x2, m4 x5, m5 x2, m6 x1, m7 x1, m8 x1 |
| campaign-test | 1 (mover 0) | 2: portals 695, 696 |
| arena | 6 (movers 0-5) | 15: m0 x1, m1 x1, m2 x4, m3 x4, m4 x1, m5 x4 |
| street | none | 0 (no movers) |

The full lines are in each `bake-extract.txt`.

## M4 leak status

No map logs a leak. A leak would show as `Exterior probe landed in a solid leaf ... boundary leak` or `No interior empty leaves remain ... unsealed or have a leak`; neither appears in any of the five logs, and each finds exterior and interior leaves (see below). The watertightness line is a separate diagnostic (open edges, "possible holes", build succeeds):

| Map | Exterior / interior empty leaves | Leak | Open edges |
|---|---|---|---|
| occlusion-test | 97 / 287 | none | 0 |
| hallway | 411 / 2,082 | none | 283 |
| campaign-test | 114 / 207 | none | 9 |
| arena | 228 / 560 | none | 24 |
| street | 177 / 1,985 | none | 396 |

## Other Reported counts the log prints

- Chunk light list: grid and max per chunk only (no drop or overflow counter in this binary): occlusion-test 300 chunks, 9 lights, max 4; hallway 5,642 chunks, 344 lights, max 8; campaign-test 1,232 chunks, 16 lights, max 6; arena 2,625 chunks, 49 lights, max 8; street 1,140 chunks, 25 lights, max 7. Shadowmask: no light dropped above the floor on occlusion-test and hallway.
- Camera cells without an id-51 eye point: 0 on all five maps. Step-limited id-51 walks: 0 on every map except the street (43,457 of 321,372).
- Lightmap charts: the frozen binary's log does not print a chart count. Blocks (one per interior cell, except the hallway's 3 extra) are in the M5 table.
- Hallway, extra: 4,197,801 animated-light chunks exceeded cap 4 at the min-extent floor (7,597,910 extra entries retained), warned by `AnimatedLightChunks`.
- Classifier output (exempt/kept brushes and clusters): none; the frozen binary has no classifier.

## Files

In this directory, per map: `<map>.poses` (the pin), `record.out`, `sweep.out`, `bakegap.out`, `structure.out`, `bake.time` (flags, wall), `bake-extract.txt` (log lines for M3, M4, counts); the arena also has `region-arena.out` and `region-stairs.out`.

Left in `target/occluder-baselines/<map>/` because together they pass 5 MB (the bake-gap records alone are 49 MB): `<map>.before.sweep` (0.5 to 2.1 MB each), `<map>.before.bakegap` (6.9 to 11.9 MB each), `bake.log`, the PRL (2.8 GB for hallway, 33 to 330 MB for the rest), and, for occlusion-test, `bake.warm-shared-cache.log` (the same bake against the shared default cache, for comparison). Gitignored; they survive sessions but not `cargo clean` of the whole `target/`.

The after run: same flags, a fresh `--cache-dir`, same pose files (`POSTRETRO_WORLD_POSES_FILE`), `POSTRETRO_WORLD_POSES_BEFORE` pointing at the kept `.before.sweep` and `.before.bakegap` records.

## Disk

Free space was 40 GB at the start, 29 GB at the end. The 27 GB private `target/occluder-baselines/cache` is the cause, nearly all from the hallway bake; it has no further use for the baselines and is safe to delete (not done here).
