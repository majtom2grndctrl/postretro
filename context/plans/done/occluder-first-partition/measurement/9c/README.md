# occluder-first-partition: Task 9c re-sweep

Recorded 2026-10-28 on `claude/occluder-first-partition` at HEAD `276df0bb0`, with Tasks 9a, 9b and 9e in. The comparison is against `../baselines/README.md` (before) and `../after/README.md` (Task 9 after, at HEAD `5426b60b4` plus its fix). Commands are those of `../world-poses/README.md`. The before pose files and before records were replayed, never re-recorded.

No committed source changed. Each configuration was a temporary constant edit that was built, frozen and then restored; `git diff` is empty.

Every table here is generated from the raw records by `collect.py`. The full set, with every configuration, is in `tables.md`. The sections below quote it.

## Configurations

| Config | `CELL_SIZE_CAP_M` | `OCCLUDER_ABSOLUTE_LENGTH_M` | Binary (SHA-1 prefix) |
|---|---|---|---|
| cap13 | 13.0048 (512 u, HEAD) | 20 | `6f7feedce785` |
| cap19 | 19.5072 (768 u) | 20 | `7a45639749a8` |
| cap26 | 26.0096 (1024 u) | 20 | `86e98bcd26a1` |
| cap26-len10 | 26.0096 | 10 | `c41db86646bc` |
| cap26-len7 | 26.0096 | 7 | `83a122ab85d8` |

The other caps' length variants were built as well (`prl-build-cap13-len*`, `prl-build-cap19-len*`) but never baked.

## Pinning (testing_guide §Resource bounds)

- **Machine.** The baselines' Mac: Intel i9-9980HK, 16 threads, 32 GiB, macOS 26.6.2 (25G83). Every row is deterministic CPU work, and wall times are for scale only.
- **Compiler.** A release `prl-build` per configuration, frozen as `target/occluder-9c/bin/prl-build-<config>` with `scripts-build` beside it (Task 9a's copy). Each `bake.time` records the binary's hash.
- **Harness.** `target/occluder-9c/bin/postretro-tests` is a copy of Task 9's frozen harness (`target/occluder-after/bin/postretro-tests`, built at `5426b60b4`). Since that build, the only change outside the compiler is `sh_streaming/controller_tests.rs`, which no harness entry reads. The rooftop before record used the baselines' own harness on the before PRL.
- **Bakes.** `RUST_LOG=info prl-build-<config> content/dev/maps/<map>.map -o target/occluder-9c/<config>/<map>/<map>.prl --baked-root target/occluder-9c/baked --no-tui -v <flags>`, with the before bakes' flags exactly:
  - hallway: `--sh-probe-spacing 4.0`;
  - arena: `--sh-probe-spacing 10.0 --lightmap-density 0.25 --no-cache`;
  - street: `--sh-probe-spacing 4.0 --lightmap-density 0.25`;
  - occlusion-test and campaign-test: none.
- **Cache mode.** As before: each bake got a fresh private `--cache-dir`, so it ran cold, and the cache was deleted when the compile finished. The arena used `--no-cache`. The hallway cache reached 25 GB per bake.
- **Inputs.** The before pose files (`../baselines/<map>/<map>.poses`), the before sweep and bake-gap records in `target/occluder-baselines/<map>/`, and the rooftop pose file in this folder.
- **Rays and walks.** 160×90 rays, 100° HFOV at 16:9, near 0.1 m. Bake gap: completed reads, L 16 m, pool cap 15, 2000 steps, 2 frames per pose on the tour.
- **Metrics.** M1 counts new drawn misses and step-cap trips, and both must be 0. M2 is deferred texel area per sequence, which must satisfy after ≤ before. Everything else is reported.
- **Upstream limits.** The walk's 5,000-test step cap; `MAX_DWELL_FRAMES` 60; the completed-reads gate's 120 s timeout; the 15-layer pool cap. No override changes the workload.
- **Cleanup.** Each hallway PRL (2.6 GB) was deleted once its records were written. The other PRLs and all raw records stay in `target/occluder-9c/<config>/<map>/`, which is gitignored. The lowest free disk observed was 26 GB, during a hallway bake.
- **Pilot.** campaign-test at cap13 ran end to end before the batch: bake, structure, sweep and bake gap paired against the before records, diffs and readers. The pairings came back valid (diff verdict PASS, no pose mismatch), and every record was present. The rooftop before run was checked at the same time: all 608 poses located interior.
- **Order.** Phase 2's four fast maps were baked while cap26's hallway replay was still running. Its hallway bakes started only once cap26 had passed hallway M2.
- **Scripts.**
  - `run9c.sh` runs one bake, one measure or the rooftop sweep.
  - `batch.sh` runs configs in turn. Bakes never overlap, and measures run in the background.
  - `copy-records.sh` copies the small records here.
  - `rooftop_poses.py` writes the rooftop pose set.
- **Scratch readers** (`tools/`, built against this checkout's crates):
  - `mandatory-reader` is Task 9a's id-51 mandatory-set reader.
  - `probe-reach` and `thin-cells` are new here.
  - The SH mandatory demand comes from the existing ignored harness entry `sh_reach_mandatory_bytes_from_prl`.

## Verdicts

Verdicts per map and configuration. M1 means no step-cap trip and no new drawn miss. M2 means deferred texels after ≤ before on every walk, and 0 stays 0.

| Row | Map | Task 9 | cap13 | cap19 | **cap26** | cap26-len10 | cap26-len7 |
|---|---|---|---|---|---|---|---|
| M1 | occlusion-test | PASS | PASS | PASS | PASS | PASS | PASS |
| M1 | hallway | PASS | PASS | PASS | PASS | PASS | PASS |
| M1 | campaign-test | PASS | PASS | PASS | PASS | PASS | PASS |
| M1 | arena | PASS | PASS | PASS | PASS | PASS | PASS |
| M1 | street | FAIL 29 trips | FAIL 113 | FAIL 29 | FAIL 19 | FAIL 67 | FAIL 63 |
| M2 | hallway | **FAIL** random 3.07 M | PASS | PASS | PASS | **FAIL** pose tour 2.18 M | PASS |
| M2 | other four maps | PASS | PASS | PASS | PASS | PASS | PASS |
| M3 | all five | PASS | PASS | PASS | PASS | PASS | PASS |
| M4 | all five | PASS | PASS | PASS | PASS | PASS | PASS |

Notes on the grid:

- **No drawn misses anywhere.** No configuration has a new drawn miss on any map or pose set. Every M1 failure is a street step-cap trip.
- **Street trips (total / also tripped before / new).** The before build tripped on 518 of the 2,960 street poses.

  | Pose set | Task 9 | cap13 | cap19 | cap26 | cap26-len10 | cap26-len7 |
  |---|---|---|---|---|---|---|
  | street level | 29 / 26 / 3 | 113 / 107 / 6 | 29 / 29 / 0 | 19 / 19 / 0 | 67 / 60 / 7 | 63 / 56 / 7 |
  | rooftop | 44 / 40 / 4 | 57 / 55 / 2 | 34 / 32 / 2 | 22 / 21 / 1 | 66 / 53 / 13 | 63 / 52 / 11 |

  The street's culled-opening poses (9; 10 at len10 and len7) are reported, not failed, as at Task 9.
- **M3.** In every configuration, every mover that logged a seal line before still logs at least one. Door recognition is unchanged: hallway 9 of 11, campaign-test 2 of 3, arena 6 of 8.
- **M4.** No log in any configuration carries a leak or unsealed line.

## Phase 1: cap sweep at length 20 m

No cap passes M1 on all five maps, because each one trips on the street. The brief's fallback rule therefore applies: take the cap that passes hallway M2 with the fewest street step-cap trips. All three pass hallway M2.

**cap26 (26.0096 m, 1024 u) is chosen.** It has 19 street trips, against 29 at cap19 and 113 at cap13, and every one of the 19 also tripped before, so none is new. It also has the lowest mean portal tests per pose of the three on every map:

| Mean portals tested per pose | before | Task 9 | cap13 | cap19 | cap26 |
|---|---|---|---|---|---|
| occlusion-test | 464.3 | 106.8 | 286.3 | 209.4 | 179.6 |
| hallway | 65.3 | 38.7 | 92.9 | 66.2 | 54.9 |
| campaign-test | 108.1 | 50.5 | 120.1 | 100.7 | 74.4 |
| arena | 58.8 | 12.5 | 78.1 | 46.2 | 33.3 |
| street | 2,042.6 | 1,240.5 | 1,599.5 | 1,353.5 | 1,271.0 |

The cap trades walk cost against cell size. A smaller cap means more cells and portals, so more portal tests, but less drawn texel area per pose.

- **cap13** walks more than before on hallway (+42%), campaign-test (+11%) and arena (+33%). It also trips 113 times on the street.
- **cap26** walks less than before on every map (−16% to −61%).
- **Hallway margin.** The id-51 lightmap mandatory set, the 9a lever, has its largest cell at 99.3 M texels at cap13, 108.3 M at cap19 and 121.6 M at cap26, against 97.3 M before and 134.1 M at Task 9. Cells over the pool cap's ~62.9 M number 346, 466 and 674 respectively, against 177 before. cap26 has the least margin under the M2 gate of the three, though it still defers nothing.

## Phase 2: horizontal absolute length at cap26

| | cap26 (20 m) | cap26-len10 | cap26-len7 |
|---|---|---|---|
| Occlude alone: occlusion-test / hallway / campaign-test / arena / street | 29 / 738 / 40 / 237 / 72 | 29 / 748 / 57 / 243 / 73 | 29 / 1,096 / 59 / 253 / 74 |
| Sections occluding: occlusion-test / hallway / campaign-test / arena / street | 5 / 12 / 7 / 2 / 13 | 5 / 25 / 0 / 5 / 13 | 5 / 18 / 1 / 5 / 13 |
| Interior cells: occlusion-test / hallway / campaign-test / arena / street | 107 / 1,428 / 129 / 389 / 850 | 107 / 1,491 / 92 / 463 / 900 | 107 / 1,624 / 142 / 454 / 830 |
| Runtime portals | 241 / 2,273 / 288 / 603 / 3,755 | 241 / 2,413 / 193 / 916 / 3,607 | 241 / 2,652 / 316 / 891 / 3,444 |
| Mean portals tested per pose | 179.6 / 54.9 / 74.4 / 33.3 / 1,271.0 | 179.6 / 56.3 / 45.2 / 57.6 / 1,450.4 | 179.6 / 59.9 / 78.4 / 55.2 / 1,422.5 |
| Hallway deferred texels (random / far / tour) | 0 / 0 / 0 | 0 / 0 / **2,182,656** | 0 / 0 / 0 |
| Street trips (street level / rooftop) | 19 / 22 | 67 / 66 | 63 / 63 |

Reading:

- **Shorter lengths make things worse at cap26.** Both lengths raise the arena's walk cost (+73% and +66% against cap26) and the street's (+14% and +12%), and roughly triple the street and rooftop trips.
- **Campaign-test is the only map that improves,** and only at 10 m: 45.2 portal tests against 74.4, and tris in frustum +51% against before, where cap26 has +73%. At 10 m, 17 more of campaign-test's brushes occlude alone, its clusters fall from 15 sections to 8, and none of those occludes.
- **Occlusion-test is unchanged** at both lengths, down to the cell.
- **len10 fails hallway M2.** It defers 1 block (2,182,656 texels) on the pose tour, at tour frame 104 (pose 52, eye −48.04, 19.38, −114.17, yaw 270°). No other frame defers. Its largest mandatory cell is 120.2 M, with 684 cells over the cap.
- **len7 passes hallway M2,** and the hallway is the one map it helps. 1,096 hallway brushes occlude alone, against 738. The largest mandatory cell is 105.9 M (291 cells over the cap), against 121.6 M (674) at cap26. Tris in frustum are +48% against before, against +69%. Portal tests rise slightly, 59.9 against 54.9. On the full tree, hallway exterior leaves go 416 → 572, but there is no leak line.

## Rooftop poses (stress-street, new)

`rooftop_poses.py` writes `stress-street.rooftop.poses` from `tools/stress_street.py`:

- **Eye points.** Two per roof footprint, 1.5 m above the roof (`building_height(storeys)`): the footprint centre, and 1 m behind the street facade line at mid-frontage. That gives 19 footprints and 38 eye points: 18 row frontages (16 buildings, with each corner piece counted once per arm) plus the free-standing building.
- **Views.** 8 yaws, at pitch 0 and −15°, for 608 poses.
- **Coordinates.** Map metres convert to engine metres as `(−y, z, −x)`, as `engine_pose` does.

All 608 poses locate interior and take the portal path, or trip.

| Rooftop row | before | Task 9 | cap13 | cap19 | cap26 | cap26-len10 | cap26-len7 |
|---|---|---|---|---|---|---|---|
| step-cap trips | 212 | 44 | 57 | 34 | 22 | 66 | 63 |
| new drawn-miss poses | 0 | 0 | 0 | 0 | 0 | 0 | 0 |
| portals tested, mean / p95 | 2,789.4 / 5,000 | 1,898.9 / 5,000 | 2,375.5 / 5,000 | 2,025.3 / 5,000 | 1,897.6 / 4,552 | 2,181.5 / 5,000 | 2,151.7 / 5,000 |
| drawn cells, mean / p95 | 426.7 / 1,409 | 219.8 / 738 | 265.5 / 822 | 197.2 / 501 | 170.4 / 410 | 211.4 / 651 | 197.8 / 592 |
| tris in frustum, mean / p95 | 3,035.3 / 10,847 | 3,396.8 / 10,932 | 3,508.7 / 12,997 | 3,139.0 / 10,060 | 3,077.3 / 8,162 | 3,910.4 / 13,657 | 3,897.7 / 13,201 |
| drawn texels, mean / p95 | 0.86 M / 2.22 M | 1.40 M / 2.61 M | 0.50 M / 1.49 M | 0.52 M / 1.21 M | 0.55 M / 1.23 M | 0.65 M / 1.82 M | 0.65 M / 1.78 M |

A tripped pose stops at 5,000 tests, so the before build's portal-test mean (212 trips) is a lower bound. Rooftops cost more than street level in every build: cap26 walks 1,897.6 tests per pose on the roofs against 1,271.0 on the street. It trips on 22 rooftop poses, one of them new (`roof:B3.s1.e@180/-15`). 20 of the 22 are on the broken row's roofs (B1 to B4), looking back along the street toward the plaza (yaw 90° to 225°), and 14 of the 22 are pitched down.

## Guardrail (per pose against before, means over the sweep poses)

| | Task 9 | cap26 | cap26-len10 | cap26-len7 |
|---|---|---|---|---|
| Tris in frustum: occlusion-test / hallway / campaign-test / arena / street | +6 / +65 / +117 / +36 / +5% | +3 / +69 / +73 / +48 / +6% | +3 / +68 / +51 / +47 / +27% | +3 / +48 / +74 / +47 / +27% |
| Drawn lightmap texels | +29 / +46 / +83 / +53 / +53% | −18 / +19 / +10 / −12 / −40% | −18 / +18 / −8 / −12 / −34% | −18 / +10 / −4 / −12 / −33% |
| Portals tested | −77 / −41 / −53 / −79 / −39% | −61 / −16 / −31 / −43 / −38% | −61 / −14 / −58 / −2 / −29% | −61 / −8 / −27 / −6 / −30% |

The street's before figures are truncated by its 518 trips.

- **Texels.** The cap brings drawn texels below before on three maps. Campaign-test is at +10% and the hallway at +19%.
- **Triangles.** Tris in frustum stay well above before on the hallway (+69%), campaign-test (+73%) and arena (+48%).
- **Faces.** PRL faces (= lightmap charts) at cap26 against before: occlusion-test +3.8%, hallway +32.1%, campaign-test +26.5%, arena +29.2%, street +36.4%.
- **More cells, smaller cells.** The cap adds cells over Task 9 on four maps (occlusion-test 461 → 484, hallway 7,111 → 7,255, campaign-test 793 → 832, arena 1,180 → 1,382). The street falls 4,559 → 4,034, because Task 9's window cells are gone (9b). Interior cell largest-extent p95 falls from 42–110 m at Task 9 to 26–29 m.
- **Uncapped corridors.** The hallway's max stays 120.3 m at every cap. Those cells are 3.25 × 4.88 × 120.3 m horizontal corridors that the cap leaves whole. Most likely the open-slice guard keeps them, since a cap line across a corridor buried in solid slices mostly solid in its tier leaf; that cause is not traced.

Full M5 to M7 tables are in `tables.md`:

- cells by kind, tier leaves, portals, faces and triangles, extent spreads, SH clusters, id-51 sets;
- per-pose drawn cells and p95s;
- the arena boxes.

**M7.** At no configuration does a stair riser plane bound a cell. The only tread-grid plane is y = 11.379, which is the arena wall band's top face, as Task 9 found. The staircase box holds 6 cells at cap26 (5 at len10 and len7), and the stepped-solid escalation stays untriggered. The cap's own lines (z = 39.014 and 52.019 at cap13) sit off the riser grid.

## Follow-up checks

### SH residency reach through section-wall doorways (from 9b)

No cheap measure isolates doorways: a PRL carries no tag for which portals are section openings. Two cheap proxies were measured instead.

| | before | Task 9 | cap26 | cap26-len10 | cap26-len7 |
|---|---|---|---|---|---|
| SH mandatory clusters per camera cell at L 16 m, max: occlusion-test / hallway / campaign-test / arena / street | 22 / 105 / 15 / 21 / 362 | 14 / 141 / 16 / 22 / 595 | 14 / 145 / 16 / 24 / 168 | 14 / 140 / 16 / 30 / 158 | 14 / 134 / 24 / 30 / 160 |
| Cluster pairs whose baked probe ranges reach into the other's cells | 158 / 2,363 / 45 / 537 / 7,917 | 77 / 3,190 / 47 / 622 / 10,021 | 86 / 3,342 / 60 / 747 / 4,525 | 86 / 3,401 / 53 / 811 / 4,116 | 86 / 3,128 / 106 / 805 / 3,963 |
| Patch-owner edges (a cluster needing a lower-id cluster's probes) | 52 / 930 / 19 / 125 / 949 | 26 / 1,200 / 22 / 93 / 1,115 | 33 / 1,179 / 26 / 132 / 510 | 33 / 1,150 / 26 / 139 / 520 | 33 / 1,226 / 47 / 139 / 526 |

- **The first row** is the existing harness `sh_reach_mandatory_bytes_from_prl`: the clusters of the id-51 entries within L, closed over baked owners. No map pins clusters.
- **The other two rows** come from `tools/probe-reach`. It locates every valid probe in each cluster's id-34 dense ranges and counts probes that land in another cluster's interior cell. It also counts the runtime's patch-owner rule (lowest cluster id carrying each probe) and the sparse halo owners. Halo edges equal the patch edges on occlusion-test, campaign-test and the street, and run at most 12% higher on the hallway and arena (`probe-reach.out`). Brick-node owners are not counted.
- **Reading.** Against before, cap26 raises owner reach on the hallway (+41% pairs, +27% owner edges, mandatory max 105 → 145) and the arena (+39% pairs). It lowers it on occlusion-test and on the street, where Task 9's window cells are gone (9b).
- **What a doorway figure would take.** `portals/occluder_tier/openings.rs` knows each section opening at compile time. Emitting those portal ids to a sidecar, a test-only log line or an ignored compiler test, would let a reader take each opening's two cells, intersect the near cell's probe box with the far cluster's probes, and count the clusters a doorway pulls in as owners. That is compiler-side work, not done here.

### Thin interior cells on campaign-test (from 9a)

`tools/thin-cells` lists interior cells whose smallest extent is 0.25–0.5 m.

| | Task 9 (no cap) | cap13 | cap19 | cap26 |
|---|---|---|---|---|
| Thin cells (all exactly 0.406 m = 16 u thick) | 41 | 99 | 88 | 66 |
| Joined to a same-slab neighbour across a cap-line portal | 0 | 81 | 69 | 44 |
| Distinct slabs once the cap splits are re-joined | 41 | 27 | 33 | 43 |
| Thin side on a cap line | 5 | 7 | 0 | 6 |

Every one is a 16 u slab: the air layer between the two faces of a 16 u floor, wall or ceiling whose planes cross open space at occluder rank, or a doorway through a 16 u wall. 62 of cap13's 99 are horizontal layers, most at y 9.754–10.160 and 13.005–13.411, and the rest are vertical wall-thickness strips and doorways. The rise from 41 comes from cap lines cutting these slabs along their length into 13 m pieces. Re-joined across cap-line portals, they are 27 slabs. Task 9 had 41, but 9b and 9e also changed the partition, so the two are not strictly comparable.

The cap creates no slivers. No thin cell's thin extent is set by a cap line. The thin-side-on-cap cases are walls that sit on the 512 u grid, where cap lines also fall, and they exist without the cap. One example is the 0.406 m layers at z −26.416 and −26.010 beside a 26 m wall that Task 9 already had. 15 of cap13's thin cells are faceless (0 at cap26).

## Owner rulings needed

1. **Street M1.** Every configuration trips on the street, so M1 fails as written. At cap26 all 19 street-level trips also tripped before, and one of 22 rooftop trips is new. Decide whether before-tripping poses are exempt. If they are, cap26 passes M1 at street level and has 1 new rooftop trip; the rooftop set is not in the brief's M1 row.
2. **Pick the cap.** cap26 is the brief's fallback choice, but its hallway margin is the thinnest of the three: largest mandatory cell 121.6 M, and 674 cells over the pool cap. Raising `CELL_SIZE_CAP_M` to 26.0096 is a source change for the next task, not made here. cap19 passes everything cap26 does, with 29 street trips (none new) and more walk cost.
3. **Absolute length.** 10 m fails hallway M2 (2.18 M texels on the pose tour). 7 m passes every gate cap26 passes and improves the hallway's margin and triangles. It costs the arena (+66% walk) and the street (63 trips, 7 new; 63 rooftop trips, 11 new). It is the only length change worth considering, and only if the hallway's margin outweighs the street. Neither length is recommended over 20 m.
4. **Guardrail.** At cap26, tris in frustum stay +48% to +73% on hallway, campaign-test and arena, and faces +26–36% on four maps. Drawn texels are below before on three maps, with campaign-test at +10% and the hallway at +19%. `face-fragment-remerge` still applies.
5. **Uncapped corridors.** The cap leaves the hallway's 120 m corridor cells whole, most likely through its open-slice guard. Decide whether that is intended.

## Files

- `README.md` (this file) and `tables.md`, which `collect.py` generates (`python3 collect.py cap13 cap19 cap26 cap26-len10 cap26-len7 > tables.md`).
- `<config>/<map>/` holds the records for that bake:
  - `sweep.out`, `bakegap.out`, `structure.out`, `diff-sweep.out`, `diff-bakegap.out`;
  - `mandatory.out`, `probe-reach.out`, and `shreach.out` (summary only);
  - `bake.time` (flags, binary hash, wall, cache size, disk), and `bake-extract.txt` (classifier, door, runtime-cell, seal, flood and watertightness lines);
  - the arena's `region-*.out`, and the street's `rooftop-sweep.out`.
- `before/` and `task9/` hold this task's side runs on the before and Task 9 PRLs: the rooftop sweep and the three readers. Their main records are in `../baselines/` and `../after/`.
- `stress-street.rooftop.poses`, `rooftop_poses.py`, `run9c.sh`, `batch.sh`, `copy-records.sh`, `collect.py`, and `tools/`.
- The raw sweep and bake-gap records, PRLs (except the hallway's), and the rooftop before record (`target/occluder-9c/before/stress-street/rooftop.sweep`) stay in `target/occluder-9c/`.
