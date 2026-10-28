# occluder-first-partition: after sweep (Task 9)

Recorded 2026-10-27 on `claude/occluder-first-partition` at HEAD `5426b60b4`, plus one uncommitted compiler fix made in this task (§Change made). Compared against `../baselines/README.md`. Commands are those of `../world-poses/README.md`. The before pose files and before records were replayed and never re-recorded.

## Build, flags and cache mode

- Compiler: release `prl-build` built from HEAD plus the fix, frozen as `target/occluder-after/bin/prl-build-fix`, with `scripts-build` beside it. `prl-build-5426b60` beside it is HEAD without the fix. It made the first after-bakes, now kept under `target/occluder-after/<map>/prefix/`.
- Engine side: the `postretro` bin test harness built from HEAD, frozen as `target/occluder-after/bin/postretro-tests`. The Task 9 fix touches only the compiler, so the harness equals HEAD's.
- Every bake: `RUST_LOG=info prl-build-fix content/dev/maps/<map>.map -o target/occluder-after/<map>/<map>.prl --baked-root target/occluder-after/baked --no-tui -v <flags>`. The flags match the before bakes exactly. Each map but the arena used a fresh private `--cache-dir` under the session scratchpad, deleted when its bake finished. The hallway's reached 26 GB.
- Machine and counts: the same Intel i9-9980HK Mac as the baselines. All counts are deterministic CPU work, and wall times are for scale only.

| Map | Flags besides the common ones | Wall before → after |
|---|---|---|
| occlusion-test | (private cache) | 10 s → 10 s |
| stress-warren-hallway-inspection | `--sh-probe-spacing 4.0` (private cache) | 1,232 s → 1,064 s |
| campaign-test | (private cache) | 29 s → 26 s |
| arena: stress-warren-showcase | `--sh-probe-spacing 10.0 --lightmap-density 0.25 --no-cache` | 12 s → 9 s |
| street: stress-street | `--sh-probe-spacing 4.0 --lightmap-density 0.25` (private cache) | 26 s → 9 s |

## Verdicts

| Row | occlusion-test | hallway | campaign-test | arena | street |
|---|---|---|---|---|---|
| **M1** ray grid ⊆ walk, no step-cap trip | PASS | PASS | PASS | PASS | **FAIL: 29 trips** (before 518), 0 drawn misses |
| **M2** deferred texel area after ≤ before; 0 stays 0 | PASS (0/0/0) | **FAIL: random walk 0 → 3,067,968 texels** (4 blocks, 2 frames) | PASS | PASS | PASS (0/0/0; still 2,746 / 4,457 / 58 trip frames, before 19,554 / 25,858 / 1,036) |
| **M3** every mover that sealed before still seals | PASS (no sealing mover) | PASS (m0–m8) | PASS (m0; m1 seals now too) | PASS (m0–m5) | PASS (no movers) |
| **M4** leak status unchanged | PASS (none) | PASS (none) | PASS (none) | PASS (none) | PASS (none) |
| M5–M7 | reported below | | | | |

M1 and M2 each fail on one map, and each escalation is below: §M1 street and §M2 hallway. Neither has a fix that keeps every Decision, so both go to the owner.

## Change made: tier portals keep fragments under the area cull

**Defect.** In the first after-bake (HEAD), the street failed M1 with one new drawn miss. At pose `g8.20.0@270`, 2 rays saw window cell 3234 through a 0.81 m² window, but the runtime portal covered only 0.637 m² of it. `generate_occluder_tier_portals` pushed each tier piece through both sides' sub-trees, and `distribute_winding` applied `MIN_PORTAL_AREA_M2` to every sub-tier fragment. Below-tier planes from the street-side leaves cut the window's east 0.36 m into two pieces under 0.1 m². Both were dropped, so the hull of the survivors did not span the window. The trace is in `escalations/street-drawn-miss.txt`. An opening above 4× the cull lost part of its portal, so under-inclusion came from the partition, not from the cull's own rule.

**Fix** (`crates/level-compiler/src/portals.rs`, `crates/level-compiler/src/portals/occluder_tier.rs`):

- `distribute_winding` takes the minimum piece area.
- The full-tree portals and the tier pieces keep the 0.1 m² cull.
- The sub-tier push keeps interior fragments down to `FRAGMENT_DUST_AREA_M2` = 1e-6 m², which drops only clipping dust.
- The pair's hull, which is the runtime portal, must pass the 0.1 m² cull.

The cull therefore still applies, unchanged, to every portal set, and no Decision moves.

**Evidence.**

- Re-baked street: 2,598 runtime portals (was 2,597), 0 drawn misses. The 29 trips and 9 reported culled-opening poses are unchanged, and the uncapped walk cost is unchanged (mean 1,247.7 vs 1,246.4 tests).
- Cells and runtime-portal counts are identical to the pre-fix bakes on the other four maps.
- Regression test `portals::occluder_tier::tests::window_split_below_the_tier_into_pieces_under_the_cull_keeps_its_portal` covers a facade window with thin exempt posts flush against it. Under the old per-fragment cull it fails: the window loses its portal outright.
- `cargo test -p postretro-level-compiler --bin prl-build`: 1,982 passed, 0 failed, 22 ignored (1,981 plus the new test).
- `cargo clippy --workspace --all-targets -- -D warnings` is clean, and `cargo fmt --all -- --check` is clean.
- F3 holds: nothing changed outside `crates/level-compiler`.

## M1: ray-grid guard (160×90 rays, 100° HFOV at 16:9, near 0.1 m)

| Map | Trips before → after | Drawn misses | Reported (culled / faceless) before → after |
|---|---|---|---|
| occlusion-test | 0 → 0 | 0 | 0 / 0 → 0 / 0 |
| hallway | 0 → 0 | 0 | 1 / 2 poses → 0 / 0 |
| campaign-test | 0 → 0 | 0 | 0 / 0 → 0 / 0 |
| arena | 0 → 0 | 0 | 13 / 0 → 0 / 0 |
| street | 518 → **29** | 0 (1 before the fix) | 0 / 0 → 9 / 0 |

The street's 9 culled-opening poses enter cell 2951, a 3.99 × 0.20 × 0.56 m sliver at the promenade floor. Its boundary with the street cell is 0.11 m², under 4× the cull, so these are reported, not failed. Five of the nine poses tripped before, so they had no before rays.

### Street step-cap trips: what drives them

Measured with `escalations/diag`, a scratch reader over the PRL. It runs the unchanged walk, either through the real crate (`diag`) or through a copy of `crates/visibility/src` with only `MAX_PORTAL_WALK_STEPS` raised (`diagu`). That copy measures uncapped cost, and no engine code changed. Data: `escalations/street-walk-cost.txt`.

- **Uncapped walk over the 2,960 before poses:**

  | Build | Mean | p95 | Max | Over the 5,000 cap |
  |---|---|---|---|---|
  | before | 2,553.7 | 8,891 | 18,513 | 518 |
  | after | 1,247.7 | 3,812 | 6,761 | 29 |

  Total portal tests fell from 7.56 M to 3.69 M. 876 poses got more expensive, and 789 reach more cells.
- **26 of the 29 trips also tripped before.** The 3 new ones are `g1.5.0@180`, `g1.6.0@180` and `g7.8.0@180`, upper-deck poses looking down the street. Uncapped they cost 5,439, 5,212 and 5,234 tests (before 4,476, 3,426 and 4,435). They reach 521–547 cells (before 289–376): coarser cells over-include there.
- **Drivers.** The facade sections carve one cell per window, 0.41 m deep with 2 portals each, and building floors become long interior cells with 50–90 window portals each. Cell 3366 is 12 × 4.5 × 76 m with 90 portals. In the tripped poses, about half the reached cells are window cells (e.g. 338 of 645 at `g5.3.0@180`). Each reached cell is expanded about 1.8 times as its rect grows (6,212 tests over a degree sum of 3,374). The remaining cost is honest walking through hundreds of real, portal-visible windows down a 140 m street. It is not a sliver artefact.
- **Options, none taken:**
  1. Raise the step cap. This is runtime, so F3 forbids it.
  2. A cell-size cap, the follow-up `occluder-cell-size-cap`. It cuts the degree of the long floor cells but adds cells, and its effect is unmeasured.
  3. Carve no cell per window. This changes the cluster Decision's openings ruling, so it is the owner's call.
  4. Change `CROSS_SECTION_WEIGHT`. This makes things worse: 43 trips at 2×, 33 at 4×, 29 at 8×, 89 at 16× (§Weight).
  5. Rule that the street's before-tripping poses are exempt. The baseline README left this open.

  The guard as written fails. Whether the 26 also-before trips are exempt, and whether the 3 new ones fail the brief, is an owner ruling.

## M2: bake gap (completed reads, L 16 m, cap 15; deferred texel area per sequence)

| Map | Random walk before → after | Far-point tour | Pose tour | Left-behind texels after (random / far / tour), before in brackets |
|---|---|---|---|---|
| occlusion-test | 0 → 0 | 0 → 0 | 0 → 0 | 0 / 0 / 0 (0 / 0 / 0) |
| hallway | **0 → 3,067,968** | 0 → 0 | 0 → 0 | 985,443,760 / 11,833,922,144 / 46,496,702,832 (28,869,408 / 40,551,792 / 23,232,976,320) |
| campaign-test | 0 → 0 | 0 → 0 | 0 → 0 | 0 / 0 / 225,671,488 (0 / 0 / 157,395,616) |
| arena | 0 → 0 | 0 → 0 | 0 → 0 | 0 / 0 / 962,160 (0 / 0 / 9,264,208) |
| street | 0 → 0 | 0 → 0 | 0 → 0 | 658,793,040 / 637,638,464 / 7,634,304 (2,020,796,272 / 2,686,359,120 / 82,482,400) |

### Hallway escalation (M2 fails)

The deferral is in random-walk frames 73 and 77, during the dwell at the walk's first steps near (−20, 22, 85). Each frame defers 2 blocks of 1,533,984 texels in all, and each has `outside = 0`: no drawn block lies outside the camera cell's baked id-51 set. A deferral here means a pair that needed pool growth while a grown-out generation was still retiring (`rendering_pipeline.md` §4). In these frames the pool grows twice in quick succession.

`escalations/hallway-bakegap-frames.txt` replays frames 55–80 as sweep poses on both builds:

| Frames | Drawn cells before → after | Drawn lightmap texels before → after |
|---|---|---|
| f73 | 12 → 41 | 3.26 M → 23.37 M |
| f77 | 13 → 41 | 4.30 M → 23.37 M |

At these frames, demand is 5–7× before at the same eye, and every drawn cell is inside the baked set. Per pose over the 4,596 sweep poses, drawn lightmap texels rose 46% (mean 6.15 M → 9.00 M, max 47.0 M → 59.4 M). Coarser cells draw bigger blocks and more of them at once, which forces growth.

**id-51 lattice, the delegated answer: not changed.** The delegation allows scaling the lattice with cell extent only when id-51 misses on coarse cells drive the failure. Here they do not: `outside` is 0 on both failing frames. Sweep `id51_outside` max per map:

| Map | Before | After |
|---|---|---|
| hallway | 21 | 6 |
| arena | 0 | 9 |
| street | 543 (mean 15.5) | 360 (mean 1.3) |
| occlusion-test, campaign-test | 0 | 0 |

No camera cell lacks an eye point on any map. The lattice stays at `LATTICE_FRACTIONS`.

**Options, none taken:**

1. Cell-size cap (`occluder-cell-size-cap`). Smaller cells give smaller blocks and lower drawn demand.
2. Split a big cell's charts into several smaller blocks in `block_layout.rs`. This is a residency-granularity change no Decision covers.
3. Size the first pool generation, or growth, for the new demand. This is a runtime change, outside this brief (F3 covers the renderer, and the residency session is engine code).

The gate is a Decision ("the bake gap is the one gate"), so this is an owner stop. The amount is 2 frames of 93,837 on one sequence, at walk start.

## M3: door and mover sealing (seal lines per mover id; portal ids are not comparable)

| Map | Before | After |
|---|---|---|
| occlusion-test | none (1 mover, not a door) | none; door recognition 0 of 1 |
| hallway | m0 5, m1 4, m2 2, m3 2, m4 5, m5 2, m6 1, m7 1, m8 1 | m0 6, m1 4, m2 4, m3 4, m4 6, m5 4, m6 2, m7 2, m8 2; recognition 9 of 11 (m0–m8) |
| campaign-test | m0 2 | m0 5, m1 5; recognition 2 of 3 (m0, m1) |
| arena | m0 1, m1 1, m2 4, m3 4, m4 1, m5 4 | m0 3, m1 3, m2 4, m3 4, m4 3, m5 5; recognition 6 of 8 (lifts 6, 7 not doors) |
| street | no movers | no movers |

## M4: leak status

No after log carries a leak or unsealed line, and neither did any before log.

| Map | Exterior / interior empty leaves before → after (full tree) | Open edges before → after |
|---|---|---|
| occlusion-test | 97 / 287 → 124 / 373 | 0 → 0 |
| hallway | 411 / 2,082 → 416 / 2,950 | 283 → 562 |
| campaign-test | 114 / 207 → 256 / 281 | 9 → 43 |
| arena | 228 / 560 → 206 / 615 | 24 → 24 |
| street | 177 / 1,985 → 306 / 2,427 | 396 → 574 |

The watertightness diagnostic counts open edges ("possible holes you can see through"; the build still succeeds). It rose on three maps. The likely cause is T-junctions where occluder planes now split faces across open space. The count is reported for the `face-fragment-remerge` decision and was not chased.

## M5: structure (from the PRL unless noted)

| Map | Cells (interior / solid / exterior) | Runtime portals (walkable) | Faces = lightmap charts / tris | Lightmap blocks, texel area | SH probes | SH clusters (oversize) | Vis components / coupled pairs | id-51 entries | Partition stage |
|---|---|---|---|---|---|---|---|---|---|
| occlusion-test | 555 (287/171/97) → 461 (69/268/124) | 960 (693) → 124 (124) | 1,018 / 2,036 → 1,047 / 2,094 (**+2.8%**) | 287, 30.98 M → 70, 26.97 M | 72,420 = | 197 (0) → 405 (2) | 173 / 6,601 → 393 / 1,413 | 81,199 → 4,247 | 0.01 → 0.03 s |
| hallway | 5,671 (2,082/3,178/411) → 7,111 (1,222/5,473/416) | 5,800 (4,722) → 1,771 (1,771) | 8,457 / 16,910 → 10,761 / 21,376 (**+27.2%**) | 2,083, 758.4 M → 1,225, 742.1 M | 39,650 = | 3,386 (0) → 6,136 (5) | 3,181 / 47,886 → 5,890 / 23,357 | 356,354 → 183,487 | 0.21 → 0.45 s |
| campaign-test | 487 (207/166/114) → 793 (71/466/256) | 837 (524) → 132 (132) | 808 / 1,612 → 978 / 1,950 (**+21.0%**) | 207, 52.08 M → 73, 48.88 M | 404,928 = | 188 (0) → 739 (4) | 168 / 4,545 → 723 / 1,401 | 24,201 → 4,096 | 0.01 → 0.04 s |
| arena | 1,479 (560/691/228) → 1,180 (199/775/206) | 1,894 (1,333) → 221 (221) | 2,239 / 4,478 → 2,526 / 5,052 (**+12.8%**) | 560, 4.69 M → 199, 4.70 M | 1,360 = | 752 (0) → 1,033 (2) | 693 / 13,235 → 982 / 3,905 | 36,968 → 5,529 | 0.03 → 0.08 s |
| street | 3,709 (1,985/1,547/177) → 4,559 (1,276/2,977/306) | 6,706 (6,288) → 2,598 (2,598) | 7,657 / 15,315 → 10,183 / 20,367 (**+33.0%**) | 1,985, 2.79 M → 1,276, 3.37 M | 7,326 = | 1,915 (0) → 3,878 (23) | 1,549 / 40,096 → 3,284 / 28,457 | 3,606,142 → 1,519,790 | 0.12 → 0.31 s |

Interior cell largest extent, p50 / p95 / max in metres:

| Map | Before | After |
|---|---|---|
| occlusion-test | 13.0 / 21.5 / 26.0 | 18.7 / 52.0 / 52.0 |
| hallway | 10.2 / 26.0 / 52.0 | 6.5 / 42.3 / 120.3 |
| campaign-test | 9.75 / 22.8 / 35.8 | 32.5 / 55.7 / 72.8 |
| arena | 16.3 / 29.3 / 94.3 | 26.0 / 110.5 / 159.3 |
| street | 5.84 / 24.4 / 88.0 | 1.75 / 76.0 / 140.0 |

The street's p50 is low because of its 0.41 m window cells.

The partition time is the bake's stage-timing table (`Partitioning`). It covers the first-pass tree, classification and the two-tier build, and stays under half a second on every map.

Each PRL face is one lightmap chart. The after count includes faces the atlas cut into sub-charts: 8 on occlusion-test and 23 on the hallway. The before bakes cut none.

**Log-only rows:**

- Chunk light lists: total indices are within 2% on every map (155 = 155; 9,585 → 9,591; 314 = 314; 4,075 → 4,071; 503 → 495). This binary prints no overflow-drop counter.
- Shadowmask owners dropped above the floor: campaign-test 0 → 1, street 4 → 2, others 0.
- id-51: camera cells without an eye point are 0 on every map. Street step-limited id-51 walks fell from 43,457 of 321,372 to 2,648 of 206,142.

Full histograms (volume, smallest extent, portal area, aspect, normals) are in each `<map>/structure.out`.

**Classifier output at the starting constants.** The constants are floor 1.2 m, absolute length 20 m, span fraction 0.5, see-over 2.5 m, coverage 0.5, and `CROSS_SECTION_WEIGHT` 8 × `SPLIT_PENALTY`.

| Map | Brushes | Occlude alone | Exempt alone: under floor / no open face / short span / low top | Clusters, sections, sections that occlude (member brushes) | Door recognition |
|---|---|---|---|---|---|
| occlusion-test | 141 | 29 | 69 / 1 / 28 / 14 | 6, 11, 5 (17) | 0 of 1 |
| hallway | 1,608 | 738 | 3 / 60 / 799 / 8 | 65, 99, 12 (51) | 9 of 11 |
| campaign-test | 143 | 40 | 14 / 13 / 65 / 11 | 8, 15, 2 (6) | 2 of 3 |
| arena | 447 | 237 | 3 / 56 / 148 / 3 | 49, 11, 2 (10) | 6 of 8 |
| street | 1,259 | 72 | 1,174 / 10 / 3 / 0 | 29, 37, 11 (827) | 0 of 0 |

## M6: per pose, the Problem's per-frame guardrail (means; full p95/max in each `sweep.out`)

| Per pose, mean | occlusion-test | hallway | campaign-test | arena | street |
|---|---|---|---|---|---|
| Portals tested | 464.3 → 106.8 (−77%) | 65.3 → 38.7 (−41%) | 108.1 → 50.5 (−53%) | 58.8 → 12.5 (−79%) | 2,042.6 → 1,240.5 (−39%, capped) |
| Drawn cells | 66.6 → 18.1 | 11.9 → 9.2 | 15.9 → 10.0 | 9.6 → 5.0 | 272.1 → 130.3 |
| Faces in drawn cells | 233.6 → 425.8 (+82%) | 50.5 → 104.2 (+106%) | 65.8 → 223.6 (+240%) | 40.2 → 81.4 (+102%) | 1,081.4 → 1,499.5 (+39%) |
| **Tris in frustum (GPU cull input)** | 385.9 → 409.4 (**+6%**) | 80.3 → 132.6 (**+65%**) | 101.1 → 218.9 (**+117%**) | 65.4 → 89.1 (**+36%**) | 1,939.3 → 2,036.1 (**+5%**) |
| **Drawn lightmap texels** | 9.41 M → 12.14 M (+29%) | 6.15 M → 9.00 M (+46%) | 5.66 M → 10.36 M (+83%) | 0.12 M → 0.18 M (+53%) | 0.64 M → 0.98 M (+53%) |
| SH clusters of drawn cells | 8.1 → 5.7 (−30%) | 2.7 → 3.5 (+30%) | 2.7 → 3.8 (+41%) | 2.0 → 2.3 (+15%) | 52.2 → 54.5 (+4%) |
| Active fog volumes | 2.6 → 4.0 (+54%) | 0 → 0 | 1.1 → 1.7 (+55%) | 0 → 0 | 0 → 0 |
| id-51 outside (mean / max) | 0 / 0 → 0 / 0 | 0 / 21 → 0 / 6 | 0 / 0 → 0 / 0 | 0 / 0 → 0 / 9 | 15.5 / 543 → 1.3 / 360 |

The street's before figures are truncated by its 518 trips.

Lights admitted by the chunk light lists at the eye are not read, as the world-poses README says.

**Reading.** The walk got 39–79% cheaper on every map, which is the brief's aim. Geometry, lightmap demand and fog processed per frame rose at the same time, because a cell now holds everything its exempt brushes used to cut apart. The worst case is campaign-test. At `g5.26.0@0` the eye stands 0.2 m from a wall inside a 17.9 × 3.3 × 72.8 m cell. The before build drew 1 cell (2 tris in frustum). The after build draws 28 cells (739 tris), while the ray grid still sees 1 cell. The Problem asks for "no significant increase in geometry, effects, textures or probes processed per frame". On campaign-test (+117% tris, +83% texels), the hallway (+65%, +46%) and the arena (+36%, +53%) that is not met as measured. This is for the owner to judge under "Measure, don't gate".

## M7: arena regions (`stress-warren-showcase/region-*.out`)

- **Arena interior box:** 58 interior cells and 117 walkable portals on 40 planes before. After: 14 cells and 19 portals on 7 planes. The planes are y = 8.128 and 11.379, and x = −45.517, −19.507, −16.256, −13.005 and 13.005.
- **Staircase box:** 32 cells and 58 portals on 25 planes before, cut on tread planes. After: 3 cells and 2 portals, both horizontal, at y = 8.128 and y = 11.379.
  - y = 8.128 m (Quake z 320) is not on the 0.3048 m tread grid (21.33 risers up).
  - y = 11.379 m (Quake z 448) is 32 risers up and coincides with the top step's tread. It is also the top face of the arena wall bands, brushes 341–344 (`z 320..448`, 384 u thick), which occlude alone. That plane spans the whole arena (cell 543), so it is the wall band's, not the step's.
  - No vertical riser plane (z = const inside the stair run) bounds any cell.

**Stepped-solid escalation: not triggered.** No riser or tread plane of the staircase reaches the occluder tier. This matches Task 4's R19 result.

## Escalation checks

### `CROSS_SECTION_WEIGHT` recheck (`escalations/weight-sweep.txt`)

Each weight was set temporarily and restored to 8 afterwards; the tree is unchanged. For each weight: the `occluder_tier` (43) and `runtime_cells` (22) tests, release bakes of occlusion-test and street, and the uncapped walk over the before poses.

| × `SPLIT_PENALTY` | Wall-top premise test | Tests | occlusion-test tier leaves / portals / faces / walk mean | street tier leaves / portals / faces / walk mean, over cap |
|---|---|---|---|---|
| 2 | **fails** (and `no_runtime_portal_leads_into_solid…` fails) | 63 / 65 | 161 / 66 / 1,027 / 45.9 | 2,219 / 2,053 / 8,111 / 1,250, 43 |
| 4 | holds (parts at tier node, +Y d = 6.5024) | 65 / 65 | 210 / 91 / **910** / 65.9 | 2,541 / 2,277 / **8,713** / 1,301, 33 |
| **8 (current)** | holds | 65 / 65 | 305 / 124 / 1,039 / 106.7 | 3,682 / 2,597 / 10,183 / 1,246, 29 |
| 16 | holds | 65 / 65 | 526 / 190 / 1,190 / 142.5 | 4,770 / 3,158 / 11,450 / 1,414, 89 |

At 4×, an additional M1 sweep ran on the four fast maps. Occlusion-test, campaign-test and the arena PASS; the street gets 33 trips. Every campaign and arena mover still seals.

Against 8×, 4× cuts faces on all four maps:

| Map | Faces at 8× | Faces at 4× | Before |
|---|---|---|---|
| occlusion-test | 1,039 | 910 | 1,018 |
| campaign-test | 978 | 856 | 808 |
| arena | 2,526 | 2,468 | 2,239 |
| street | 10,183 | 8,713 | 7,657 |

Per-pose means at 4× against 8×:

| Map | Portals tested 8× → 4× | Tris in frustum 8× → 4× |
|---|---|---|
| campaign-test | 50.5 → 21.3 | 219 → 157 |
| arena | 12.5 → 26.5 | 89 (both) |
| occlusion-test | 106.8 → 65.9 | 409 → 408 |

At 4× the street is a little worse: mean 1,291 portals tested and 33 trips.

**Recommendation: keep 8.** The constant was not changed. 4× is the alternative if the owner weighs face growth and campaign-test geometry over street walk cost. Its margin on the wall-top premise is one step from failing, since 2× fails. Changing it would mean re-baking and re-measuring all five maps, so it is left for the owner.

### Floor-plane locate note

Points were located in both builds:

- **Pose points:** at every distinct before pose, the feet, feet + 1 mm, the capsule's lower and upper core points (0.2 and 1.8 m), the capsule centre (1.0 m) and the eye (1.5 m).
- **Spawns:** each map's `player_spawn` origin at the same offsets.

Results:

- Points exactly on a floor's top plane go interior → solid in 190 of 309 floors on occlusion-test, 3 of 1,149 on the hallway, 28 of 1,279 on the arena and 132 of 740 on the street. They go solid → interior in 180 hallway floors and 2 street floors.
- On-plane feet were already solid before in 65, 665, 1,089 and 201 floors respectively, so the on-plane ambiguity predates the change.
- At 1 mm above the plane every point is interior in both builds on every map. The same holds for every capsule core point, capsule centre, eye and spawn.
- `capsule_fits_combined` locates only core points a radius off the floor, so physics and spawns are unaffected.

No action is needed.

### Face and chart growth

Faces, which equal lightmap charts, and triangles rose on every map:

| Map | Growth |
|---|---|
| occlusion-test | +2.8% |
| arena | +12.8% |
| campaign-test | +21.0% |
| hallway | +27.2% |
| street | +33.0% |

Lightmap block texel area moved −13% to +21%. Open-edge diagnostics doubled on the hallway and rose 4.8× on campaign-test.

**Recommendation:** open `face-fragment-remerge`. Above 20% on three of five maps reads as significant, and the remerge would also likely remove the new open edges. The owner judges. `CROSS_SECTION_WEIGHT` 4× is a cheaper partial lever (−12% faces on occlusion-test and campaign-test, −14% on the street).

### Mixed-height sections (open owner question from Tasks 1–7)

The street data does not bear on it directly. Facade A segment 1 and B3 stay exempt as before, with sky above the shorter buildings counting against coverage. The trips and walk cost come from the facades that do occlude: their window cells, and the long floor cells behind them. Occluding more facades would add window cells and portals, not remove them.

## Open owner judgements

1. **M2 hallway, the gate.** 3.07 M deferred texels in 2 random-walk frames, caused by pool growth from coarser cells drawing 5–7× the texels at those eyes, not by id-51 misses. The options are in §M2. The brief cannot land on this gate without a ruling or a further change.
2. **M1 street.** 29 trips (518 before): 26 also tripped before, 3 are new and 4–9% over the cap uncapped. The drawn miss was fixed. The owner rules whether before-tripping street poses are exempt, and whether a cell-size cap or the per-window cells should be pursued.
3. **The per-frame guardrail (M6).** Triangles in frustum rose 117% on campaign-test, 65% on the hallway and 36% on the arena, and drawn lightmap texels rose 29–83% on all five maps, against the Problem's "no significant increase".
4. **Face growth.** +13% to +33% on four maps, so `face-fragment-remerge` is recommended.
5. **`CROSS_SECTION_WEIGHT`.** Keep 8, or take 4× for fewer faces at a slightly costlier street walk.

## Files

Per map, beside this file:

- `<map>/sweep.out`, `bakegap.out`, `structure.out`
- `diff-sweep.out`, `diff-bakegap.out`
- `bake.time`, and `bake-extract.txt` (classifier, door, structure, timing table and seal lines)
- for the arena, also `region-arena.out` and `region-stairs.out`

`escalations/` holds:

- `street-drawn-miss.txt`, `street-walk-cost.txt`
- `hallway-bakegap-frames.txt`
- `weight-sweep.txt`
- `diag/`, the scratch PRL reader (miss trace, leaf sampling, cell portal listing, before/after locate). Build it with `cargo build --release` there. The uncapped variant is the same source built against a copy of `crates/visibility` with `MAX_PORTAL_WALK_STEPS` raised.

Left in `target/occluder-after/<map>/` (gitignored, about 3.3 GB with the PRLs):

- `bake.log`, the after PRL, `<map>.after.sweep` and `<map>.after.bakegap`
- `prefix/`, the HEAD-without-fix logs and records
- `target/occluder-after/bin/`, the frozen binaries
