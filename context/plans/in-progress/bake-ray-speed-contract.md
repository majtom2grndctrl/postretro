# Bake ray speed — contract

## Goal

Cut the CPU time the bake spends on ray queries without a visible change to any baked output and with no new engine runtime cost. A warren-mini lightmap profile put about 78% of the stage in BVH traversal, mostly in the per-node ray-box test. The traversal code is shared, so the same wins reach the SH, delta and billboard-scatter bakes.

## Decisions (owner, 2026-10-08)

- **Bar: visually identical; exact wherever the change allows.** Steps 1–3 below are exact by construction and must keep every `.prl` byte-identical. Step 4 is the only step allowed to move bytes. Consequence: steps 1–3 land and are proven against `main` before step 4 starts, so a hash diff is always unambiguous.
- **Scope: four steps, each kept only if it measurably pays.** A step that does not beat run-to-run noise on warren-mini is reverted, not landed.
  1. Fused node test: one pass computes the slab values once and makes both decisions (stock accept, bound prune).
  2. Last-occluder cache in the lightmap layer bake's occlusion queries.
  3. Flat bake-tree traversal: our own packed node arrays and stack loop in place of the `bvh` crate's enum nodes and iterator. The `bvh` crate still builds the tree.
  4. Sample-point trig hoist in the area-sample targets.
- **Step 4 bumps cache epochs.** The area-sample target function is shared by every stage that calls `soft_visibility`; each such stage's epoch advances, so warm caches re-bake those stages once. Steps 1–3 bump nothing (`build_pipeline.md` §Build Cache, stage version bump rule).
- **Landing: one PR** from `claude/friendly-meitner-f6jmdm`, with measurements and acceptance results in the body.
- **Measurement machine: this container** (4 cores, Linux). Before and after use the same machine, and the conclusion is the ratio. Absolute seconds do not transfer to the owner's yardstick.

## Invariants

- **Accept set.** A node test accepts a box iff the stock `bvh` 0.12 slab test accepts it and its entry distance is `<= prune_limit(bound)`. Any NaN in the six slab values rejects the box, as the stock test does. `prune_limit` and its slack are unchanged.
- **Visit order.** Traversal is depth-first and left-first. The right child's box is tested when the left subtree is finished, using the bound as it stands then, exactly as `BvhTraverseIterator` does. Leaves are yielded in the same order. This keeps closest-hit tie winners unchanged.
- **Occlusion answers.** The occluder cache changes only which triangle is tested first. A hit counts with the same predicate the traversal loop uses (`dist > 0.0 && dist < max_distance`) on the same ray. Cache state is local to one `(light, chart)` closure, never shared across threads.
- **Determinism.** Output must not depend on thread count or scheduling.
- **No runtime or load-time change.** No runtime crate, shipped section layout, or shader changes. The shipped BVH section (id 19) stays byte-identical. Step 4 may change texel and coefficient values but never a section's size or layout, so the engine loads the same number of bytes in the same shape (owner, 2026-10-08: load time must not be noticeably affected).
- **Layering.** All code stays in `postretro-level-compiler`. No new dependencies. No `unsafe`.
- **File size.** `development_guide.md` §2: new traversal code goes in its own module under `ray_traversal`, not appended to `sh_bake.rs` or `lightmap_bake.rs`.

## File ownership

Single track; the session lead builds all four steps in order on the branch.

| Step | Files |
|---|---|
| 1 | `crates/level-compiler/src/ray_traversal.rs` |
| 2 | `lightmap_layer.rs`, `lightmap_bake.rs` (an occluder-cached `segment_clear` variant) |
| 3 | `ray_traversal` (new submodule for the flat tree), call sites in `lightmap_bake.rs`, `sh_bake.rs`, `chunk_light_list_bake.rs`, `billboard_direct_scatter_bake.rs`, the BVH build stage that hands the tree to bakes |
| 4 | `lightmap_bake.rs` sample targets; the epoch constants of each stage that calls `soft_visibility` |

## Acceptance

Baseline: `prl-build` and `scripts-build` built in release from `main` at 61254d8, kept outside the tree.

| Row | Proof | Expected |
|---|---|---|
| A1 (steps 1–3) | Cold `--release` bake of the 11 fixtures in `measurements/bake-performance/fixture-bytes.ps1` (Linux port), digests against the baseline | 11/11 identical |
| A2 (steps 1–3) | Cold `--release` warren-mini `.prl` sha256 against the baseline | identical |
| A3 (step 1) | Unit test: the fused test and the stock-plus-entry test agree on a large randomized set of rays and boxes, including axis-parallel rays, rays in a box face plane, origins inside boxes, and infinite and zero bounds | 0 disagreements |
| A4 (step 3) | Unit test: the flat traversal yields the same leaf sequence as `traverse_iterator` with `BoundedRay` across randomized rays, including a shrinking bound | identical sequences |
| A5 | Existing parity tests (`segment_clear` against the full scan, SH and scatter) | pass |
| A6 (each step) | Warren-mini cold bake, per-stage wall time from the plain reporter, before and after | gain beyond the before-run spread, or the step is reverted |
| A7 (step 4) | Test: lightmap layer texels on the gate fixtures with the old and new sample targets | changed `(texel, light)` visibility values ≤ 0.1% of those evaluated; max change recorded |
| A7b (step 4) | Warren-mini section table: every section id's byte length before and after | identical lengths |
| A8 (step 4) | Each stage calling `soft_visibility` has its epoch advanced | grep in PR body |
| A9 | `/preflight` | green |

## Open

- Step 4's by-eye check of warren-mini in the engine needs a GPU. This container may not have one; if not, it goes to the owner as manual proof.
