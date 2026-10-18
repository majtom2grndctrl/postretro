# fluid-surface-t-junction-closure

Brief · compact · reads: `context/lib/build_pipeline.md` §Compiler pipeline (Fluid surfaces), §PRL section IDs (52, 54); `context/lib/rendering_pipeline.md` §7.3a; `context/lib/development_guide.md` §1.4; `context/lib/testing_guide.md` §Slow / cold-bake suites · read at 64d069e19

## Problem
Developer-raised, from the `fluid-base-pass-cost` build. Its grid cuts a shared edge identically from both sides, except an edge the compiler left with a T-junction: there a lattice plane at a shallow angle can give one side a crossing the other lacks, which its A3 row records as an open case, pin P13 (`research.md` §The grid case). The defect predates the grid. The compiler closes fluid-surface T-junctions only in a volume a push cut (`close_t_junctions`, gated on `push_cut`), and never makes shared corners identical. So T-junctions from burial, from unequal siblings and between volumes reach the renderer open: hairline cracks in a translucent surface, wherever an edge holds a vertex its neighbour lacks. Done means: in every compiled level, two fluid faces that meet carry identical vertices along their shared boundary, within a volume and across volumes.

## Decisions
- **Close at bake, in the compiler, for every fluid face; the push-cut gate goes.** This is the owner's preferred route. It bakes over computing (`index.md` §2), and the compiler already owns closure and holds f64 clip positions. Every surveyed BSP compiler closes T-junctions at bake, and no surveyed engine closes them at load (`research.md` §Prior art, §Rivals).
- **Scope: every pair of fluid surface faces in the level.** That covers faces within a volume (burial, siblings, push parts) and across volumes, fluids, planes and facings. The tolerance stays the compiler's split epsilon of 0.1 mm, because the renderer's collinear tolerance and the grid's plane tolerance derive from it (`COLLINEAR_DISTANCE`, `PLANE_TOLERANCE`). Prior art's coarser tolerances are sized for map-unit snapping (`research.md` §Prior art).
- **Shared means bit-identical.** A face vertex within tolerance of another face's boundary is, in the encoded f32, one of that face's vertices, whether a corner or a T-vertex. The grid compares shared edges exactly, and watertight rasterization needs identical vertices; today only inserted T-vertices are identical (`research.md` §Current behaviour).
- **Vertices move, faces do not.** Closure inserts vertices and snaps them by at most the tolerance, each taking its UV from its own side's projection; it adds, drops and reorders no face and changes no texture index. So id-54's (volume, face) bindings and per-volume drift maps keep their meaning, there is no format, version, section, loader or wire change, and the level content digest still skips surface faces (`networking.md` §Level identity and level content digest).
- **Runs last, on the final face set:** after subtraction, sibling overlap and push splitting, across all volumes, before encoding. A later face-emission rule then composes ahead of it.
- **Retires the byte-identity promise.** This diverges from `plans/done/fluid-surface-motion` (A11) and from `build_pipeline.md` §Fluid surfaces ("a map without a partial push keeps identical id-52 bytes"): the gate existed to keep those bytes, and it is what leaves burial's T-junctions open. What survives: a level whose fluid faces have no T-junction or near-coincident vertex keeps identical id-52 bytes. Each affected map recaptures its probes once, because id-54's cache key hashes id-52.
- **Bounded, deterministic bake.** The stage is uncached and runs on every warm build, so candidate lookup goes through a spatial index and never pairs every face with every other, whether in the level or in a volume. It is sized for an authored ocean of many abutting brushes (`development_guide.md` §1.4), and its output is identical warm and cold, run to run, and across worker counts.
- **Install stays bounded, in render-cpu.** Closure can give one face many vertices, as on an ocean edge beside many small pieces. This change bounds the install steps that are superlinear in a face's vertex count: the drift map's cubic vertex-triple search and the grid's quadratic ear clip (`research.md` §Consumers).
- **Supersedes `fluid-base-pass-cost`'s carve-out.** That brief keeps the compiler's open T-junctions in its Decisions and in its A3 row (P13); both narrow to nothing once this lands. Whichever brief lands second edits the other's record.
- **Non-goals:**
  - Removing the faces two adjacent volumes draw against each other (the "reflective water walls"). That is face emission, and adjacent tops still meet without those faces; because closure runs last, it composes with such a rule in either landing order.
  - Fluid-to-world welding. The opaque, depth-written wall shows through such a seam, which blends no surface twice, and ericw-tools never welds translucent faces to opaque ones.
  - World and mover T-junctions. Those are opaque geometry, covered by the `geometry-integrity-validation` draft.

## Acceptance
### Automated
**Shared boundaries are identical.** No vertex of a fluid face lies within 0.1 mm of another fluid face's boundary unless it is, bit for bit, one of that face's vertices. The check runs level-wide, across volumes.
- [ ] Holds for fixtures covering:
  - a partly buried side;
  - unequal siblings;
  - a push-cut volume;
  - two volumes of different fluids side by side with mismatched brush sizes;
  - a top meeting another volume's side;
  - a sloped brush meeting a neighbour;
  - two corners within 0.1 mm but not equal;
  - a vertex within 0.1 mm of a neighbour's corner;
  - e24.
- [ ] Holds on campaign-test, reflection-probes-test, vault-test and stress-env-volumes (`#[ignore]` suite).
- [ ] End to end through the renderer's packing: faces compiled from a fixture with today's open T-junction, at a shallow angle to a lattice plane within tolerance of the T-vertex, carry identical vertices along the shared edge on a lattice. Today the same fixture fails that.

**Nothing else moves.**
- [ ] Closure changes no face count, face order or texture index. Every vertex's UV is its own side's projection at its position.
- [ ] No level-format, loader, wire or digest code changes, and a PRL built earlier still loads.
- [ ] Every face keeps at least three distinct vertices, and its area changes by at most 0.1 mm × its perimeter.
- [ ] A level whose fluid faces have no T-junction or near-coincident vertex keeps identical id-52 bytes (regression guard).
- [ ] On e24 and the probes map, every face resolves the same drift push as before closure, and id-54 binds the same (volume, face) pairs to the same probes.
- [ ] id-52 bytes are identical across two cold builds, warm against cold, and across worker counts.
- [ ] Today's pin that burial and sibling T-junctions stay open now asserts they close.

**Bounded.**
- [ ] On a synthetic ocean (a grid of abutting fluid brushes, and one face bordered by thousands of pieces), closure's vertex-to-edge candidate tests grow no faster than n log n in the level's vertex count, compared at two sizes. The proof is a count, not wall time.
- [ ] A face carrying thousands of inserted vertices packs, gridded and ungridded, with a drift map that matches the same face without them within the fit tolerance. Its install work grows no faster than n log n in its vertex count, by count at two sizes.
### Manual
- [ ] Bake cost, cold, before and after: the fluid-surface log line and the whole build on stress-env-volumes and campaign-test, reported.
- [ ] Fluid install time on stress-env-volumes, before and after, reported.
- [ ] Owner look, in engine: e24's sunk pools and a cross-volume seam at grazing angles, above and below. No hairline sparkle at burial edges or seams, compared side by side with today's build.

## Path
- **Seams.**
  - `fluid_surface_pieces`, `close_t_junctions` and `edge_holding` (`environment_volumes/surface.rs`).
  - `encode_environment_volumes_section` builds and encodes each volume in one pass; closure needs every volume's pieces first.
  - `SolidIndex` is the deterministic-index precedent.
  - `partition/manifold.rs`'s quantized line keys need neighbour-cell lookup, or near-equal points split across a cell boundary (dmap's `GetHashVert` comment).
- **Weld then insert**, as dmap and vbsp do. Choose each canonical position deterministically, and do not chain merges past the tolerance (`research.md` §Pins P3, P4, P8).
- **Consumers to bound.** `FaceMap::fit` (`render-cpu/src/fluid_surface/drift.rs`) is cubic; `grid::triangulate_convex` is quadratic per part. The runtime drift resolve uses the vertex-mean centroid, which inserts move but keep inside the piece.
- **Tests.** `surface_tests.rs`'s `t_junctions` helper is per record; make it level-wide. `level-compiler` already dev-depends on `render-cpu`, which the end-to-end row needs.
- **Shape and rival.** Chosen: level-wide weld-and-insert in the compiler on the final pieces. Rival: weld at fluid install before the grid cut. It needs no rebake, but costs every load, works on f32, splits closure between two owners, and leaves ungridded levels cracked if gated on the grid (`research.md` §Rivals).
- **First slice.** A level-wide detector, then count today's open T-junctions and non-identical shared corners on e24 and stress-env-volumes. That sizes the weld's need and the bake work before building either.
- **Large files.** `surface_tests.rs` is past 800 lines; split it before extending, behaviour-preserving, in its own commit.
- **At promotion.** `build_pipeline.md` §Fluid surfaces loses the push-cut gate and the byte-identity sentence.

## Open questions
- Sequencing with `fluid-base-pass-cost`. Should it land first, recording P13 as a known residual that points here? Or hold until this lands? Recommendation: land it first. The crack it leaves is no wider than today's, and this brief's end-to-end row needs the grid on `main`. — owner: Dan — **blocks build**
- Weld mechanism, canonical choice and index shape — **delegated**: the executor picks them from the first slice's counts and reports them.
- Bounding the install consumers: rewrite the triple search or fit on corner vertices only — **delegated**.
