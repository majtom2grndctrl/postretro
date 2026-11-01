# watertight-tracer — plan of record

mode: compact
status: active
read at: 26ab6c534

## Corrections
- Brief read at 343ff98b6 → since then only `partition/face_extract.rs`, `brush_bsp.rs`, `surface_materials.rs` and the residency dry run changed in `level-compiler` (#636, the brief's prerequisite). No tracer, cache, BVH or margin source moved; the brief's Decision reads stand.
- "hallway" → `content/dev/maps/stress-warren-hallway-inspection.map`; "hallway-mini" → `stress-warren-hallway-inspection-mini.map`; "warren-mini" → `stress-warren-mini.map`. A cold hallway bake runs for hours (bake-parallelism-large-maps R0: Lightmap 6,311 s on Windows before its levers), so hallway runs are the owner's (the gate-runs preamble allows it). Planning around it by reading hallway-mini and warren-mini at the first slice as the early cost read; a breach of ~10% there stops the build for the owner before any hallway run.
- The cache stores today a triangle offset only. The gated cache needs the leaf the traversal tests, so it stores the leaf's node index with the triangle (`BvhPrimitive::bh_node_index`); the leaf box is the parent's stored child box, or a root leaf's primitive box.

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
| 1 | Counters (byte-neutral): box tests, leaf visits, triangle tests, MT det rejections per shadow / gather ray; hallway-mini `#[ignore]` report; baseline numbers | integrating executor | — | |
| 2 | Move the lightmap tracer functions into a `ray_traversal/` module, behavior-preserving | integrating executor | 1 | |
| 3 | Shared watertight f64 hit test with τ in `ray_traversal`; every tracer calls it; brute-force reference; AC 1–7 tests | integrating executor | 2 | |
| 4 | Shared-edge census harness; set τ (AC 26) | integrating executor | 3 | |
| 5 | First-slice cost read: counters after, warren-mini and hallway-mini cold Lightmap Bake f64 and plain f32 against baseline (AC 23, 24 first read); stop if >~10% | integrating executor | 3, 4 | |
| 6 | Conservative box test from the hit test's ray; padded bake trees with containment assertion; flat-box witness (AC 9–16) | integrating executor | 5 | |
| 7 | Gated occluder cache (AC 8, 12) | integrating executor | 6 | |
| 8 | Shaft-cull margins re-derived for the f64 test and τ (AC 18) | integrating executor | 6 | |
| 9 | `TRACER_EPOCH` folded into every tracing stage key; key tests; landing value last (AC 17, 22) | integrating executor | 7, 8 | |
| 10 | Gate runs: parity suites, PRL diffs, warm-over-old-cache, landing timings, grep gates (AC 19–27) | integrating executor; owner for hallway | 9 | |
