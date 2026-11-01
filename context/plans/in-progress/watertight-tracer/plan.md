# watertight-tracer — plan of record

mode: compact
status: active
read at: 26ab6c534

## Corrections
- Brief read at 343ff98b6 → since then only `partition/face_extract.rs`, `brush_bsp.rs`, `surface_materials.rs` and the residency dry run changed in `level-compiler` (#636, the brief's prerequisite). No tracer, cache, BVH or margin source moved; the brief's Decision reads stand.
- "hallway" → `content/dev/maps/stress-warren-hallway-inspection.map`; "hallway-mini" → `stress-warren-hallway-inspection-mini.map`; "warren-mini" → `stress-warren-mini.map`. A cold hallway bake runs for hours (bake-parallelism-large-maps R0: Lightmap 6,311 s on Windows before its levers), so hallway runs are the owner's (the gate-runs preamble allows it). Planning around it by reading hallway-mini and warren-mini at the first slice as the early cost read; a breach of ~10% there stops the build for the owner before any hallway run.
- Tasks 3, 6 and 7 land as one milestone: a tolerance hit lies up to τ outside its triangle's exact box, so brute-force parity holds only once the boxes are padded and the box test conservative.
- The flat-box witness pinned at baseline (`sh_bake/trace_tests.rs`) was rejected only through `bvh::Ray`'s re-normalized direction, which the box-query Decision removes. Re-found by search for the query built from the hit test's own ray, crossing the edge at τ = 0. On it the conservative factor alone and the pad alone each block it (AC 9's PR record).
- Möller–Trumbore is exactly watertight on an axis-aligned shared edge when both triangles list it in the same corner slot (the edge tests are exact negatives). The shared-edge fixture rotates each triangle's corners, as face fans do, so the MT-restored run slips (AC 2).
- Reflection-probe consumers of the hit test: the capture's masked re-trace stores `f32` distances and passes them back as `min_distance`, so `SceneTracer::nearest` bounds the `f32` distance too; the exclusion trace treats a tolerance hit on an excluded face whose hull the ray never enters as no crossing. Two probe tests read τ: the open-top distance test skips texels whose rays cross a wall's plane within 2τ above its top, and the placement-invariance test's precondition moves from the derived box (which differed under MT only by `f32` rounding noise, 3.2511997 vs 3.2512) to the capture point, which deleting the pillar genuinely moves.
- The cache stores today a triangle offset only. The gated cache needs the leaf the traversal tests, so it stores the leaf's node index with the triangle (`BvhPrimitive::bh_node_index`); the leaf box is the parent's stored child box, or a root leaf's primitive box.

## Cost gate (Task 5) — breached, then cleared by optimization

Resolution: an optimization round on the same answers (no output byte moves) brings warren-mini Lightmap Bake to +8.3% and SH Bake to parity, inside the gate; the owner had also said they were inclined to accept the cost, since the accuracy unblocks later briefs. Details at the end of this section. hallway-mini and hallway timings remain for the landing read (AC 24).

The first-slice cost read breached the ~10% gate on warren-mini; per the cost Decision the build stops for the owner before any fallback or hallway run. Cold `--release --no-tui` bakes on the Intel Mac, `/usr/bin/time -l` (instruction counts are clock-independent; the wall times are not):

| Build | Lightmap Bake | SH Bake | Delta SH | Direct SH | Total | Instructions retired |
|---|---|---|---|---|---|---|
| baseline (a443aecfe, MT) | 222.55 s | 23.68 s | 6.51 s | 1.16 s | 306.07 s | 10.03 T |
| f64 watertight (92b6da145) | 283.45 s (+27%) | 32.66 s (+38%) | 9.51 s | 1.36 s | 385.42 s | 14.03 T (+40%) |
| plain f32, same algorithm | 283.06 s (+27%) | 33.90 s | 10.07 s | 1.38 s | 389.00 s | 14.00 T |
| baseline, rerun | 238.93 s | 21.90 s | 6.12 s | 1.09 s | 331.02 s | 10.03 T |
| f64 + per-ray rows, inlined ray setup, leaf box stored in the occluder | 404.65 s (throttled: CPU_Speed_Limit 52 at exit) | 50.01 s | 15.27 s | 2.08 s | 547.90 s | 14.33 T |

- Clean rerun (idle machine, CPU_Speed_Limit 100 throughout; an unrelated agent may have shared the CPU during the earlier rounds): baseline Lightmap 215.06 s, SH 21.78 s, total 293.41 s, 10.03 T instructions; f64 (92b6da145) Lightmap 275.78 s (+28.2%), SH 29.62 s (+36%), total 374.80 s, 14.03 T. The breach stands.
- `f32` ≈ `f64`: precision is not the cost, so the plain-`f32` fallback buys no time.
- Sampled profile of Lightmap Bake (30 s each, symbolized): tracer share of busy samples 21.8% at baseline (`segment_clear_remembering` + closure 11.1%, MT 5.4%, traversal 5.3%) against 34.4% after (`segment_clear_remembering` 16.4%, `TraceRay::hit` 10.7%, traversal 5.9%, corner fetch 1.3%): the shadow tracer costs about 1.9× baseline. The hit test is ~2× MT (MT rejects most misses after one barycentric; the watertight test computes all three edge functions and runs the τ reach check on every miss). Two overheads found and fixed in the working tree: a 128-byte `TraceRay` moved out of `segment_ray` by `rep movsq` per ray, and the cache gate's two dependent node loads (the leaf box now rides in `Occluder`). Neither moved the instruction count; the per-ray row vectors add a multiply-add per projection over the indexed form and may cost more than they save.
- hallway-mini not timed (stopped before it to profile).

Optimization round (working tree after aad4145da; output `cmp`-identical to aad4145da on warren-mini):
- `BoundedRay::conservative_entry`: a NaN axis takes the whole line as its slab through selects instead of a `continue`, so the slab loop unrolls with plain comparisons (~100 → ~45 instructions per box); the prune limit is kept beside the bound.
- `TraceRay`: the per-ray rows are replaced by an `Axes` permutation dispatched once per triangle into a const-generic `crossing::<KX, KY, KZ>`, with the 92b6da145 arithmetic; `hit` is always inlined and returns undivided values, so shadow callers do no divide; the tolerance prefilter uses plain comparisons; the distance check is a cold out-of-line function; `triangle_points` is spelled out per corner.

| Same-session pair | Instructions | Lightmap Bake | SH Bake | Delta SH | Direct SH | Total |
|---|---|---|---|---|---|---|
| MT baseline | 10.03 T | 227.92 s | 23.88 s | 6.62 s | 1.28 s | 312.80 s |
| optimized | 10.65 T (+6.2%) | 246.93 s (+8.3%) | 22.49 s | 6.36 s | 1.11 s | 327.11 s (+4.6%) |

Remaining cost is serial per-ray setup (the shear divides and the cache gate's box test, both needed for these answers). Further levers move bytes (reciprocal-multiply shear, ~1–2%) or need SIMD target features.

Options that were put to the owner: an optimization round on the same answers (per-ray monomorphized axis permutation instead of rows or runtime indexing; a cheaper miss path), with no promise it reaches 10%; accept the cost; or restate the gate.

## Delegated answers
- None; the brief has no open questions.

## AC-to-proof

| AC | Proof | Status |
|---|---|---|
| 1 Brute-force reference replaces every stock-ray reference scan; every tracer equals it on parity and rounding-stress fixtures | `ray_traversal` brute-force helpers; parity tests in lightmap, SH, chunk light list, scatter tracers and `sh_bake/trace_tests.rs` | achievable as stated |
| 2 Shared-edge proptest, every entry point, τ = 0 and shipped τ; fails at τ = 0 with MT restored | proptest in `ray_traversal` tests over every entry point; MT-restored failure shown by a test-only MT reference asserting it misses on the same cases | achievable as stated |
| 3 Cracks: narrow crease and T-junction block; wide crack clears; narrow fail at τ = 0 | focused tests on the hit test and `segment_clear` | achievable as stated |
| 4 Silhouette bounded by τ, sliver acute vertex included | focused test | achievable as stated |
| 5 1 mm triangle at 0.5° blocks; fails with old det threshold | focused test | achievable as stated |
| 6 Both windings; just short / just past segment end, either det sign | focused test | achievable as stated |
| 7 Front-face callers: ±x/±y/±z dominant axes; front hit, reversed skipped, det sign agrees with normal | focused test on the shared front-face predicate and both callers | achievable as stated |
| 8 Cache independence: empty vs each primitive cached; edge/vertex hits equal brute force; one-primitive tree; planted sliver past prune limit fails without leaf-box check | test with a `#[cfg(test)]` seam on the hit test's `t` | achievable as stated |
| 9 Flat-box witness blocked with empty cache; clear with both factor and pad removed; PR records which alone blocks | `sh_bake/trace_tests.rs` witness, inverted | achievable as stated |
| 10 Box test ⊇ stock on lattice proptest; face-plane rays both zero signs, min and max faces, flat and thick boxes; traversal reaches the triangle | `ray_traversal` proptest and focused test | achievable as stated |
| 11 Exact box: accepts witness leaf box stock rejects; rejects with factor removed | focused test | achievable as stated |
| 12 Segment ending short of box entry tests none of its triangles, empty cache and cached | counter-based test | achievable as stated |
| 13 Padded box ⊇ exact box grown by τ + ulp(E) on every face, exact compare, coordinate 0 and inward-rounding coordinate; holds every accepted point; shipped tree exact | focused tests on the pad and `build_bvh` | achievable as stated |
| 14 Every traced tree padded: pre-cut, post-cut rebuild, no-cut map, constructor; assertion runs on each | tests over `build_bvh` and the pipeline's rebuild site | achievable as stated |
| 15 Planted parent omitting a child sliver trips the assertion (debug-only test); every fixture BVH passes | `#[cfg(debug_assertions)]` `#[should_panic]` test | achievable as stated |
| 16 Shared-crease fixture: padded tree yields leaves in exact tree's order | focused test | achievable as stated |
| 17 Tracer epoch passed to each key builder changes each key; grep gate on other key sites | key-builder tests; grep in PR | achievable as stated |
| 18 Margin searches find no accepted hit beyond re-derived margins; rescaled draws; reach guards hold; query-growth test; exact-skips A11/A12 at 0.95× and 1.05× | `shaft_cull/tests.rs` and exact-skips tests | achievable as stated |
| 19 Existing parity: layered = monolithic, warm = cold, worker-count identity | existing gates, `--ignored` focused | achievable as stated |
| 20 Whole-PRL `cmp` and per-section diff on every gate map; tracer-fed sections named first; other sections and id 19 identical; owner reviews | executor runs the small gate maps; owner runs hallway; owner reviews diff | owner, before landing |
| 21 Warm over a pre-change cache equals cold; every tracing stage with an entry logs a miss | executor run per gate map (hallway: owner) | achievable as stated |
| 22 Epoch landing value set in or after the last answer-changing commit | branch log check | achievable as stated |
| 23 Counters: hallway-mini box tests, leaf visits, triangle tests, `det == 0` rejections per shadow and gather ray, before/after, deterministic; byte-neutral commit | `#[ignore]` counter harness; digest equality on its commit | achievable as stated |
| 24 Timing: hallway and warren-mini cold Lightmap Bake vs baseline, SH/Delta/Direct SH recorded, hallway in plain f32 too; first slice and landing | executor: warren-mini and hallway-mini; owner: hallway | manual (owner for hallway) |
| 25 f32 fallback origin check | only under the fallback | n/a unless the owner orders the fallback |
| 26 Shared-edge census on every gate map, pre- and post-cut, bands; τ covers widest crease crack | `#[ignore]` census harness | achievable as stated |
| 27 Grep gate: no MT, no det threshold in bake tracers (spine test exempt); f64 hit test; no runtime crate or shader changed | grep in PR | achievable as stated |

## Tasks

| # | Task | Owner | Depends on | Status |
|---|---|---|---|---|
| 1 | Counters (byte-neutral): box tests, leaf visits, triangle tests, MT det rejections per shadow / gather ray; hallway-mini `#[ignore]` report; baseline numbers | integrating executor | — | done a443aecfe. Before (MT, hallway-mini, `trace_counts_on_a_map`): shadow 10,621,462,554 rays, 13.895 box tests, 0.013 leaf visits, 0.732 triangle tests, 0.000467 det rejections per ray; gather 368,646,144 rays, 49.269 / 1.778 / 3.576 / 0 |
| 2 | Move the lightmap tracer functions into a `ray_traversal/` module, behavior-preserving | integrating executor | 1 | done 66e05981f |
| 3 | Shared watertight f64 hit test with τ in `ray_traversal`; every tracer calls it; brute-force reference; AC 1–7 tests | integrating executor | 2 | done with 6 and 7: `ray_traversal/{hit_test,occlusion,pad}.rs`, tests in `hit_test_tests.rs`, `tree_tests.rs` (33 tests) |
| 4 | Shared-edge census harness; set τ (AC 26) | integrating executor | 3 | done: `ray_traversal/{census,census_tests}.rs`, `#[ignore]` `shared_edge_census_on_gate_maps` (geometry only, all gate maps in ~5 s, deterministic) and a fixture unit test. Widest crease crack on any gate map, pre- or post-cut: 198.664 µm (movement-feel, sloped concrete_stone_022 brushes 25/28); hallway 16.1 µm (45° walls), hallway-mini 8.1 µm, kinematic-platform 4.8 µm, the rest < 0.1 µm. No mismatched edge above 0.25 mm, so no open boundary. Only kinematic-platform's cut moves geometry (20 faces, 74 → 94; +34 T-junction edges, all ≤ 1 µm). Pre-cut T-junctions are common (hallway 13,386), from face extraction, gaps 0 or within f32 rounding. τ = 0.5 mm stands (covers the widest crack 2.5×) |
| 5 | First-slice cost read: counters after, warren-mini and hallway-mini cold Lightmap Bake f64 and plain f32 against baseline (AC 23, 24 first read); stop if >~10% | integrating executor | 3, 4 | warren-mini read: breached (+27% Lightmap, +38% SH), cleared by the optimization round (+8.3% Lightmap, SH parity); see Cost gate. Counters after (bd684310e, hallway-mini, `trace_counts_on_a_map`): shadow 11,285,249,511 rays (+6.2%; likely the τ-grown shaft margins clearing fewer tiles, not yet confirmed), 15.320 box tests (+10%; padded boxes), 0.013 leaf visits, 0.622 triangle tests (−15%; likely the cache gate skipping out-of-reach occluders), 0.000042 `det == 0` rejections per ray; gather 368,646,144 rays (same), 49.601 / 1.951 / 3.925 / 0. hallway-mini timing still to run |
| 6 | Conservative box test from the hit test's ray; padded bake trees with containment assertion; flat-box witness (AC 9–16) | integrating executor | 3 | done with 3 |
| 7 | Gated occluder cache (AC 8, 12) | integrating executor | 6 | done with 3 |
| 8 | Shaft-cull margins re-derived for the f64 test and τ (AC 18) | integrating executor | 6 | done: `shaft_cull/margin.rs` re-derivation; `place` window tightened to ±2% (τ puts margins near its 1 mm start); 55 `shaft_cull`/`texel_footprint` tests pass |
| 9 | `TRACER_EPOCH` folded into every tracing stage key; key tests; landing value last (AC 17, 22) | integrating executor | 7, 8 | folded at all 12 research-table keys as a builder input (chunk light list: the parameterized builder; its wrapper supplies the constant, as it does the stage version); 12 `*_key_changes_with_tracer_epoch` tests; landing value still pending (AC 22) |
| 10 | Gate runs: parity suites, PRL diffs, warm-over-old-cache, landing timings, grep gates (AC 19–27) | integrating executor; owner for hallway | 9 | |
