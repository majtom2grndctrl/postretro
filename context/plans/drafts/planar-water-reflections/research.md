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

Frame times at this pose are CPU-bound by the main walk's own step-cap trip (~2.9 ms of a ~5 ms frame), the defect `portal-walk-bounded-regions` fixes. Read the reflection GPU spans as the cost signal; frame-time comparisons wait for that brief.

Why a budget matches unlimited: a walk that trips its step limit discards its partial set and frustum-culls every non-solid cell in the mirror frustum. Any budget that trips yields the unlimited set. The open risk is the opposite pose, where the full walk finishes seeded with a tight set that budget 0 would widen.

Measurement state for the walk-budget runs: engine window visible and unoccluded, never foreground (Windows withholds focus from a background launch); GeForce overlay present in most samples. The earlier bounded-live controls reproduce within ~2%, so they likely ran in the same state.
| Windows capture, 4K across lake | Half 13.8 ms vs High 18.3 vs Medium 14.0 | Large-water GPU benefit grows with coverage |
| Windows capture, e24 and probe room | Mirror-only cells 4–18, zero lightmap misses | Residency unions hold |
| Mac Metal trace, 960×540 | Reflection GPU work below High, completion slower | Mac native run is a manual row |

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

Engineering triage, not legal advice. Sources: `plans/done/fluid-ssr-reflections/ip-check.md`, `research/gi-rtx-lite/index.md`, spike contract IP rows.

| Reference | Fence | How the brief honours it |
|---|---|---|
| NetEase CN117496039A | Static from probe, dynamic from planar | Mirror always draws the static world; planar/probe split by D and R only |
| Tencent CN116778049B | Multi-threaded mirror pipeline | Single encoder, single-threaded recording |
| Microsoft US10586375B2 family | Direction-based selection or weighting; `ip-check.md` W4 warns per-pixel technique selection reads closer to "adaptively selecting" | No Fresnel or view-direction cut in planar passes. Routing is per surface by plane identity, and each surface keeps its blend |
| Tencent WO2026113651A1 claim 15 | Adding content where the capture shadow fires | Planar reads no s, probe colour, distance tile or probe binding |
| NetEase CN114748874B | Probe lookup seeded by another layer's hit | Probe lookup unchanged |

## Adjacent plans

| Plan | Relation |
|---|---|
| reflection-probe fluid-pass performance (cell-rate spike on `claude/cell-rate-spike-1660`, undrafted) | Rewrites fluid shaders and pipelines; inherits per-draw routing after this lands |
| reflection-probe streaming (`claude/probe-streaming-brief-364a5e`) | Non-resident probe ⇒ no reflecting variant ⇒ no planar on that water; mirror cells stay out of probe demand |
| reflection-probe seams | Independent; blended probe slots into the composite unchanged |
| ghost-entities (in progress) | Not drawn in the mirror; pass order recorded in plan |
| ssr-decals (placeholder) | Owns non-fluid puddles |
