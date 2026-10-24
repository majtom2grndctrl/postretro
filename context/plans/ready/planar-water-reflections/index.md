# planar-water-reflections

Brief · resumable · reads: `context/lib/rendering_pipeline.md` §2, §4, §7.3a, §7.8, §10 · `context/lib/player_options.md` §4 · `context/lib/resource_management.md` §5, §9 · `context/lib/scripting.md` (reflections style) · `context/plans/large-map-spatial-residency.md` · read at a0a2c0239 (spike at `claude/planar-water-spike` 8201dfb8e)

## Problem

Requested capability, from the owner after the planar water spike. Today large water reflects through a compile-time probe plus SSR, and SSR loses anything that leaves the main screen, so nearby walls, posts, movers and characters vanish from a lake as the camera turns. The spike showed a mirrored second view over one water plane fixes that at full reach, and that drawing it is cheap: at one lake pose on the GTX 1660, its reflection GPU work is 0.21 ms against SSR very_high's 1.75. Its extra cost was the mirror's portal walk, which the decided bounded-region walk replaces. When this is done, a player at `reflection_quality` very_high or ultra sees the largest visible water plane reflect the room, its movers, characters, sprites and particles from a mirrored view, whether or not they are on screen, in the author's chosen reflection style. Every other reflecting water surface, puddles included, keeps SSR plus probe. Tiers high and below look and cost exactly as today.

## Decisions

- **One mirroring system per surface.** The selected plane reflects planar plus probe; every other reflecting face reflects SSR plus probe, and the SSR pass skips the selected plane's draws. The §7.3a composite `F·[c·X + (1−c)·s·R]` holds per surface, with X planar on the selected plane and SSR elsewhere.
- **Planar at very_high and ultra only.** It shows what very_high's SSR shows within the mirror's content, plus off-screen content, so a lower tier never reaches further (`player_options.md` §4). Planar at high waits on new measurements.
- **Layer chosen per draw; fluid shaders unedited.** Selected-plane draws bind a group-5 scene-inputs group carrying the planar target, others the SSR group, as the spike's `split_planar_ranges` routing already does. Each group carries its own scale uniform, since pixel style reads a layer at its own scale. `fluid_surface.wgsl`, `fluid_reflection.wgsl`, the CPU reference in `render-cpu/src/fluid_surface/` and their text pins stay unedited.
- **The planar layer follows `reflections.style`.** Style is the author's look (`scripting.md`), so a tier never changes it. In pixel style the planar pass tilts each water cell's ray exactly as SSR and the probe term do, then samples the mirror where that ray lands; tilt moves where it samples, never c. Smooth style tilts nothing, and the pass rasterizes the selected plane's draws because a full-screen resolve has no per-cell inputs.
- **Mirror visibility rides the bounded-region walk.** The mirror floods from its seed cells through the seeded entry of `portal-walk-bounded-regions`; the step cap is a backstop, not a path. Reach is full by default; D stays an engine mechanism, off by default.
- **Mirror demand follows the path rules.** On a walk path, mirror cells join mesh and mover collection, SH sample regions, lightmap demand and animated-lightmap compose; on any fallback they add no new reads, and they never request probes (`rendering_pipeline.md` §4). Their over-inclusion counts against the bake-gap gate, and a fixture that fails it stops the build for an owner decision, with D as the first lever.
- **Presets are engine constants.** Both tiers start at half scale, D and R off, and the owner retunes them late in the build from a motion read. No preset becomes a player setting or map key; the spike's env seeds, dev-panel sliders and capture controls stay dev-tools or capture only.
- **Selection stays automatic, behind an engine-owned area floor.** The spike's `select_plane` scores projected solid angle with 1.25× hysteresis, so without a floor a puddle near the eye can win. Planes below the floor never compete. No mapper override for now; one in the `_reflection_capture` pattern can follow if content needs it.
- **No plane, no planar.** Eye below the plane or within `FLUID_FOG_BAND`, no plane above the floor, or a tier below very_high: that water renders SSR plus probe at the tier. Planar keeps the reflecting gate's probe requirement; the probe fills pixels the mirror misses.
- **Mirror view content.** Static world always, plus movers, skinned meshes, billboards and particles. No fog or smoke raymarching, ghosts, SSR, bloom or viewmodel; unfogged reflections are a known gap with their own roadmap item.
- **IP rules.** The spike contract's rows carry forward: static world always drawn and any planar/probe split by distance only (NetEase CN117496039A); single-threaded recording on the frame encoder (Tencent CN116778049B); no Fresnel or view-direction cut, routing by plane identity never per pixel (Microsoft US10586375B2); no s, probe colour, distance tile or probe binding (Tencent WO2026113651A1 cl. 15). The pixel tilt follows `ip-check.md` D1–D9, path P2.
- **Counsel gates build start, not promotion.** `ip-check.md` §For counsel completes before the build begins; the brief waits behind `portal-walk-bounded-regions` either way.
- **Planar target lifecycle mirrors SSR's.** Allocated at scene extent times preset scale only at a planar tier with a selected plane; rebuilt on resize; released at lower tiers and unload. Tier changes compile nothing.
- **Spike records land with the change.** The spike's contract and findings move to `plans/done/planar-water-spike/`, and source comments citing the spike contract repoint to `context/lib/`.
- **Sequence.** After `portal-walk-bounded-regions`; before reflection-probe fluid-pass performance, which carries per-draw routing into its own draw shape.
- **Non-goals.** Fog in the mirror: own roadmap item. Multiple planes, non-horizontal mirrors, planar on non-fluid surfaces: one plane per frame is the cost cap. Non-fluid puddles and wet floors: `ssr-decals`. Water with no probe or a non-resident one: probe streaming owns the miss policy.

## Acceptance

### Automated
- [ ] The spike's planar CPU, visibility, renderer and app tests pass on the build, with a non-zero test count per filter.
- [ ] Existing `fluid_` renderer tests pass with no edits, and the diff touches no file under the protected fluid shader and CPU-reference paths.
- [ ] Shader source-scan IP tests cover the planar-authored shader text (not composed shared modules): no Fresnel term, no `fluid_probe` binding, no call to `fluid_reflection_gate`, `fluid_reflection_seen_from_above` or `fluid_reflection_cell_within_box`, and no direction quantity in a comparison or `select`. `dot(` is allowed only inside the composed, pinned helpers.
- [ ] FXC compiles every full-init pipeline, run explicitly on Windows.

**One frame, two layers**
- [ ] At very_high, one GPU frame holding a large plane and a second reflecting plane at another height: the large plane reads the planar layer (an asymmetric object lands on the mirrored pixel) and the second plane reads SSR.
- [ ] The SSR pass records for the second plane and spends nothing on the selected plane's draws in that frame.
- [ ] Pixel style: each layer is read at its own target scale when planar and SSR scales differ.
- [ ] Pixel style: for the same cell, clock and texels-per-pixel, the planar pass's tilted ray equals the SSR pass's ray (CPU reference). On one GPU frame, an asymmetric reflected edge on the selected plane and on an SSR plane at the same height shifts in the same direction and by the same texel count, away from depth edges. Smooth style: the planar layer applies no tilt.
- [ ] Tilt never changes c: at a fixed pose, c over the selected plane matches the same frame with the offset forced to zero, except where the tilted lookup lands on an empty mirror texel.
- [ ] A billboard and a particle above the selected plane land on their mirrored pixels, facing the mirror camera.

**Mirror visibility and demand**
- [ ] On the lake, e24 and probe-room fixtures at very_high, no ordinary pose trips the mirror walk's step cap.
- [ ] A pose whose main view is on a fallback path: the mirror adds no lightmap or SH reads; already-resident mirror blocks still draw; misses are counted.
- [ ] A forced mirror step-cap trip adds no new reads and logs no SolidCellFallback.
- [ ] A pose with mirror-only cells on a walk path: those cells reach lightmap demand, SH sample regions and compose with zero visible lightmap misses, and request no probe.
- [ ] The bake-gap gate holds with planar on: deferred lightmap blocks on the gate's fixtures do not exceed the same build with planar off.
- [ ] Mesh-instance cap and shadow-ranking occupancy with and without the mirror view, at the same poses, are reported in the plan of record.

**Selection**
- [ ] A plane below the area floor is never selected, even when nearer and larger in projected solid angle than the lake. A plane just above the floor is selectable.
- [ ] Eye below the plane, and eye inside the fog band: no plane selected; that water reads SSR at the tier.
- [ ] Hysteresis: a challenger below 1.25× the held score does not switch the plane; one above it does.

**Tiers and lifetime**
- [ ] At high and below, no planar pass records, no planar target is allocated, and a capture matches the same capture without this change byte for byte.
- [ ] Switching high → very_high → high creates no pipeline, allocates the planar target, then releases it. Resize rebuilds it at the new extent. Unload releases it.
- [ ] Zero selected planes at very_high: the planar pass records nothing and SSR covers all water.

### Manual
- [ ] Owner motion read on `reflection-lake-test` and `e24` at very_high against high, in pixel and smooth style: plane switch, grazing precision, waterline seam, shimmer, movers, skinned meshes, sprites and particles, posts and far walls as they leave the main screen. The owner fixes the presets; the plan of record records them.
- [ ] Windows GTX 1660, release, VSync off, 1600×900, reverse-order repeats, engine window unoccluded: frame time, mirror CPU and reflection GPU at very_high with planar against very_high with SSR only, on lake across and down poses and one e24 pose with movers and particles. Report, don't gate.
- [ ] Mac 5300M: a native bounded run reaches world frames at very_high with planar. Report frame time against SSR-only very_high. A failed native reveal is a finding, not a skip.
- [ ] Resident memory at 4K, ultra, lake pose: planar colour and depth targets plus per-view cull buffers, counted against the 2 GB pool.

## Path

- **Start from the spike.** Its renderer side merges cleanly onto a0a2c0239; recheck after `portal-walk-bounded-regions` lands, since it rewrites `portal_vis.rs`. Keep `MirrorCamera`, `select_plane`, `determine_mirror_visible_cells`, the mirror cull and mask, and the residency unions. Port `portal_traverse_seeded` onto the new walk's seeded entry. Drop or gate the scaffolding: `POSTRETRO_PLANAR_*` seeds, `launch_settings.rs`, the dev-panel header, and the SSR early return on `planar_on`. Evidence stays on the spike branch; images and large per-run files live only in the Windows spike worktree (`NOT-IN-GIT.md`).
- **Riskiest assumption first.** A second group-5 bind group with its own uniform must pass the source-text pins on `record_draws`, `rebind_reflection_target`, bind-group creation counts and the "exactly two `scene_inputs.rebuild(` callers" pin. Build routing and the two-layer GPU test before anything else.
- **Rivals.** Planar blended over a full SSR layer on the selected plane: two mirroring systems on one surface; rejected by the owner. A step-budgeted walk whose frustum fallback is the mirror's normal path: matched the unlimited image at one pose, but makes a fallback ordinary and widens never-refused demand; rejected.
- **Pixel tilt.** `ip-check.md` §The tilt as built today maps SSR's helpers; path P2 there is the chosen one. Its rival P1, a DuDv-style screen offset, is low risk too but only approximates SSR's tilt. The spike's full-screen resolve has no UV, texture size or derivatives, so the planar resolve becomes a raster pass over the selected plane's fluid draws.
- **Billboards and particles** are camera-facing. Check where their facing basis enters the draw; the mirror pass needs it from the mirror camera.
- **Area floor.** Measure on the plane's total up-facing area, not per face. Puddle-scale and lake-scale fixtures bracket the value.
- **Split first.** `frame_loop/mod.rs` and `debug_ui/mod.rs` are past 800 lines; extend through new sibling files as the spike did.
- **Ghost entities** is in progress. Keep its pass out of the mirror and record the ghost pass's order relative to the mirror and fluid passes in the plan.
- `research.md` holds the measurements, commitment map and IP map behind these decisions.

## Open questions

- Area-floor value — **delegated**: executor proposes from fixtures and reports it in the plan of record.
- Where the spike's capture-scene `planar_reflection` controls live (§7.8 capture fields, or retired) — **delegated**.
