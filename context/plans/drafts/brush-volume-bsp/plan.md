# Brush-Volume BSP Construction

> **Status:** draft
> **Depends on:** face `brush_index` ownership tracking (in-flight parallel work), f64 precision boundary at parse (done), BSP convexity termination fix (done), narrow_frustum portal fix (done).
> **Related:** `context/lib/build_pipeline.md` · `context/lib/development_guide.md` · `context/plans/done/portal-bsp-vis/`

---

## Goal

Reframe PRL BSP construction so brush volumes are first-class. The tree is built by recursively partitioning space with brush-derived planes; each leaf naturally knows which brush volumes it lies inside. Faces are derived from brush sides at the tail of the pipeline, not the head. This matches how qbsp and ericw-tools operate and eliminates the class of bugs that arise from reconstructing brush ownership post-hoc.

---

## Motivation

Today `build_bsp_tree` takes a `Vec<Face>` and partitions polygons. Brush volumes exist only as a sidecar used by `classify_leaf_solidity` after the tree is built. Even with face-level `brush_index` tracking, brushes remain an afterthought: the tree's shape is driven by face planes, leaf interiors are never tracked during descent, and solidity is inferred from whatever faces happened to land in a leaf.

This has produced a recurring bug family:

- Face centroids sitting on brush surfaces false-positive as "inside."
- Leaves span solid and air regions because the splitter cannot see brush boundaries that no face lies on.
- Small air gaps between adjacent brushes get classified solid because no face marks the gap.
- Recent fixes (convexity termination, tight face-centroid epsilon, `SOLID_EPSILON`/`FACE_SOLID_EPSILON` split) are symptomatic patches for a post-hoc classification step that lacks the structural information it needs.

qbsp and ericw-tools avoid this by construction: they partition **space** using brush planes, track the set of brushes that contain each region as the tree descends, and terminate when a region is uniformly inside one brush set. Faces are produced last, by clipping each brush's sides against every solid region. A leaf's solid/empty state is known exactly because it was computed during construction.

This refactor adopts that architecture.

---

## Scope

### In scope

- Introduce a brush-volume-centric BSP builder in `postretro-level-compiler/src/partition/`.
- Track "inside set" (set of brush indices whose half-spaces all contain the region) per tree region during recursive partitioning.
- Derive `Face` polygons at the end by clipping each brush's sides against the completed tree (or against the complement of all other solid regions inside that brush).
- Replace `classify_leaf_solidity`'s heuristic with structural solidity derived from construction state.
- Update `partition.rs` orchestration to thread `BrushVolume`s through BSP construction, not just CSG.
- Update portal generation to consume the new tree shape (minimal changes — portals already operate on the BSP tree, not faces).
- Update tests in `partition/bsp.rs` that construct bare-face inputs: they need to supply brushes alongside faces or use a new brush-first entry point.

### Out of scope

- Runtime engine changes. The PRL file format stays fixed; what lands on disk is byte-identical in shape (same BspNodes, BspLeaves, geometry sections).
- Portal generation algorithm. Only the input tree structure changes; the portal-distribution recursion is unchanged.
- PVS computation (`visibility/`). Consumes portals, unaffected.
- PRL pack format or section IDs.
- CSG face clipping (`csg.rs`). Still runs before BSP; its output feeds the brush-volume builder as bounded "brush sides" rather than world faces. The Sutherland-Hodgman clipping logic is reused.
- Parser (`parse.rs`) — brush volumes and brush indices are already produced at parse time after the parallel `brush_index` work lands.
- BSP path (`.bsp` legacy loader in the engine). Independent pipeline.
- Entity brush handling. Same stubs, same passthrough.

### Non-goals

- Matching qbsp bit-for-bit. We adopt the architectural pattern, not its file format or surface code.
- Optimizing BSP construction speed. Correctness first; profile after.
- Supporting non-convex brushes. Same invariant as today — brushes are convex hulls of half-planes.
- Changing how plane candidates are scored beyond what the new algorithm requires. The existing SAH-style balance + split penalty can be reused with face counts replaced by brush-coverage counts.

---

## Background: how qbsp does it

For reference — this is the algorithm postretro will adopt, described in engine-neutral terms. qbsp's `SolidBSP` and `PartitionBrushes` are the canonical source.

1. Start with a bounded region (world AABB + slack) and the full set of brushes.
2. Pick a splitting plane from brush sides that intersect the current region.
3. Classify each brush relative to the plane: entirely front, entirely back, or spanning.
4. Recurse into front and back sub-regions. Spanning brushes appear on both sides; non-spanning brushes go to one side only.
5. Terminate when the sub-region is entirely inside every brush in its set (fully solid) or entirely outside every brush (fully empty). Either condition produces a leaf.
6. After the tree is built, walk each brush's original sides and clip them against the tree: the fragment that lands in an empty leaf adjacent to a solid leaf owned by this brush becomes a world face.

Key properties:

- The recursion operates on **brushes**, not faces. Face production is a post-pass.
- A leaf's solidity is structural: it's "inside" the intersection of its bounding brushes' half-spaces, which the builder tracks directly.
- Adjacent brush boundaries and narrow air gaps are detected because brush planes become splitter candidates even when no world face lies on them.
- Shared faces between adjacent solid brushes are never produced — the face clipping step sees that the fragment would land in a solid leaf and drops it. This subsumes CSG face clipping.

---

## Shared Context

### Terminology

| Term | Meaning |
|---|---|
| Region | A convex sub-volume during BSP descent, defined by the bounding AABB and the stack of ancestor splitting planes. |
| Inside set | The set of brush indices whose half-spaces all fully contain the current region. Populated as the recursion descends. |
| Spanning set | The set of brushes whose bounding planes cross the region — candidates for further splitting. |
| Brush side | One of the half-plane faces bounding a brush volume. Has the brush's texture and projection metadata. The successor to today's parse-time `Face`. |
| World face | An output polygon in the final geometry section. Produced by clipping brush sides against the completed tree. |

### Data contract

At the BSP stage boundary, input is `&[BrushVolume]` plus each brush's list of brush sides (textured half-planes). Output is `(BspTree, Vec<Face>)` where every leaf's `is_solid` is authoritative (no separate classify pass) and every face is owned by exactly one brush and lies on the boundary of exactly one empty leaf.

### Invariants

- Every empty leaf's bounding polygons lie on the boundary between that leaf and an adjacent solid leaf.
- No two leaves overlap in space; every point in the world AABB maps to exactly one leaf.
- Solid leaves have no faces in their own `face_indices` list. (Currently enforced by classification; will be enforced by construction.)
- Portal generation still operates on the tree's internal nodes and empty-leaf adjacency — no change to its contract.

### Parser assumption (from parallel work)

After the in-flight brush-ownership work lands, `Face` carries `brush_index: usize` and parser output is two arrays: brush sides grouped by brush, and brush volumes. The refactor builds directly on that output — it does not need to invent brush ownership.

---

## Approach

High-level algorithm. No code.

### Phase A: Brush-volume BSP descent

Replace `build_bsp_tree(faces: Vec<Face>)` with a builder that takes `&[BrushVolume]` and a world AABB. The recursion state carries:

- Current region AABB (shrunk by ancestor planes — or conservative via the AABB clipped against the ancestor stack).
- Candidate brushes (those whose AABB still overlaps the region).
- Inside set (brushes whose half-spaces fully contain the region).

At each step:

1. If every candidate brush is in the inside set → **solid leaf**. Pick one such brush as "owning" the leaf (for texture attribution later; multi-owned leaves are the exception and can use the first).
2. If the candidate list is empty → **empty leaf**.
3. Otherwise, pick a splitting plane from the bounding planes of candidate brushes that actually partition the region (reject planes that leave one side empty). Reuse the existing score function, but count brushes on each side instead of faces.
4. Partition candidate brushes across the plane:
   - Brushes entirely front or back go to one child's candidate list.
   - Brushes spanning the plane go to both children.
   - The inside set propagates: a brush stays in the inside set of a child only if the child's region remains behind all of that brush's planes. In practice, the child's inside set is recomputed by testing each candidate brush against the updated region stack.
5. Recurse. Make sure the splitter-selection loop always makes progress — if no candidate plane produces a non-trivial split, make a leaf and flag the case for diagnostics (should not happen for well-formed input, but we need to not loop).

### Phase B: Face extraction

Once the tree is complete, generate world faces by walking each brush's brush sides and pushing each side polygon down the tree:

1. For each brush side, start at the root with the full side polygon.
2. At each internal node, classify and split the polygon by the node's plane (reuse `split_polygon` from `geometry_utils.rs`).
3. Push fragments into the respective children.
4. At each leaf:
   - **Empty leaf adjacent to the owning brush's solid region**: emit the fragment as a world face. "Adjacent" means the brush that owns this side is in the leaf's neighboring solid region across the side's plane.
   - **Empty leaf not adjacent**: discard (this fragment is inside another brush's air space, not a bounding wall).
   - **Solid leaf**: discard (fragment is inside another brush — the equivalent of today's CSG clip).

This step replaces both CSG face clipping and the current face-oriented BSP face flow. It produces fewer, cleaner faces: no duplicates on shared brush boundaries, no stray fragments inside solids, no faces that cross leaves.

### Phase C: Leaf solidity and bounds

Solidity is assigned during Phase A. `classify_leaf_solidity` is removed — the function's role (assign `is_solid`) is handled by construction. Leaf bounds come from the accumulated region AABB at leaf creation time, not from face vertices.

### Phase D: Portal generation shim

`portals.rs` already operates on the tree, not on faces. The only adjustment: portal generation should continue to treat empty-leaf-to-empty-leaf adjacencies as portal pairs and skip solid leaves. This is already what it does; the change is that solidity flags are now correct by construction, so the algorithm sees the real empty space.

---

## Tasks

### Task 1: Extract brush side representation
**Description:** Today `parse.rs` produces a flat `Vec<Face>` of world brush faces. Refactor parse output so each brush volume carries its own brush sides (the half-plane polygons that bound it) alongside its `BrushPlane`s. Retain the flat face list as a parse-boundary convenience if needed by CSG, but internally brush-keyed. This is a data model change only — no algorithm changes yet.

**Acceptance criteria:**
- [ ] `BrushVolume` or a sibling type carries the list of brush sides (polygon + texture + projection) for that brush.
- [ ] Parser populates brush sides at parse time.
- [ ] Existing CSG stage still compiles against the new shape (adapter over brush-keyed sides, or unchanged if it already takes a flat list).
- [ ] Existing BSP stage still compiles and passes tests using an adapter that flattens brush-keyed sides into `Vec<Face>` (temporary shim).
- [ ] `cargo test -p postretro-level-compiler` passes.

**Depends on:** face `brush_index` work (external).

### Task 2: Brush-volume BSP builder (new entry point)
**Description:** Add a new `build_bsp_from_brushes(&[BrushVolume], world_aabb) -> BspTree` alongside the current `build_bsp_tree`. Implement the Phase A descent: candidate-brush tracking, inside-set tracking, brush-plane splitter selection, solidity assigned during construction. No face output yet — leaves have empty `face_indices`. Portal generation is not yet rewired.

**Acceptance criteria:**
- [ ] New builder produces a tree whose leaves' `is_solid` flags are correct for a hollow room, a room with a pillar, a room with a doorway, and adjacent brushes with a narrow air gap.
- [ ] Unit tests cover each shape above, asserting leaf solidity by descending from a test point.
- [ ] No shared state with `build_bsp_tree`; old function remains for now.
- [ ] Plane candidate scoring reuses the existing balance/split penalty with counts redefined as brush-spanning counts.
- [ ] Recursion terminates for well-formed input (pathological inputs return an error rather than stack-overflow).

**Depends on:** Task 1.

### Task 3: Brush-side face extraction
**Description:** Implement Phase B: walk each brush's sides, push polygons through the tree, emit surviving fragments as world faces attached to the adjacent empty leaf. Replaces the face-population path in the builder.

**Acceptance criteria:**
- [ ] New function `extract_faces(tree, brushes) -> Vec<Face>` produces a face list whose members each reference exactly one leaf.
- [ ] No duplicate faces on shared boundaries between adjacent solid brushes.
- [ ] Every emitted face lies on the boundary of an empty leaf.
- [ ] For a hollow-room test: face count matches the interior surface count (six quads for a simple room, no interior duplicates).
- [ ] For two adjacent-but-not-touching brushes: the air gap between them appears as an empty leaf with bounding faces on both brush-facing sides.
- [ ] Tests cover the three cases from Task 2 plus shared-boundary dedup.

**Depends on:** Task 2.

### Task 4: Swap the pipeline over
**Description:** Replace `build_bsp_tree` + `classify_leaf_solidity` + CSG face clipping in `partition.rs` orchestration with the new `build_bsp_from_brushes` + `extract_faces` pair. Remove the old functions and their tests (keep the geometry fixtures — rewrite the tests to use brush inputs). CSG face clipping in `csg.rs` is removed because Phase B subsumes it; keep `geometry_utils::split_polygon` — it's reused.

**Acceptance criteria:**
- [ ] `partition.rs` wires brush volumes directly into the new builder.
- [ ] `classify_leaf_solidity` and the old `build_bsp_tree` are deleted.
- [ ] `csg.rs` face clipping pass is deleted. `geometry_utils.rs` is retained.
- [ ] Portal generation receives the new tree and produces portals for all adjacent empty-leaf pairs. No portal count regression on test maps.
- [ ] `assets/maps/test.map` compiles to a `.prl` that loads and renders in the engine. Visual parity with pre-refactor output on the same map.
- [ ] `cargo test -p postretro-level-compiler` passes.
- [ ] `RUST_LOG=info cargo run -p postretro -- assets/maps/test.prl` runs without geometry errors.

**Depends on:** Task 3.

### Task 5: Test-map regression sweep
**Description:** Compile every `.map` under `assets/maps/` (and any test fixtures in the compiler crate) with the new pipeline. Manually inspect each in the engine. Capture any divergence — faces missing, unexpected solid leaves, portal-count changes — in issues or follow-up tasks.

**Acceptance criteria:**
- [ ] Every existing test map compiles without errors or warnings from the new stages.
- [ ] Every existing test map loads and renders in the engine at visual parity with the pre-refactor output.
- [ ] Portal count, leaf count, and face count per map are recorded in the task notes for the pre/post comparison.
- [ ] Any regression is either fixed before merging or split into a follow-up task with a reproducer.

**Depends on:** Task 4.

### Task 6: Documentation update
**Description:** Update `context/lib/build_pipeline.md` §PRL Compilation to describe the brush-volume pipeline. Remove the "CSG face clipping" stage from the pipeline diagram and prose; the stage no longer exists. Clarify that leaf solidity is established during construction, not post-hoc. Reference the `Face` extraction step as "brush side projection."

**Acceptance criteria:**
- [ ] Pipeline diagram in `build_pipeline.md` reflects: parse → BSP construction (brush-volume) → face extraction → portal generation → portal vis → geometry → pack.
- [ ] "CSG face clipping" section is removed from the document.
- [ ] Description of leaf solidity is updated to reflect structural derivation.
- [ ] Style-guide compliant: no function names in the durable doc.

**Depends on:** Task 4. Can overlap Task 5.

---

## Sequencing

**Phase 1 (sequential):**
- Task 1 — data model change. Everything downstream depends on brush-keyed sides.

**Phase 2 (sequential):**
- Task 2 — builder. Needs Task 1's data shape.
- Task 3 — face extraction. Needs the tree from Task 2 to emit faces into.

Tasks 2 and 3 are sequential because Task 3 consumes the tree Task 2 produces and both touch `partition/bsp.rs`.

**Phase 3 (sequential):**
- Task 4 — pipeline swap. Removes the old path once the new path is proven. Touches `partition.rs`, `csg.rs`, and test fixtures.

**Phase 4 (concurrent):**
- Task 5 — regression sweep on test maps.
- Task 6 — documentation update.

These can run in parallel: Task 5 is validation, Task 6 touches only `context/lib/build_pipeline.md`.

---

## Risks and Mitigations

| Risk | Mitigation |
|---|---|
| New builder loops or stack-overflows on pathological brush configurations. | Hard recursion depth cap. Emit a compiler error rather than panic. Test maps include a deliberately messy brush pile. |
| Face extraction produces fewer faces than the current pipeline on a valid map (dropped geometry). | Task 5 captures face counts pre/post per map. Any unexplained reduction blocks merge. Test on the hollow-room-with-pillar fixture that already exercises the failure mode. |
| Face extraction produces duplicates on shared brush boundaries. | Explicit "adjacent leaf is this brush's solid" check in emission. Unit test on two abutting brushes asserts exactly one face per shared surface. |
| Plane candidate selection from brush planes yields very different trees than the face-plane version, regressing portal counts or leaf balance. | Task 5 captures leaf/portal counts. The scoring function is tunable without changing the algorithm. Acceptable if counts differ but visual output and PVS are unchanged. |
| Portal generation breaks because leaves are now solid where they weren't, or vice versa. | The parallel `brush_index` work already moves solidity to an ownership basis, so the portal stage should already tolerate accurate solidity. Verify with the existing portal-vis test maps in Task 5. |
| CSG deletion loses a subtle behavior (e.g., texture priority on shared surfaces). | Before deleting `csg.rs`, document what it does and confirm face extraction's "owning brush" choice preserves the intended texture. May need a tiebreaker rule for coplanar brush sides. |
| `classify_leaf_solidity`'s tight/loose epsilon tuning gets lost and a new class of tolerance bugs emerges during Phase A classification. | Phase A uses structural tests (plane-vs-plane containment), not centroid tests. Epsilons become a splitter-precision concern, not a classification concern, which is a cleaner problem domain. |
| Task 1's data model change breaks CSG before CSG is removed. | Task 1 includes an adapter so CSG keeps working. Task 4 removes both together. |

---

## Acceptance Criteria

The refactor is done when all of the following hold:

1. `build_bsp_from_brushes` is the single BSP builder. `build_bsp_tree` and `classify_leaf_solidity` are deleted.
2. `csg.rs` face clipping is deleted. Face-into-solid-brush culling is handled by face extraction.
3. Every leaf's `is_solid` flag is assigned during construction; no post-pass.
4. Every existing test map in `assets/maps/` compiles, loads, and renders at visual parity with the pre-refactor output.
5. Unit tests cover: hollow room, room with central pillar, adjacent brushes with narrow air gap, two abutting brushes (shared face dedup), floating brush surrounded by air.
6. `context/lib/build_pipeline.md` reflects the new pipeline shape.
7. `cargo test --workspace` passes. `cargo clippy --workspace -- -D warnings` passes.
8. Portal generation and PVS computation produce equivalent output on test maps (per-leaf PVS bitsets may differ in leaves whose geometry changed, but total visible-leaf counts should be stable).

---

## Complexity

Honest assessment:

- **Files touched:** `postretro-level-compiler/src/partition/bsp.rs` (rewrite), `partition/types.rs` (minor additions), `partition.rs` (wiring), `map_data.rs` (brush-side grouping), `parse.rs` (emit brush sides), `csg.rs` (delete), and the tests in each. Downstream: `context/lib/build_pipeline.md`.
- **Conceptual difficulty:** high for Phase A and Phase B, low for everything else. The algorithm itself is well-documented in qbsp, but adapting it to our epsilon conventions, glam types, and existing tree shape is where the bugs will live. The "emit a face only into an empty leaf adjacent to the owning brush's solid region" check needs careful definition — it's the step most likely to produce off-by-one or duplicated geometry.
- **Test surface:** medium. The hollow-room-with-pillar fixture already catches the bug class we care about. A handful of additional fixtures (shared-boundary, narrow-gap, floating brush) cover the new invariants. Integration tests use existing `assets/maps/`.
- **Blast radius:** confined to the compiler crate. The engine runtime, PRL format, and portal/PVS algorithms are unaffected. The one external dependency is the parallel `brush_index` work, which is a prerequisite.
- **Reversibility:** high. If Task 5 reveals regressions that can't be fixed quickly, the swap in Task 4 is the revertible boundary. Tasks 1–3 add new code alongside the old, so reverting Task 4 restores the current pipeline.

Not a weekend task. Not a months-long epic. Roughly the same shape as `portal-bsp-vis` but contained to the construction layer.

---

## What Carries Forward

| Capability | Enables |
|---|---|
| Per-leaf brush ownership known during construction | Brush-entity properties (e.g., `func_detail`, `func_illusionary`) applied correctly to leaf contents without a separate pass. |
| Face extraction from brush sides | Detail brushes that contribute faces without splitting the structural tree (qbsp's detail brush handling is a natural extension). |
| Structural solidity | Reliable point-in-solid queries for tooling: leak detection, brush overlap warnings, automated playtest collision. |
| Cleaner CSG (no separate face-clipping pass) | Simpler compile pipeline; one fewer stage to debug when geometry looks wrong. |
| Tracked inside-set during descent | Future support for brush-level materials (volumetric fog, water, reverb zones) that need to know "which brush(es) contain this region." |

### Replaced

| Removed artifact | Replacement |
|---|---|
| `classify_leaf_solidity` heuristic with centroid tests | Structural solidity assigned during construction. |
| `csg.rs` Sutherland-Hodgman face-into-brush clipping | Face extraction via brush-side projection through the tree. |
| `SOLID_EPSILON` and `FACE_SOLID_EPSILON` tuning in `bsp.rs` | Structural tests — no centroid epsilons needed. `PLANE_EPSILON` for splitter arithmetic remains. |
| Face-driven splitter candidate selection | Brush-plane splitter candidates (superset — every face-plane was a brush-plane, plus internal brush boundaries no face sits on). |

---

## Notes

### Open questions

- **Splitter candidate pool.** qbsp picks from all planes of candidate brushes, which includes internal-seeming planes. Should we include only "visible" planes (sides that bound at least one brush in the current candidate set) or the full half-plane set? Start with all candidate brushes' planes; prune if tree quality suffers.
- **Texture attribution on shared boundaries.** When two brushes share a plane, both contribute a face at extraction time. One wins. Propose: the brush with lower index (stable), with a compiler warning if textures differ. Alternative: keep both if the engine supports it (it does — nothing forbids coincident faces on opposite leaves).
- **World bounds.** The initial region needs a world AABB. Use the union of all brush AABBs with a small slack margin. This is cleaner than today's implicit unbounded root.
- **Recursion budget.** Pick a hard depth cap (256?) and emit a compiler error on overflow. Today's builder doesn't cap depth; pathological inputs are a latent bug.

### Alternatives considered

- **Keep face-based construction, rely on `brush_index` for classification.** This is the parallel work. It fixes the immediate bugs but leaves the architectural mismatch intact. This plan is the follow-up that closes the mismatch.
- **Hybrid: face-based splitter choice, brush-based classification.** Splitter selection from face planes ignores brush boundaries that no face lies on, so narrow-gap bugs persist. Rejected.
- **Precompute all brush intersections as a BSP input.** Equivalent to Phase A but less incremental and harder to test. Rejected.
