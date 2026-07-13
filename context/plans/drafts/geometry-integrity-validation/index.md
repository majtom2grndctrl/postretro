# Geometry Integrity — Hole Root-Cause and Compile-Time Validation

> **Status:** draft — not yet reviewed. Follow-up to the shipped watertightness diagnostic.
>
> **Area:** level compiler (`postretro-level-compiler`), geometry pipeline.
>
> **Related:** `context/lib/build_pipeline.md` §PRL Compilation · `context/research/code-quality-tooling.md` §Custom repo-specific lint rules.

## Background

Compiled `.prl` maps show holes in the static world geometry — faces you can see through. A prior fix (`7fa6a95`) removed the large holes; smaller ones remain and shift as brushes change.

A watertightness diagnostic now ships (`crates/level-compiler/src/partition/manifold.rs`, called from `main.rs` after `partition::partition`). It groups each emitted face edge by its supporting line and warns on sub-intervals covered an odd number of times — open boundaries that signal a dropped face. Warning only, never a build failure. Runs on the pre-exterior-cull face set, where the solid boundary is closed.

## What the measurements show

Probed via `parse_map_file` → `partition` → `check_watertight` across the dev maps:

- Most maps report **0** open edges, including the 5.2k-face `stress-warren`.
- `movement-feel` reports 152, `occlusion-test` 64, `campaign-test` 57.
- The residual is **real, not a diagnostic artifact.** Open-edge counts are stable under the line-grouping quantum (`ANCHOR_QUANTUM` 1e-4→1e-2: 152→148). So these are genuine odd-covered boundaries, not failed T-junction cancellation.
- The residual is **scattered, not localized.** `movement-feel`'s 152 edges spread across 83 distinct 1 m cells, ≤4 per cell. A single missing face is a tight loop of edges in one spot; this pattern reads as many hairline gaps, not a few big holes.

### Ruled out — do not re-test

Both epsilon knobs the pipeline investigation suspected have **no effect** on the residual:

| Knob | File | Swept | Result |
|------|------|-------|--------|
| `SPLIT_EPSILON` (clip snap-and-drop) | `face_extract.rs:19` | 0.1 → 1e-3 → 1e-4 | `movement-feel` 152 → 150; others unchanged |
| `COPLANAR_DISTANCE_EPSILON` (coplanar dedup) | `face_extract.rs:29` | 1e-3 → 1e-5 → 1e-7 | no change on any map |

The clip epsilon is not the cause. The earlier draft's central hypothesis was wrong.

## Open questions

Two unknowns gate any fix. Resolve them first — in order.

1. **Do the diagnostic's open edges correspond to the holes seen in-game?** Unconfirmed from the compiler alone. The scattered, low-density pattern may be hairline seam cracks, distinct from a "hole you can see through." This must be settled by rendering `movement-feel` and inspecting the reported coordinates in the engine.
2. **What mechanism drops the faces?** Not the clip or coplanar epsilons (ruled out above). Remaining candidates: fragments buried in solid leaves during the Pass-1 tree walk, residue of the convex-hull merge the big-holes fix hardened, or a structural classification error. Unknown until Q1 confirms what to chase.

## Goal

Confirm whether the diagnostic finds the real holes, find the mechanism that drops faces, and drive the residual toward zero — without ever blocking a level designer on a compiler bug.

## Scope

### In scope

1. **Correlate diagnostic output with real holes (prerequisite).** Compile `movement-feel` (highest count; carried the original big hole), inspect the reported coordinates in-engine. Decide: are the open edges the visible holes, hairline cracks, or a mix? This determines whether the rest is a face-drop hunt (Q2) or a diagnostic-severity problem (item 3).

2. **Root-cause the face drop (if Q1 finds real dropped faces).** Use the diagnostic as the metric. `SPLIT_EPSILON` and coplanar dedup are already eliminated — start at the buried-in-solid fragment path and the hull-merge residue. Land the smallest change that holds the dev map set at (or near) zero, with a before/after count table. Preserve compile determinism (`build_pipeline.md` §Determinism invariant).

3. **Cluster open edges into hole regions, ranked by significance.** Connected open edges become one reported region with an enclosed-area or span estimate. Isolated hairline edges rank below closed loops. This sharpens the warning ("1 hole near X" over "N loose edges") and, per Q1, may itself be the deliverable if the residual is mostly cracks rather than holes. A unit test covers the clustering.

4. **Per-face near-zero-area check after triangulation.** In `geometry.rs`, post fan-triangulation, warn on emitted triangles below an area threshold — catches degenerate survivors. Reuse the `polygon_area` pattern from `portals.rs`. Unit test on a degenerate input.

5. **Exterior leak pointfile.** `visibility::find_exterior_leaves` (`visibility/mod.rs:39`) flood-fills from outside the map and today only `warn!`s. Emit a pointfile tracing a leaked interior entity to the void, and offer a hard-error opt-in. State in output and spec that this does **not** cover dropped-face holes — leak detection and watertightness are complementary (solidity and portals derive from brush half-spaces, independent of the face list, so a dropped face leaves the portal graph sealed).

6. **Graduation decision.** Once the map set holds at zero, decide whether to add an opt-in gate (`--strict-geometry`) that fails on open edges. Default stays warn — a compiler bug must never block authoring.

### Out of scope

- General T-junction welding (inserting vertices so cracks close in the rendered mesh). Larger meshing change; revisit only if Q1/Q2 show welding is the needed fix.
- Runtime geometry validation — the runtime already cross-validates portals and `CellDrawIndex` at load.
- Changes to BSP solidity classification or portal generation.

## Acceptance criteria

1. Q1 answered in writing: the correspondence between diagnostic open edges and in-game holes on `movement-feel`, with engine evidence.
2. If real dropped faces exist: the diagnostic reports 0 across `content/dev/maps/*.map` after the fix, or each remaining non-zero is a documented authored open. Before/after count table in completion notes. No net increase in near-zero-area faces.
3. `geometry.rs` warns (not fails) on degenerate triangles; unit test covers it.
4. The leak check emits a pointfile on a deliberately unsealed fixture, and its output disclaims dropped-face coverage.
5. The report clusters open edges into ranked regions with a representative location and brush per region; unit test covers the clustering.
6. Every check is a warning or opt-in gate. A default build never fails on a geometry-integrity finding. Compile determinism preserved.

## Notes for the implementer

- **Fast iteration.** The diagnostic runs at the partition stage, before the lightmap and SH bakes. Drive it through `parse_map_file` → `partition` → `check_watertight` (as the `partition_with_test_map` test does) to skip the multi-minute full bake while tuning.
- **Start at `movement-feel.map`** — highest count, and the map the original big hole lived in.
- **Diagnostic constants interact.** If item 2 lowers a clip tolerance, re-check that `MIN_OPEN_SPAN` (1e-3) still filters clip noise without hiding real small holes.
