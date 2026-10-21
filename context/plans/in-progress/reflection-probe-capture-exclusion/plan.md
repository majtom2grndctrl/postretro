# reflection-probe-capture-exclusion — plan of record

mode: resumable
status: approved
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

| AC | Proof | Status |
|---|---|---|
| 1 t(W) values, continuity, monotonic | `classify` unit test | achievable as stated |
| 2 W = √area on L pool and canal | `classify` unit test over `PlacedProbe::bound_area` from fixture geometry | achievable as stated |
| 3 predicate t ± ε at 5 m and 50 m | `classify` unit test | achievable as stated |
| 4 floor both sides; post; f follows coarsest stride; f < t | `classify` unit tests | achievable as stated |
| 5 shore rail kept, post excluded at W = 2 m | `classify` unit test | achievable as stated |
| 6 vertical reflector normal | `classify` unit test (size/thin functions take the normal) | achievable as stated |
| 7 floor-ratio drift test | `#[cfg(test)]` drift test reading render-cpu presets + HFOV pin | achievable as stated |
| 8 lake dock/beam kept over water, excluded off it (f ≈ 0.37 m, see Corrections) | underside unit tests on built geometry | achievable as stated |
| 9 pillar and post in water excluded | underside unit test | achievable as stated |
| 10 off-projection and edge-only ledge excluded | underside unit test | achievable as stated |
| 11 deck under authored `_capture_offset` kept | underside test through authored placement | achievable as stated |
| 12 sunk crate on dock excluded | underside test through CSG geometry | achievable as stated |
| 13 footbridge over pool A only: A keeps, B excludes | per-probe classification test | achievable as stated |
| 14 two-height authored box deck | underside test | achievable as stated |
| 15 45° wedge kept; 2° kept, 0.5° excluded | underside test | achievable as stated |
| 16 face crossing the plane, part above kept | underside test | achievable as stated |
| 17 pillar body reads wall, distance/mip from capture point | capture test, pillar fixture | achievable as stated |
| 18 ceiling fill: material, distance, static + SH within 5%, deterministic, albedo at UV | capture test | achievable as stated |
| 19 sdf and animated fill within 5%; pillar-only animated light no share | capture test | achievable as stated |
| 20 derived box on wall/ceiling: no leak guard through pillar or patch | capture test (instrumented outcome counts) | achievable as stated |
| 21 wall keeps pillar sdf shadow | capture test | achievable as stated |
| 22 untouched texels bit-identical; untouched BC6H blocks byte-identical | capture test, filtered vs unfiltered | achievable as stated |
| 23 sunk pillar fills; overlapping kept brush both orders fills | capture tests | achievable as stated |
| 24 pillar 0.5 mm short reads intact ceiling | capture test | achievable as stated |
| 25 embedded leak-guard brush reads itself | capture test | achievable as stated |
| 26 closed room: no miss, none outside | capture test | achievable as stated |
| 27 chained closed room O3 | capture test | achievable as stated |
| 28 seal fixture both backdrops; post-then-`never` wall leak guard reads post | capture test | achievable as stated |
| 29 coplanar trim both orders | capture test | achievable as stated |
| 30 placement identical filter on/off | placement test | achievable as stated |
| 31 two probes, O7 | capture test | achievable as stated |
| 32 all ≥ t: id 54 byte-identical to forced-empty; old capture tests unchanged | capture test + existing suite | achievable as stated |
| 33 oversize cut: face → brush remap, mask skips exactly the pillar's sub-faces | unit test through `plan_cut_charts` remap | achievable as stated |
| 34 end to end prl-build, with and without forced cut | integration test in `tests/`, `.map` fixture | achievable as stated |
| 35 hidden-water warning fires unfiltered, not filtered, cold and warm | capture test with log capture | achievable as stated |
| 36 override on func_group and switch | parse + classify test from `.map` text | achievable as stated |
| 37 degenerate first brush in `never` group | parse test | achievable as stated |
| 38 invalid value warns once per entity, cold/warm/no water | parse test with log capture + stage test | achievable as stated |
| 39 authored func_group no marker warning; TB group with empty id still warns | parse test with log capture | achievable as stated |
| 40 worldspawn `never` inert; `_tb_group` / `_tb_layer` ignored with warning | parse test | achievable as stated |
| 41 key misses on override, anchors, floor ratio, moved brush; hits otherwise; earlier stages hit | `stage_key` tests + pipeline stage-cache test | achievable as stated |
| 42 flush vs sunk: differ in key or byte-identical cold | stage test | achievable as stated |
| 43 moving one excluded hull plane or contact side misses | `stage_key` test | achievable as stated |
| 44 `--no-cache` jobs 1 vs 4 byte-identical | integration test (extend the existing worker-count test) | achievable as stated |
| 45 source gate grep | gate test or scripted grep in preflight, recorded here | achievable as stated |
| 46 FGD `func_group` + `switch` offer values; docs; no xtask in docs | grep + FGD parse test if one exists | achievable as stated |
| 47 ip-check §9 carries the line | grep | achievable as stated |
| 48 visual check on three maps, anchor retune | owner, in-engine | manual |
| 49 cold/warm probe-stage time on e24 before/after | owner's terminal (long bake) | manual |

## Tasks

| # | Task | Owner | Depends on | Status |
|---|---|---|---|---|
| 1 | **First slice, filter and leak guard.** Build the face → brush table in `extract_geometry`'s walk and remap it through the oversize cut. Give `closest_hit` a per-call face mask, a nearest-skipped-hit report and a min distance (`fit_box` and `segment_clear` stay unmasked). Split `radiance` into trace and shade. Add hull exit classification (O3 rules 1, 3, 4) and the leak guard with the box test (O6). Hard-wire exclusion of one pillar in a pool fixture under a ceiling. Proves ACs 17, 21, 22 (partial), 25, 33. | delegated (Opus worker), reviewed by executor | — | done: `capture_exclusion.rs`; `rays_through_an_excluded_pillar_read_the_wall_behind_from_the_capture_point` (17), `the_wall_behind_an_excluded_pillar_keeps_its_sdf_shadow` (21), `texels_no_ray_sent_into_the_pillar_capture_bit_identically_when_it_is_excluded` (22 partial), `an_excluded_cap_embedded_in_a_kept_beam_reads_its_own_surface` (25), `oversize_cut_remaps_each_sub_face_to_its_parent_brush_and_the_mask_follows` (33), `rays_through_a_pillar_short_of_the_ceiling_re_trace_onto_the_intact_ceiling` (24 at 2 mm); `--bin prl-build` 1708 passed, clippy clean |
| 2 | **First slice, fill.** Contact candidates per probe, the covering kept side (O3 rule 2, O9, O11, O12), and synthesized `SurfacePoint` lighting from the nearest coplanar post-cut face (O8), with material and UV via the widened `compute_texel_uv`. If the fill proves infeasible, stop, apply the leak guard everywhere and tell the owner (Decisions). Proves ACs 18–20, 22–24, 26–29. | integrating executor | 1 | |
| 3 | **Classification module.** t(W), W, in-plane size and normal thinness against a reflector normal, the floor constant with drift and HFOV pin tests, and the underside test over post-cut faces and bound water. Pure functions in a new `reflection_probes/exclusion.rs` (or a sibling). Proves ACs 1–16. | delegated (Opus worker), reviewed by executor | — (contract: face → brush table type from 1) | |
| 4 | **Override authoring.** `ReflectionCapture` on `MapData`. Read `_reflection_capture` in parse.rs's `func_group` (no `_tb_type`) and `switch` branches through a new parse submodule, with warnings (O5), the `_tb_type` ignore-and-warn, and the marker-warning narrowing. Translate at the retained-hull seam (O4). FGD `func_group` entry plus `switch` key; `docs/level_design.md` §Fluid reflections. Proves ACs 36 (parse half), 37–40, 46. | delegated (Sonnet or Opus worker), reviewed by executor | — | |
| 5 | **Integration.** Classification per probe feeds masks and contact candidates into `capture_section`. Pass brush volumes, the override table and the face → brush table through `ReflectionProbeInputs`. Cache key additions and `STAGE_VERSION` 10. Hidden-water warning over the filtered section. End-to-end `.map` fixture through prl-build. Proves ACs 30–32, 34–36, 41–44. | integrating executor | 1, 2, 3, 4 | |
| 6 | **Gates and durable text.** Source-gate check (45), ip-check §9 line (47), and `resource_management.md` §5.2 "decided; not built" wording updated on landing. | integrating executor | 5 | |
| 7 | Preflight, review panel, fix loop, full gate. | integrating executor | 6 | |

Tasks 3 and 4 touch disjoint files (classification module vs parse/FGD/docs) and can run concurrently with task 1. Cargo runs only in the integrating executor, sequentially.
