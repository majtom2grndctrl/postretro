# fluid-surface-t-junction-closure

Brief · compact · reads: `context/lib/build_pipeline.md` §Compiler pipeline (Fluid surfaces), §PRL section IDs (52, 54); `context/lib/rendering_pipeline.md` §7.3a; `context/lib/development_guide.md` §1.4; `context/lib/testing_guide.md` §Slow / cold-bake suites · read at 8bb02a18c

## Problem
Developer-raised, from the `fluid-base-pass-cost` build. Its grid cuts a shared edge identically from both sides, except an edge the compiler left with a T-junction: there a lattice plane at a shallow angle can give one side a crossing the other lacks, which it landed as an accepted limit (`rendering_pipeline.md` §7.3a; `research.md` §The grid case). The defect predates the grid. The compiler closes fluid-surface T-junctions only in a volume a push cut (`close_t_junctions`, gated on `push_cut`), and never makes shared corners identical. So T-junctions from burial, from unequal siblings and between volumes reach the renderer open: hairline cracks in a translucent surface, wherever an edge holds a vertex its neighbour lacks. Done means: in every compiled level, two fluid faces that meet carry identical vertices along their shared boundary, within a volume and across volumes.

## Decisions
- **Close at bake, in the compiler, for every fluid face; the push-cut gate goes.** It bakes over computing (`index.md` §2), and the compiler already owns closure and every face's clip context. Every surveyed BSP compiler closes T-junctions at bake, and no surveyed engine closes them at load (`research.md` §Prior art, §Rivals).
- **Scope: every pair of fluid surface faces in the level.** That covers faces within a volume (burial, siblings, push parts) and across volumes, fluids, planes and facings. The tolerance stays the compiler's split epsilon of 0.1 mm, because the renderer's collinear tolerance and the grid's plane tolerance derive from it (`COLLINEAR_DISTANCE`, `PLANE_TOLERANCE`).
- **Shared means bit-identical, as encoded.** A face vertex within tolerance of another face's boundary is, bit for bit, one of that face's vertices, whether a corner or a T-vertex, with the tolerance measured on the f32 positions the renderer reads. The grid compares shared edges exactly and watertight rasterization needs identical vertices; today only inserted T-vertices are guaranteed identical (`research.md` §Current behaviour). Identity outranks the up-facing test: a weld may tilt a narrow piece off up-facing and cost it its probe binding.
- **Vertices move, faces do not.** Closure inserts vertices and snaps them by at most the tolerance, each taking its UV from its own side's projection; it adds, drops and reorders no face and changes no texture index. So id-54's (volume, face) bindings and per-volume drift maps keep their meaning, there is no format, version, section, loader or wire change, and the level content digest still skips surface faces (`networking.md` §Level identity and level content digest).
- **Runs last, on the final face set:** after subtraction, sibling overlap and push splitting, across all volumes, before encoding.
- **Retires the byte-identity promise.** This diverges from `plans/done/fluid-surface-motion` (A11) and `build_pipeline.md` §Fluid surfaces: the gate existed to keep id-52 bytes, and it is what leaves burial's T-junctions open. A level whose fluid faces have no T-junction or near-coincident vertex still keeps identical id-52 bytes; each affected map recaptures its probes once, because id-54's cache key hashes id-52.
- **Narrows `fluid-base-pass-cost`'s accepted limit** (`rendering_pipeline.md` §7.3a) to what closure leaves: a PRL built before it, two faces further apart than the tolerance (P9), and a pair a weld cluster carries out of tolerance (P12, P20, P21). Each crack stays on its own edge's line.
- **Bounded, deterministic bake.** The stage is uncached and runs on every warm build, so candidate lookup is output-sensitive and never pairs every face with every other, whether in the level or in a volume. It is sized for an authored ocean of many abutting brushes (`development_guide.md` §1.4), and its output is identical warm and cold, run to run, and across worker counts.
- **Install stays bounded, in render-cpu.** Closure can give one face many vertices, as on an ocean edge beside many small pieces, so this change keeps every install step that reads a face's vertices within n log n of their count (`research.md` §Consumers).
- **Non-goals:**
  - Removing the faces two adjacent volumes draw against each other (the "reflective water walls"). That is face emission, and adjacent tops still meet without those faces; because closure runs last, it composes with such a rule in either landing order.
  - Fluid-to-world welding. The opaque, depth-written wall shows through such a seam, which blends no surface twice, and ericw-tools never welds translucent faces to opaque ones.
  - World and mover T-junctions. This is a fluid-surface brief, and opaque cracks are a different defect: `geometry-integrity-validation` holds welding out of scope for the world until its root-cause questions show it is the fix, and no plan yet covers movers.

## Acceptance
### Automated
**Shared boundaries are identical.** No vertex of a fluid face lies within 0.1 mm of another fluid face's boundary unless it is, bit for bit, one of that face's vertices. The check runs level-wide, across volumes, on encoded positions.
- [ ] Holds for fixtures covering the cases below. Each fixture asserts that its faces before closure fail the check.
  - a partly buried side;
  - unequal siblings;
  - a push-cut volume (P1);
  - two volumes of different fluids side by side with mismatched brush sizes, in both entity orders (P2);
  - a top meeting another volume's side;
  - a sloped brush meeting a neighbour;
  - two corners within 0.1 mm but not equal (P3);
  - a vertex within 0.1 mm of a neighbour's corner (P4);
  - two near-coincident corners on a third face's edge (P11);
  - a corner the weld moves into, and one it moves out of, a third face's tolerance (P12);
  - a vertex just over 0.1 mm from a neighbour's edge in f64 that f32 encoding brings under 0.1 mm (P16);
  - a sliver triangle whose 0.05 mm edge lies on a neighbour's edge, once with one end at the neighbour's corner (P17);
  - a corner that an insert's bend brings within 0.1 mm of a neighbour's edge (P18);
  - a weld that moves the endpoint of an edge another face's corner lies on (P20);
  - a zero-face volume between two fluid volumes that share an open T-junction, and a level with a single fluid face, which closure leaves as it was and is exempt from failing before (P15).
- [ ] Holds on e24, and in the `#[ignore]` suite on campaign-test, reflection-probes-test, vault-test and stress-env-volumes.
- [ ] End to end through the renderer's packing: faces compiled from a fixture with today's open T-junction, at a shallow angle to a lattice plane within tolerance of the T-vertex, carry identical vertices along the shared edge on a lattice. Today the same fixture fails that.

**Nothing else moves.** Measured on encoded positions; each per-face row holds on every fixture and map the shared-boundary rows run on.
- [ ] Closure changes no face count, face order or texture index, compared with the same build's faces before closure (P5). Every vertex's UV is its own side's projection at its position.
- [ ] A vertex 0.15 mm from a neighbour's corner, or from a neighbour's edge, is left as it was, and the neighbour gains no vertex (P9); so is one just under 0.1 mm from a neighbour's edge in f64 that f32 encoding puts just over (P19).
- [ ] Every vertex the weld moves stays within 0.1 mm of where it was, and every corner before closure has a vertex after it within 0.1 mm, including along a weld chain (P10, P21). An inserted vertex lies within 0.1 mm of the edge it joins as closure leaves it, which an earlier insert may have bent (P18).
- [ ] No level-format, loader, wire or digest code changes, and a PRL built earlier still loads (P7). Proved by a diff gate: no change under `crates/level-format`, `crates/level-loader` or the level content digest, against `main`.
- [ ] Every face keeps at least three distinct vertices and a normal, and its area changes by at most 0.1 mm × its perimeter, compared with the same build's faces before closure (P8, P17). P14's 6 mm square stays up-facing; its ~1 mm² piece may tilt off it.
- [ ] A level whose fluid faces have no T-junction or near-coincident vertex keeps identical id-52 bytes, against bytes captured from the compiler before this change (regression guard).
- [ ] On e24 and reflection-probes-test, every face resolves the same drift push as the same build's faces before closure, no face tilts off up-facing, and id-54 binds the same (volume, face) pairs to the same probes.
- [ ] id-52 bytes are identical across two cold builds, warm against cold, and across worker counts, so the id-54 key hits on the next warm build (P6).
- [ ] No `context/lib` text says the compiler leaves fluid T-junctions open or closes them only where a push cut. Proved by a grep gate over `build_pipeline.md` and `rendering_pipeline.md`: neither file's fluid-surface text holds "only a push cut", "stay open", "without a partial push", "keeps its subtraction output exactly" or "leaves some T-junctions open".

**Bounded.**
- [ ] On a synthetic ocean (a grid of abutting fluid brushes, once as one fluid entity and once as one entity per brush, and one face bordered by thousands of pieces), closure's candidate tests, vertex-to-vertex for the weld and vertex-to-edge for the insert, grow no faster than n log n in the level's vertex count, compared at two sizes. The proof is a count, not wall time.
- [ ] A face carrying thousands of inserted vertices packs, gridded and ungridded (gridded with a lattice plane crossing the face, so that one part holds the inserted vertices), with a drift map that matches the same face without them within the fit tolerance. Its install work grows no faster than n log n in its vertex count, by count at two sizes.
- [ ] A piece clear of a push, at least 100 m from the origin, whose edge on the push boundary carries thousands of inserted vertices, resolves the same drift push as without them (P13).
### Manual
- [ ] Bake cost, cold, before and after: the fluid-surface log line and the whole build on stress-env-volumes and campaign-test, reported.
- [ ] Fluid install time on stress-env-volumes, before and after, reported.
- [ ] Owner look, in engine: stress-env-volumes' burial edges and cross-volume seams at grazing angles, above and below. No hairline sparkle, compared side by side with today's build.

## Path
- **Seams.**
  - `fluid_surface_pieces`, `close_t_junctions` and `edge_holding` (`environment_volumes/surface.rs`).
  - `encode_environment_volumes_section` builds and encodes each volume in one pass; closure needs every volume's pieces first.
  - `SolidIndex` is the deterministic-index precedent.
  - `partition/manifold.rs`'s quantized line keys need neighbour-cell lookup, or near-equal points split across a cell boundary (dmap's `GetHashVert` comment).
- **Weld then insert**, as dmap and vbsp do. Choose each canonical position deterministically, judge insert candidates on welded positions, and move no vertex past the tolerance (`research.md` §Pins P3, P4, P8, P10–P12).
- **Consumers to bound.** `FaceMap::fit` (`render-cpu/src/fluid_surface/drift.rs`) is cubic over a face; `grid::triangulate_convex` is quadratic per part. The runtime drift resolve takes the vertex mean in f32, which inserts keep inside the piece only in exact arithmetic (P13).
- **Tests.** `surface_tests.rs`'s `t_junctions` helper is per record; make it level-wide. `level-compiler` already dev-depends on `render-cpu`, which the end-to-end row needs.
- **Shape and rival.** Chosen: level-wide weld-and-insert in the compiler on the final pieces. Rival: weld at fluid install before the grid cut. It needs no rebake, but costs every load, works on f32, splits closure between two owners, and leaves ungridded levels cracked if gated on the grid (`research.md` §Rivals).
- **First slice.** A level-wide detector, then count today's open T-junctions and non-identical shared corners on e24 and stress-env-volumes, and the largest distance a weld would move a vertex. The counts size the index and the bake work; the weld is owed regardless, since the grid compares shared edges exactly. Capture today's build at a known open seam on stress-env-volumes at a grazing angle, as the side-by-side baseline: the seams are counted (`research.md` §First slice), not yet seen in engine. e24 has none.
- **Large files.** `surface_tests.rs` is past 800 lines; split it before extending, behaviour-preserving, in its own commit.
- **With the change.** `build_pipeline.md` §Fluid surfaces loses the push-cut gate and the byte-identity sentence; `rendering_pipeline.md` §7.3a's accepted limit narrows as its Decision states.

## Open questions
- Weld mechanism, canonical choice and index shape — **delegated**: the executor picks them from the first slice's counts and reports them.
- Bounding the install consumers: rewrite the triple search or fit on corner vertices only — **delegated**.
