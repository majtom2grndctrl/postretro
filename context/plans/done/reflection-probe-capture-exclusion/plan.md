# reflection-probe-capture-exclusion — plan of record

mode: resumable
status: done
read at: d6f3471c0

Owner approved 2026-10-20 ("Plan approved. Start the build.").

## Shared contracts (integrating executor owns)
- `map_data::ReflectionCapture { Auto (default), Always, Never }`: seeded with the approval commit.
- `MapData::brush_capture_override: Vec<ReflectionCapture>`, index-aligned with `brush_volumes` (task 4). Named so it never contains the substring `_reflection_capture`, which the AC 45 source gate greps for.
- Face → brush table: `Vec<Option<u32>>`, index-aligned with the post-cut output faces (the `SurfaceHit::face` / `face_index_ranges` space), each entry indexing `MapData::brush_volumes`; `None` for a face the oversize cut emptied. Built in `extract_geometry`'s walk beside `GeometryResult`, remapped by `face_remap` (task 1).
- Per-probe exclusion: a sorted, deduplicated `Vec<u32>` of excluded brush indices per placed probe (task 3 produces, tasks 1/2/5 consume). The capture derives its face mask from it through the face → brush table.

Branch: `claude/reflection-probe-capture-exclusion`, cut from `main` at the claim commit. `fluid-ssr-reflections` has landed on main (PR #610), so every **[SSR]** fact in the brief was re-read against main.

## Corrections
- **SSR presets moved after af8bfef.** Brief and research: D_min = 135 (ultra), f ÷ W = 0.00993, f ≈ 0.50 m at W = 50 m. Now (`render-cpu fluid_surface/reflection_quality.rs`, `FLUID_REFLECTION_PRESETS`): medium 180, high 180, very_high 360, ultra 540. The coarsest stride among SSR tiers is divisor 180, so f ÷ W = 2·tan(θv/2)/180 ≈ 0.00745, and f ≈ 0.37 m at 50 m, 3.7 cm at 5 m. The Decision's definition (smallest divisor among tiers that run SSR) is unchanged; only the value moved, as the Path anticipated ("re-derived if the presets change"). AC "On a 50 m lake (t = 16.5 m, f ≈ 0.50 m)" reads f ≈ 0.37 m; its fixtures (dock 1 m thick, beam 0.6 m deep) stay above f, so the row keeps its meaning.
- **Stride is fractional now** (`FluidReflectionMarch::stride` = `max(1, rows / divisor)`, never floored). The research note "the integer floor only makes the stride finer" no longer applies; the angle derivation 2·tan(θv/2)/D now holds exactly above the 1-pixel clamp.
- **SSR reach.** High, very_high and ultra all reach about half a screen height; medium about a quarter. The research reach figures (0.36 / 0.27 / 1.9) are stale. This affects only the manual check on tall excluded objects.
- **`fluid-ssr-reflections` is in `plans/done/`**, not `in-progress/`. The ip-check §9 pre-ship list is `context/plans/done/fluid-ssr-reflections/ip-check.md` §9. Its bullets are `- **<id/holder>:** <prose>`, and the new line follows that form.
- **`FaceMeta` fields are `leaf_index` and `texture_index`.** `extract_geometry` holds `face_idx` per output face in its `ordered_faces` loop, but `GeometryResult` keeps no record of it. The face → brush table is built in that loop as a parallel vector beside `GeometryResult`, not inside it, which keeps it out of the id-54 geometry serialization and the stage key's postcard of `GeometryResult`.
- **`radiance` returns `(Vec3, Option<f32>)`, and `SurfacePoint` carries no face index.** `SurfaceHit` does carry one. The trace/shade split therefore returns the `SurfaceHit` from trace, and shade takes a `SurfacePoint` plus a hit distance. A fill synthesizes the `SurfacePoint` from the covering kept face.
- **`closest_hit` does not offset its origin.** It floors hits at `t > RAY_EPSILON` (1 mm, `hit.rs`). O10's "trace on from the exit, accepting any front-face hit past the exit itself" is a re-trace from the exit point with the hit floor measured from the exit, not from the exit plus 1 mm. Equivalently, it is one trace with a lower bound t ≥ t_exit. Choice: add a `min_distance` to the masked call so a re-trace never steps over a face within 1 mm of the exit.
- **No shared box-containment test.** `authored.rs` `Candidate::contains` is private, so the probe stage gains its own projection-box test with `PLANE_TOLERANCE_M` (`placement.rs`, `pub(crate)`, 1 mm), which ignores the box floor (O6).
- **`compute_texel_uv` and `engine_to_quake` are private** in `geometry.rs`. They widen to `pub(crate)`, as the research table says; callers divide by `units_to_meters` first.
- **AC 24's "0.5 mm short, so CSG cuts no hole" is false** (task 1). Brush-side projection treats a gap of ≤ 1 mm as contact: at 0.5 mm and 1 mm both the ceiling face above the pillar and the pillar's top are removed; at ≥ 1.5 mm both survive. The row's outcome (every direction through the pillar toward the ceiling reads the ceiling at the ceiling's distance; none takes the leak guard) keeps its meaning and is proved twice: at a 2 mm gap through the intact face (rule 4 re-trace, O10), and at 0.5 mm through the fill, whose plane match uses a tolerance ≥ the 1 mm hit epsilon (O10's last clause).
- **Pillar fixtures end the pool flush with the pillar's back** (task 1). Water continuing behind a pillar standing in the pool is cut into its own face, which fits its own box at the pillar's back and gives a second derived probe. The flush fixture keeps one probe whose box reaches the walls and ceiling, as AC 20 assumes.
- **"A face the cut emptied maps to none"**: an emptied face has no post-cut index, so it leaves no entry in the table (`geometry::remap_face_brushes`); `None` marks only a post-cut slot no range reaches.
- **Excluded sets are bounded by the projection box** (task 5). Classification is map-wide, and contact candidates scan every brush per excluded brush per probe, which is quadratic on large maps. A brush wholly outside a probe's projection box (floor ignored, ± `PLANE_TOLERANCE_M`) captures identically kept or excluded: any ray reaching it ends outside the box, where the leak guard shows the same surface at the same distance. So each probe's excluded set, and its contact candidates, are restricted to brushes whose hulls touch that box. The capture is unchanged; the work and the cache key's brush share shrink to the box.
- **O11's overlap exists only where the trim straddles the wall's end** (task 2). A trim wholly inside a wall's face, its front coplanar with the wall's, loses that front to CSG in either map order, so the wall is always traced. The AC 29 fixture is a kept wall panel with the trim across its end, as the row's "straddling its end" wording says.
- **AC 28's wall stub cannot stand with the exterior behind it** (task 2): openings beside a stub onto the exterior leak the map. The stub is built with a second room behind, under an authored box over the first room; the exterior backdrop is proved by the `never` outer wall, both backdrops. The row keeps its meaning.
- **A covering side with no surviving coplanar face ends the fill search for that hull** (task 2): the exit classification decides, so a side the excluded brush covered whole takes the leak guard (research.md Contact holes). With pooled sides (O12), material and UV come from the side of the brush owning the nearest pooled face, so material and lighting name one surface.
- **AC 42's two builds share vertex positions and indices at 0.5 m sunk** (task 5); they differ in `FaceMeta` leaf indices. The key still differs through the hashed excluded hull planes, which is the row's first outcome.
- **A `switch` owns one brush** (task 5); the parser refuses a multi-brush switch, so AC 36's switch fixtures give each brush its own switch.
- **prl-build has no pool-edge flag** (task 5); AC 34's forced-cut half forces the oversize cut end to end with a 91 m hall at the default density (log: "cut 8 oversize lightmap face(s)").
- **`build_brush_volumes_with_ids` drops degenerate brushes and sides.** `BrushVolume::sides` and `planes` are not index-aligned. The exit test uses `planes`, and the covering-side test uses `sides` (vertices, normal, distance).

## Delegated answers
- Canonical override type name: `ReflectionCapture { Auto, Always, Never }` in `map_data.rs`. `MapData::brush_capture_override: Vec<ReflectionCapture>` is index-aligned with `brush_volumes`, built at the retained-hull seam beside `brush_assembly`. Reason: it mirrors `brush_assembly`, and the name is format-neutral.
- Where the floor constant lives: a compiler constant `SSR_COARSEST_STRIDE_DIVISOR = 180` and `DEFAULT_HFOV_DEGREES = 100` in the new classification module, with f's ratio computed from them. A `#[cfg(test)]` drift test reads `FluidReflectionQuality::ALL` / `preset()` (dev-dependency) and asserts it, and a pin test asserts HFOV 100° against a copy, since `camera::HFOV` lives in the binary crate, which level-compiler cannot depend on. Reason: the Path chose this, and render-cpu stays a dev-dependency.
- Vertical FOV: θv = 2·atan(tan(HFOV/2) · 9/16).

## AC-to-proof

| AC | Proof | Status | Result |
|---|---|---|---|
| 1 t(W) values, continuity, monotonic | `classify` unit test | achievable as stated | pass: `size_threshold_meets_the_anchors_and_rises_continuously_with_width` |
| 2 W = √area on L pool and canal | `classify` unit test over `PlacedProbe::bound_area` from fixture geometry | achievable as stated | pass: `water_width_is_the_root_of_the_bound_area_for_an_l_pool_and_a_canal` |
| 3 predicate t ± ε at 5 m and 50 m | `classify` unit test | achievable as stated | pass: `brush_just_under_the_threshold_is_excluded_and_just_over_it_kept` |
| 4 floor both sides; post; f follows coarsest stride; f < t | `classify` unit tests | achievable as stated | pass: `brush_just_under_the_floor_is_kept_and_just_over_it_excluded` |
| 5 shore rail kept, post excluded at W = 2 m | `classify` unit test | achievable as stated | pass: `shore_rail_is_kept_and_standing_post_excluded_on_two_metre_water` |
| 6 vertical reflector normal | `classify` unit test (size/thin functions take the normal) | achievable as stated | pass: `brush_extents_against_a_vertical_normal_read_vertical_and_along_wall_size_and_depth_thinness` |
| 7 floor-ratio drift test | `#[cfg(test)]` drift test reading render-cpu presets + HFOV pin | achievable as stated | pass: `floor_stride_divisor_matches_the_coarsest_screen_space_tier`, `floor_field_of_view_matches_the_engine_default` |
| 8 lake dock/beam kept over water, excluded off it (f ≈ 0.37 m, see Corrections) | underside unit tests on built geometry | achievable as stated | pass: `lake_dock_and_beam_are_kept_over_the_water_and_excluded_off_it` |
| 9 pillar and post in water excluded | underside unit test | achievable as stated | pass: `pillar_and_post_standing_in_the_water_are_excluded` |
| 10 off-projection and edge-only ledge excluded | underside unit test | achievable as stated | pass: `downward_faces_off_the_water_or_meeting_it_only_along_its_edge_are_excluded` |
| 11 deck under authored `_capture_offset` kept | underside test through authored placement | achievable as stated | pass: `deck_over_an_authored_capture_offset_stays_in_the_capture` |
| 12 sunk crate on dock excluded | underside test through CSG geometry | achievable as stated | pass: `crate_sunk_into_a_dock_over_the_water_is_excluded_and_the_dock_kept` |
| 13 footbridge over pool A only: A keeps, B excludes | per-probe classification test | achievable as stated | pass: `footbridge_over_one_pool_is_kept_by_its_probe_and_excluded_by_the_other` |
| 14 two-height authored box deck | underside test | achievable as stated | pass: `deck_between_two_bound_planes_is_kept_over_the_lower_water_and_excluded_under_the_upper` |
| 15 45° wedge kept; 2° kept, 0.5° excluded | underside test | achievable as stated | pass: `faces_sloping_down_past_one_degree_over_the_water_are_undersides` |
| 16 face crossing the plane, part above kept | underside test | achievable as stated | pass: `face_crossing_the_water_plane_counts_by_its_part_above` |
| 17 pillar body reads wall, distance/mip from capture point | capture test, pillar fixture | achievable as stated | pass: `rays_through_an_excluded_pillar_read_the_wall_behind_from_the_capture_point` |
| 18 ceiling fill: material, distance, static + SH within 5%, deterministic, albedo at UV | capture test | achievable as stated | pass: `the_filled_ceiling_patch_reads_the_ceilings_material_and_lighting` |
| 19 sdf and animated fill within 5%; pillar-only animated light no share | capture test | achievable as stated | pass: `the_filled_patch_takes_the_ceilings_sdf_and_animated_light_and_the_pillars_light_drops_out` |
| 20 derived box on wall/ceiling: no leak guard through pillar or patch | capture test (instrumented outcome counts) | achievable as stated | pass: `no_ray_through_the_pillar_or_its_filled_patches_takes_the_leak_guard` |
| 21 wall keeps pillar sdf shadow | capture test | achievable as stated | pass: `the_wall_behind_an_excluded_pillar_keeps_its_sdf_shadow` |
| 22 untouched texels bit-identical; untouched BC6H blocks byte-identical | capture test, filtered vs unfiltered | achievable as stated | pass: `texels_no_ray_sent_into_the_pillar_capture_bit_identically_when_it_is_excluded` |
| 23 sunk pillar fills; overlapping kept brush both orders fills | capture tests | achievable as stated | pass: `a_sunk_pillar_and_an_overlapping_kept_brush_both_fill_the_ceiling_patch` |
| 24 pillar 0.5 mm short reads intact ceiling | capture test | achievable as stated | pass (per Corrections): 2 mm `rays_through_a_pillar_short_of_the_ceiling_re_trace_onto_the_intact_ceiling`; 0.5 mm `rays_through_a_pillar_half_a_millimetre_short_of_the_ceiling_fill_at_the_ceiling`, `shallow_rays_through_a_pillar_half_a_millimetre_short_of_the_ceiling_fill_at_the_ceiling` |
| 25 embedded leak-guard brush reads itself | capture test | achievable as stated | pass: `an_excluded_cap_embedded_in_a_kept_beam_reads_its_own_surface` |
| 26 closed room: no miss, none outside | capture test | achievable as stated | pass: `a_closed_room_with_every_decor_brush_excluded_never_misses_or_leaves_the_room` |
| 27 chained closed room O3 | capture test | achievable as stated | pass: `a_chained_closed_room_fills_every_contact_patch_behind_its_excluded_brushes`; sub-mm: `rays_through_stacked_crates_half_a_millimetre_apart_fill_the_ceiling_patch`, `shallow_rays_from_a_crate_into_one_half_a_millimetre_above_fill_the_ceiling_patch` |
| 28 seal fixture both backdrops; post-then-`never` wall leak guard reads post | capture test | achievable as stated | pass (per Corrections): `excluded_seals_read_their_own_surfaces_and_a_post_before_one_reads_itself` |
| 29 coplanar trim both orders | capture test | achievable as stated | pass (per Corrections): `a_coplanar_trim_over_a_wall_reads_the_wall_in_either_map_order` |
| 30 placement identical filter on/off | placement test | achievable as stated | pass: `placed_probes_are_identical_with_the_filter_on_and_off` |
| 31 two probes, O7 | capture test | achievable as stated | pass: `a_brush_one_probe_excludes_fills_only_in_the_probe_that_keeps_it` |
| 32 all ≥ t: id 54 byte-identical to forced-empty; old capture tests unchanged | capture test + existing suite | achievable as stated | pass: `a_level_whose_brushes_all_reach_the_threshold_captures_byte_identically_unfiltered`; every pre-existing capture test unchanged |
| 33 oversize cut: face → brush remap, mask skips exactly the pillar's sub-faces | unit test through `plan_cut_charts` remap | achievable as stated | pass: `oversize_cut_remaps_each_sub_face_to_its_parent_brush_and_the_mask_follows`, `remap_face_brushes_follows_the_cut_and_drops_an_emptied_face` |
| 34 end to end prl-build, with and without forced cut | integration test in `tests/`, `.map` fixture | achievable as stated | pass: `prl_build_captures_the_wall_behind_a_pillar_the_rule_excludes`, `prl_build_captures_through_an_excluded_pillar_with_the_oversize_cut_forced` |
| 35 hidden-water warning fires unfiltered, not filtered, cold and warm | capture test with log capture | achievable as stated | pass: `hidden_water_warns_unfiltered_and_not_filtered_cold_and_warm` |
| 36 override on func_group and switch | parse + classify test from `.map` text | achievable as stated | pass: `func_group_and_switch_reflection_capture_lands_on_every_brush`, `capture_overrides_on_func_group_and_switch_beat_the_rule_through_parse` |
| 37 degenerate first brush in `never` group | parse test | achievable as stated | pass: `never_func_group_with_degenerate_first_brush_excludes_its_other_brushes`, `retained_hull_seam_keeps_values_on_their_own_brushes_when_a_brush_drops` |
| 38 invalid value warns once per entity, cold/warm/no water | parse test with log capture + stage test | achievable as stated | pass: `invalid_reflection_capture_warns_once_naming_the_entity_and_acts_as_auto`, `an_invalid_override_warns_once_cold_warm_and_without_water` |
| 39 authored func_group no marker warning; TB group with empty id still warns | parse test with log capture | achievable as stated | pass: `authored_func_group_without_tb_keys_raises_no_empty_marker_warning` |
| 40 worldspawn `never` inert; `_tb_group` / `_tb_layer` ignored with warning | parse test | achievable as stated | pass: `worldspawn_reflection_capture_never_leaves_world_brushes_auto`, `trenchbroom_group_and_layer_ignore_reflection_capture_with_one_warning_each` |
| 41 key misses on override, anchors, floor ratio, moved brush; hits otherwise; earlier stages hit | `stage_key` tests + pipeline stage-cache test | achievable as stated | pass: `capture_key_misses_on_a_resolved_override_or_any_rule_constant_and_hits_otherwise`, `an_override_only_edit_misses_only_the_capture_stage`, `a_perturbed_rule_constant_moves_a_brush_near_the_threshold_and_misses_the_capture` |
| 42 flush vs sunk: differ in key or byte-identical cold | stage test | achievable as stated | pass: `flush_and_sunk_excluded_pillars_key_the_capture_differently` |
| 43 moving one excluded hull plane or contact side misses | `stage_key` test | achievable as stated | pass: `moving_an_excluded_hull_plane_or_a_contact_side_misses_the_capture` |
| 44 `--no-cache` jobs 1 vs 4 byte-identical | integration test (extend the existing worker-count test) | achievable as stated | pass: `prl_build_excluding_capture_is_byte_identical_at_one_and_four_jobs`, `an_excluding_capture_is_byte_identical_across_worker_counts` |
| 45 source gate grep | gate test or scripted grep in preflight, recorded here | achievable as stated | pass: greps at 0755a3fbd: non-test compiler hits only `parse/reflection_capture.rs`; none in renderer, render-cpu, level-format, level-loader; level-format reflection_probes unchanged from main; `FluidReflectionQuality` only in `exclusion_tests.rs` |
| 46 FGD `func_group` + `switch` offer values; docs; no xtask in docs | grep + FGD parse test if one exists | achievable as stated | pass: `fgd_func_group_and_switch_offer_every_reflection_capture_value`; 0 `cargo run -p xtask` under docs/ |
| 47 ip-check §9 carries the line | grep | achievable as stated | pass: ip-check.md section 9 carries the line |
| 48 visual check on three maps, anchor retune | owner, in-engine | manual | outstanding (manual): owner looked at reflection-probes-test, e24 and campaign-test on 2026-10-20 and reported it looked good; the 50 m+ lake fixture is not built, the listed judgments are not recorded, anchors not retuned |
| 49 cold/warm probe-stage time on e24 before/after | owner's terminal (long bake) | manual | outstanding (manual): cold and warm probe-stage time on e24, before and after |

## Tasks

| # | Task | Owner | Depends on | Status |
|---|---|---|---|---|
| 1 | **First slice, filter and leak guard.** Build the face → brush table in `extract_geometry`'s walk and remap it through the oversize cut. Give `closest_hit` a per-call face mask, a nearest-skipped-hit report and a min distance (`fit_box` and `segment_clear` stay unmasked). Split `radiance` into trace and shade. Add hull exit classification (O3 rules 1, 3, 4) and the leak guard with the box test (O6). Hard-wire exclusion of one pillar in a pool fixture under a ceiling. Proves ACs 17, 21, 22 (partial), 25, 33. | delegated (Opus worker), reviewed by executor | — | done: `capture_exclusion.rs`; `rays_through_an_excluded_pillar_read_the_wall_behind_from_the_capture_point` (17), `the_wall_behind_an_excluded_pillar_keeps_its_sdf_shadow` (21), `texels_no_ray_sent_into_the_pillar_capture_bit_identically_when_it_is_excluded` (22 partial), `an_excluded_cap_embedded_in_a_kept_beam_reads_its_own_surface` (25), `oversize_cut_remaps_each_sub_face_to_its_parent_brush_and_the_mask_follows` (33), `rays_through_a_pillar_short_of_the_ceiling_re_trace_onto_the_intact_ceiling` (24 at 2 mm); `--bin prl-build` 1708 passed, clippy clean |
| 2 | **First slice, fill.** Contact candidates per probe, the covering kept side (O3 rule 2, O9, O11, O12), and synthesized `SurfacePoint` lighting from the nearest coplanar post-cut face (O8), with material and UV via the widened `compute_texel_uv`. If the fill proves infeasible, stop, apply the leak guard everywhere and tell the owner (Decisions). Proves ACs 18–20, 22–24, 26–29. | integrating executor | 1 | |
| 3 | **Classification module.** t(W), W, in-plane size and normal thinness against a reflector normal, the floor constant with drift and HFOV pin tests, and the underside test over post-cut faces and bound water. Pure functions in a new `reflection_probes/exclusion.rs` (or a sibling). Proves ACs 1–16. | delegated (Opus worker), reviewed by executor | — (contract: face → brush table type from 1) | |
| 4 | **Override authoring.** `ReflectionCapture` on `MapData`. Read `_reflection_capture` in parse.rs's `func_group` (no `_tb_type`) and `switch` branches through a new parse submodule, with warnings (O5), the `_tb_type` ignore-and-warn, and the marker-warning narrowing. Translate at the retained-hull seam (O4). FGD `func_group` entry plus `switch` key; `docs/level_design.md` §Fluid reflections. Proves ACs 36 (parse half), 37–40, 46. | delegated (Sonnet or Opus worker), reviewed by executor | — | |
| 5 | **Integration.** Classification per probe feeds masks and contact candidates into `capture_section`. Pass brush volumes, the override table and the face → brush table through `ReflectionProbeInputs`. Cache key additions and `STAGE_VERSION` 10. Hidden-water warning over the filtered section. End-to-end `.map` fixture through prl-build. Proves ACs 30–32, 34–36, 41–44. | integrating executor | 1, 2, 3, 4 | |
| 6 | **Gates and durable text.** Source-gate check (45), ip-check §9 line (47), and `resource_management.md` §5.2 "decided; not built" wording updated on landing. | integrating executor | 5 | |
| 7 | Preflight, review panel, fix loop, full gate. | integrating executor | 6 | done: see Gate and review |

Tasks 3 and 4 touch disjoint files (classification module vs parse/FGD/docs) and can run concurrently with task 1. Cargo runs only in the integrating executor, sequentially.

## Gate and review (at 0755a3fbd)

- Preflight, run at e7cce95e0 (later commits touch only an xtask gate and one doc comment, re-checked):
  - `cargo fmt --check` ✓; `cargo clippy --target-dir target/preflight-clippy -- -D warnings` ✓; level-compiler `--all-targets` clippy ✓.
  - `cargo test`: 12012 passed, 1 failed: `xtask` `no_world_material_fgd_key_or_texture_rule_takes_a_reflection_input`. Fixed in 0755a3fbd; `cargo test -p xtask --bin xtask` 34 passed.
  - `cargo check --release` ✓; `crate-graph --check` ✓.
  - level-compiler `--ignored`: all pass but `cluster_directory_threshold_dry_run_from_prl` and `lightmap_residency_dry_run_from_prl`, which need `POSTRETRO_CLUSTER_DRY_RUN_PRL` / `POSTRETRO_LIGHTMAP_RESIDENCY_DRY_RUN_PRL` naming a prebuilt PRL (environment, not this change).
- The reflection-input gate (fluid-ssr-reflections: "no FGD key gains a reflection input") now allows `_reflection_capture` on `func_group` and `switch` only. The key selects which brushes a probe's bake keeps, not how a surface reflects; the gate's material and texture-rule halves are unchanged.
- Review panel:
  - Round 1, 7 lenses: one 🟡 seam bug (sub-millimetre contact, found twice), the rest hygiene and drift. All acted on except the `_tb_type` exact match (the brief's Decision reads any `_tb_type`), redundant length guards, and a method/free-function name pair.
  - Round 2, 2 lenses: one 🟡 (contact measured along the ray instead of off the hull's planes), fixed with slant-ray tests.
  - Round 3, tracer: clean. 🟢 left: a sub-centimetre grazing band at a covered kept end can read the kept brush's own edge material instead of the leak guard; no slant test for the 2 mm gap, or for a fill on the intact ceiling just outside a pillar's footprint.
- In-engine (owner, 2026-10-20): reflection-probes-test (probes exclude 17, 10 and 8 brushes), e24 (12 probes, 0 to 18 each) and campaign-test (1 probe, 7) bake and run with no new warnings or errors.

## Landing (2026-10-21)

- Landed at the owner's "land the plane" with AC 48 and AC 49 still manual: the 55 m lake fixture (`content/dev/maps/reflection-lake-test.map`) is built and was viewed in-engine, but the listed judgments are not recorded and the curve anchors are not retuned; e24 probe-stage timing is not measured.
- Owner experiments left out of this change: an SSR screen-edge fade at 6% on a smoothstep curve (saved as a patch, not applied); lowering SSR reach. Both belong to the SSR layer (`rendering_pipeline.md` §7.3a), not this brief.
- origin/main merged at ecd91adf8 with no conflicts; post-merge fmt, workspace all-targets check, xtask (34) and level-compiler (1765) pass.
