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
| Windows release live, 1600×900, VSync off | High 4.83 ms; half unlimited 7.83; half D30 5.82; half D15 5.07; quarter D15 5.03 | Planar is a quality tier, not a speed tier; D is the lever |
| Same | Mirror CPU: unlimited 2.75 ms (20,000-step fallback); D30 0.78; D15 0.06; R8 2.77 | D bounds the walk; R does not |
| Same, stills | Quarter D15 byte-identical to probes-only; half D15 shows near-water planar | Quarter-scale decision |
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
| Probe term holds the static world; undersides stay unreflected | §7.3a Limits | Wording updates: planar fills both within D |
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
