# reflection-probe-capture-exclusion — research

Read at `9add6bb` (main). Facts marked **[SSR]** come from `origin/claude/fluid-ssr-reflections-0b3212` at `af8bfef`, which is not on main yet; re-read them once it lands.

## Seams in the compiler

- **One primary-ray chokepoint.** `CaptureScene::radiance` (shading.rs) calls `SceneTracer::closest_hit` once per supersample; `capture_row` (capture.rs) feeds colour, distance and share from that one call. Shadow rays go through `SceneTracer::segment_clear` → `lightmap_bake::segment_clear`. `fit_box` (box_fit.rs) also calls `closest_hit`, through `place_probes` → `derive_probes`. `capture_section` (mod.rs) runs `capture_row` per `(probe, row)` item, so a per-probe filter reaches the call as an argument, not as tracer state.
- **A BVH primitive is one face.** `SceneTracer::face_by_offset` maps a primitive's `index_offset` to its face. A face filter can skip a whole primitive before its triangle loop.
- **Face → brush.** `map_data::Face::brush_index` indexes `MapData::brush_volumes`. `geometry::extract_geometry` walks `build_leaf_ordered_faces` and writes `FaceMeta` (leaf, texture) only, so the brush index stops there. The map from output face to source face is available inside that walk (`ordered_faces`).
- **The oversize cut renumbers faces.** `atlas_stage::prepare_atlas_stage` runs `lightmap_bake::plan_cut_charts` on `&mut GeometryResult`. When a face is cut, `face_remap: Vec<Range<usize>>` maps each pre-cut face to its sub-face range, and `rebuild_face_identity` rebuilds the BVH and leaf ranges (`RebuiltFaceIdentity`). The probe stage reads the post-cut `geo_result`, `bvh` and `bvh_primitives` (pipeline.rs). `build_pipeline.md` §Compiler pipeline step 10: "A new face-naming stage joins that set or runs after the cut." A face → brush table built at geometry extraction must be remapped by `face_remap`; each sub-face takes its parent's brush.
- **Brush hull bounds.** `BrushVolume::aabb` (`partition::Aabb`, engine metres, Y-up) is built from the brush's unclipped vertices in `build_brush_volumes_with_ids` (parse.rs). It is the hull before CSG, so a brush half sunk in a wall measures its whole hull. A rotated brush measures its axis-aligned bounds: a square post turned 45° reads √2 wider.
- **Grouping.** `world_brush_ids` (parse.rs) collects worldspawn, `func_group` (`quake_map::is_editor_group_classname`, which matches TrenchBroom groups, layers and authored `func_group` brush entities alike) and `switch`. `MapData::brush_assembly` is built through `brush_assembly_for_retained_hulls` from `_tb_id`; `switch` brushes get `None` on purpose, and an authored `func_group` with no `_tb_id` matches no assembly. So the override cannot ride `MapAssembly`. It needs its own per-retained-hull table, read in the `func_group` and `switch` branches with `collect_entity_properties` and translated at the same retained-hull seam (`brush_assembly_for_retained_hulls` is the precedent; degenerate brushes drop there).
- **Units.** `PlacedProbe::bound_area` sums `FaceGeometry::area` of bound faces in engine m², so √area is metres.
- **Crate edge.** level-compiler already depends on postretro-render-cpu (Cargo.toml), so the bake can read **[SSR]** `FluidReflectionQuality::ALL` / `preset()` directly.
- **Cache.** `stage_key::cache_key` hashes geometry, primitives, environment volumes, placement (source, box, point, faces), SH, materials and listed constants. Nothing about brushes enters today. `STAGE_VERSION = 9`.
- **Hidden-water warning.** `warn_hidden_water` (capture_visibility.rs) reads the finished section's distance tile, cold or warm. With exclusion it judges the filtered capture, which is what the runtime draws.
- **Large files.** parse.rs (~3 000 non-test lines) and pipeline.rs (~2 200) are past the ~800-line mark.

## Contact holes

Brush-side projection drops face fragments in solid space (`build_pipeline.md` §Compiler pipeline step 3). Where an excluded brush touches a kept one, the kept face has a hole the excluded brush filled. Examples: a pillar under the ceiling, a crate against a wall, a half-column set into a wall, a post on the pool floor. A primary ray that passes through the excluded brush to that hole crosses the kept brush's back faces, which are culled, and lands beyond: the next room's faces, or nothing (distance 0, black). From the capture point C, a hole leaks only when its kept face faces C and the excluded brush stands between them: walls behind decor and ceilings above pillars. Decks under crates and pool floors leak only in the lower hemisphere, which water never reads, though the distance tile keeps the full sphere. Deleting the brush instead re-runs CSG, which is why "equals a build with the pillar deleted" could not hold through a hole.

**The fill is feasible from existing data, with no new pipeline stage.** It is a contained addition to the probe stage, not a large mechanism:

| Need | Source at capture time |
|---|---|
| Which excluded brush the ray crossed | The nearest skipped face hit in `closest_hit`, through the face → brush table |
| Hull exit point and side | `BrushVolume::planes` (convex: least exit t over planes the ray leaves) |
| Kept brush in contact | Kept `BrushVolume`s whose `aabb` touches the excluded one, precomputed per excluded brush; the exit point nudged along the ray lies inside one's planes |
| Covering kept side | `BrushVolume::sides`: a side on the exit plane, opposite-facing, whose `vertices` polygon holds the exit point |
| Material and UV | `BrushSide::texture` → `TextureNamesSection` index; `BrushSide::tex_projection` through `geometry::compute_texel_uv` and `engine_to_quake` (private; widen to `pub(crate)`) |
| Coplanar kept face and its lighting | Post-cut geometry faces of that kept brush on that plane (face → brush table plus plane match). Nearest point by `capture_point::nearest_point_on_polygon` (2D; project into the plane basis); that point's barycentrics on the face give `lightmap_uv`, `lightmap_block` and `animated_block` for `LightmapSampler::irradiance` and `AnimatedDirectSampler` |
| SH, SDF, emissive, mip | Unchanged: `CaptureScene::radiance` evaluates them at a `SurfacePoint`; split trace from shade so a synthesized point shades the same way. Texel density from the kept face |

`map_data` is in scope where pipeline.rs builds `ReflectionProbeInputs` (it already passes `map_data.reflection_volumes`), so the brush hulls can join the inputs. The leak guard falls out of the same exit test. If the exit point lies inside kept solid but on no covering kept side, or the covering side has no surviving geometry face to light it (a side the excluded brush covered whole), the ray records the excluded brush's own hit, which the filter skipped. A brush partly sunk into a kept one exits inside kept solid off any side, so it takes the leak guard and shows itself in those directions.

## Thin-brush floor, grounded in the march [SSR]

- `screen_march` (render-cpu `fluid_surface/screen_march.rs`) samples depth at `p0.lerp(p1, s)`, stepping `stride` full-resolution pixels per step for `step_budget` steps. It hits when a sample lies behind the depth buffer and `min(ray_w, last_w) <= scene_w + thickness`. An object no sample lands on is missed. `the_march_at_one_tuning_reaches_the_same_hit_at_every_target_fraction` (screen_march_tests.rs) shows this: a one-row bar across a mostly vertical screen path is hit at stride 1 and stepped over at stride 4.
- `FluidReflectionMarch::stride` = `max(1, scene_rows / rows_per_stride_pixel)` full-resolution pixels. The target fraction never changes it, because the pass binds full-resolution depth.
- The SSR brief's "same stride at every tier" was superseded on its branch (plan of record, owner 2026-10-20). `FLUID_REFLECTION_PRESETS` (render-cpu `fluid_surface/reflection_quality.rs`) holds placeholders until perf data. `low` is the probe term alone (`runs_screen_space` false), and `off` turns off both layers. The owner chose the coarsest stride among SSR tiers, so the bake floor holds for every tier.

| Tier | Fraction | Steps | Divisor D | Thickness |
|---|---|---|---|---|
| medium | ½ | 48 | 180 | 1.0 m |
| high | 1 | 96 | 270 | 1.0 m |
| very_high | 1 | 192 | 180 | 1.5 m |
| ultra | 1 | 256 | 135 | 2.5 m |

- **Stride as an angle.** One pixel spans about 2·tan(θv/2)/rows at screen centre, so one stride spans 2·tan(θv/2)/D: resolution cancels. The integer floor of rows/D only makes the stride finer. An object of extent e at view distance d is sampled reliably when e/d ≥ that angle. The engine default is HFOV 100° (`camera::HFOV`, binary crate); at 16:9, tan(θv/2) ≈ 0.670. With d = W (owner):

| D | Tier now | f ÷ W | at W = 5 m | at W = 50 m |
|---|---|---|---|---|
| 135 | ultra (coarsest; the floor) | 0.00993 | 5.0 cm | 0.50 m |
| 180 | medium, very_high | 0.00745 | 3.7 cm | 0.37 m |
| 270 | high | 0.00497 | 2.5 cm | 0.25 m |

  At HFOV 130°, the player maximum, the coarsest ratio is 0.0179. The bake uses the default FOV. f stays far below t, since p(W) ≥ 0.085 up to 500 m.
- **Why thinness is measured along the normal.** For water, a reflected ray's screen path runs mostly vertically. A vertical post crosses it along its height and is sampled along its length. A horizontal bar or a slab seen edge-on crosses it across its extent along the normal, which a stride can step over. On the march alone, extent along the normal predicts misses better than the smallest in-plane extent. The owner first chose in-plane thinness, then switched to extent along the normal (2026-10-20) once it was clear that in-plane thinness keeps poles in the probe.
- **Reach, not stride.** Reach is `step_budget × stride` pixels: high ≈ 0.36 screen heights, medium ≈ 0.27, ultra ≈ 1.9. A tall excluded object near the camera can reflect past reach at lower SSR tiers, and its upper part then fades out with no probe fallback. In-plane size ignores height, so tall thin poles are excluded. This is a manual-check item.
- **Thickness band.** 1–2.5 m. Decor a few centimetres thick reads as a metre deep, so SSR may draw a short false streak behind it.
- **Medium's ½ target.** Each target texel covers 2×2 scene pixels, so a pole one pixel wide can shimmer at medium.

## TrenchBroom groups

Read from TrenchBroom master at 29ef55a (sparse clone). `mdl::Group` holds only `m_name` and `m_transformation` (Group.h). `createGroupNode` (MapReader.cpp) reads `_tb_name`, `_tb_id`, `_tb_linked_group_id` and the transform, and keeps no other property. `NodeSerializer::groupProperties` writes back only classname, `_tb_type`, name, id, link id and transform. So a key hand-added to a group's `func_group` is dropped on the next TrenchBroom save, and the editor offers no property editor for groups. The manual's map model agrees: "a Group is composed of a name, zero or more groups, zero or more entities, and zero or more brushes."

`isGroup` (EntityProperties.cpp) requires `classname func_group` with `_tb_type _tb_group`. A `func_group` without `_tb_type` loads as an ordinary brush entity with editable properties. TrenchBroom lists it under Create Brush Entity only when the FGD defines it, and postretro.fgd has no `func_group` today. Layers are the same `func_group` shape with `_tb_type _tb_layer`, and carry no custom keys either. The compiler's `is_editor_group_classname` matches all three by classname, so the parser needs no new branch.

## Threshold curve

p(W) = 0.85 for W ≤ 10 m; 0.85·(W/10)^−a above, with a = ln(0.85/0.33)/ln 5 ≈ 0.58787, so 50 m gives exactly 0.33. t = p·W: 2 → 1.7, 10 → 8.5, 50 → 16.5, 100 → 21.956, 500 → 42.6. The rounded a = 0.588 gives 16.497 at 50 m and 21.949 at 100 m, both outside a 1e-3 check against 16.5 and 21.9. The handoff's "≈21.9" is the rounded value.

W = √(bound area): a 10 × 2.4 m canal gives 4.90 m, where its long side is 10 m. A canal-length rail of 6 m stays in the probe even though it runs the whole canal.

## IP

Exclusion changes only which static geometry the offline capture holds. It adds no runtime input, no camera dependence and no fill keyed to the capture-shadow test. On the readings in `plans/done/reflection-probes/ip-check.md` (claim chart: 4–9 camera-placed probes, 15 completion) and `plans/in-progress/fluid-ssr-reflections/ip-check.md` (§3 Microsoft per-pixel technique choice, §1 Tencent claim 15), nothing moves. Perfect World WO2022227553A1 (SSR ip-check §5) classifies reflected objects static/dynamic for SSR and was screened at abstract level only; a size split between layers sits near it, which is why the pre-ship line names it. Engineering triage, not legal advice.
