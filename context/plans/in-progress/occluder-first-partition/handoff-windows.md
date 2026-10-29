# Windows handoff: review-fix agents B and C

From the Mac session **Mac Build: Occluder-First Partition [a15d47]**, 2026-10-29. The owner asked for the classifier (B) and prism/cap (C) review fixes to run on the Windows machine, with your own sub-agents, while the Mac finishes agent A (portals and doors).

## Setup

- Fetch `origin/claude/occluder-first-partition` and branch from its tip as **`claude/occluder-first-partition-win`**. Push only that branch; never push to `claude/occluder-first-partition` (the Mac merges yours after agent A lands).
- Read, in order: `CLAUDE.md`, `context/lib/index.md`, `context/lib/development_guide.md`, `context/lib/testing_guide.md`, `context/lib/context_style_guide.md`, `context/lib/build_pipeline.md` §Compiler pipeline, then this brief folder's `index.md` (Decisions and Acceptance are the contract; note the Clusters and wall-free-ends Decisions were reworded on 2026-10-29), `research.md` §Shadow prisms, and `plan.md` (Corrections, Delegated answers, §Block rulings, especially "After the review panel", and Tasks 9i–9m, 10).
- **Do not edit agent A's files** (in progress on the Mac): `crates/level-compiler/src/partition/door_recognition.rs`, `partition/brush_bsp/occluder_tier.rs`, `portals/occluder_tier.rs`, `portals/occluder_tier/tests.rs`, `pipeline/runtime_cells/door_tests.rs`. If a fix needs one of them, stop and report it instead.
- Unlike the Mac's edit-only agents, you can compile and test on your machine. Compiler internals test with `cargo test -p postretro-level-compiler --bin prl-build` (`--lib` reaches none of them); the bin target was 2,016 passed / 23 ignored before the fix pass, plus whatever the Mac's small fixes added. Every implementation agent does one self-review-and-fix round of its diff before reporting.
- B before C: C's prism gate reuses the classifier face test that B may touch in `broad_face.rs`.
- Verify each finding against live code before fixing; drop any that no longer hold and say so. Findings are from a read-only review panel; line numbers are approximate.

## Agent B: classifier and clusters

Files: `crates/level-compiler/src/partition/occluder_classify.rs`, `partition/occluder_classify/` (incl. `broad_face.rs`, `test_fixtures.rs`), `partition/occluder_cluster.rs`, `partition/occluder_cluster/` (`section.rs`, `coverage.rs`; `contact.rs` already fixed), and their tests (`portals/occluder_tier/classification_tests.rs`, `cluster_tests.rs`, `classification_test_fixtures.rs`).

1. **Section normal depends on world orientation** (`section.rs` ~:127-140 `candidate_normals`): one representative per 10° group, the first in lexicographic xyz order. A facade turned 45° beside a touching exempt sign at 50° is judged on the sign's normal; no two strips share a slab, and the facade stops occluding (threatens A11/A12 at 45°, R5). Try each member's own normal (every distinct normal) so the result is orientation-free.
2. **A member set seen once is judged under one normal only** (`section.rs` ~:109, `seen.insert`): two touching 1.5 m cubes stacked into a 3 m pillar in a 2.5 m corridor form the same set under X and Z; whichever sorts first decides, so turning the map 90° flips the verdict. Judge each set under every normal it forms under and take occludes-if-any, deterministically.
3. **Brush frame depends on side order** (`occluder_classify.rs` ~:466-474): only side pairs i<j are tried (u = a, v = b⊥a); for near-perpendicular sides swapping them gives a frame tilted up to ~1e-4 rad, shifting a 10 m near-cube's extents ~1 mm (= `AXIS_TIE_EPSILON_M`), so tie and R18 verdicts can depend on side order. Try both orders of each pair.
4. **A face leaning back reads itself as the floor** (`broad_face.rs` ~:224-229 `drop_from_top`, and `broad_face_floor`): the drop starts at top + 0.02·normal and goes straight down; when the face leans back the line re-enters the brush after ~0.02/sinθ, so a solid-backed wall leaning 0.5° reads `LowTop`. Start the drop horizontally clear of the brush.
5. **Roofline uses the lower of the two floors** (`occluder_cluster.rs` ~:221-226, `coverage.rs` ~:94-96): a balustrade (0.1 m posts every 1 m on a 0.3 m rail, 2.3 m tall) on a deck edge 5 m up reads ~100% coverage (true ~20%) and occludes. Use the floor in front of the face being judged (Clusters Decision: "the see-over height above the floor in front").
6. **An occluding section can have no tier plane** (`occluder_cluster.rs` ~:35, ~:269-280 `outline_planes`): a plane is flagged only within ~0.003° of a box axis while sections admit members within 10°. Flag member planes within the section's agreement angle of the box faces (or otherwise guarantee an occluding section bounds the tier), with a test.
7. **Duplicate helper** (`section.rs` ~:42-50 `Member::broad_normals` re-implements `thinnest_axes` from `occluder_classify.rs` ~:423-428): reuse it.
8. Comment drift: production comments citing plan pin IDs (`occluder_classify.rs` ~:42-46, `occluder_cluster.rs` ~:174) should state the reason inline (pins stay in test fixtures); `occluder_classify.rs` ~:66-67 `with()` `#[allow(dead_code)]` needs its reason (bin-side-only caller); `occluder_flags()` doc (~:390-397) vs a `SectionMember` with no flagged plane; any comment quoting the old Clusters wording "bounding broad face passes" → the reworded Decision.
9. Tests: 45° facade + 50° sign occludes; stacked cubes give the same verdict rotated 90°; near-perpendicular side swap gives the same frame and verdict; leaning solid-backed wall occludes; deck-edge balustrade stays exempt; off-axis section bounds the tier.

## Agent C: wall free ends, cap, shipped cells

Files: `crates/level-compiler/src/partition/brush_bsp/shadow_prism.rs`, `partition/brush_bsp/cell_cap.rs`, `pipeline/runtime_cells.rs`, `pipeline.rs` (call sites only), `pipeline/runtime_cells/{prism_tests,cap_tests,tests,leak_tests}.rs`.

1. **Prism floor gate → the classifier's face test** (owner ruling, `plan.md` §Block "After the review panel"; Decision "A wall's free ends…" now reads "both broad faces counting under the occluder rule's face test (most of its open samples at least the floor deep)"): today `broad_face_deepest_open_depth` passes on any one deep sample. Use the classifier's face predicate (share one function with `broad_face.rs`, don't duplicate).
2. **A joint between in-line wall brushes reads as a free end** (`shadow_prism.rs` ~:262, ~:268): the probes sit 0.1–1.0 m in front of the face past the end, never inside the wall's own thickness, so wall A x 0..20 abutting wall B x 20..40 (both z 8..8.3, 4 m tall) gets a splitter at x = 20. Also probe within the wall's thickness just past the end; solid there means abutting. Same gap for a perpendicular wall under 0.1 m thick. Test both.
3. **Two-layer walls get no prism**: two 0.15 m brushes back to back each find their inner face fronting the other brush, so neither gets a prism, reopening C18's leak for that wall. Measure the opposite face's open space through touching solid (the composite wall's far face). Test with a C18-style fixture whose wall is two layers.
4. **Prism overlap tolerance** (`shadow_prism.rs` ~:45, ~:214): overlap uses a 1e-4 shrink while splitting needs `REGION_CLASSIFICATION_EPSILON` (1e-3) spanning, so a region reaching 0.1–1 mm into a prism is offered its end planes (extra portals). Align the overlap tolerance with the spanning tolerance. (Agent A is fixing the matching door case in `door_recognition.rs`.)
5. **Cap open-slice guard is brush-order dependent when a grid line lies on a brush face** (`cell_cap.rs` ~:226 `slice_mostly_open` via `OpenSpace::open_area`, which files on-plane polygons to the front side): blocks solid on either side of a grid line flip the 50% verdict with first-pass brush order. Evaluate symmetrically (e.g. both sides at ±epsilon, take the consistent reading); add a permutation test.
6. **`ShippedCells::build` duplicates the pipeline sequence** (`runtime_cells.rs` ~:67-88 vs `pipeline.rs` ~:552-560): make the pipeline and the test path call one shared constructor.
7. Comment drift: `cell_cap.rs` header (~:1-3) and ~:116 omit the prism rank (order is occluder → wall free ends → door → cap); `shadow_prism.rs` ~:19-23 "prism sections" → "deep parts of a prism"; `runtime_cells.rs` "today's tree" (×2) → "the single-tier tree of every static brush".
8. C18 (`walk_from_in_front_of_a_laned_wall_draws_no_cell_holding_space_hidden_behind_it`) must stay at 0.00%.

## After B and C

On Windows: `cargo fmt --check`, `cargo clippy --target-dir target/preflight-clippy -- -D warnings`, `cargo test --no-fail-fast`, `cargo check --release`. Report the counts and any failure with its file.

## Report back

Push `claude/occluder-first-partition-win`, then message **Mac Build: Occluder-First Partition [a15d47]** (or tell the owner) with: the branch tip sha; per finding, fixed / dropped (with reason) and the test names; any change you made to Decision-adjacent behaviour; the Windows gate result; and anything that needed one of agent A's files.
