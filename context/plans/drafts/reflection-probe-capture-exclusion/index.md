# reflection-probe-capture-exclusion

Brief · resumable · reads: `context/lib/resource_management.md` §5 · `rendering_pipeline.md` §7.3a · `build_pipeline.md` §Source-format neutrality, §Custom FGD, §Compiler pipeline, §PRL section IDs, §Build Cache · `plans/done/reflection-probes/` · `plans/in-progress/fluid-ssr-reflections/` · read at 9add6bb (main) and af8bfef (`origin/claude/fluid-ssr-reflections-0b3212`; facts marked **[SSR]** are not on main yet) · builds after `fluid-ssr-reflections` lands on main

## Problem
The owner requested this capability as a follow-up to `fluid-ssr-reflections`. A probe captures every static brush from one point 2 mm above its water (`resource_management.md` §5.1). Small decorative geometry near the water, such as pillars, posts, trim, crates and steps, is either too small to resolve in a 128² tile or shows the wrong parallax from that single point. Once fluid SSR lands, SSR shows that geometry better: at screen resolution, with true parallax. Exclusion changes only pixels where SSR is not fully confident, trading the object's misplaced copy for its absence wherever SSR misses (`research.md` §Composite). When this is done, each probe's bake leaves out static brushes that are small relative to that probe's water, so they reflect only through SSR. Room-scale walls, floors and ceilings stay in the probe, and so does anything whose underside the water reflects, which the camera cannot see and SSR cannot show. The size threshold scales with the water. A mapper can force a brush in or out with a `func_group` brush entity or a `switch`.

## Decisions
- **Bake time, primary rays only.** The filter skips excluded brushes' faces in capture primary rays only; the colour tile, distance tile and light share follow. Shadow rays (`SceneTracer::segment_clear`) and `fit_box` see the whole world, so captured walls keep excluded brushes' shadows and placement is unchanged.
- **The runtime never knows.** No exclusion data reaches id 54, the renderer or render-cpu, and the id 54 format and version are unchanged. The readings in the `reflection-probes` and `fluid-ssr-reflections` IP checks stand: Tencent claim 15 (no completion) and Microsoft US10586375B2 (no reflected-toward-camera determination) (`research.md` §IP).
- **Water width W = √(`PlacedProbe::bound_area`), per probe.** Each probe has its own excluded set. One brush can be kept by one probe and excluded by another; the seams that follow are the ones `reflection-probes` already accepts.
- **Threshold t = p(W)·W.** p = 0.85 for W ≤ 10 m. Above 10 m, p = 0.85·(W/10)^−a, with a = ln(0.85/0.33)/ln 5, from the owner's anchors (85% up to 10 m, 33% at 50 m). Initial values; the owner tunes them visually after the build. Rejected: a curve linear in p, whose t peaks near 38 m and then shrinks.
- **Object size is the brush's longest extent in the reflector's plane**, written against the reflector normal. Water: longest horizontal side of the hull bounds. Future vertical reflector: longest of vertical and along-wall, ignoring depth. Rival rejected: vertical extent only for walls, which would drop floors and ceilings while keeping tall thin poles.
- **The rule.** A brush is excluded from a probe when its size < t, its thinness ≥ f and it has no underside over that probe's water. `always` and `never` override all three.
- **Thin-brush floor.** A brush whose extent along the reflector normal is below f(W) stays in the probe, since exclusion happens only where SSR shows a brush better. For water, thinness is height: horizontal bars and slabs seen edge-on stay, and thin vertical poles go to SSR, which samples them along their length (`research.md` §Thin-brush floor). Beyond the underside rule, the floor covers thin decor outside the water's horizontal projection, such as shore rails and bars. Rival rejected: smallest in-plane extent, which kept poles in the probe and sent the bars the march can step over to SSR.
  - **[SSR]** f(W) = W · 2·tan(θv/2) / D_min: the extent one march stride spans at view distance W. D_min is the coarsest stride among tiers that run SSR, so f is the same at every tier. θv is the vertical FOV at the default HFOV 100° and 16:9.
  - f's ratio is a compiler constant, not a live read of the placeholder presets, so a retune never re-keys every fluid map's capture. A test fails when the presets or the pinned default FOV drift from it.
- **Undersides stay in the probe.** A brush has an underside over a probe's water when a surviving (post-CSG) face of it points down, n·N ≤ −sin(1°) as in placement's 1° up-facing cone, and overlaps a bound water face of the probe in projection with positive area while lying above that face's plane. Edge-only contact does not count. The camera never sees those faces, so SSR can never return them (`rendering_pipeline.md` §7.3a Limits; `fluid-ssr-reflections` Non-goals), and an authored `_capture_offset` beneath a deck keeps working.
- **Rivals weighed.** Excluding nothing and letting SSR's confidence cover the object keeps the wrong-parallax copies and capture shadows wherever c < 1. Authored opt-out only, with every brush kept by default, was rejected because the owner asked for an automatic default; the override covers what the rule gets wrong.
- **Contact holes and seals: one exit rule.** CSG removes kept faces where an excluded brush touched them, and a brush that seals the room opens onto the exterior or another room (`research.md` §Contact holes). A ray that leaves a skipped excluded brush's hull classifies the exit point in order: inside another excluded brush, repeat from its exit; inside kept solid on a covering kept side with a surviving coplanar face, fill; inside kept solid otherwise, leak guard; in empty space, trace on under the same mask (`research.md` §Orderings O3). A ray that has skipped an excluded brush and then misses, or ends on a hit outside the probe's projection box, takes the leak guard: it records the first excluded surface it skipped.
  - No capture ray that crosses an excluded brush misses or ends outside the probe's projection box. A brush that seals the room, such as a wall stub between openings or an outer wall set to `never`, shows itself in those directions, so `never` cannot open a room.
  - **Fill.** Material and UV come from the covering kept side at the exit point. Lightmap, SH and sdf direct, with its shadow rays, come from the nearest point on the coplanar kept face, so all lighting reads one surviving surface point.
  - Rivals rejected: accepting the leak, which shows wherever SSR misses and always at `low`; the leak guard everywhere, which shows pillar stubs in the ceiling-patch directions; keep-on-contact, which keeps ceiling pillars, the commonest decor. If the first slice shows the fill infeasible, the leak guard applies everywhere and the owner is told.
- **Accepted limit: `low` shows no excluded decor.** **[SSR]** `low` is the probe term alone (`FluidReflectionQuality::runs_screen_space`), so excluded brushes never reflect there. This is a regression against today's probe-only picture, which shows that decor. The owner accepted it (2026-10-20) over baking a second, unfiltered capture for Low, which doubles probe memory and capture time.
- **Override: `_reflection_capture` = `auto` | `always` | `never`** on `func_group` and `switch`, for every brush of that entity: `always` keeps, `never` excludes, `auto` follows the rule. Worldspawn brushes are always `auto`. A bad value warns once per entity, naming it, and acts as `auto`.
- **Authoring through a `func_group` brush entity, not a TrenchBroom group.** TrenchBroom drops every group key but name, id, link id and transform on load and save (`research.md` §TrenchBroom). The FGD gains a `func_group` brush entity carrying `_reflection_capture`, and `switch` gains the key. `docs/` tells mappers to override with Create Brush Entity → `func_group`, and names no `cargo run -p xtask` command.
- **Layers.** Shared compiler stages see only a canonical per-brush value (`build_pipeline.md` §Source-format neutrality). The raw key is read at the entity-parse layer, beside `_capture_offset` (`parse/reflection_volumes.rs`). Classification, filtering and fill live in the reflection-probe stage.
- **Cache.** Every edit to an override value, a curve constant or the floor constant misses the capture stage. The key hashes the canonical override table, the curve and floor constants, each probe's excluded set, and the hull planes of excluded brushes plus their contact candidates' sides.
- **IP pre-ship line.** Add to the owner's pre-ship check: "size-based capture exclusion: compare with UE Visible in Reflection Captures / Cull Distance Volumes, Unity Reflection Probe Static / culling mask / near clip; read Perfect World WO2022227553A1." Engineering triage, not legal advice.
- **Non-goals.**
  - Runtime or view-dependent exclusion: probe content tied to the camera is Tencent claims 4–14 and breaks `resource_management.md` §9.
  - Probes on non-fluid surfaces. Size and thinness are written against the reflector normal; the underside rule and the floor's derivation assume water.
  - Excluding props. `prop_mesh` never enters the BVH, so this is already true.
  - A detail brush class, and per-brush keys on worldspawn: worldspawn brushes carry no per-brush keys in the map format, and a mapper wraps a brush in a `func_group` to override it.

## Acceptance

### Automated
Threshold and size:
- [ ] t(W) at 2, 10, 50 and 100 m is within 1e-3 of 1.7, 8.5, 16.5 and 21.956. It is continuous at 10 m and strictly increasing over 0.5–500 m.
- [ ] W from bound area: an L-shaped pool fixture and a 10 × 2.4 m canal fixture each give √area.
- [ ] Predicate, both sides of t: a brush with in-plane extent t − ε is excluded and t + ε is kept, at W = 5 m and at W = 50 m.
- [ ] Floor, both sides, at W = 5 m and 50 m: a brush whose extent along the normal is just below f is kept, and one just above f (and below t) is excluded. A post thinner than f in plane but taller than f is excluded. f follows the SSR tiers' coarsest stride. f < t over 0.5–500 m.
- [ ] Given a vertical reflector normal, size comes from the vertical and along-wall extents, and thinness from depth along the normal.
- [ ] The floor-ratio test fails when render-cpu's coarsest SSR stride or the pinned default FOV changes, and passes at the shipped presets.

Undersides, both sides of the rule (fixtures below t and above f):
- [ ] On a 50 m lake (t = 16.5 m, f ≈ 0.50 m), a dock 10 m long and 1 m thick over the water, and a beam 12 m long and 0.6 m deep over the water, are kept. Moved off the water's horizontal projection, the same two brushes are excluded.
- [ ] A pillar standing in the water and a post beside it, with no downward face over the water above the plane, are excluded.
- [ ] A brush below t and above f, whose downward face lies above the plane but outside the bound water's horizontal projection, is excluded.
- [ ] A deck below t and above f, under an authored `reflection_volume` whose `_capture_offset` sits beneath it, stays in that probe's capture.
- [ ] A crate sunk 5 cm into a dock over the water, below t and above f, is excluded: CSG removes its bottom face, and its hull's bottom side does not count as an underside.
- [ ] In one map with two pools, a footbridge 0.6 m deep over pool A only, below both probes' t, is kept by A's probe and excluded by B's.
- [ ] On an authored box binding water at two heights, a deck set between the two planes and over the lower water is kept. The same deck moved over the upper water, below that water's plane, is excluded.
- [ ] A wedge whose only face over the water slopes down at 45° is kept.

Capture, on a pillar in a pool touching the ceiling:
- [ ] Directions through the pillar's body read the wall behind it, in colour and distance.
- [ ] Directions through the filled ceiling patch read the ceiling's material, at the distance of the ceiling plane, lit from the kept ceiling face's lightmap at the nearest point on that face. The ceiling's texture differs from the pillar's and the wall's, and its lightmap texels vary across the face.
- [ ] With an sdf light below the ceiling, the filled ceiling patch's sdf direct is within 5% of the adjacent kept ceiling's.
- [ ] The wall behind keeps the pillar's shadow from an sdf light in the colour tile, because the capture's shadow ray still sees the pillar. No lightmap shadow of the pillar exists for the wall to read instead.
- [ ] Texels no supersample of the unfiltered build sent into the pillar are bit-identical in the pre-encode capture rows, colour and distance. Every BC6H block holding none of the pillar's texels is byte-identical.

Capture, other cases:
- [ ] Leak-guard fixture: a small brush embedded in a kept brush, with no coplanar kept face, is in the probe's excluded set and reads its own surface in the directions where it is the first hit.
- [ ] Closed-room fixture: every brush placed against walls, under the ceiling and embedded is in the probe's excluded set. No capture ray misses, and none lands outside the room.
- [ ] Chained closed room (research.md O3): three excluded pillars stand in a row between the capture point and a wall, and the farthest touches the wall and the ceiling. Two touching excluded crates are stacked against the wall, and the upper one touches the ceiling. No ray through any of them misses or lands outside the room. Rays reaching the farthest pillar's wall and ceiling patches, and the upper crate's ceiling patch, read the fill.
- [ ] Seal fixture (research.md O3): a 2 m wall stub between two openings and an outer wall set to `never`, built once with the exterior behind them and once with a second room behind. Rays through them read their own surfaces; none misses or lands outside the probe's projection box.
- [ ] The placed probe set (boxes, points, bindings) is identical with the filter on and off, on a fixture whose derived box changes when the excluded brush is deleted from the map.
- [ ] In one map, one brush is excluded from a small probe's capture and kept by a large probe's.
- [ ] A map whose brushes are all ≥ every probe's t emits id 54 byte-identical to the same build with every probe's excluded set forced empty (cache aside). The capture tests that predate this change pass unchanged.
- [ ] With the oversize cut forced by a small pool edge (research.md O2), every post-cut face maps to its parent's brush and a face the cut emptied maps to none. When a cut kept face precedes the pillar's faces in output order, the mask skips exactly the pillar's post-cut faces.

Override:
- [ ] `never` excludes a brush ≥ t, `always` keeps a brush < t, and `auto` follows the rule, on a `func_group` brush entity and on a `switch`.
- [ ] `never` excludes a dock the underside rule keeps and a slab thinner than f. `always` keeps a post the rule excludes.
- [ ] A `func_group` set to `never` whose first brush is degenerate excludes its other brushes. The next entity's brushes keep their own values (research.md O4).
- [ ] An invalid value on a three-brush `func_group`, in a map with two probes, warns exactly once, naming the entity, on a cold build, on a warm build and in a map with no water, and acts as `auto` (research.md O5).
- [ ] `_reflection_capture` `never` on worldspawn changes no probe's excluded set.

Cache and determinism:
- [ ] Changing a `_reflection_capture` value or a curve constant misses the capture stage. An edit that moves no brush also misses. An unchanged rebuild hits.
- [ ] Two builds differ only in an excluded pillar's hull where CSG hides it: flush with the ceiling in one, sunk 0.5 m into it in the other. They key the capture stage differently, or emit byte-identical id 54 cold.
- [ ] `--no-cache` at `--jobs 1` and at `--jobs 4` gives byte-identical id 54.

Gates:
- [ ] Source gate (grep): `_reflection_capture` and the canonical override type's name appear in no file under the renderer, render-cpu, level-format or level-loader crates, and the id 54 section module in level-format is unchanged from main. In the compiler, `_reflection_capture` appears only in the entity-parse layer, on its side of the canonical map data. Outside `#[cfg(test)]`, the compiler names no `FluidReflectionQuality`.
- [ ] The FGD's `func_group` and `switch` entries offer `auto`, `always` and `never`. `docs/` documents the key through a `func_group` brush entity and contains no `cargo run -p xtask`.
- [ ] The `fluid-ssr-reflections` ip-check §9 pre-ship list, or its successor, carries the exclusion line.

### Manual
- [ ] Visual check on `reflection-probes-test.map`, e24 and a new lake fixture at least 50 m across, with posts, a dock, a boathouse and shore walls:
  - small geometry reflects via SSR with correct parallax, and walls stay in the probe;
  - judge losses at the screen edge, SSR on thin decor, and tall excluded objects past the march's reach, at each SSR tier;
  - check that filled patches read as the surface around them, and judge whether the floor ever fires, and on what;
  - check that dock and footbridge undersides still reflect, including under an authored `_capture_offset`;
  - look down at high `surfaceReflectivity` water, where reflected rays leave the top of the screen, and judge the loss of excluded decor there;
  - compare `low` against today's probe-only picture on the same views;
  - retune the anchors and record the final values.
- [ ] Cold and warm probe-stage time on e24, before and after.

## Path
- **Filter seam.** `SceneTracer::closest_hit` (hit.rs) loops over `bvh.traverse_iterator`. Each primitive is one face (`face_by_offset`), so a per-probe face mask skips a primitive whole. For the exit rule it also reports the nearest skipped hit; an exit into empty space re-traces from there. Pass the mask per call through `capture_section` → `capture_row` → `CaptureScene::radiance`, never as tracer state. `fit_box` and `segment_clear` call the tracer without it.
- **Face → brush.** `extract_geometry` drops `Face::brush_index`. Carry a compiler-side table in output face order (the `ordered_faces` walk). The oversize cut renumbers faces (`plan_cut_charts` `face_remap`, `rebuild_face_identity`), so remap the table there (`research.md` §Orderings O2; `build_pipeline.md` §Compiler pipeline step 10). Keep it out of level-format `FaceMeta`.
- **Sizes.** Use `BrushVolume::aabb`, the unclipped hull in engine metres.
- **Fill seams.** `research.md` §Contact holes maps each need to its source. Split `CaptureScene::radiance` into trace and shade, so a synthesized `SurfacePoint` shades like a traced one, and precompute contact candidates per excluded brush: kept brushes whose bounds touch its own.
- **Override table.** `MapAssembly` will not carry it: `switch` brushes have no assembly, and a `func_group` brush entity without `_tb_id` matches none. Read the key in the `func_group` and `switch` branches of parse.rs with `collect_entity_properties`, warn there (`research.md` §Orderings O5), and translate it at the retained-hull seam `brush_assembly_for_retained_hulls` uses (O4). A brush takes its own entity's value. `is_editor_group_classname` already matches groups, layers and authored `func_group` brush entities, so no parser branch is new.
- **Floor inputs.** render-cpu is a dev-dependency of level-compiler, so only the drift test reads `FluidReflectionQuality::ALL` and `preset()`; the bake reads the compiler constant. D_min is 135 (ultra) at af8bfef (`research.md` §Thin-brush floor); re-derive the constant if the presets change before SSR lands. The default FOV lives in the binary (`camera::HFOV`): pin a compiler copy with a test.
- **Underside test.** The face → brush table gives each brush's surviving faces. Whether a crate sitting flush on a deck keeps a bottom face after CSG is unverified; if it does, the crate is kept, which is the conservative outcome.
- **Cache key.** Hash the override table and constants directly, as `stage_key` hashes shaping constants today; excluded sets in post-cut indices. `STAGE_VERSION` → 10.
- **Docs.** `docs/level_design.md` §Fluid reflections; `sdk/TrenchBroom/postretro.fgd` `switch`, plus a new `func_group` entry.
- **Large files.** parse.rs and pipeline.rs are past ~800 lines. Put new logic in its own module and keep their edits to call sites.
- **Shape.** Chosen: a per-call face mask over the one shared BVH, with exit tests against hull planes. Rival: one BVH per distinct excluded set, which costs a build and its memory per probe and still leaks.
- **First slice.** A pillar touching the ceiling, with exclusion hard-wired: the filter, the contact-hole leak, then the fill. It tests the riskiest assumption, that a fill can be found and shaded from existing data, before any key or curve work.

## Boundary inventory

| Name | Rust | Wire / serde | JS / TS | Luau | FGD KVP |
|---|---|---|---|---|---|
| Capture override | canonical per-brush enum on `MapData` (name the executor's) | n/a; never in PRL | n/a | n/a | `_reflection_capture`: `auto` / `always` / `never`, default `auto`, on `func_group` (brush entity) and `switch` |

## Open questions
None.
