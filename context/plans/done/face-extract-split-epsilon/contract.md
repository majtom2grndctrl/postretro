# Face-extract split epsilon — contract

## Goal

Face extraction drops real surface. It splits brush sides with a 0.1 m tolerance, while the production BSP classifies regions at 1 mm, so any exposed strip of a brush side narrower than 10 cm next to a splitting plane is discarded. The BSP still calls that region open, but no face bounds it. The result is see-through holes in the world and shadow rays that cross metres of solid. Close the holes by matching face extraction to the BSP tolerance. This is a standalone bug fix. It also gives the later light-reach cull a correct byte baseline.

Evidence is in `context/plans/drafts/lightmap-stage-sizing/leak-diagnosis-findings.md` on branch `spike/lightmap-flood-cull-sizing`. Read it with `git show spike/lightmap-flood-cull-sizing:<path>`.

## Decisions

- **Tolerance = 1 mm, tied to the BSP.** The face-extract split epsilon derives from `REGION_CLASSIFICATION_EPSILON` (`partition/brush_bsp.rs`), not a second literal. The two tolerances drifted once already because they were separate constants.
- **No tighter.** The spike measured 1e-5: it adds sliver triangles and slows the bake (lightmap shaft-clear texels 67.6 M → 53.3 M on movement-feel). The sub-millimetre crease cracks that remain at 1 mm are a known, invisible residue; they are out of scope here.
- **Bytes change, by design.** PRL output changes on every map with such strips. The restored faces block light that used to leak, so the shaft cull clears less; the spike saw 5–8% more lightmap time. Both are expected.
- **The watertightness check upgrade is not in this fix.** Replacing the open-edge parity count with an exact hole probe belongs to the light-reach brief, which needs it as its precondition check.

## Invariants

- A cached bake must not serve geometry extracted at the old tolerance. *Amended after review:* face extraction itself is uncached, and every downstream cache key folds the extracted geometry (or, for cell visibility and residency, everything it reads), so old-tolerance entries miss on their own. No epoch bump is needed.
- No spike instrumentation lands. The spike's `POSTRETRO_FACE_SPLIT_EPS` override and counters stay on the spike branch.
- The epsilon at the polygon-clipping site that "must not round off real edges" (`face_extract.rs`, the comment near the second epsilon) keeps its meaning. Don't merge the two constants unless they are the same quantity.

## File ownership

Single track: `crates/level-compiler/` (face extraction, its tests, the cache key) and any `context/lib/` sentence that states the old tolerance.

## Acceptance

| # | Check | Expected |
|---|---|---|
| 1 | `cargo test -p postretro-level-compiler --bin prl-build face_extract` | New fixture(s) pass with N > 0 tests; the axis-aligned and sloped strip fixtures both fail against the 0.1 m tolerance (a brush side with a sub-10 cm exposed strip keeps its face) |
| 2 | `grep -rn '0\.1' crates/level-compiler/src/partition/face_extract.rs` | No split-tolerance literal remains |
| 3 | Cold `--release` bakes of `movement-feel` and `stress-warren-hallway-inspection-mini`: compare the watertightness open-edge warning count to `main` | Count drops; report before and after |
| 4 | Manual-visual (owner) | On movement-feel, where the sloped brushes meet the wall under the ceiling, no see-through gap |

## Open questions

None.
