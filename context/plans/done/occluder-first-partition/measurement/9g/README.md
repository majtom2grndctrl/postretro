# occluder-first-partition: Task 9g M1/M2 re-run

Recorded 2026-10-28 on `claude/occluder-first-partition` at HEAD `d2efc11bb`, with Task 9f's pool growth and the corridor fix to the open-slice guard in. All three sides run HEAD's harness, so they share one pool policy (`rendering_pipeline.md` §2, §4): the pool grows to the whole targeted demand, times `GROWTH_HEADROOM` 4/3. Commands are those of `../world-poses/README.md`, and the template is `../9c/`.

No committed source changed. cap19 was a temporary edit of `CELL_SIZE_CAP_M`, which was built, frozen and then edited back. A rebuild after the restore hashes the same as the frozen cap26 binary (`5910f5856f08`), and `git diff` is empty.

Every table is generated from the raw records by `collect.py`. The full set is in `tables.md`, and the sections below quote it.

## Configurations

| Side | Compiler | `CELL_SIZE_CAP_M` | Binary (SHA-1 prefix) |
|---|---|---|---|
| before | the frozen before bakes in `target/occluder-baselines/<map>/`, none rebaked | – | `prl-build-8a9a341` |
| cap19 | HEAD with one constant edit | 19.5072 (768 u) | `dd9962f623e0` |
| cap26 | HEAD as is | 26.0096 (1024 u) | `5910f5856f08` |

## Pinning (testing_guide §Resource bounds)

- **Machine.** The baselines' Mac: Intel i9-9980HK, 16 threads, 32 GiB. Every row is deterministic CPU work, and wall times are for scale only.
- **Binaries.** Everything is frozen in `target/occluder-9g/bin/`:
  - `prl-build-cap19` and `prl-build-cap26`, with `scripts-build` beside them;
  - `postretro-tests`, the `postretro` bin test harness built at HEAD (`99d11077425c`);
  - `mandatory-reader`, copied from 9c;
  - `uncap9g`, described below.
- **Harness drift check.** Re-measured with HEAD's harness, every before sweep record is byte-identical to the baselines' record on all five maps. So is the rooftop record against 9c's. The sweep has not drifted, and the before sweep numbers equal 9c's.
- **Bakes.** `RUST_LOG=info prl-build-<config> content/dev/maps/<map>.map -o target/occluder-9g/<config>/<map>/<map>.prl --baked-root target/occluder-9g/baked --no-tui -v <flags>`. The flags are the before bakes' flags. Each bake used a fresh private `--cache-dir`, deleted after the compile, with two exceptions:
  - The arena takes `--no-cache`, as before.
  - **Deviation: the hallway takes `--no-cache` too.** A cold private hallway cache reached 25 GB in 9c, more than the 21 to 29 GB free here. On occlusion-test at cap26, a `--no-cache` bake and a cold-cache bake gave identical structure output and an identical id-51 hash, and the same PRL size. 62 KB differed, which is the SH values: a warm SH bake is approximate and a no-cache SH bake is exact. No M row reads SH values.
- **Before re-measure.**
  - The sweep and rooftop sweep run on the before PRLs, each paired against its own old record.
  - The bake gap replays the baselines' before frames under HEAD's pool policy. That record, `target/occluder-9g/before/<map>/<map>.before.bakegap`, is the BEFORE for both configs. All three sides therefore replay bit-identical frames, and every pairing came back valid, with no INVALID.
  - The id-51 and SH readers on the before side are 9c's side runs. They read the PRL only, so no harness change touches them.
- **Rays and walks.** As in 9c:
  - 160×90 rays, 100° HFOV at 16:9, near 0.1 m.
  - Bake gap: completed reads, L 16 m, pool cap 15, 2000 steps, 2 frames per pose on the tour.
- **Uncapped walk.** `tools/uncap9g.main.rs` (its Cargo manifest is beside it) is a scratch build outside the workspace.
  - It runs `determine_visible_cells` from a copy of `crates/visibility/src` at HEAD whose only edit is `MAX_PORTAL_WALK_STEPS` = 10,000,000.
  - Its camera is `render_camera`, yaw and pitch included.
  - Cross-check (`xcheck.py`): on every pose the capped walk did not trip, uncapped portal tests equal the sweep record's `considered`. That holds for 2,442, 2,931 and 2,941 street poses, and for 396, 574 and 586 rooftop poses, with 0 differences. Every tripped pose costs over 5,000 uncapped.
  - It reproduces 9b's before figures exactly: mean 2,553.7, p95 8,891, max 18,513, 518 poses over 5,000.
- **Metrics.**
  - M1: new drawn misses and step-cap trips, both must be 0. The rooftop set joins M1 by the owner's step-cap ruling.
  - M2: deferred texel area per sequence, after ≤ before, and 0 stays 0.
  - Everything else is reported.
- **Upstream limits.** The 5,000-test step cap, `MAX_DWELL_FRAMES` 60, the completed-reads gate's 120 s timeout, and the 15-layer pool cap. No override changes the workload.
- **Pilot.** occlusion-test ran end to end at cap26 first: bake, before re-measure, sweep, structure, readers, and a paired bake gap and diffs. Every pairing was valid and every verdict PASS. The batch then aborted on any INVALID pairing or missing verdict, and none occurred.
- **Disk.** Superseded PRLs under `target/occluder-{after,9a,9c}` were deleted first, which took free space from 21 to 29 GB. The lowest free space recorded at a compile boundary was 25 GB, so `target/debug/incremental` was not touched. Each hallway PRL was deleted once its records were written.
- **Scripts.** `run9g.sh` runs one bake, measure or before re-measure, and `batch.sh` runs the configs. `copy-records.sh` copies the small records here. `collect.py` is 9c's, re-rooted, with pool and uncapped tables added. `xcheck.py` is the uncapped cross-check.

## Verdicts

| Row | Map / set | cap19 | **cap26** |
|---|---|---|---|
| M1 | occlusion-test, hallway, campaign-test, arena | PASS | PASS |
| M1 | street level (2,960 poses) | **FAIL** 29 trips (29 also before, 0 new) | **FAIL** 19 trips (19 also before, 0 new) |
| M1 | rooftop (608 poses) | **FAIL** 34 trips (2 new: `roof:B3.s1.e@180/-15`, `@180/0`) | **FAIL** 22 trips (1 new: `roof:B3.s1.e@180/-15`) |
| M1 | new drawn misses, every map and set | 0 | 0 |
| M2 | all five maps, all three walks | PASS (0 / 0 / 0) | PASS (0 / 0 / 0) |
| M3 | all five | PASS | PASS |
| M4 | all five | PASS | PASS |

The before build deferred nothing under the new policy either, so M2 is the strict "0 stays 0" on every walk. M3: every mover that logged a seal line before still logs at least one, on every map. M4: no leak or unsealed line anywhere.

Corridor fix: at both caps, the street-level and rooftop trip counts equal 9c's at the same cap: cap19 29 and 34, cap26 19 and 22. The new-trip counts match too (2 and 1), and cap26's new trip is the same pose 9c named. The fix changed the hallway (interior largest extent max 120.3 → 53.6 m at cap26) and not the street.

## Uncapped portal tests (step-cap re-size input)

| Set | Side | mean | p50 | p95 | p99 | max | worst pose | over 5,000 | 2× max |
|---|---|---|---|---|---|---|---|---|---|
| street | before | 2,553.7 | 1,409 | 8,891 | 12,352 | 18,513 | `g5.0.0@180` | 518 | 37,026 |
| street | cap19 | 1,359.4 | 1,021 | 3,787 | 4,998 | 6,862 | `g6.3.0@180` | 29 | 13,724 |
| street | cap26 | 1,273.9 | 1,017 | 3,560 | 4,507 | 6,068 | `g5.3.0@180` | 19 | 12,136 |
| rooftop | before | 4,354.5 | 2,671 | 13,007 | 16,459 | 17,598 | `roof:A7.s2.c@225/-15` | 212 | 35,196 |
| rooftop | cap19 | 2,063.3 | 1,871 | 5,105 | 6,247 | 7,242 | `roof:B4.s2.c@180/-15` | 34 | 14,484 |
| rooftop | cap26 | 1,913.8 | 1,731 | 4,552 | 5,523 | 6,234 | `roof:B4.s2.c@180/-15` | 22 | 12,468 |

Percentiles are nearest-rank. The other four maps never trip, so their sweep maxima are uncapped. At cap26 they are 596, 755, 378 and 371 (occlusion-test, hallway, campaign-test, arena), all far under the street.

**Implied cap by §2's 2× rule over the worst swept walk:** cap26 ≥ 12,468 (the rooftop worst, 6,234), and cap19 ≥ 14,484 (7,242). At either value, no swept pose would trip and M1 would pass on both sets. At cap26, the costliest street walks face yaw 180° from column 5–6 (`g5.3.0`, `g6.3.0`, `g5.4.0`; eye y 22.5 m). The costliest rooftop walks sit on the broken row's roofs (B2 to B4) at yaw 135–225°. Most of them are pitched down (`uncapped-*.top.txt`).

## M2 pool rows (new policy)

Peak layers exclude the spare layer. MiB is active bytes with the spare, at 14.0 MiB per layer. Headroom cost is peak layers minus peak occupied layers over the walk.

| Map | Side | Peak layers, random / far / tour | Active MiB at peak | Headroom cost, layers (MiB) | first → peak, growths |
|---|---|---|---|---|---|
| hallway | before | 34 / 38 / 34 | 490 / 546 / 490 | 0 / 0 / 7 (98.0) | 15→34, 12 / 15→38, 6 / 15→34, 3 |
| hallway | cap19 | 34 / 36 / 32 | 490 / 518 / 462 | 0 / 0 / 2 (28.0) | 15→34, 6 / 15→36, 6 / 15→32, 2 |
| hallway | cap26 | 37 / **44** / 30 | 532 / **630** / 434 | 0 / 0 / 2 (28.0) | 15→37, 7 / 15→44, 10 / 15→30, 2 |
| campaign-test | before / cap19 / cap26 | 15–16 / 15–17 / 14–15 | 224–238 / 224–252 / 210–224 | 0 | far tour 1 / 2 / 1 growth, others 0 |
| occlusion-test | before / cap19 / cap26 | 10 / 10 / 9 | 154 / 154 / 140 | 1 (14.0) / 1 (14.0) / 0 | none |
| arena, street | all | 2–3 | 42–56 | 0–1 (≤ 14.0) | ≤ 1 |

- **Hallway margin.** cap26's far-point tour peaks at 44 layers, 630 MiB active, against 38 (546 MiB) before under the same policy. That is 6 layers, or 84 MiB more. cap19 peaks at 36 (518 MiB), under before.
- **Transient bytes.** The bake-gap entry does not print the transient while a generation retires. 9f estimated 0.8–1.1 GB for the hallway at cap26.
- **Growths.** Growth still happens 6 to 12 times per hallway walk on every side. The reserve sizes each growth to the demand targeted at that drain, and the walk keeps moving the target. No growth deferred a block.
- **Largest mandatory set** (id 51 at L 16 m, max per camera cell): hallway before 97.3 M (177 cells over 62.9 M), cap19 102.7 M (455), cap26 116.3 M (714). After the corridor fix, cap26's is 116.3 M, against 121.6 M in 9c.

## Headline guardrail (per-pose means against before)

| | occlusion-test | hallway | campaign-test | arena | street |
|---|---|---|---|---|---|
| Portals tested, cap19 / cap26 | −55 / −61% | −1 / −18% | −7 / −31% | −21 / −43% | −34 / −38% |
| Tris in frustum, cap19 / cap26 | +4 / +3% | +67 / +66% | +76 / +73% | +57 / +48% | +5 / +6% |
| Drawn lightmap texels, cap19 / cap26 | −23 / −18% | −3 / +13% | −9 / +10% | −23 / −12% | −46 / −40% |
| PRL faces, cap19 / cap26 | +5.5 / +3.8% | +40.0 / +34.4% | +40.5 / +26.5% | +47.1 / +29.5% | +38.3 / +36.4% |
| Runtime walkable portals, before → cap19 / cap26 | 693 → 291 / 241 | 4,722 → 2,950 / 2,368 | 524 → 439 / 288 | 1,333 → 912 / 608 | 6,288 → 4,248 / 3,755 |

The street's before means are truncated by its 518 trips. cap26 walks least on every map. cap19 draws fewer texels on every map, at about the same triangle cost. M5/M6 detail, including cells by kind, extents, SH clusters, M7 boxes and the rooftop rows, is in `tables.md`. M7 is unchanged from 9c: no riser or tread plane bounds a cell. The SH probe-reach table is empty, because `probe-reach` was not re-run.

## PRLs kept for the live frame-cost run (H3)

`measurement/frame-cost/poses.txt` holds occlusion-test poses (the `fc:` poses in its pose file). Task 9d also times street and rooftop walks for the step-cap re-size, so the street is kept too:

- `target/occluder-9g/before/occlusion-test/occlusion-test.prl` (a hard link to the baselines' before PRL)
- `target/occluder-9g/cap19/occlusion-test/occlusion-test.prl`
- `target/occluder-9g/cap26/occlusion-test/occlusion-test.prl`
- `target/occluder-9g/before/stress-street/stress-street.prl` (a hard link to the baselines' before PRL)
- `target/occluder-9g/cap19/stress-street/stress-street.prl`
- `target/occluder-9g/cap26/stress-street/stress-street.prl`

The campaign-test and arena PRLs for both caps also remain. The hallway PRLs were deleted.

## Owner rulings needed

1. **Step-cap re-size.** M1 still fails as written, on the street and rooftop sets only: cap26 has 19 + 22 trips, and cap19 29 + 34. All the street-level trips also tripped before. 1 rooftop trip is new at cap26 and 2 at cap19. Under the 2× rule, the cap would be ≥ 12,468 at cap26 or ≥ 14,484 at cap19, which clears every swept pose. Per the ruling, this waits on H3's street and rooftop walk times.
2. **Cap value.** Both caps pass M2, M3 and M4 under the new policy.
   - cap26 walks less (−18 to −61% portal tests) and trips less.
   - cap19 draws fewer texels, holds the hallway pool under before (36 against 38 peak layers), and has a smaller largest mandatory set (102.7 M against 116.3 M).
   - cap26's hallway pool peak is 630 MiB active (44 layers), plus a retiring generation during growth, which matters for Mac VRAM in H3.
3. **Growth count.** Demand-sized growth still grows the hallway pool 6 to 12 times per walk, on every side, before included, without deferring anything. If fewer growths matter for frame time, that is a 9f follow-up, not a gate issue.
4. **Guardrail.** Tris in frustum stay +48% to +76% on the hallway, campaign-test and arena at both caps. Faces rise 26–47% on four maps. `face-fragment-remerge` still applies.

## Files

- `README.md` (this file) and `tables.md` (`python3 collect.py cap19 cap26 > tables.md`).
- `<side>/<map>/` holds the small records for each side:
  - `sweep.out`, `bakegap.out`, `structure.out`, `diff-*.out`, `mandatory.out`, and `shreach.out` (summary only);
  - `bake.time`, and `bake-extract.txt`;
  - the arena's `region-*.out`;
  - on the street, `rooftop-sweep.out` and `uncapped-{street,rooftop}.top.txt` (summary plus the 40 costliest poses).
  - `before/` holds only the HEAD-harness re-measure. Its bake-side records stay in `../baselines/`.
- `run9g.sh`, `batch.sh`, `copy-records.sh`, `collect.py`, `xcheck.py`, and `tools/uncap9g.*`.
- The raw `.sweep` and `.bakegap` records, the full uncapped per-pose lists, the bake logs and the PRLs stay in `target/occluder-9g/`, which is gitignored.
