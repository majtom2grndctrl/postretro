# Geometry Integrity — Hole Root-Cause and Compile-Time Validation

> **Status:** draft — not yet reviewed. Follow-up work spun out of the shipped watertightness diagnostic.
>
> **Area:** level compiler (`postretro-level-compiler`), geometry pipeline.
>
> **Related context:** `context/lib/build_pipeline.md` §PRL Compilation · `context/research/code-quality-tooling.md` §Custom repo-specific lint rules (compiler validation family).

## Background

Compiled `.prl` maps show **holes** in the static world geometry — faces you can see
through. A prior fix (`7fa6a95`, non-robust convex hull in face extraction) removed the
large holes; smaller ones remain and shift as brushes are added or removed.

A **watertightness diagnostic** now ships (`crates/level-compiler/src/partition/manifold.rs`,
called from `main.rs` after `partition::partition`). It groups every emitted face edge by
its supporting line and reports sub-intervals covered an odd number of times — open
boundaries that indicate a dropped face. It is a **warning, never a build failure**, and it
runs on the pre-exterior-cull face set (where the solid boundary is closed). Measured
baseline across dev maps: most report **0** open edges (including the 5.2k-face
`stress-warren`); `movement-feel` reports ~152, `occlusion-test` ~64, `campaign-test` ~57.

This plan does the follow-up the diagnostic set up: find and fix the mechanism producing the
residual holes, add the cheaper companion checks, and decide whether the diagnostic graduates
from warning to gate.

## Goal

Drive the residual small-hole count toward zero by fixing the compiler mechanism that drops
faces, and harden the compile-time geometry checks so a future regression surfaces
immediately — without ever blocking a level designer on a compiler bug.

## Motivating hypothesis

The most likely remaining hole generator is the clipping epsilon in face extraction:

- **`SPLIT_EPSILON = 0.1` (`face_extract.rs:19`) is 0.1 metre** — the coordinate space is
  meters by this stage. It is orders of magnitude larger than its neighbours
  (`PLANE_DISTANCE_EPSILON = 1e-4` at `:24`, portal `PORTAL_EPSILON = 0.01`, region
  `REGION_CLASSIFICATION_EPSILON = 1e-3`). During clipping, a vertex within 10 cm of a split
  plane is snapped on-plane and duplicated to both sides; a fragment that then falls below 3
  vertices is dropped. A face fragment thinner than ~10 cm along a split direction can vanish,
  and because it depends on the split configuration, the hole moves as geometry changes —
  matching the observed symptom.
- Secondary suspect: the coplanar dedup tolerance (`COPLANAR_DISTANCE_EPSILON = 1e-3` at
  `:29`; containment test in `convex_contains`). A partially-overlapping coplanar side falsely
  judged "contained" is dropped, leaving a mm–cm gap.

`SPLIT_EPSILON` is probably load-bearing against slivers and numerical noise, so it must not be
lowered blind — the shipped diagnostic is the metric that makes tuning it safe (measure hole
count before/after; watch for new degenerate slivers).

## Scope

### In scope

1. **Root-cause the residual holes.** Use the watertightness diagnostic as the deterministic
   metric. Reduce `SPLIT_EPSILON` (and, if implicated, the coplanar tolerance) incrementally,
   re-measuring open-edge counts across the dev map set (`movement-feel`, `campaign-test`,
   `occlusion-test` are the current non-zero maps). Confirm holes drop without a rise in
   degenerate/near-zero-area faces or new z-fighting. Land the smallest epsilon that holds the
   whole map set at (or near) zero. If the epsilon is genuinely load-bearing and can't be
   reduced, replace the snap-and-drop with a split that preserves thin fragments instead.

2. **Per-face near-zero-area assertion after triangulation** (`geometry.rs`, post
   fan-triangulation ~`:135`). Reuse the `polygon_area` pattern from `portals.rs`. Warn (do not
   fail) on emitted triangles below an area threshold — catches degenerate survivors and sliver
   artifacts of the clip, and guards the epsilon change in item 1.

3. **Promote the exterior flood-fill leak check to actionable.** `visibility::find_exterior_leaves`
   (`visibility/mod.rs:39`) already flood-fills from outside the map and currently only `warn!`s
   (`:70`, `:123`). Emit a **pointfile** (world-space trace from a leaked interior entity to the
   void) so genuine unsealed maps are diagnosable, and consider a hard-error opt-in
   (`--strict-seal` or worldspawn KVP). **Note explicitly** in the spec and output that this does
   *not* cover dropped-face holes — leak detection and watertightness are complementary
   (solidity/portals derive from brush half-spaces independent of the face list, so a dropped
   face leaves the portal graph sealed).

4. **Harden the watertightness diagnostic.**
   - **Cluster** adjacent open spans into hole *regions* (connected open edges) so the report
     says "1 hole near (x,y,z)" instead of N loose edges — better signal for the designer.
   - **Classify** residual open spans: true missing face vs. T-junction crack the coverage test
     didn't fully cancel (e.g. non-collinear near-coincident edges). Determines whether the
     `movement-feel`/`campaign-test` residuals are real holes or a diagnostic noise floor.
   - **Coincident-face robustness:** confirm behaviour when three+ coplanar faces overlap
     (odd coverage from z-fighting brushes, not a hole); exempt or report distinctly.
   - **Graduation decision:** once the dev map set holds at zero after item 1, decide whether to
     add an opt-in gate (`--strict-geometry`) that fails the build on open edges — default stays
     warn so a compiler bug never blocks authoring.

### Out of scope

- A general T-junction *welding* pass (inserting the missing vertices so cracks close in the
  rendered mesh). That is a larger meshing change; this plan detects and root-causes, and only
  welds if item 1 shows welding is the necessary fix rather than the epsilon.
- Runtime geometry validation (the runtime already cross-validates portals/`CellDrawIndex` at
  load; this plan is compile-time).
- Any change to the BSP solidity classification or portal generation.

## Acceptance criteria

1. The watertightness diagnostic reports **0 open edges** on the full dev map set
   (`content/dev/maps/*.map`) after the root-cause fix, or every remaining non-zero is
   confirmed a genuine authored open (documented per map) rather than a compiler drop.
2. The epsilon/clip change lands with a before/after open-edge count table in the plan's
   completion notes, and no net increase in near-zero-area emitted faces (item 2's check).
3. `geometry.rs` emits a warning, not a failure, on degenerate triangles; a unit test covers a
   degenerate input.
4. The exterior leak check emits a pointfile on a deliberately unsealed fixture map, and the
   output states it does not cover dropped-face holes.
5. The watertightness report clusters open spans into regions and names a representative
   location + brush per region; a unit test covers the clustering.
6. Every change is a compiler warning or opt-in gate — a default build never fails on a
   geometry-integrity finding. Determinism of the compile output is preserved (see
   `build_pipeline.md` §Determinism invariant).

## Notes for the implementer

- The diagnostic's constants (`SPLIT_EPSILON` sibling tolerances, `MIN_OPEN_SPAN = 1e-3`,
  `ANCHOR_QUANTUM = 1e-4`) interact: if item 1 lowers `SPLIT_EPSILON`, re-check that
  `MIN_OPEN_SPAN` still filters clip noise without hiding real small holes.
- Fast iteration path: the diagnostic runs at the partition stage, before the lightmap/SH
  bakes. Exercise it via `parse_map_file` → `partition::partition` → `check_watertight` (as the
  `partition_with_test_map` test does) to avoid the multi-minute full bake while tuning.
- `movement-feel.map` is the highest-signal map (it carried the original big hole); start there.
