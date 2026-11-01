# watertight-tracer — research

Derivation behind the brief's Decisions. Source read at 343ff98b6; `bvh` 0.12.0 read from the cargo registry.

| Evidence | Origin |
|---|---|
| `drafts/lightmap-reach-cull/leak-diagnosis-findings.md` §Verdict, §Cause 2 in detail, §H3 residue, §Visible impact, §Fix sizing | Spike: rays within 1 µm of a shared edge miss both triangles on movement-feel; crease edges miss each other by 1–250 µm; all leaks on buried texels; option (a), an in-plane tolerance, against option (b), snapping |
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

The shaft cull and the clip-reach texel query walk the BVH with box queries, not rays. Padded boxes only widen their candidate sets. The shaft cull's `meets_spine` is an exact f64 recompute with Möller–Trumbore's shape, not a hit test; a wrong `true` costs only speed, so the grep gate exempts it.

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
- Not yet on `main`: `ready/mover-light-flow--foundation/` gates probes on a segment clear of static geometry and memoizes the result, a cached stage that reaches the tracer (C12).

## Cache independence

The cache Decision's argument, step by step:

1. bvh 0.12 stores each child's box in its parent node, an exact componentwise union (`utils::joint_aabb_of_shapes`). A one-shape join is the shape's box exactly.
2. The iterator tests a node's child boxes and descends only into those that pass (`bvh/iter.rs`). A leaf's box is tested as its parent's child box; a root leaf as the shape's box, since bvh 0.12 writes a root leaf's parent index as itself (C6). With a fixed bound it yields every leaf whose ancestor chain passes.
3. The f32 predicate is monotone in containment. Rounding is monotone, so a larger box's computed entry is no later and its exit no earlier. With the NaN rule, a degenerate axis constrains nothing in both boxes.
4. So a leaf whose box passes has ancestors that pass, and the traversal reaches it. A cached primitive tested only after its box passes is one of the traversal's own candidates. The cache changes order, never the candidate set or the per-triangle predicate.
5. The occlusion bound is fixed at the segment end, so steps 2–4 hold on the prune path too.

The cached box must be the box the parent stores, so the pad applies to both. Padding is uniform and rounding monotone, so pad(A ∪ B) = pad(A) ∪ pad(B): parents stay exact unions of padded children.

The shaft cull's `last_blocker` is not a tracer cache. It re-tests its triangle's box against the tile's f64 query bounds, which the outward-rounded BVH query contains, so a remembered blocker is one the query would reach, and a clear verdict still needs every candidate separated.

## Distance prune

`prune_limit` keeps boxes entered within 1 cm plus 0.01% of the bound. The padded box holds every point the hit test accepts: the triangle, its τ growth, and the test's rounding. So its exact entry is no later than any accepted hit's exact `t`, and a pruned box holds no accepted hit before the segment end. Today the cache can still report a hit there, since MT's `t` error near a sliver's end reaches metres (`sh_bake/trace_tests.rs`, rounding-stress doc). With the cache gated, it tests that box first and skips it, as the traversal does.

## Tolerance

The test accepts a crossing whose point in the triangle's plane lies within τ of the triangle: the triangle grown by an in-plane disk of radius τ, with rounded corners.

- **Not the edge-offset triangle.** Shifting each edge function by τ grows an acute vertex of angle α by τ/sin(α/2), unbounded on slivers. That breaks the pad (no box grown by τ holds it) and the silhouette bound.
- **Not a tolerance across the ray.** Woop's edge functions live in the plane perpendicular to the ray. A fixed tolerance there maps to up to τ/sin θ in the triangle's plane at crossing angle θ.
- **Crack width.** The largest distance from a point of a face's open edge to the nearest other face. For a point in the other face's plane it is the in-plane distance τ bounds; off that plane the 3D distance is never smaller than its projection's, so it errs toward calling a crack wide. The census reports it, and `lightmap-reach-cull`'s hole check calls a span a crack only when it proves this width at most τ.
- **What it closes.** A ray through a gap of width w between two edges passes within about w/2 of one of them, so τ closes gaps up to about 2τ; sizing τ to the full crack width leaves that slack, and the cull's hole check keeps its boundary at τ. Rays nearly parallel to a crease can cross both planes farther out, so the census bound is the honest one. T-junctions: the oversize-face cut's clip (`lightmap_bake/face_cut.rs` `clip_slab`, `crossing`) adds vertices on a cut face's boundary edges that the neighbour across them lacks.
- **Why not welding alone.** Crease edges come from different brushes' planes, so their vertices differ by 7 µm–0.25 mm (leak diagnosis §Cause 2 in detail); there is nothing bit-identical to weld. Closing them by geometry means snapping crease edges onto the BSP crease line, a geometry-wide pass that moves shipped geometry and every bake (§Fix sizing option (b)). A T-junction has no partner vertex at all. τ stays inside the tracer and leaves shipped geometry alone.
- **Cost.** Up to τ of extra shadow past each silhouette edge: at 0.5 mm, a fiftieth of the default 2.5 cm lightmap texel (`DEFAULT_TEXEL_DENSITY_METERS`).
- **Bounds.** Face extraction splits at `REGION_CLASSIFICATION_EPSILON` (1 mm), so τ stops there. A crack wider than τ is a hole, which the cull's hole check reports.

## Precision and pad

Bake vertices and ray origins and directions are f32. In f64, v − o is exact whenever the two magnitudes lie within 2²⁸ of each other, and rounds once at 2⁻⁵³ otherwise; the budget below covers that rounding. The shear and the two-term edge functions add about c·2⁻⁵³·‖v − o‖, for a small constant c.

- The pad is τ plus one f32 ulp of the extent E, E the largest coordinate magnitude of the geometry. τ holds the grown triangle. The test's rounding stacks on top of τ at the boundary, so the terms add. One ulp of E, at least 2⁻²⁴·E, covers the f64 rounding for any origin within about 2²⁹/c extents, so no ray origin needs bounding and the chunk light list's sun segments keep their 10 km start.
- The pad is applied with outward rounding: each grown coordinate steps one f32 outward (`next_down` on a min, `next_up` on a max) after the add, because an f32 add rounds to nearest and can land half a step inside the exact sum (C13).
- The rounding term is one ulp of E, not the next f32 outward from each coordinate: at a coordinate of 0, where authored walls and floors often sit, that step is subnormal. The outward step after the add covers only the add's own rounding.
- The design is a hybrid: f32 conservative box tests, which stay open to SIMD, and a scalar f64 hit test. pbrt-v4 and Embree keep the hit test in f32 partly for SIMD width.
- The f32 fallback rounds v − o at u = 2⁻²⁴, so the pad grows to τ + c·u·E′ (c from Woop et al.'s error analysis), rounded outward the same way, and ‖v − o‖ must stay within E′. E′ covers the geometry, every light origin and every probe position, and every segment starts inside it: lightmap sun segments already start at the texel (`lightmap_bake::area_sample_target`); chunk light list sun segments would move to start at the sample.
- Smaller, interlocking maps, the seamless-level-loading direction, shrink E′ and so the fallback's pad. They don't lift the in-extent-origin rule: a sun origin 10 km out sets E′ whatever the map's size.

## Precedent

| Tool | Hit test | Precision | Robustness | Source |
|---|---|---|---|---|
| VRAD (Source) | KD tree over brush-winding triangles, `FourRays` SIMD | f32 | None in the trace: occluded when `HitDistance < len`; a 1/32-unit nudge only in box clipping | [trace.cpp](https://raw.githubusercontent.com/ValveSoftware/source-sdk-2013/master/src/utils/vrad/trace.cpp) |
| Godot LightmapGI, GPU | Möller–Trumbore family, exact barycentric compare | f32 | `|n·d| < 1e-5` parallel reject; `bias` as a minimum distance and origin offset; front hits pulled toward the origin | [lm_compute.glsl](https://raw.githubusercontent.com/godotengine/godot/master/modules/lightmapper_rd/lm_compute.glsl) |
| Godot LightmapGI, Embree | Embree defaults | f32 | No scene flags; robust flag unset | [lightmap_raycaster_embree.cpp](https://raw.githubusercontent.com/godotengine/godot/master/modules/raycast/lightmap_raycaster_embree.cpp) |
| Godot `precision=double` | Engine-wide f64 positions | f64 | Addresses positional jitter far from the origin, not ray robustness | [Large world coordinates](https://docs.godotengine.org/en/stable/tutorials/physics/large_world_coordinates.html) |
| ericw-tools (Quake) | Embree | f32 vertex buffers | `RTC_SCENE_FLAG_NONE`, `tnear = 0`; filter callbacks for gameplay, none for leaks | [trace_embree.cc](https://raw.githubusercontent.com/ericwa/ericw-tools/master/light/trace_embree.cc) |
| Ize 2013, "Robust BVH Ray Traversal" | Traversal only | f32 | Conservative slab: scale `tFar` by 1 + 2 ulp; the 2015 correction confirms | [robustBVH-v1.pdf](https://www.sci.utah.edu/~thiago/papers/robustBVH-v1.pdf) |
| Xash3D / VHLT | Unknown | Unknown | No primary source retrieved | — |

No peer baker is watertight; each relies on bias and hand fixes, and both Embree users leave its robust flag off. The robust pieces this brief takes are published (Woop 2013, Ize 2013, pbrt-v4) but unused by these bakers. A scalar f64 hit test is cheap insurance they forgo by accepting leaks.

## Margins

`margin.rs` sums MT's rounding: γ₅√2 per cross-then-dot numerator, and the shape term ρ, because MT's products scale with ‖e1‖‖e2‖ (`done/lightmap-exact-skips/research.md`). `|det| ≥ 1e-8` is an accepted-hit condition, not a margin term; dropping it adds none.

- The watertight test has no e1, e2 or s. In f64 its rounding sits in the shear and the two-term edge-function products at about 2⁻⁵³, so the hit-test terms shrink by about 2²⁹; the sampler-endpoint terms are f32 segment ends, stay, and likely dominate.
- τ adds to m_A, summed with the hit-test rounding terms as in the pad: an accepted point lies up to τ off the triangle, in its plane, and the test's rounding can carry it further. m_N bounds height off the plane and gains nothing from τ.
- Expect ρ to change form and the 2° floor to stay, since in-plane reach still grows as 1/sin θ.
- Both searches draw their gaps as multiples of MT's f32 rounding scale; the draws rescale to the new margins (C10).
- Under the f32 fallback the hit-test terms return at u, in v − o and the shear. Not derived here.

## Unverified

- The widest crease crack on each gate map, and the T-junctions the oversize-face cut adds; the census measures both (C11).
- The cost of the f64 test against f32 on these maps.

## Pins

| id | Scenario | Ordering | Expected outcome |
|---|---|---|---|
| C1 | Cached primitive whose box fails the predicate | The cache is consulted first; the box test fails | Its triangles are not tested; the traversal decides; the answer equals an empty cache's |
| C2 | Cached primitive the traversal also reaches | The cache misses; the traversal reaches the same leaf | The answer equals an empty cache's |
| C3 | Pre-change cache, new binary warm | Entries were written under keys without the epoch | Every stage above misses; the PRL equals the cold bake |
| C4 | `lightmap-reach-cull` P4 | Skipped texels write no occluder, so the next traced texel holds a different cache | Its record equals the empty-cache and brute-force answers |
| C5 | Bake trees at both build sites | `build_bvh` flattens id 19 before it returns. The pipeline builds the pre-cut tree, which SH, delta and direct SH, chunk light list and the emitter tests trace. It rebuilds tree, primitives and id 19 together only when a face is cut. Fixture tests call `build_bvh` directly | Every tree a bake or test traces is padded and passes the containment assertion after the pad, whether or not a rebuild runs. Id 19 comes from the exact boxes of the tree that ships |
| C6 | One-primitive and empty trees | The iterator tests a root leaf as the shape's own box, not a parent's child box. bvh 0.12 writes the root leaf's parent index as 0, which is itself. An empty tree yields nothing | The gated cache tests a root leaf's padded primitive box and answers as an empty cache does. An empty tree pads nothing, and its census is zero |
| C7 | Cache filled mid-branch | A branch binary built after `TRACER_EPOCH` lands, but before a later slice changes answers (box test, pad, cache gate, margins), fills the dev cache. The landing binary then builds warm over it | Every tracer stage misses, because the landing epoch value differs from every value a commit with different answers carried |
| C8 | Precision and τ set before the rest of the cost lands | Slice 1 times f64 against baseline with MT-sized margins, the stock box test, an unpadded tree and an ungated cache, and the census sets τ. Later slices add cost, then re-derive the margins | The 10% gate is read again on the landing build. If precision or τ changes after the margins are derived, they are re-derived, and both searches and A11 and A12 rerun at what ships |
| C9 | Counters before the hit test | The box-test, leaf-visit, triangle-test and `det == 0` counters do not exist on the baseline | They land in a behavior-preserving commit ahead of the hit test. "Before" is that commit, and its PRLs equal baseline |
| C10 | Margin searches after the precision and τ change | Both searches draw their gaps as multiples of MT's f32 rounding scale, and their reach guards demand over 100 phantom hits, over 10,000 certified cases and a worst ratio above 0.01 | The draws rescale to the re-derived margins at the shipped precision and τ, and the reach guards still hold. A guard that cannot hold stops for the owner instead of being loosened |
| C11 | Census geometry | The oversize-face cut adds vertices on a cut face's boundary edges. SH and the chunk light list trace pre-cut geometry; the lightmap, weight maps and reflection probes trace post-cut geometry | The census, counts and gap widths, is reported for each geometry, so T-junctions the cut adds show up and τ covers both |
| C12 | `mover-light-flow--foundation` lands in either order | Its probe gating traces static geometry and memoizes the result | Whichever plan lands second folds that stage into `TRACER_EPOCH` and the epoch row's set |
| C13 | Pad on a coordinate whose f32 sum with the pad rounds inward | The pad is added in f32, rounding to nearest, then the result steps one f32 outward | The grown coordinate lies at or beyond the exact sum on every face, so a hit accepted at τ, with the test's own rounding, lies inside its box |
