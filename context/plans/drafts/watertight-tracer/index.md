# watertight-tracer

Brief · compact · reads: `context/lib/build_pipeline.md` §Ray traversal, §Build Cache · `context/lib/development_guide.md` §3.6 · `context/lib/testing_guide.md` §Slow / cold-bake suites · read at 343ff98b6

## Problem

Raised by the owner from two findings. The `lightmap-reach-cull` review found that the remembered occluder (`last_occluder`, `clip_occluder`) is tested whether or not the traversal would reach it, so a shadow answer changes with cache contents and walk order. That blocks exact culls such as `lightmap-reach-cull` and adds noise to the per-state comparisons of `ready/mover-light-flow--foundation/`. The leak diagnosis observed rays within 1 µm of a shared edge miss both triangles on movement-feel, and every leaking texel it found is buried (`drafts/lightmap-reach-cull/leak-diagnosis-findings.md` §H3 residue, §Visible impact); no visible leak from the tracer has been seen. Two more holes have no measured occurrence on a map: Möller–Trumbore's scale-dependent `|det| < 1e-8` rejection drops real crossings on small triangles at grazing angles, and the f32 box test can reject a box whose triangle the ray hits at an axis-aligned edge or vertex. The diagnosis calls the hit test sound and shared-edge watertightness "none proportionate". This brief goes further because the owner's bar is no visible crack or leak, and once the cache can't change answers, exact culls rely on the hit test's error bound. When this is done, every bake's shadow and closest-hit answer is a function of the segment and the geometry alone. A ray through an edge or vertex that two triangles share with bit-identical vertices hits one of them, no box holding a hit is rejected, and the cache only reorders work. How many edges are shared that way, rather than meeting at T-junctions, is unmeasured; this brief counts them. Output bytes move once, and the owner reviews the move.

## Decisions

- **Watertight hit test in every bake tracer, in f64.** Woop/Benthin/Wald 2013 replaces every Möller–Trumbore copy (research §Tracer inventory). Bake vertices and rays are f32, so the first subtraction is exact in f64 and the shear adds about 2⁻⁵³ relative error (research §Precision and pad). It is two-sided: it accepts when all three edge functions share a sign, compares `t` against `t_max·|det|` with det's sign folded in, and rejects only at `det == 0`.
- **f32 only if f64 fails the cost gate.** The first slice compares f64 with the same test in f32 on hallway-mini's deterministic counters, then times both on hallway cold `--release` Lightmap Bake. Only if f64 fails the gate below does the test run in f32, with a pad sized to the extent; every tracer segment must then start inside the padded extent, so the chunk light list's sun segments start at the sample, not 10 km out.
- **Conservative box test, pbrt-v4 style.** `tFar *= 1 + 2γ₃`, and a NaN slab constrains nothing. The query is built from the hit test's own origin and direction, not from `bvh::Ray`, which re-normalizes. The accept set becomes a superset of the stock test's, replacing `build_pipeline.md` §Ray traversal's "accepts exactly the stock boxes".
- **Padded boxes, id 19 untouched.** Build the tree on exact boxes and write the shipped BVH section (id 19) from it. Then grow every box of the bake's tree, node child boxes and primitive boxes alike, outward by one f32 ulp of the bake's extent, its largest coordinate magnitude. That step is ray-independent and covers the f64 test's rounding (research §Precision and pad), so an axis-aligned shared edge can't leak between two triangles' boxes. A uniform step keeps every parent containing its children and the topology, so the shipped section and closest-hit tie order stay put (`done/bake-ray-speed-contract.md` held id 19 byte-identical). A debug assertion checks that every parent box contains its children (`development_guide.md` §3.6).
- **The cache can't change the answer.** The remembered occluder is stored as a primitive index and trusted only when its leaf box passes the same `BoundedRay` predicate the traversal applies. Parents are exact unions, the predicate is monotone in containment, and the iterator reaches every leaf whose ancestor chain passes, so the cache only reorders the traversal's own candidates (research §Cache independence). The answer equals the cache-free traversal's on the slab path and the distance-prune path.
- **Keep the distance prune.** A primitive box is the min/max of the vertices the hit test reads, so exact box entry ≤ exact `t`. A pruned box holds no true hit before the segment end; a cached "hit" there is the hit test's `t` error, which the cache's box check refuses.
- **`TRACER_EPOCH` in `ray_traversal.rs`,** folded into the key of every cached stage whose computation reaches the tracer, as `PORTAL_WALK_EPOCH` and `SOFT_VISIBILITY_EPOCH` are (research §Stages that reach the tracer). Any later change to tracer answers bumps it. Uncached tracing stages, entity-shadow select and texel steps among them, need no fold; texel steps' stored report rides the lightmap-section and weight-map keys.
- **Shaft-cull margins re-derived for the watertight test.** `margin.rs` sizes each triangle's reach to Möller–Trumbore's `e1`, `e2`, `s`, its det threshold and the shape term ρ. The executor re-derives them, reruns the two 150k-case margin searches and reruns `done/lightmap-exact-skips/` A11 and A12. A margin too small breaks bytes, not light: the cull would clear a tile the tracer blocks.
- **Bytes move once.** This lands after `face-extract-split-epsilon` (branch `fix/face-extract-split-epsilon`) and before `lightmap-reach-cull`, which names it as a prerequisite. The whole move is this brief's, and the owner reviews it per section before landing.
- **Correctness outranks speed.** No fallback to a test that isn't watertight. Above about 10% of hallway Lightmap Bake, the build stops for the owner.
- **Named gaps, counted, not closed here:** edges whose triangles share no bit-identical vertices, crease cracks and T-junctions among them; rays lying in a face's plane (`det == 0`). Compiler-side edge snapping is the fix if either ever shows (leak diagnosis §Recommendation 5).
- **No obstacle for lighting states.** Doors, large movers, a rising room and destruction get per-state masks that compare shadow answers across poses (`ready/mover-light-flow--foundation/`). A cache-independent, watertight answer serves them as is. A per-state BVH needs only the padded-box invariant: every parent contains its children.
- **Layer: compiler only.** All code stays in `postretro-level-compiler`. No runtime crate, shader or shipped section layout changes. Runtime ray tests in `sim` read collision, not the bake BVH, and stay as they are.

## Acceptance

### Automated

Tracer answers:
- [ ] A brute-force reference that tests every triangle, with no BVH and no box test, replaces every stock-ray reference scan. Every bake tracer's occlusion answer equals it on the existing parity fixtures and the rounding-stress fixture, whose rays stay inside the prune slack; closest hits match its distance, and a tie may name either triangle.
- [ ] Shared edges, proptest at map coordinates 10–500 m: two triangles in different faces share an axis-aligned edge with bit-identical vertices, and a ray crosses exactly on it, or on a shared vertex, at any angle. The full tracer, BVH included, reports a hit on every case. It fails with Möller–Trumbore restored.
- [ ] A triangle with 1 mm edges, crossed under 1° from its plane, blocks. The test fails with the old det threshold restored.
- [ ] Both windings block a crossing segment. A hit just short of the segment end blocks and one just past it clears, for either sign of det.

Cache independence:
- [ ] For each segment, the shadow answer with an empty cache equals the answer with each primitive cached in turn. Segments include hits exactly at an axis-aligned edge or vertex, where the answer also equals the brute-force reference. They also include a grazing segment near a sliver's end whose computed `t` falls inside the segment while its box starts past the prune limit, found by search or planted through a test seam on the hit test's `t`. The sliver case fails with the leaf-box check removed.
- [ ] The flat-box rounding witness, a steep ray through a flat triangle's axis-aligned edge, reads blocked with an empty cache; it reads clear with the box fix removed.

Box test and tree:
- [ ] The box test accepts every box the stock test accepts, on the existing lattice proptest, and accepts a ray lying in a box's face plane.
- [ ] A shadow segment that ends well short of a box's entry tests none of that box's triangles.
- [ ] Every bake-tree box, node child and primitive alike, contains its exact box grown by the step on every face, including faces at coordinate 0; the shipped tree keeps the exact boxes.
- [ ] A planted tree whose parent box omits a sliver of one child trips the containment assertion; every fixture's BVH passes it.

Stages and margins:
- [ ] Changing the tracer epoch changes the key of every stage in research §Stages that reach the tracer, and of no other stage.
- [ ] Both shaft-cull margin searches, 150k cases each against the watertight test, find no accepted hit beyond the re-derived margins. The A11 and A12 fixtures pass at 0.95× and 1.05× the new margins.
- [ ] Existing parity holds: the layered lightmap equals the monolithic oracle at each floor setting, warm equals cold, and output is identical across worker counts.

### Gate runs

Machine-checked, launched by hand; the owner may run the long ones. Baseline: `main` after `face-extract-split-epsilon`, same session, idle machine, one cold `--release` bake at a time. Gate maps: hallway, hallway-mini, movement-feel, kinematic-platform, `gate-heavily-lit`, `combat-demo`, `soft_shadow_test` and `reflection-probes-test`; with `--no-penumbra-floor`, hallway-mini and `soft_shadow_test`.
- [ ] Whole-PRL `cmp` and a per-section diff against baseline on every gate map, recorded in the PR. Sections no tracer feeds are identical, and so is the BVH section (id 19), byte for byte. **The owner reviews the diff before landing.**
- [ ] Warm over a cache the pre-change binary filled equals cold on every gate map, and every stage that reaches the tracer logs a miss.
- [ ] Cost, counters first: on hallway-mini, the test-only counters report box tests, leaf visits and triangle tests per shadow and gather ray, before and after, deterministic across runs.
- [ ] Cost, timing: hallway and warren-mini cold `--release` Lightmap Bake against baseline, with SH Bake, Delta SH Bake and Direct SH Bake recorded, and hallway also with the test in f32. Reported to the owner with the precision that ships. **Above about 10% of hallway Lightmap Bake, stop for the owner.** Under the f32 fallback, a test or debug assertion shows every bake's segment origins on the parity fixtures inside the padded extent.
- [ ] Shared-edge census, reported, not thresholded: on every gate map, the count of triangle edges another triangle shares with bit-identical vertices, against T-junctions and other mismatched edges.
- [ ] Grep gate on the diff: no Möller–Trumbore test and no det threshold remain in the compiler, the hit test's arithmetic is f64 unless the PR records the fallback, and no runtime crate or shader changed.

### Manual

None required beyond the owner's diff review. If the diff isn't trivial, the owner may A/B a gate map in the engine.

## Path

- **One shared hit test.** Put the watertight test and its per-ray precompute (axis permutation, shear) beside `BoundedRay`, and have every tracer call it. Closest-hit callers need `t` and a normal (SH) or barycentrics (reflection probes); lower bounds (`> 0`, `> RAY_EPSILON`, `> min_distance`) need a scaled compare or a divide after acceptance. The rival, patching each copy in place, invites drift.
- **Front-face filters.** Reflection probes and `front_faces_clear` filter by their own f32 winding test before the hit test. Take front from det's sign instead, so filter and test agree on grazing rays.
- **Rival to the gated cache: drop it.** Answer-neutral, but the remembered occluder answers 65.8% of hallway-mini's shadow rays, mostly on dark texels (`drafts/lightmap-stage-sizing/findings.md` §1); the gated cache keeps that speed with the same answer.
- **The leaf box.** The iterator tests a leaf as its parent's stored child box, not the primitive's own (`bvh` 0.12 `bvh/iter.rs`). The cache must test the same box. Cache users: `lightmap_layer::chart_walk`, `shadowmask_bake::estimate`, `lightmap_bake::reference`, `texel_footprint::test_fixtures`. The shaft cull's `last_blocker` is a cull, not a tracer cache, and stays.
- **Box query.** `BoundedRay::new` takes the hit test's origin and direction and derives the inverse itself. pbrt-v4's `Bounds3::IntersectP` is the precedent.
- **Pins that invert.** `sh_bake/trace_tests.rs`: the flat-box witness, `bounded_traversal_matches_stock_rejection_of_undefined_slab`, and the rounding-stress envelope comment. `ray_traversal.rs`: the face-plane rejection test and the exact two-pass proptest. Stock-ray references: `segment_clear_full_scan` in the lightmap, SH, chunk light list and scatter tracers, and SH `closest_hit_full_scan`.
- **First slice.** The shared hit test with the shared-edge proptest, the small-triangle row and the shared-edge census, then f64 against f32: hallway-mini counters first, then the hallway timing. Then the box test and padding with the flat-box witness. Then the cache gate.
- **File size.** `lightmap_bake.rs` is past 1,500 lines and `chunk_light_list_bake.rs` past 3,500. Move the lightmap tracer functions into `ray_traversal` first, behavior-preserving, in their own commit; the other copies are deleted.
- **Docs at landing.** `build_pipeline.md` §Ray traversal: the watertight test and its precision, the superset box test, padded boxes, the cache-independence contract and the named gaps. §Build Cache: `TRACER_EPOCH` and its bump rule.

## Open questions

None.
