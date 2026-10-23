# planar-water-reflections

Brief · resumable · reads: `context/lib/rendering_pipeline.md` §7.3a, §7.8, §10 · `context/lib/player_options.md` §4 · `context/lib/resource_management.md` §5, §9 · read at a0a2c0239 (spike at `claude/planar-water-spike`)

## Problem

Requested capability, from the owner after the planar water spike. Today large water reflects through a compile-time probe plus SSR, and SSR loses anything that leaves the main screen, so nearby walls, posts, movers and characters vanish from a lake as the camera turns. The spike showed a mirrored second view over one water plane fixes that, and that a reflected-distance limit (D) makes it affordable: on the GTX 1660 at 1600×900, half-scale D15 runs 5.07 ms/frame against SSR High's 4.83, and mirror CPU drops from 2.75 to 0.06 ms. When this is done, a player at `reflection_quality` very_high or ultra sees the largest visible water plane reflect nearby static and moving content from a mirrored view, handing distant reflection to the probe. Every other reflecting water surface, puddles included, keeps SSR plus probe. Tiers high and below look and cost exactly as today.

## Decisions

- **Planar plus probe on one selected plane, at very_high and ultra only.** Planar's frame cost sits near SSR High's, so it is a quality step, not a speed step; `player_options.md` §4 keeps tiers a cost lever, and a lower tier still never reaches further than a higher one. Moving planar down to high is a later decision on new measurements, not this brief.
- **SSR stays on.** At very_high and ultra, every reflecting face off the selected plane keeps its tier's SSR plus probe. The SSR pass skips the selected plane's draws, so no march is spent on pixels planar owns. This replaces the spike's all-or-nothing swap (`planar_override` in `reflection_pass.rs` releasing the SSR target).
- **Per-draw layer routing; fluid shaders unedited.** Selected-plane draws bind a group-5 scene-inputs group carrying the planar target; all others bind the SSR group, as the spike's `record_planar_material_range` / `split_planar_ranges` already do with a zero group. Each group carries its own scale uniform, because pixel style reads the layer at the target's own scale and `FluidSceneInputs::with_reflection_target` shares one uniform today. `fluid_surface.wgsl`, `fluid_reflection.wgsl`, the CPU reference in `render-cpu/src/fluid_surface/` and their text pins stay unedited. The §7.3a composite `F·[c·X + (1−c)·s·R]` holds per surface, with X the planar layer on the selected plane and SSR elsewhere.
- **Selection stays automatic, behind an engine-owned area floor.** The spike's `select_plane` scores projected solid angle with 1.25× hysteresis, so a puddle near the eye can win. A plane whose total up-facing area falls below the floor never competes and stays SSR plus probe. No FGD key and no mapper override: the mapper surface stays closed until a content need is shown.
- **Presets are engine constants, not knobs.** Both tiers start at half scale, D 15 m, R unlimited, 4 m fade bands. The owner retunes them late in the build from a motion read, comparing D30 for reach. Scale, D, R and bands never become player settings or map keys; the spike's env seeds, dev-panel sliders and capture-scene `planar_reflection` controls stay dev-tools or capture only.
- **Quarter scale is unexplained until proven.** Quarter-D15 renders byte-identical to probes-only at the bounded-live pose, while half-D15 shows near-water planar. The cause is found and fixed or explained before any preset uses quarter scale.
- **No plane, no planar.** With no selected plane — eye below the plane or within `FLUID_FOG_BAND` of it, no face above the floor, the tier below very_high — that water renders as SSR plus probe at the tier. Planar keeps the reflecting gate's probe requirement (`fluid_reflection_gate`, `FluidParamsRow::shader_variant`); the probe term is its fallback past D and R.
- **Mirror view content follows the spike.** It draws the static world always, plus movers and skinned meshes. It does not draw fog or smoke raymarching, billboards, particles, ghosts, SSR, bloom or the viewmodel. Recording stays on the frame's single encoder. These are the spike contract's IP rows, carried forward: NetEase CN117496039A (static world always drawn; planar/probe split by distance only), Tencent CN116778049B (single encoder), Microsoft US10586375B2 (mask, resolve and weight cut on R, occlusion, coverage and D, never Fresnel or view direction, and no per-pixel switch between techniques), Tencent WO2026113651A1 claim 15 (planar reads no s, probe colour, distance tile or probe binding).
- **Mirror-visible cells join lighting residency, not probe demand.** They union into mesh and mover collection, SH sample regions, lightmap block demand and animated-lightmap compose, as in the spike. They never request probes, so probe streaming's per-camera-cell mandatory fit (`large-map-spatial-residency.md`) is untouched.
- **Planar target lifecycle mirrors SSR's.** Allocated at the scene extent times the preset scale only at a planar tier with a selected plane, rebuilt on resize, released at lower tiers and unload. Tier changes compile nothing; planar pipelines exist from full init.
- **Reopened commitments land with the change.** `resource_management.md` §9's "no render-to-texture for mirrors" narrows to permit one mirrored water view per frame. The planar rejection in `plans/done/fluid-ssr-reflections` (`ip-check.md` D8) is superseded by name. §7.3a gains the planar layer, routing, floor and IP rows; `player_options.md` §4 gains the tier mapping. The spike's contract and findings move to `plans/done/planar-water-spike/` with the brief, and source comments that cite the spike contract repoint there.
- **Lands before reflection-probe fluid-pass performance.** That later brief inherits per-draw layer routing into whichever draw shape it picks; this brief does not wait on it.
- **Non-goals.** Ripple-normal distortion of the planar lookup: it edits the pinned reflecting entry and needs IP review, so it is its own roadmap item. Multiple planes, non-horizontal mirrors and planar on non-fluid surfaces: one plane per frame is the cost cap. Non-fluid puddles and wet floors: `ssr-decals` owns them. Planar on water bound to no probe, or whose probe is not resident: probe streaming owns the miss policy.

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
- [ ] Quarter scale: a test pins the cause of the quarter-D15 null contribution, then shows quarter producing planar coverage where half does at the same pose — or quarter is removed as a preset option and the test pins why.

### Manual
- [ ] Owner motion read on `reflection-lake-test` and `e24` at D15, compared with D30: D handoff to probe, plane switch, grazing precision, waterline seam, movers and skinned meshes. The owner fixes the presets; the plan of record records them.
- [ ] Windows GTX 1660, release, VSync off, 1600×900, reverse-order repeats: frame time, mirror CPU and reflection GPU at very_high with planar against very_high with SSR only, on lake across and down poses. Report, don't gate.
- [ ] Mac 5300M: a native bounded run reaches world frames at very_high with planar. Report frame time against SSR-only very_high. A failed native reveal is a finding, not a skip.
- [ ] Resident memory at 4K, ultra, lake pose: planar colour and depth targets plus per-view cull buffers, counted against the 2 GB pool.

## Path

- **Start from the spike.** It merges cleanly onto a0a2c0239, and main has not touched `renderer`, `visibility`, `render-cpu` or shaders since its base. Keep `MirrorCamera`, `select_plane`, `determine_mirror_visible_cells`, `portal_traverse_seeded`, the mirror cull and mask, and the residency unions. Drop or gate the scaffolding: `POSTRETRO_PLANAR_*` seeds, `launch_settings.rs`, the dev-panel header, and the SSR early return on `planar_on`. Evidence stays on the spike branch; images and large per-run files live only in the Windows spike worktree (`NOT-IN-GIT.md`).
- **Riskiest assumption first.** A second group-5 bind group with its own uniform must pass the source-text pins on `record_draws`, `rebind_reflection_target`, bind-group creation counts and the "exactly two `scene_inputs.rebuild(` callers" pin. Build routing and the two-layer GPU test before anything else.
- **Rival shape.** The planar resolve writes into the SSR target for selected-plane pixels. That ties planar's scale and lifetime to SSR's tier and needs blending to avoid erasing SSR. Rejected.
- **Area floor.** Measure on the plane's total up-facing area, not per face. Puddle-scale and lake-scale fixtures bracket the value.
- **Split first.** `frame_loop/mod.rs` and `debug_ui/mod.rs` are past 800 lines; extend through new sibling files as the spike did.
- **Ghost entities** is in progress. Keep its pass out of the mirror and record the ghost pass's order relative to the mirror and fluid passes in the plan.
- `research.md` holds the measurements, commitment map and IP map behind these decisions.

## Open questions

- Area-floor value — **delegated**: executor proposes from fixtures and reports it in the plan of record.
- Where the spike's capture-scene `planar_reflection` controls live (§7.8 capture fields, or retired) — **delegated**.
