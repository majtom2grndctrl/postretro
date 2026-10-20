# reflection-probe-capture-exclusion

Brief · resumable · reads: `context/lib/resource_management.md` §5 · `rendering_pipeline.md` §7.3a · `build_pipeline.md` §Source-format neutrality, §Custom FGD, §Compiler pipeline, §PRL section IDs, §Build Cache · `plans/done/reflection-probes/` · `plans/in-progress/fluid-ssr-reflections/` · read at 9add6bb (main) and af8bfef (`origin/claude/fluid-ssr-reflections-0b3212`; facts marked **[SSR]** are not on main yet) · builds after `fluid-ssr-reflections` lands on main

## Problem
The owner requested this capability as a follow-up to `fluid-ssr-reflections`. A probe captures every static brush from one point 2 mm above its water (`resource_management.md` §5.1). Small decorative geometry near the water, such as pillars, posts, trim, crates and steps, is either too small to resolve in a 128² tile or shows the wrong parallax from that single point. Once fluid SSR lands, SSR shows that geometry better: at screen resolution, with true parallax. The composite is `F·[c·SSR + (1−c)·s·Probe]`, so where SSR is confident (c = 1) it already replaces the probe's misplaced copy, and exclusion changes only pixels with c < 1. There it makes a deliberate trade: a small object's wrong-parallax copy, its capture shadow on the water and the double image in SSR's screen-edge fade band go away, and the object is absent wherever SSR misses. When this is done, each probe's bake leaves out static brushes that are small relative to that probe's water, so they reflect only through SSR. Room-scale walls, floors and ceilings stay in the probe, and so does anything whose underside the water reflects, which the camera cannot see and SSR cannot show. The size threshold scales with the water. A mapper can force a brush in or out with a `func_group` brush entity or a `switch`. The runtime is unchanged.

## Decisions
- **Bake time, primary rays only.** The filter skips excluded brushes' faces in capture primary rays only; the colour tile, distance tile and light share follow. Shadow rays (`SceneTracer::segment_clear`) and `fit_box` see the whole world, so captured walls keep excluded brushes' shadows and placement is unchanged. The filter is a per-call option on `SceneTracer`, never global.
- **The runtime never knows.** No exclusion data reaches id 54, the renderer or render-cpu, and the id 54 format and version are unchanged. The exclusion changes only what the offline capture holds. The readings in both IP checks stand: Tencent claim 15 (no completion) and Microsoft US10586375B2 (no per-pixel technique choice) (`research.md` §IP).
- **Water width W = √(`PlacedProbe::bound_area`), per probe.** Each probe has its own excluded set. One brush can be kept by one probe and excluded by another; the seams that follow are the ones `reflection-probes` already accepts.
- **Threshold t = p(W)·W.** p = 0.85 for W ≤ 10 m. Above 10 m, p = 0.85·(W/10)^−a, with a = ln(0.85/0.33)/ln 5, from the owner's anchors (85% up to 10 m, 33% at 50 m). These are initial values; the owner tunes them visually after the build. A curve linear in p was rejected because its t peaks near 38 m and then shrinks.
- **Object size is the brush's longest extent in the reflector's plane** (where W is measured), written against the reflector normal. Water: longest horizontal side of the hull bounds. Future vertical reflector: longest of vertical and along-wall, ignoring depth. Rival rejected: vertical extent only for walls, which would drop floors and ceilings, the room-scale structure the rule keeps, while keeping tall thin poles.
- **Thin-brush floor.** Exclusion happens only where SSR shows a brush better. A brush whose extent along the reflector normal is below f(W) stays in the probe, even below t. So a brush is excluded when its thinness ≥ f, its size < t and it has no underside over the water (below), unless overridden. For water, thinness is height: horizontal bars and slabs seen edge-on stay in the probe, and thin vertical poles go to SSR, which samples them along their length (`research.md` §Thin-brush floor). Rival rejected: smallest in-plane extent, which kept poles in the probe and sent the bars the march can step over to SSR.
  - **[SSR]** f(W) = W · 2·tan(θv/2) / D_min: the extent one march stride spans at view distance W.
    - D_min is the smallest `rows_per_stride_pixel` among tiers that run SSR, which is the coarsest stride (`FluidReflectionMarch::stride`, `FLUID_REFLECTION_PRESETS`, the `screen_march` sample loop).
    - θv is the vertical FOV at the default HFOV 100° and 16:9.
  - At af8bfef, D_min = 135 (ultra), so f ≈ 0.0099·W: 5 cm at 5 m, 0.5 m at 50 m.
  - f's ratio is a compiler constant, not a live read of the presets, because the presets are placeholders until perf data and a live read would re-key every fluid map's capture on each retune. A test derives the ratio from render-cpu's presets and the pinned default FOV and fails when they drift apart, so a preset change updates the constant deliberately. Re-derive the value at build time if the presets changed before SSR lands.
  - f is the same at every tier.
- **Undersides stay in the probe.** A brush is never excluded by the size rule when any of its surviving faces (post-CSG, through the face → brush table) faces down against the reflector normal, lies above the water plane, and overlaps the probe's bound water in horizontal projection. Docks, footbridges, decks, lintels and beams over the water keep their undersides in the probe. The camera never sees those faces, so SSR can never return them (`rendering_pipeline.md` §7.3a Limits; `fluid-ssr-reflections` Non-goals), and an authored `_capture_offset` beneath a deck keeps working. `never` still excludes such a brush. Pillars and posts standing in the water, and walls beside it, follow the size rule.
- **Rivals weighed.** Excluding nothing and letting SSR's confidence cover the object keeps the wrong-parallax copies and capture shadows wherever c < 1. Authored opt-out only, with every brush kept by default, was rejected because the owner asked for an automatic default; the override covers what the rule gets wrong.
- **Contact holes: fill first, leak guard as the fallback.** CSG removes kept faces where an excluded brush touched them (`research.md` §Contact holes).
  - **Fill.** A ray that skips an excluded brush, then leaves its hull through a side coplanar with and covered by a kept brush's side, records a hit at the exit point. It shades as that kept surface:
    - material and UV from the kept side's texture and projection;
    - lighting from the nearest lightmap texel of the coplanar kept face, clamped to that face;
    - SH and SDF lights at the exit point.
  - **Leak guard.** Where no coplanar kept face exists, such as a brush embedded in a kept brush, the ray records the excluded brush's own hit.
  - A capture ray from a closed room never reaches the void or another room through an excluded brush.
  - Rival rejected: accepting the leak, which shows wherever SSR misses and always at `low`.
- **Accepted limit: `low` shows no excluded decor.** **[SSR]** `low` is the probe term alone (`FluidReflectionQuality::runs_screen_space`), so excluded brushes never reflect there. This is a regression against today's probe-only picture, which shows that decor, and Low is the likely tier on the Mac perf floor. The owner accepted it (2026-10-20) over baking a second, unfiltered capture for Low, which doubles probe memory and capture time.
- **Override: `_reflection_capture` = `auto` | `always` | `never`** on `func_group` and `switch`, for every brush of that entity: `always` keeps, `never` excludes, `auto` follows the rule. Worldspawn brushes are always `auto`. A bad value warns once, naming the entity, and acts as `auto`.
- **Authoring through a `func_group` brush entity, not a TrenchBroom group.** TrenchBroom keeps only a group's name, id, link id and transform, and drops any other key on load and save (`research.md` §TrenchBroom). The FGD gains a `func_group` brush entity carrying `_reflection_capture`, and `switch` gains the key. `docs/` tells mappers to use Create Brush Entity → `func_group` to override, and names no `cargo run -p xtask` command. The parser keeps handling both kinds through `is_editor_group_classname`.
- **Layers.** The Quake adapter reads the key and hands shared stages a canonical per-brush value, so `_reflection_capture` never crosses `format/` (`build_pipeline.md` §Source-format neutrality). Classification, filtering and fill live in the reflection-probe stage.
- **Cache.** The stage key covers each probe's exclusion as the capture applies it, so an edit to an override, a curve constant or the floor constant re-keys the stage. `STAGE_VERSION` → 10.
- **Sequencing.** This builds after `fluid-ssr-reflections` lands on main.
- **IP pre-ship line.** Add to the owner's pre-ship check: "size-based capture exclusion: compare with UE Visible in Reflection Captures / Cull Distance Volumes, Unity Reflection Probe Static / culling mask / near clip; read Perfect World WO2022227553A1." Engineering triage, not legal advice.
- **Non-goals.**
  - Runtime or view-dependent exclusion: probe content tied to the camera is Tencent claims 4–14 and breaks `resource_management.md` §9.
  - Probes on non-fluid surfaces. The rule is defined for them; nothing builds them.
  - Cross-probe threshold agreement: seams are accepted.
  - Excluding props. `prop_mesh` never enters the BVH, so this is already true.
  - A detail brush class, and per-brush keys on worldspawn.
  - Excluding brushes from shadow rays or from the box fit.

## Acceptance

### Automated
Threshold and size:
- [ ] t(W) at 2, 10, 50 and 100 m is within 1e-3 of 1.7, 8.5, 16.5 and 21.956. It is continuous at 10 m and strictly increasing over 0.5–500 m.
- [ ] W from bound area: an L-shaped pool fixture and a 10 × 2.4 m canal fixture each give √area.
- [ ] Predicate, both sides of t: a brush with in-plane extent t − ε is excluded and t + ε is kept, at W = 5 m and at W = 50 m.
- [ ] Floor, both sides, at W = 5 m and 50 m: a brush whose extent along the normal is just below f is kept, and one just above f (and below t) is excluded. A post thinner than f in plane but taller than f is excluded. f follows the SSR tiers' coarsest stride. f < t over 0.5–500 m.
- [ ] Given a vertical reflector normal, size comes from the vertical and along-wall extents, and thinness from depth along the normal.
- [ ] The floor-ratio test fails when render-cpu's coarsest SSR stride or the pinned default FOV changes, and passes at the shipped presets.

Undersides, both sides of the rule:
- [ ] On a 50 m lake, a 10 m × 0.3 m dock over the water (below t, above f) is kept, and so is a beam spanning the water.
- [ ] A pillar standing in the water and a post beside it, with no downward face over the water above the plane, are excluded.
- [ ] A brush whose downward face lies above the plane but outside the bound water's horizontal projection follows the size rule.
- [ ] A deck under an authored `reflection_volume` whose `_capture_offset` sits beneath it stays in that probe's capture.

Capture, on a pillar in a pool touching the ceiling:
- [ ] Directions through the pillar's body read the wall behind it, in colour and distance.
- [ ] Directions through the filled ceiling patch read the ceiling's material, at the distance of the ceiling plane, lit by the nearest texel of the kept ceiling face.
- [ ] The wall behind keeps the pillar's shadow in the colour tile, because the shadow ray still sees the pillar.
- [ ] Texels the pillar never covered are bit-identical to the unfiltered build.

Capture, other cases:
- [ ] Leak-guard fixture: a small brush embedded in a kept brush, with no coplanar kept face, reads its own surface in the directions where it is the first hit.
- [ ] Closed-room fixture with excluded brushes against walls, under the ceiling and embedded: no capture ray misses, and none lands outside the room.
- [ ] The placed probe set (boxes, points, bindings) is identical with the filter on and off.
- [ ] In one map, one brush is excluded from a small probe's capture and kept by a large probe's.
- [ ] A map whose brushes are all ≥ t emits id 54 byte-identical to the pre-change compiler (cache aside).

Override:
- [ ] `never` excludes a brush ≥ t, `always` keeps a brush < t, and `auto` follows the rule, on a `func_group` brush entity and on a `switch`.
- [ ] An invalid value warns once, naming the entity, and acts as `auto`.

Cache and determinism:
- [ ] Changing a `_reflection_capture` value or a curve constant misses the capture stage. An unchanged rebuild hits.
- [ ] `--no-cache` at `--jobs 1` and at `--jobs 4` gives byte-identical id 54.

Gates:
- [ ] Source gate: no exclusion symbol appears in renderer or render-cpu, and the id 54 version constant is unchanged. In the compiler, `_reflection_capture` appears only on the Quake adapter's side of the canonical map data.
- [ ] The FGD's `func_group` and `switch` entries offer the three values. `docs/` documents the key through a `func_group` brush entity and contains no `cargo run -p xtask`.

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
- **Filter seam.** `SceneTracer::closest_hit` (hit.rs) loops over `bvh.traverse_iterator`. Each primitive is one face (`face_by_offset`), so a per-probe face mask skips a primitive whole. For the fill it also reports the nearest skipped hit. Pass the mask through `capture_section` → `capture_row` → `CaptureScene::radiance`. `fit_box` and `segment_clear` call the tracer without it.
- **Face → brush.** `extract_geometry` drops `Face::brush_index`. Carry a compiler-side table in output face order (the `ordered_faces` walk). The oversize cut renumbers faces (`plan_cut_charts` `face_remap`, `rebuild_face_identity`), so remap the table there, each sub-face taking its parent's brush (`build_pipeline.md` §Compiler pipeline step 10). Keep it out of level-format `FaceMeta`.
- **Sizes.** Use `BrushVolume::aabb`, the unclipped hull in engine metres.
- **Fill seams.** All reachable at capture time; `map_data` is in scope at the probe stage call in pipeline.rs (`research.md` §Contact holes).
  - Hull exit from `BrushVolume::planes`; a covering side from `BrushVolume::sides` (`BrushSide::texture`, `tex_projection`, `vertices`).
  - UV from `geometry::compute_texel_uv` with `engine_to_quake` (both private today).
  - The coplanar kept face from the face → brush table plus a plane match. Its nearest point comes from `capture_point::nearest_point_on_polygon` (2D: project into the plane's basis). Its vertices' `lightmap_block`, `lightmap_uv` and `animated_block` feed `LightmapSampler::irradiance` and the animated samplers.
  - Split `CaptureScene::radiance` into trace and shade, so a synthesized `SurfacePoint` shades like a traced one.
  - Precompute contact candidates per excluded brush: kept brushes whose bounds touch its own.
- **Override table.** `MapAssembly` will not carry it: `switch` brushes have no assembly, and a `func_group` brush entity without `_tb_id` matches none. Read the key in the `func_group` and `switch` branches of parse.rs with `collect_entity_properties`, and translate it at the retained-hull seam `brush_assembly_for_retained_hulls` uses. A brush takes its own entity's value.
- **Floor inputs.** level-compiler already depends on render-cpu, so the drift test reads `FluidReflectionQuality::ALL` and `preset()`; the bake reads only the compiler constant. The default FOV lives in the binary (`camera::HFOV`): pin a compiler copy with a test.
- **Underside test.** The face → brush table gives each brush's surviving faces. Whether a crate sitting flush on a deck keeps a bottom face after CSG is unverified; if it does, the crate is kept, which is the conservative outcome.
- **Cache key.** Hashing each probe's excluded faces (post-cut indices) subsumes overrides, curve, floor and the face → brush table.
- **Docs.** `docs/level_design.md` §Fluid reflections; `sdk/TrenchBroom/postretro.fgd` `switch`, plus a new `func_group` entry.
- **Large files.** parse.rs and pipeline.rs are past ~800 lines. Put new logic in its own module and keep their edits to call sites.
- **Shape.** Chosen: a per-call face mask over the one shared BVH, with exit tests against hull planes. Rival: one BVH per distinct excluded set, which costs a build and its memory per probe and still leaks.
- **First slice.** A pillar touching the ceiling, with exclusion hard-wired: the filter, the contact-hole leak, then the fill. It tests the riskiest assumption, that a fill can be found and shaded from existing data, before any key or curve work.

## Boundary inventory

| Name | Rust | Wire / serde | JS / TS | Luau | FGD KVP |
|---|---|---|---|---|---|
| Capture override | canonical per-brush enum on `MapData` (name the executor's) | n/a; never in PRL | n/a | n/a | `_reflection_capture`: `auto` / `always` / `never`, default `auto`, on `func_group` (brush entity) and `switch` |

## Open questions
- Rays crossing several excluded brushes, or an excluded brush partly sunk into a kept one, whose exit lies inside kept solid off any kept side — **delegated**: chain the exit test per excluded brush. Off-side exits take the leak guard by the rule. Report the trace strategy in the plan of record.
