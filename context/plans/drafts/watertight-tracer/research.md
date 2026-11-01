# watertight-tracer — research

Derivation behind the brief's Decisions. Source read at 343ff98b6; `bvh` 0.12.0 read from the cargo registry.

| Evidence | Origin |
|---|---|
| `drafts/lightmap-reach-cull/leak-diagnosis-findings.md` §H3 residue, §Visible impact, §Fix sizing | Spike: rays within 1 µm of a shared edge miss both triangles on movement-feel, all on buried texels; shared-edge watertightness judged "none proportionate" |
| `drafts/lightmap-stage-sizing/findings.md` §1 | The remembered occluder answers 65.8% of hallway-mini's shadow rays, mostly on dark texels |
| `drafts/lightmap-reach-cull/research.md` §Occluder caches | The downstream cull, and why it needs the cache to be answer-neutral |
| `lightmap-reach-cull` brief review, premise P1–P2 and rows R7–R9, R15, R16, R25 | The prune path, the stock-ray reference, stale warm entries, pins that encode today's leak, SH cost |
| `done/lightmap-exact-skips/research.md` §Margin derivation | The shaft-cull margins, sized to Möller–Trumbore |
| `done/bake-ray-speed-contract.md` | The fused node test, the occluder cache, and the id-19 byte-identity invariant |

## Tracer inventory

Five Möller–Trumbore copies, each with the `|det| < 1e-8` rejection.

| Hit test | Reached by |
|---|---|
| `lightmap_bake::ray_triangle_hit`, two-sided | `segment_clear_remembering`: lightmap chart walk (`last_occluder`, `clip_occluder`), shadowmask cut-face estimate, the monolithic oracle, weight-map face bake, texel steps, entity-shadow select, reflection-probe shadow rays. `texel_footprint::clip_reach::front_faces_clear`: lead-in and embedded-emitter tests, with a front-face filter ahead of the hit test. |
| `sh_bake::trace::ray_triangle_hit`, two-sided, returns the normal | SH `closest_hit` (probe gather, patch bounce) and SH `segment_clear` (probe direct, bounce visibility, surface-radiance layers, indirect delta) |
| `chunk_light_list_bake::ray_triangle_hit` | chunk-light `segment_clear` |
| `billboard_direct_scatter_bake::ray_triangle_distance` | scatter `segment_clear` |
| `reflection_probes::hit::front_face_hit`, front filter then MT, returns barycentrics | probe primary rays |

The shaft cull and the clip-reach texel query walk the BVH with box queries, not rays. Padded boxes only widen their candidate sets. The shaft cull's `meets_spine` is an exact f64 recompute, not a hit test.

Out of scope: runtime ray tests in `sim` (hitscan, nav clearance). They read collision, not the bake BVH.

## Stages that reach the tracer

| Cache entry | Version constant | Reaches the tracer through |
|---|---|---|
| `lightmap_layer` | `LAYER_FORMAT_VERSION` | chart walk, clip, lead-in, embedded-emitter test |
| `lightmap_section` | `LIGHTMAP_SECTION_VERSION` | the stored texel-step report |
| `shadowmask_atlas` | `SHADOWMASK_ATLAS_STAGE_VERSION` | cut-face estimate |
| `sh_patch_layer` | `SH_PATCH_LAYER_STAGE_VERSION` | soft visibility |
| `sh_base_volume` | `SH_BASE_VOLUME_STAGE_VERSION` | gather and bounce closest hits |
| `indirect_delta_sh_subblock` | `INDIRECT_DELTA_SH_STAGE_VERSION` | probe indirect |
| `direct_sh_volume` | `DIRECT_SH_STAGE_VERSION` | probe direct |
| chunk light list | `CHUNK_LIGHT_LIST_STAGE_VERSION` | sample visibility |
| `billboard_direct_scatter`, `animated_billboard_direct_scatter` | their two stage versions | scatter visibility |
| `animated_lm_weight_maps` | `STAGE_VERSION` | face bake, stored texel-step report |
| `reflection_probes` | `STAGE_VERSION` | primary and shadow rays |

- No tracer: `cell_visibility`, `cell_residency_set`, `sdf_atlas`, `navmesh`.
- Uncached but tracing: entity-shadow select (id 40), the direct and animated-direct SH deltas (ids 41, 45), and texel steps. They recompute every build, so their bytes move with no key to fold.
- Some keys would re-key transitively; the section memo folds layer fingerprints, for one. Folding the epoch into each directly makes the dependency explicit. The executor confirms each fold site.

## Cache independence

The cache Decision's argument, step by step:

1. bvh 0.12 stores each child's box in its parent node, an exact componentwise union (`utils::joint_aabb_of_shapes`). A one-shape join is the shape's box exactly.
2. The iterator tests a node's child boxes and descends only into those that pass (`bvh/iter.rs`). A leaf's box is tested as its parent's child box; a root leaf as the shape's box. With a fixed bound it yields every leaf whose ancestor chain passes.
3. The f32 predicate is monotone in containment. Rounding is monotone, so a larger box's computed entry is no later and its exit no earlier. With the NaN rule, a degenerate axis constrains nothing in both boxes.
4. So a leaf whose box passes has ancestors that pass, and the traversal reaches it. A cached primitive tested only after its box passes is one of the traversal's own candidates. The cache changes order, never the candidate set or the per-triangle predicate.
5. The occlusion bound is fixed at the segment end, so steps 2–4 hold on the prune path too.

The cached box must be the box the parent stores, so any padding applies to both.

## Distance prune

`prune_limit` keeps boxes entered within 1 cm plus 0.01% of the bound. Exact box entry ≤ exact `t` because the box is the min/max of the vertices the hit test reads, and padding only lowers entry. A pruned box therefore holds no exact hit before the segment end. Today the cache can still report a hit there, since MT's `t` error near a sliver's end reaches metres (`sh_bake/trace_tests.rs`, rounding-stress doc). With the cache gated, it tests that box first and skips it, as the traversal does.

## Precision and pad

Bake vertices and ray origins and directions are f32. In f64, v − o is exact whenever the two magnitudes lie within 2²⁸ of each other, and rounds once at 2⁻⁵³ otherwise. The shear and the two-term edge functions add about c·2⁻⁵³·‖v − o‖, for a small constant c.

- One f32 ulp of the extent E (the largest coordinate magnitude of the geometry), at least 2⁻²⁴·E, covers that for any origin within about 2²⁹/c extents. No ray origin needs bounding, so the chunk light list's sun segments keep their 10 km start.
- The step is one ulp of E, not the next f32 outward from each coordinate: at a coordinate of 0, where authored walls and floors often sit, that step is subnormal.
- Counterpoint: pbrt-v4 and Embree keep the test in f32 partly for SIMD width; this tracer is scalar.
- The f32 fallback rounds v − o at u = 2⁻²⁴, so the pad grows to about c·u·E′ (c from Woop et al.'s error analysis) and ‖v − o‖ must stay within E′. E′ covers the geometry, every light origin and every probe position, and every segment starts inside it: lightmap sun segments already start at the texel (`lightmap_bake::area_sample_target`); chunk light list sun segments would move to start at the sample.

## Margins

`margin.rs` sums MT's rounding: γ₅√2 per cross-then-dot numerator, the shape term ρ because MT's products scale with ‖e1‖‖e2‖, and the accepted-hit conditions (`|det| ≥ 1e-8`, u, v, t ranges) from `done/lightmap-exact-skips/research.md`. The watertight test has no e1, e2 or s. In f64 its rounding sits in the shear and the two-term edge-function products at about 2⁻⁵³, so the hit-test terms shrink by about 2²⁹; the sampler-endpoint terms are f32 segment ends, stay, and likely dominate. Expect ρ to change form and the 2° floor to stay, since in-plane reach still grows as 1/sin θ. Under the f32 fallback the hit-test terms return at u, in v − o and the shear. Not derived here.

## Unverified

- How many edges adjacent triangles share with bit-identical vertices. A T-junction or crease crack leaves an edge with no shared vertex, which the watertight test does not close; the shared-edge census measures it.
- The cost of the f64 test against f32 on these maps.

## Pins

| id | Scenario | Ordering | Expected outcome |
|---|---|---|---|
| C1 | Cached primitive whose box fails the predicate | The cache is consulted first; the box test fails | Its triangles are not tested; the traversal decides; the answer equals an empty cache's |
| C2 | Cached primitive the traversal also reaches | The cache misses; the traversal reaches the same leaf | The answer equals an empty cache's |
| C3 | Pre-change cache, new binary warm | Entries were written under keys without the epoch | Every stage above misses; the PRL equals the cold bake |
| C4 | `lightmap-reach-cull` P4 | Skipped texels write no occluder, so the next traced texel holds a different cache | Its record equals the empty-cache and brute-force answers |
