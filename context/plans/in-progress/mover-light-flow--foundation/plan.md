# mover-light-flow--foundation — plan of record

mode: compact
status: active
read at: af1aab9ef

## Corrections

- **Row 14 restated (owner, 2026-11-02).** An animated-only level bakes no id-25 weights: with `StaticBakedLights` empty, atlas prep plans no placements (`lightmap_bake/atlas_layout.rs:156,236`), and `build_placed_animated_light_chunks` returns empty (`animated_light_chunks.rs:36-45`). The owner chose restatement (a): that level reports gated probes and zero gated texels. No SDF-dependent fixture was added, because SDF shadows are soft-deprecated. Rows 10, 14 and 18 still list SDF-shadow lights as not analysed.
- **Partition order.** The brief says partitions are consumed "in global light order". In fact items run layer-major: all lights for layer 0, then layer 1 (`pipeline/lightmap_stage.rs:501-505`). The gate keys tallies by `(mover_id, terminus, source_index)`. The report sorts in one light order, so arrival order never reaches the output.
- **Serial consumer.** The window's serial callback (`lightmap_stage.rs:553-585`) costs about 273 s on the full hallway (roadmap, lightmap stage scheduling). A third consumer there would add straight to wall time.
  - The static gate copies the `StepDetector` shape (`pipeline/lightmap_stage/texel_steps.rs`) instead. It runs per partition on the producing worker under its permit, in both layer-cache `load` hits (`partitions.rs:296`) and `finish` (`:380`). Results gather under a `Mutex` and sort by item.
- **Post-hit pass.** `LayerItems::selected` (`lightmap_stage.rs:612-670`) re-reads only lights the shadowmask still needs.
  - It must select the union of `needs_source` and the lights the gate needs (P8), and its progress math (`:646-669`) must count that union.
  - `FusedShadowmaskPlan::consume_partition` must stay a no-op for sources that cannot own, or shadowmask bytes move.
- **Animated stage.** `AnimatedWeightMaps` is `StageId::AnimatedWeightMaps` (`pipeline.rs:1797-1859`), not a type.
  - To map an id-25 texel back to its chart: `AnimatedLightChunk.face_index` → chart, then `TexelLight` offsets within `AnimatedBlock.lightmap_block`.
- **Mover paths.** `resolve_kinematic_path` (`parse.rs:2448`) drops names. The gate re-walks the `next` chain from `mover.path` over `map_data.kinematic_waypoints` (`map_data.rs:704`) to find the first and last waypoint.
- **Build Summary.** `reporter.rs:291-307` and `tui.rs:268-283` print timings only, to stdout, through `Reporter::finalize(timings, total)`. A third implementor is at `surface_radiance/pipeline_tests.rs:62`. All three take the mover-lighting block.
- **Packed bytes.** `report_section_footprint` (`pack_output.rs:51`, computed at `pack.rs:696`) is only logged today. The id 22/25/42 packed sizes must be returned to the pipeline (`pipeline.rs:~2108`) for the percentage. Every real PRL ships an id-22 header, so the "n/a" branch is proven by a unit test of the formatter.
- **Parent-commit PRL precedent.** `tests/oversize_lightmap_maps.rs:190-224` skips a missing baseline. The new parity test fails on a missing baseline and asserts that two maps were compared, as the row requires.
- **Reach verify mode in tests.** `tests/prl_build_support/mod.rs` sets `POSTRETRO_LIGHTMAP_REACH_VERIFY=1`. With verify on, the stage reads neither the layer cache nor the section memo (`lightmap_stage.rs:316,419`; `partitions.rs:257`). Every warm build in row 18 therefore sets it to `"0"`, or the post-hit path is never exercised.
  - Evidence lines: `[cache] lightmap_section hit`, `[cache] shadowmask_atlas hit`, and the reach stage line "served from the lightmap_section memo … no pair walked".
- **Reach-culled pairs.** A culled texel stores a present record with `raw_visibility 0.0` (`lightmap_layer/chart_walk.rs:301-308`). Layer bytes are identical to an uncut walk. "Lit = visibility > 0" therefore sees the same set as before the reach cull. If "culled pairs dropped whole" later lands, it must cover this consumer.
- **No reach-flood pruning.** `LightmapReachIndex` exists only inside `bake_fused_windowed`. It is absent for suns, flagged emitters, lights in solid and holed maps. It is also unsound for texels whose centre segment crosses static solid. The gate prefilters with each light's influence sphere against the posed AABB instead.
- **Radiance cache layers.** The `surface_radiance` per-light patch layers now exist but are freed before atlas prep. Bounce stays a non-goal on the owner's ruling, and the gate never reads them.
- **Probe memo.** The probe gate traces the BVH, so its memo folds `TRACER_EPOCH`.
- **Hallway light count.** The brief's "345 lights" doesn't match the hallway `.map`, which holds 764 light entities (85 `light`, 678 `light_spot`, 1 dynamic). The manual row takes the count from the compile log.
- **Cost budget.** Hallway cold Lightmap Bake is about 557 s after the reach cull (was about 965 s), so ≤3% is about 17 s.
- **Research doc.** The `research.md` list of "existing reach-like data" predates `LightmapReachIndex` (per-light open-leaf flood, compile-only, `lightmap_reach.rs`). It is a candidate base for research step 3's redefined reach and is not used here.

## Build notes

- **P1 strip (task 1).** At the open terminus, arena light entity 9 has 464 open-only texels. They lie in a back-room floor strip just east of the pocket-mouth plane, up to about 0.030 m from it. One texel pitch is 0.025 m. Their centre segments cross static wall, and the bake lit them only through off-centre samples. This is the brief's named static-edge bias. Task 8 measures "within one texel column" by texel column from the mouth, not by metres, before asserting it. If even that fails, row 1's P1 clause goes to the owner.
- **Cut sub-charts.** Texels where cut sub-charts overlap are counted once per sub-chart. That is what a per-chart mask would store, so the estimates keep it.
- **Section-memo hit.** On a `lightmap_section` hit the static gate doesn't run yet (`FusedLightingOutput.mover_light` is `None`). Task 7 closes this.

## Delegated answers

- **Module home.** `src/mover_light_gate.rs` plus `mover_light_gate/` (pose, segment, rows, probes, animated, side_count, report, cache). A thin adapter, `pipeline/lightmap_stage/mover_gate.rs`, keeps the large stage files small.
- **Caching shape.** Two memos:
  - static-texel tallies, inside the lightmap stage;
  - animated-texel and probe tallies, after `StageId::AnimatedWeightMaps`.

  An animated-only edit (P7) then reruns only the second. Labels, side counts, not-analysed lists and the percentage are recomputed every build.
- **Subsystem tag.** `[MoverLight]`. Row 17 filters warn lines by this tag.
- **Fixtures.** Integration tests generate small maps in-test as `tests/buried_lights_compile.rs` does. One hand map, `content/dev/maps/test_mover_light_gate.map`, serves the in-process window test. The crate is binary-only (`fixture_pipeline.rs:3-8`), so in-process tests live inside the crate.
- **Side count.**
  - Dynamic lights marked bake-only have no runtime presence, so they are left out.
  - A light reaches a mover when its falloff sphere at its authored origin touches the swept bounds; a sun always reaches. Spot cones are not narrowed.
  - For a carried light this test is a superset of the exact one, because the light moves with its mover.
- **Blocks across layers.** Per-partition distinct block counts are summed per row. This is exact because each `CellBlock` sits in one layer, and a partition is one (light, layer).
- **Window sizes.** The window-size check runs `bake_fused_windowed` in-process at windows 1, 3 and 8. The worker-count check runs `-j 1` against `-j 4`.

## AC-to-proof

Rows are numbered in brief order: Automated, then Manual.

| AC | Proof | Status |
|---|---|---|
| 1 `setpiece_wall` closed and open rows (P1) | `tests/mover_light_gate_setpiece.rs`, subprocess cold bake, `-v` + `RUST_LOG=debug` | achievable as stated |
| 2 `setpiece_room` lowered and raised, shaft shared | same file | achievable as stated |
| 3 hollow mover posed as brushes | `mover_light_gate/tests.rs` pose unit test + generated map in `tests/mover_light_gate_compile.rs` | achievable as stated |
| 4 each mover posed alone | compile file, two-mover generated map | achievable as stated |
| 5 side count: room lamp carried, wall none, no dynamic in rows | setpiece file | achievable as stated |
| 6 shared + differing = union; estimates ordered; P5 | proptest in `mover_light_gate/rows.rs` + P5 generated map | achievable as stated |
| 7 single-block estimate; percentage divisor; n/a | unit tests (arithmetic, n/a) + subprocess divisor check against the PRL section table | achievable as stated |
| 8 non-meeting light has no row; probe-only row | compile file | achievable as stated |
| 9 probe exclusions (range, cone, static, invalid) | compile file + unit test on a hand-built probe layout | achievable as stated |
| 10 directional, animated ordering, SDF not analysed | compile file | achievable as stated |
| 11 spin-axis movers not analysed; zero axis analysed | compile file | achievable as stated |
| 12 three-waypoint mover uses first and last | compile file | achievable as stated |
| 13 inside flag at one terminus only (P6) | compile file | achievable as stated |
| 14 no-baked / SDF-only / animated-only levels; animated-only reports probes, zero texels (P2) | compile file | achievable as restated |
| 15 `info` line names mover and ≤4 lights, "and N more"; unnamed by bounds centre | setpiece file + generated unnamed mover | achievable as stated |
| 16 pinned order; identical across cold builds, workers, windows (P4) | subprocess `-j 1` vs `-j 4` + in-process windows 1/3/8 in `pipeline/lightmap_stage/tests.rs` | achievable as stated |
| 17 no analysis line at warn or above | every subprocess test filters by `[MoverLight]` | achievable as stated |
| 18 warm rows (P3, P7, P8 and the listed edits) | `tests/mover_light_gate_cache.rs`, subprocess, shared `--cache-dir`, verify `0` on warm builds | achievable as stated |
| 19 packed PRL byte-identical to parent for `campaign-test`, `mover-setpiece` | `tests/mover_light_gate_prl_parity.rs`, `#[ignore]`, fails on missing baseline, asserts 2 compared | achievable as stated |
| 20 no-mover level prints only the summary line | compile file | achievable as stated |
| 21 hallway cost ≤3% of Lightmap Bake; peak RAM growth reported | manual, idle cold runs vs parent, same `-j` | manual |
| 22 report totals in `research.md` for five maps + step-5 read | manual measurement | manual |

## Tasks

| # | Task | Owner | Depends on | Status |
|---|---|---|---|---|
| 1 | Thinnest slice: termini, posing, inclusive segment-vs-brush test, inside test, static gate on the producing worker (compose path, uncached), `debug`/`info` rows; `setpiece_wall` closed rows green | worker | — | done: `mover_light_gate_setpiece` (1, 11.4 s, not ignored), `mover_light_gate::tests` (7); closed rows = the five non-shaft lights, gated-centre AABBs on the far side of the wall; packed PRL byte-identical; gate ≈0.64 s CPU, wall time within noise |
| 2 | Shared/differing sets, touched (row, block), both estimates, proptest | worker | 1 | done: `rows::tests` proptest (row 6) + single-block estimate (row 7), `mover_light_gate_compile` P5 |
| 3 | Probe gate: reuse the SH stage's `ProbeGridLayout`, per light in parallel under permits | worker | 1 | |
| 4 | Animated gate on the id-25 section | worker | 3 (both edit `pipeline.rs`) | |
| 5 | Side count: dynamic lights vs swept bounds, carried via `carried_light_links` | worker | 1 | done: `mover_light_gate_setpiece` row 5 (room: lamp carried, 0 other; wall: 0/0) |
| 6 | Report: Build Summary through `Reporter` (3 implementors), id 22/25/42 bytes from `pack.rs`, pinned ordering | worker | 2, 5 | |
| 7 | Two memos, post-hit union and progress, peak term (`peak.rs`, `tests/lightmap_working_set_contract.rs`) | integrating executor | 2–4 | |
| 8 | Edge fixtures and tests (rows 3–15, 17, 20) | worker | 6 | |
| 9 | Determinism (16) and warm rows (18) | worker | 7 | |
| 10 | PRL parity test (19) | worker | — | test written (`mover_light_gate_prl_parity`, ignored, `POSTRETRO_MOVER_LIGHT_PARITY_BASELINE_DIR`); baselines built at the gate run |
| 11 | Docs (`build_pipeline.md` step 12, §Build Cache), manual measurements (21, 22), `research.md` totals | integrating executor | 9 | |

Tasks 2, 3, 5 and 10 can run concurrently once task 1 lands. Tasks 6 and 7 share no files.
