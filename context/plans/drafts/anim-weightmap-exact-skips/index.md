# anim-weightmap-exact-skips

Brief · compact · reads: `context/lib/build_pipeline.md` §Compiler pipeline step 12, §Ray traversal, §Build Cache · read at 020cf04b5

## Problem

The `lightmap-exact-skips` brief left the animated weight-map bake out of scope and asked for its cost first. Its build measured it cold: AnimWeightMaps 13.4 s against Lightmap Bake's 133.8 s on `stress-warren-mini`, and 2.4 s against 11.5 s on `kinematic-platform`. The full-size maps with baked animated lights are unmeasured. The basis is an anticipated need, not a defect. The walk repeats work the static walk has now dropped: a lit texel traces every footprint ray to an emitter no triangle could block. And the stage caches as one whole entry, so any keyed edit re-bakes all of it. When this is done, one of two things is true. A measurement shows the stage is not worth the work, and the brief stops with its findings. Or the weight walk skips every tile that provably sees its emitter, by the precedent's proof, with weight-map bytes, the texel-step report and the cache key unchanged on every gate.

## Decisions

- **Build for the levels authors will make, not today's.** Owner ruling: assume bigger levels and more detail than current content, so an idea someone falls in love with just works. No stop threshold gates the build. Task 1 still bakes the full-size maps with baked animated lights cold on the base commit first (research §Maps with baked animated lights, §Measurement method): it picks the gate map, reports the lit / dark / uncovered split, and lets the plan of record pin the wall-time target. Today the whole stage is 5.5% of `stress-warren-mini`'s build; the cull is for the maps that come after it.
- **Builds on `lightmap-exact-skips` as landed.** It starts after that brief merges, from main, not its branch. One implementation of the shaft test, its margins and the coverage predicate serves both walks. A second copy of a rounding proof would drift. If that code changes in review, this brief re-reads it before building.
- **Exact by construction, verified by digest.** The standard is the precedent's (`plans/done/bake-ray-speed-contract.md`): byte identity by fixture parity and cold whole-PRL `cmp`, with the same accepted gaps (rays under the 2° angle floor, slivers past ρ_cap outside the query).
  - No byte moves, so `STAGE_VERSION` and the key stay, and base-built cache entries stay valid.
  - A gate that moves a byte stops the build and goes to the owner as a measured amendment.
- **Tile shaft cull: every light the stage bakes, on both samplers.**
  - The stage bakes points and spots only, with no suns (research §Premises).
  - None of them moves: an animation changes brightness, colour and spot aim, never origin or size.
  - The precedent's hull containment and margin derivation transfer unchanged. The sampler and site are the same, and so is the tracer: `segment_clear` is `segment_clear_remembering` with a fresh cache.
  - A sampler, tracer or texel-frame change re-checks both walks.
- **No occluder cache, so a simpler exactness argument.** A clear verdict means no triangle gives an accepted hit on any segment the tile's texels would trace, so each has visibility exactly 1.0. Nothing else in the walk reads those rays. The verdict depends only on light, chart and tile, so walk order changes how often a tile is tested, never a byte. Verify mode checks the 1.0 alone.
- **Tiles anchor on the chart's interior grid, as in the static walk, with one verdict per (face, light) task.**
  - A tile is tested lazily, on its first texel past the weight gate.
  - Its verdict holds across every chunk of the face that lists the light.
  - The hull spans the whole tile, including texels in chunks that do not list the light. That costs only speed, and it keeps hulls independent of chunk splits, which move with light counts.
- **Coverage cull only if the split pays.** Chunk lists filter by range alone, so lights behind a face and spots aimed away still reach the walk (research §What the chunk lists already drop). There an uncovered texel traces no ray, so the cull saves only arithmetic and task overhead.
  - It is built only if task 1's split puts uncovered visits at ≥ 10% of stage CPU on the gate map.
  - If built, it applies the precedent's predicate to the face's chart before the walk. A dropped (face, light) stores no entry and reports no step.
- **Embedded emitters take the shaft test unchanged.** Their tiles never clear.
- **Culls-off is a test seam only.** It adds no flag, env var or key input, and production always culls.
- **The oracles stay cull-free.** The texel-major reference has its own walk. The full-set reference shares the production walk, so it bakes with the culls off. Both then check the culls.
- **Non-goals:**
  - **An occluder cache in this walk.** Dark texels are likely its larger cost, and neither skip touches them. But a cache can flip clear to blocked at a box-boundary hit, so it moves bytes in principle: an owner decision on measured bytes, not an exact skip. Task 1's split reports the dark share.
  - **Narrowing the cache key.** It folds animation curves the bake never reads, so a curve edit re-bakes the stage to identical bytes. That is a key change, not a skip.
  - **One shaft scene shared across stages.** Each stage builds its own from its own inputs.
  - **The texel-step detector's surface traces.**
  - **Chart plane drift.** The hull follows the texel formula, whichever frame lands (research §Adjacent findings).

## Acceptance

### Automated

Shaft cull. Every row compares weight-map section bytes and the step report with culls on and off, at floor on and off, with verify on:
- [ ] An open floor under a soft point light, where tiles test clear, and the same floor with an occluder shadowing part of it.
- [ ] The split-face parity fixture. A light both chunks list tests the tile across the split once (research W1). For a light one chunk lists, the other chunk gains no entry (W2).
- [ ] A tile that a spot's cone edge, the range edge or the weight-epsilon fringe crosses, so its first texel past the gate is not its first (W3).
- [ ] A chart whose interior is not a multiple of 8 on either axis, a one-texel chart, and a chunk clipped to the chart interior. Each has an occluder just past its last texel column, inside the pad (W4).
- [ ] A triangle outside the padded hull by less than its margin keeps the tile blocked, and one outside by more clears it.
- [ ] A cut sub-chart whose window origin is not zero (W5).
- [ ] A hard point, a spot and a flagged embedded emitter, whose tiles never clear. An `sdf` light on the same face is never baked or tested.
- [ ] The occluder fixture translated to at least 4 km, where the cull may decline every tile.
- [ ] A floor-off hard light whose edge steps beside a clear tile. The report names the same (light, face) with the same pairs (W7).
- [ ] Two lights on one face, and a multi-face scene: bytes and report identical across 1 and 4 workers, and warm equal to cold (W6).
- [ ] Verify mode re-traces every covered texel of each clear tile and gets exactly 1.0. A tile forced clear over a shadow fails it.
- [ ] In the open-floor fixture, a tile with a texel past the gate is tested once per (face, light), and a tile with none never is. A clear tile's texels trace no shadow, clip or lead-in segment.
- [ ] Permit side. A cull that never fires, or a margin too wide for authored scale, fails here:
  - the open-floor fixture records a clear tile at the origin. It also does so translated to ~104 m, with the floor one triangle tens of metres long;
  - the split-face parity fixture records a clear tile at each floor setting, so the cull-free oracle checks the cull.
- [ ] Existing parity holds:
  - the face bake matches the texel-major reference at each floor setting and sample count;
  - the full-set reference;
  - an embedded emitter bakes alike on every path;
  - two builds are byte-identical;
  - the stage-version key tests pass unchanged.

Coverage cull, if built. Each row asserts the (face, light) is dropped or kept, and that bytes and report match culls off:
- [ ] A point light 1 cm behind the face is dropped, and one just in front is kept.
- [ ] A spot just outside its outer cone is dropped, and one just inside is kept.
- [ ] A light just past `falloff_range` of the nearest texel centre is dropped, and one exactly at range is kept.
- [ ] On one face, a dropped light's slots stay empty and a kept light's entries are unchanged. The unlit-chunk cull drops the same chunks, and progress completes at its published total (W8).
- [ ] The split-face parity fixture records a dropped pair.

### Manual

- [ ] Task 1, before any code: research §Measurement method on `stress-warren-hallway-inspection`, `stress-warren-showcase`, `stress-env-volumes` and `campaign-test`. Report AnimWeightMaps, Lightmap Bake, SH Bake and the total, with the animated light and chunk counts. Report the split (lit, dark, uncovered) on each, and name the gate map: the one with the largest AnimWeightMaps time.
- [ ] Cold whole-PRL `cmp`, base-commit binary against the new one, identical:
  - floored: `kinematic-platform`, `stress-warren-mini`, `campaign-test`, `animated-layer-spill`, every `test_animated_weight_maps_*` map, and each task-1 map that passed;
  - `--no-penumbra-floor`: `kinematic-platform`, `test_animated_weight_maps_occluded`.
- [ ] Warm on `kinematic-platform` and `stress-warren-mini`, the new binary over a cache the base binary filled. It logs an `animated_lm_weight_maps` hit, and the PRL equals the cold bake (W9).
- [ ] If any gate moves a byte, the build stops. It reports the moved-texel count, the largest weight change, and each moved texel's blocking hit re-tested in double precision.
- [ ] On each gate map, count the triangles past ρ_cap, which sizes the accepted sliver gap there.
- [ ] Grep gate on the diff:
  - no new prl-build flag, no new `POSTRETRO_` env var, and no counters in production code;
  - `STAGE_VERSION` unchanged, and no key input added;
  - the margin constants and the shaft test exist once in the crate.
- [ ] Wall time, against a same-session base-commit baseline, in task 1's configuration, with SH Bake as control. The owner may launch the long runs.
  - On the gate map, AnimWeightMaps meets the target the plan of record pins from task 1's split before building.
  - No measured map's AnimWeightMaps is slower than baseline beyond run-to-run noise; `kinematic-platform` takes a median of three.

## Path

- **Precedent code** (6e0c28578): `lightmap_layer/shaft_cull.rs::ShaftCull` (`new`, `tile_clear`), `shaft_cull/margin.rs::ShaftScene`, `chart_cull.rs::{chart_may_cover, ExactSkips}`, `chart_walk.rs` (lazy tile verdicts, `verify_clear_texel`), `skip_stats.rs`. Fit: research §Precedent API fit.
- **Shared home.** Beside the sampler in `texel_footprint/`, whose segments the hull proof is about, or crate re-exports from `lightmap_layer`. A move goes first, behavior-preserving, in its own commit.
- **Insertion.** In `FaceSetup::bake_light`, inside the weight-gated branch, before `sampler.visibility`. Size the verdicts to the task's `region` in tiles.
- **Shaft scene.** Build it once per stage, before the (face, light) tasks. A permitted item must never wait on another (`build_pipeline.md` §Progress reporting).
- **Seam.** Thread the skips through `bake_weight_maps_on`, which already takes the early-out choice. `bake_animated_light_weight_maps_with` stays always-on.
- **Fixtures.** `texel_footprint/test_fixtures.rs::Fixture::bake_weight_map` runs the precedent's shaft scenes through this stage. `split_face_scene` already holds W1 and W2: at its 5 cm texels, the mid-interior split falls inside the second tile column.
- **Rival.** Tiles anchored per chunk give smaller hulls at a split. But hulls then move with light counts, and a tile split by a chunk edge is tested twice.
- **First slice.** Task 1. Then the shaft cull on the open-floor and split-face fixtures, which tests the API fit.
- **File size.** `animated_light_weight_maps/tests.rs` is past 800 lines, so new fixtures go in a sibling module.
- **Docs at landing.** `build_pipeline.md` step 12 extends the exact skips to the weight walk.

## Open questions

- Shared code home — **delegated**.
- Wall-time target on the gate map — **delegated**: the plan of record pins it from task 1's split, before building.
