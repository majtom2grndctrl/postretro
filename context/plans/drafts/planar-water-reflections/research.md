# planar-water-reflections — research

Derivation behind the brief. Read at a0a2c0239; spike at `claude/planar-water-spike`.

## Spike sources

On `claude/planar-water-spike`, under `context/plans/in-progress/`:
- `planar-water-spike-contract.md`: decisions 1–14, IP rows, invariants (plane, oblique raster, D/R/fade definitions, per-view state, pass order, timing pairs).
- `planar-water-spike-findings.md`: cross-platform synthesis.
- `planar-water-spike-drafting-handoff.md`: owner's product direction and the decisions it left to drafting.
- `planar-water-spike-evidence/windows-bounded-live-2026-10-23/findings.md`: the release live matrix that sets the presets.
- `planar-water-spike-evidence/windows-2026-10-23/`: captures, coverage and tiers, DX12, FXC, test logs. Images and files over 1 MB are listed in `NOT-IN-GIT.md` and live only in the Windows spike worktree.
- `planar-water-spike-evidence/knob-sweep/`, `metal-2026-10-22/`: Mac 5300M.

## Measurements that set decisions

Do not merge numbers across these regimes: synchronized capture, dev/VSync live, release live, and Metal trace each measure something different.

| Regime | Finding | Drives |
|---|---|---|
| Windows release live, walk budget (`windows-walk-budget-2026-10-23/`) | High 4.73 ms; very_high 5.07; half unlimited 7.68 (walk 2.69); budget 2,000 5.23 (0.28); budget 500 5.00 (0.07); budget 0 4.97 (0); D15 4.96 (0.06). Reflection GPU: High 0.95, very_high 1.75, planar unlimited 0.21 | Walk is the whole extra mirror cost; drawing at full reach is cheap. The budget/frustum mechanism itself is rejected (fallback as normal path) |
| Same, stills | Budgets 0/500/2,000 byte-identical to unlimited; D15 differs from probes-only by 2 pixels | Full reach is free once the walk is bounded; D15 reaches nothing from a standing lake pose |
| Earlier bounded live (`windows-bounded-live-2026-10-23/`) | High 4.83; half unlimited 7.83; D30 5.82; D15 5.07; R8 keeps the walk cost | Superseded as a preset basis; its "broad near-water region" at half D15 is not in the pixel data |
| Windows capture, 4K across lake | Half 13.8 ms vs High 18.3 vs Medium 14.0 | Large-water GPU benefit grows with coverage |
| Windows capture, e24 and probe room | Mirror-only cells 4–18, zero lightmap misses | Residency unions hold |
| Mac Metal trace, 960×540 | Reflection GPU work below High, completion slower | Mac native run is a manual row |

Frame times at this pose are CPU-bound by the main walk's own step-cap trip (~2.9 ms of a ~5 ms frame), the defect `portal-walk-bounded-regions` has since fixed. Read the reflection GPU spans as the cost signal; frame-time comparisons need a rerun on the landed walk.

Why a budget matches unlimited: a walk that trips its step limit discards its partial set and frustum-culls every non-solid cell in the mirror frustum. Any budget that trips yields the unlimited set. The open risk is the opposite pose, where the full walk finishes seeded with a tight set that budget 0 would widen.

Measurement state for the walk-budget runs: engine window visible and unoccluded, never foreground (Windows withholds focus from a background launch); GeForce overlay present in most samples. The earlier bounded-live controls reproduce within ~2%, so they likely ran in the same state.

## Coexistence premise

- The fluid fragment reads one screen layer, `reflection_target` (group 5 binding 2). Pins in `fluid_surface_tests.rs` fix the reflecting entry body, `REFLECTION_GLOBALS`, the group-5 inventory, and its entry count. A second layer per fragment means a new binding plus shader and test edits, which reopens IP review. Rejected.
- `record_planar_material_range` already splits each material range by plane height (`split_planar_ranges`) and sets `FLUID_SCENE_INPUTS_GROUP` per part. Routing selected parts to a planar group and the rest to the SSR group needs no shader edit.
- `FluidSceneInputs::with_reflection_target` reuses one uniform buffer, so all groups share one `ssr_scale`. Smooth style samples at `frag_xy / extent` and does not care; pixel style does. Hence a second uniform.
- SSR has no per-pixel exclusion besides the probe gate. Skipping the selected plane is a CPU split in `record_reflection_draws`.
- Binding budget: fluid fragment stage 11/16 sampled textures, 8/8 storage buffers (`rendering_pipeline.md` §10). Routing adds neither.

## Commitments touched

| Commitment | Where | Brief's stance |
|---|---|---|
| No render-to-texture for mirrors | `resource_management.md` §9 | Narrowed to one mirrored water view per frame |
| Planar rejected for fluid reflections | `plans/done/fluid-ssr-reflections` Path, `ip-check.md` D8 | Superseded |
| Composite `F·[c·SSR + (1−c)·s·R]`, c = reach × edge fade | `rendering_pipeline.md` §7.3a | Holds per surface; X is planar on the selected plane |
| Tiers are a cost lever; lower never reaches further | `player_options.md` §4 | Planar at very_high and ultra |
| Probe term holds the static world; undersides stay unreflected | §7.3a Limits | Wording updates: planar fills both on the selected plane |
| Non-portal paths demand only the camera cell's baked set, nothing new | `rendering_pipeline.md` §4 | Mirror adds no reads on any fallback |
| No pop-in: a tier is valid only when every camera cell's mandatory set fits | `large-map-spatial-residency.md` | Mirror demand outside the baked set is over-inclusion, gated below |
| Bake gap is gated, not widened | `portal-walk-bounded-regions` Decisions | Mirror over-inclusion counts against the same gate |
| No ordinary pose reaches the step-cap fallback | `portal-walk-bounded-regions` Problem | Mirror rides that walk; cap is a backstop |
| `reflections.style` sets every fluid reflection layer; style is the author's | `scripting.md`, `player_options.md` §4 | Planar resolve applies SSR's style tilt |
| Screen-space march needs owner sign-off | `index.md` §1.1 | Not triggered: planar is raster |
| 2 GB resident VRAM | `index.md` §1.1 | Manual memory row |

## IP map

Engineering triage, not legal advice. The pixel tilt has its own chart, paths and design rules in sibling `ip-check.md`. Sources: `plans/done/fluid-ssr-reflections/ip-check.md`, `research/gi-rtx-lite/index.md`, spike contract IP rows.

| Reference | Fence | How the brief honours it |
|---|---|---|
| NetEase CN117496039A | Static from probe, dynamic from planar | Mirror always draws the static world; planar/probe split by D and R only |
| Tencent CN116778049B | Multi-threaded mirror pipeline | Single encoder, single-threaded recording |
| Microsoft US10586375B2 family | Direction-based selection or weighting; `ip-check.md` W4 warns per-pixel technique selection reads closer to "adaptively selecting" | No Fresnel or view-direction cut in planar passes. Routing is per surface by plane identity, and each surface keeps its blend |
| Tencent WO2026113651A1 claim 15 | Adding content where the capture shadow fires | Planar reads no s, probe colour, distance tile or probe binding |
| NetEase CN114748874B | Probe lookup seeded by another layer's hit | Probe lookup unchanged |

## Post-walk refresh (2026-10-25, at a5937add6)

Read after `portal-walk-bounded-regions` landed. Spike facts are from `claude/planar-water-spike` 8201dfb8e.

| Fact | Source | Drives |
|---|---|---|
| A tripped or portal-less mirror walk keeps every non-solid, non-exterior cell in the mirror frustum, then marks them drawn, so the fallback issues lightmap reads | spike `mirror_visibility::determine_with_step_limit`, `render_preparation/planar.rs::prepare`, `lightmap_streaming/demand.rs::update` | Trip set Decision |
| The seeded entry starts every usable seed at full screen; the caller owns eye, view-projection and near plane | `region_walk/entries.rs::portal_walk_seeded` | Crop Decision |
| No projected screen bounds exist; selection scores world-space faces against the main frustum only, so a plane behind a wall can win with no seeds | spike `render-cpu/planar_reflection.rs::select_plane` | Crop is new work |
| Pixel tilt is an angle: per-axis offset at most 2, times 0.05, so the normal turns at most ~0.2 rad and the ray ~0.4 rad; landing distance scales with mirror depth | `fluid_reflection_ray.wgsl`, `fluid_motion.wgsl`, `ip-check.md` P2 | Crop pad is angular |
| Bake-gap fixtures (hallway, movement-feel) hold no water; a planar-on run there selects no plane | `done/portal-walk-bounded-regions/` gate definition | Water fixtures for the gate |
| Drawn cells outside id 51 are never-refused visible demand; id 51 is never widened for over-inclusion; widening was measured as growing the mandatory set (2026-10-13 ruling) | `rendering_pipeline.md` §2, §4; `done/portal-walk-bounded-regions/research.md` | Lever order; no id-51 widening |
| Lightmap settle set holds every block the pose draws on any path; SH settles its mandatory tier only. The held Settling frame walks only the camera today, and the spike passes no mirror cells there | `boot_sequence.md` §1; `app/world_less_frame.rs` | Settle row is lightmap-only; Settling runs the mirror (O2) |
| Camera walk quiet R1, epoch 3: lake 1,511 tests max, 238 µs p99; worst map 2,021 tests (occlusion-test), cap 2.47×; seeded fixture worst 771 | `done/portal-walk-bounded-regions/plan.md` | Cap-headroom row |
| The owner's Mac lake run was GPU-bound (102 of 106 slow windows in `wait_acquire`) | same plan, owner run | Crop's value is fewer cells and draws, not CPU alone |
| The walk's capture A/B saw the dynamic-light term black at every pose, so shadow-slot competition was never exercised | same plan, capture A/B | Mesh and shadow occupancy row has no inherited baseline |

## Terms

| Term | Meaning (spike contract) |
|---|---|
| D | Reflected path length \|e′ − p\| in metres, p the reflected point from mirror depth; fades c over a 4 m band and, when set, replaces the mirror's far plane. Off means unlimited |
| R | 3D distance \|e − w\| in metres, w the main-camera ray's hit on the plane; fades c over a 4 m band. Off means unlimited; it did not cut walk cost |

## Ordering pins

| id | scenario | ordering | expected outcome |
|---|---|---|---|
| O1 | Camera walk and mirror walk in one frame | Camera walk, then each candidate's seed cells from this frame's camera reach, then selection, then the crop, then the mirror walk on its own scratch, then the unions for demand and lighting | The main view's visible and fog sets are identical with planar on and off at the same pose |
| O2 | Settling frame at a pose with a selected plane | Held frame: O1's order, all from the held pose, before the streaming step; mirror cells enter the drawn set Settling demands | Settle set holds the mirror's blocks; the reveal frame selects the same plane and draws zero lightmap misses |
| O3 | Settling frame with the main view on a fallback and the mirror walk completed | Mirror cells join the Settling drawn set as the main view's drawn cells do; the no-new-reads rule applies from reveal | Settle set holds the mirror's blocks on that path too |
| O4 | Selection switches from plane A to plane B on frame N | Selection, mask, draw routing, SSR skip, crop and mirror walk all read frame N's selection | On frame N, B reads this frame's mirror and A reads SSR; no frame shows A's mirror on B |
| O5 | Selection switches on the frame the mirror walk trips | Trip set formed after selection, from the new plane's cropped mirror frustum | Draws only camera-visible cells in B's mirror frustum; no new reads |
| O6 | Mirror recording against the fluid pass in one frame | Planar pass records on the frame encoder before the fluid pass that samples its target | The selected plane shows this frame's mirror |
| O7 | A mirror-only light holds a shadow slot from frame N−1; on frame N a main-view light needs it and scores under the eviction margin | Mirror-only lights rank after every main-view admission, incumbency included | The main-view light takes the slot on frame N |
| O8 | Tier changes between frames | The tier is read once per frame, before selection; selection, mirror walk, demand, routing and recording that frame use it | No frame walks the mirror for a tier that does not draw it |
| O9 | Resize lands on the frame the planar target is first allocated | Extent commits at frame start; the target, the group carrying it and its scale build after it, before the fluid pass | One allocation at the new extent; that frame's selected-plane draws read it at its own scale |
| O10 | Selection drops for a frame at very_high (eye enters the fog band, or the plane leaves view) | The target stays while the tier is planar and the level has a candidate plane | No reallocation; the planar pass records nothing that frame |
| O11 | Level change or fluid refresh with a plane held | Candidates and selection history reset at install, before the first frame | The first frame selects without hysteresis from the old candidate index |
| O12 | A higher-scoring plane in the frustum has no seed cell in the camera's reach | Candidates are filtered by seeds before scoring and hysteresis | The seeded plane is selected; a held plane without seeds yields that frame |
| O13 | Held plane scores zero (out of frustum) while a challenger scores | Hysteresis applies only to a held plane with a non-zero score | Challenger selected that frame |
| O14 | Main view on a fallback path | The fallback's drawn set is the reach the seed filter and the mirror's seeds read, taken after the camera walk and before scoring | A plane in view stays selectable; selection may move to a plane the camera cannot see that frame, and yields on the next walk-path frame |
| O15 | A light or instance's reach changes between frames, or the main view reaches an instance only as a shadow caster while the mirror draws it | Tags come from frame N's camera and mirror reaches, before shadow ranking and the instance planner; any main-view reach, shadow casting included, tags it main-view | A main-view shadow caster keeps its main-view rank with planar on; a light that turned mirror-only on frame N has no incumbency against a main-view light on frame N |

## Adjacent plans

| Plan | Relation |
|---|---|
| reflection-probe fluid-pass performance (cell-rate spike on `claude/cell-rate-spike-1660`, undrafted) | Rewrites fluid shaders and pipelines; inherits per-draw routing after this lands |
| reflection-probe streaming (`claude/probe-streaming-brief-364a5e`) | Non-resident probe ⇒ no reflecting variant ⇒ no planar on that water; mirror cells stay out of probe demand |
| reflection-probe seams | Independent; blended probe slots into the composite unchanged |
| ghost-entities (done) | Not drawn in the mirror; pass order recorded in plan |
| ssr-decals (placeholder) | Owns non-fluid puddles |
