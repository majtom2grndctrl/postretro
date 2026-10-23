# planar-water-reflections

Brief · resumable · reads: `context/lib/rendering_pipeline.md` §2, §7.3a, §7.8, §10 · `context/lib/player_options.md` §4 · `context/lib/resource_management.md` §5, §9 · read at a0a2c0239 (spike at `claude/planar-water-spike` 8201dfb8e)

## Problem

Requested capability, from the owner after the planar water spike. Today large water reflects through a compile-time probe plus SSR, and SSR loses anything that leaves the main screen, so nearby walls, posts, movers and characters vanish from a lake as the camera turns. The spike showed a mirrored second view over one water plane fixes that at full reach. Its cost was the mirror's portal walk, not drawing. On the GTX 1660 at 1600×900, unlimited-reach planar with the walk skipped runs 4.97 ms/frame against SSR very_high's 5.07 and High's 4.73, with a byte-identical image to the full walk. When this is done, a player at `reflection_quality` very_high or ultra sees the largest visible water plane reflect the room, its movers, characters, sprites and particles from a mirrored view, whether or not they are on screen. Every other reflecting water surface, puddles included, keeps SSR plus probe. Tiers high and below look and cost exactly as today.

## Decisions

- **One mirroring system per surface.** The selected plane reflects planar plus probe; every other reflecting face reflects SSR plus probe. The SSR pass skips the selected plane's draws. The §7.3a composite `F·[c·X + (1−c)·s·R]` holds per surface, with X the planar layer on the selected plane and SSR elsewhere.
- **Planar at very_high and ultra only.** On the selected plane it shows what very_high's SSR shows within the mirror's content, plus off-screen content, at slightly less frame cost; `player_options.md` §4's rule that a lower tier never reaches further holds. Planar at high is a later decision on new measurements.
- **Per-draw layer routing; fluid shaders unedited.** Selected-plane draws bind a group-5 scene-inputs group carrying the planar target; all others bind the SSR group, as the spike's `record_planar_material_range` / `split_planar_ranges` already do with a zero group. Each group carries its own scale uniform, because pixel style reads the layer at the target's own scale and `FluidSceneInputs::with_reflection_target` shares one uniform today. `fluid_surface.wgsl`, `fluid_reflection.wgsl`, the CPU reference in `render-cpu/src/fluid_surface/` and their text pins stay unedited.
- **Full reach; the walk is bounded, not the distance.** Mirror visibility runs the seeded walk under a small step budget, then the mirror-frustum cull. A walk that trips its limit already discards its partial set and draws the frustum set, so a budgeted walk yields the same image as an unbounded one, at ~0.07 ms instead of ~2.7. The budget keeps tight seeded sets where the walk finishes early. D and R stay as engine mechanisms, off by default.
- **Presets are engine constants, not knobs.** Both tiers start at half scale, D and R off. The owner retunes them late in the build from a motion read. Scale, D, R, bands and the walk budget never become player settings or map keys; the spike's env seeds, dev-panel sliders and capture-scene `planar_reflection` controls stay dev-tools or capture only.
- **Selection stays automatic, behind an engine-owned area floor.** The spike's `select_plane` scores projected solid angle with 1.25× hysteresis, so a puddle near the eye can win. A plane whose total up-facing area falls below the floor never competes and stays SSR plus probe. No FGD key and no mapper override: the mapper surface stays closed until a content need is shown.
- **No plane, no planar.** With no selected plane — eye below the plane or within `FLUID_FOG_BAND` of it, no plane above the floor, the tier below very_high — that water renders SSR plus probe at the tier. Planar keeps the reflecting gate's probe requirement (`fluid_reflection_gate`, `FluidParamsRow::shader_variant`); the probe term fills pixels the mirror does not cover.
- **Mirror view content.** The static world always, plus movers, skinned meshes, billboards and particles. Not fog or smoke raymarching, ghosts, SSR, bloom or the viewmodel. Unfogged reflections over fogged rooms are a known gap with its own roadmap item. Recording stays on the frame's single encoder.
- **IP rows carry forward from the spike contract.** NetEase CN117496039A: static world always drawn; any planar/probe split by distance only. Tencent CN116778049B: single-threaded recording. Microsoft US10586375B2: mask, resolve and weight cut on R, occlusion, coverage and D, never Fresnel or view direction; routing is per surface by plane identity, never per pixel. Tencent WO2026113651A1 claim 15: planar reads no s, probe colour, distance tile or probe binding.
- **Mirror-visible cells join lighting residency, not probe demand.** They union into mesh and mover collection, SH sample regions, lightmap block demand and animated-lightmap compose, as in the spike. They never request probes, so probe streaming's per-camera-cell mandatory fit (`large-map-spatial-residency.md`) is untouched. They do add lightmap and SH demand, so the mandatory-set fit that makes a tier valid is checked with mirror cells counted. Frustum-culled mirror sets are wider than walked ones, and extra cells press on capped consumers — the mesh-instance cap and shadow ranking (`rendering_pipeline.md` §2: extra cells are measured, not assumed free).
- **Planar target lifecycle mirrors SSR's.** Allocated at the scene extent times the preset scale only at a planar tier with a selected plane, rebuilt on resize, released at lower tiers and unload. Tier changes compile nothing; planar pipelines exist from full init.
- **Reopened commitments land with the change.** `resource_management.md` §9's "no render-to-texture for mirrors" narrows to permit one mirrored water view per frame. The planar rejection in `plans/done/fluid-ssr-reflections` (`ip-check.md` D8) is superseded by name. §7.3a gains the planar layer, routing, floor and IP rows, and its Limits wording drops "undersides stay unreflected" for the selected plane; `player_options.md` §4 gains the tier mapping. The spike's contract and findings move to `plans/done/planar-water-spike/` with the brief, and source comments that cite the spike contract repoint there.
- **Lands before reflection-probe fluid-pass performance.** That later brief inherits per-draw layer routing into whichever draw shape it picks.
- **Non-goals.** Fog in the mirror: its own roadmap item, measured separately. Ripple-normal distortion of the planar lookup: it edits the pinned reflecting entry and needs IP review, so it is its own roadmap item. Multiple planes, non-horizontal mirrors and planar on non-fluid surfaces: one plane per frame is the cost cap. Non-fluid puddles and wet floors: `ssr-decals` owns them. Planar on water bound to no probe, or whose probe is not resident: probe streaming owns the miss policy.

## Acceptance

### Automated
- [ ] The spike's planar CPU, visibility, renderer and app tests pass on the build, with a non-zero test count per filter.
- [ ] Existing `fluid_` renderer tests pass with no edits, and the diff touches no file under the protected fluid shader and CPU-reference paths.
- [ ] Shader source-scan IP tests cover every new planar shader: no Fresnel term, no probe binding, no view-direction dot product.
- [ ] FXC compiles every full-init pipeline, run explicitly on Windows.

**One frame, two layers**
- [ ] At very_high, one GPU frame holding a large plane and a second reflecting plane at another height: the large plane reads the planar layer (an asymmetric object lands on the mirrored pixel) and the second plane reads SSR.
- [ ] The SSR pass records for the second plane and spends nothing on the selected plane's draws in that frame.
- [ ] Pixel style: each layer is read at its own target scale when planar and SSR scales differ.
- [ ] A billboard and a particle above the selected plane land on their mirrored pixels, facing the mirror camera.

**Mirror visibility**
- [ ] A pose whose seeded walk finishes within the budget keeps the walked set, not the frustum set.
- [ ] A pose whose seeded walk exceeds the budget yields the frustum set, and its capture matches the same pose with the budget raised past the walk's natural length, byte for byte.
- [ ] Budget 0 and the default budget both bound mirror CPU: no pose in the lake, e24 or probe-room fixtures spends more than the budget's steps.

**Selection**
- [ ] A plane below the area floor is never selected, even when nearer and larger in projected solid angle than the lake. A plane just above the floor is selectable.
- [ ] Eye below the plane, and eye inside the fog band: no plane selected; that water reads SSR at the tier.
- [ ] Hysteresis: a challenger below 1.25× the held score does not switch the plane; one above it does.

**Tiers and lifetime**
- [ ] At high and below, no planar pass records, no planar target is allocated, and a capture matches the same capture at a0a2c0239 byte for byte.
- [ ] Switching high → very_high → high creates no pipeline, allocates the planar target, then releases it. Resize rebuilds it at the new extent. Unload releases it.
- [ ] Zero selected planes at very_high: the planar pass records nothing and SSR covers all water.

**Residency**
- [ ] A pose with mirror-only cells: those cells reach lightmap demand, SH sample regions and compose, with zero visible lightmap misses, and request no probe.
- [ ] The lightmap mandatory-set fit holds at very_high on the lake, e24 and probe-room fixtures with mirror cells counted; a fixture that would overflow is reported, not silently degraded.
- [ ] Mesh-instance cap and shadow-ranking occupancy with and without the mirror view, at the same poses, are reported in the plan of record.

### Manual
- [ ] Owner motion read on `reflection-lake-test` and `e24` at very_high against high: plane switch, grazing precision, waterline seam, movers, skinned meshes, sprites and particles, posts and far walls as they leave the main screen. The owner fixes the presets; the plan of record records them.
- [ ] Windows GTX 1660, release, VSync off, 1600×900, reverse-order repeats, engine window unoccluded: frame time, mirror CPU and reflection GPU at very_high with planar against very_high with SSR only, on lake across and down poses and one e24 pose with movers and particles. Report, don't gate.
- [ ] Mac 5300M: a native bounded run reaches world frames at very_high with planar. Report frame time against SSR-only very_high. A failed native reveal is a finding, not a skip.
- [ ] Resident memory at 4K, ultra, lake pose: planar colour and depth targets plus per-view cull buffers, counted against the 2 GB pool.

## Path

- **Start from the spike.** Its renderer side merges cleanly onto a0a2c0239; main has not touched `renderer`, `visibility`, `render-cpu` or shaders since its base. Keep `MirrorCamera`, `select_plane`, `determine_mirror_visible_cells`, `portal_traverse_seeded`, the mirror cull and mask, and the residency unions. Drop or gate the scaffolding: `POSTRETRO_PLANAR_*` seeds, `launch_settings.rs`, the dev-panel header, and the SSR early return on `planar_on`. Evidence stays on the spike branch; images and large per-run files live only in the Windows spike worktree (`NOT-IN-GIT.md`).
- **Riskiest assumption first.** A second group-5 bind group with its own uniform must pass the source-text pins on `record_draws`, `rebind_reflection_target`, bind-group creation counts and the "exactly two `scene_inputs.rebuild(` callers" pin. Build routing and the two-layer GPU test before anything else.
- **Rival shape.** Planar blended over a full SSR layer on the selected plane. It keeps SSR's view of whatever the mirror skips, but runs two mirroring systems on one surface and pays both. Rejected by the owner.
- **Walk budget.** The spike's `POSTRETRO_PLANAR_WALK_STEPS` seed measured 0, 500 and 2,000 at one pose. Pick the constant from a pose where the walk finishes seeded and one where it trips.
- **Billboards and particles** are camera-facing. Check where their facing basis enters the draw; the mirror pass needs it from the mirror camera.
- **Area floor.** Measure on the plane's total up-facing area, not per face. Puddle-scale and lake-scale fixtures bracket the value.
- **Split first.** `frame_loop/mod.rs` and `debug_ui/mod.rs` are past 800 lines; extend through new sibling files as the spike did.
- **Portal walk rewrite.** The drafted `portal-walk-bounded-regions` replaces today's walk; the mirror's seeded entry (`portal_traverse_seeded`) is a second caller it must carry. Both drafts note the coupling.
- **Ghost entities** is in progress. Keep its pass out of the mirror and record the ghost pass's order relative to the mirror and fluid passes in the plan.
- `research.md` holds the measurements, commitment map and IP map behind these decisions.

## Open questions

- Area-floor value — **delegated**: executor proposes from fixtures and reports it in the plan of record.
- Walk-budget value — **delegated**: executor picks it from the two-pose measurement and reports it.
- Where the spike's capture-scene `planar_reflection` controls live (§7.8 capture fields, or retired) — **delegated**.
