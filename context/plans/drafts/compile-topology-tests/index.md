# Compile-Pipeline Topology Tests

> **Status:** draft
> **Depends on:** none. All additions are test-only inside `postretro-level-compiler`. No production code changes required.
> **Related:** commit `6e72040` (portal brush filter) · `postretro-level-compiler/src/visibility/mod.rs` · `postretro-level-compiler/src/portals.rs`

---

## Context

A diagnostic session around the exterior-leaf-culling stage discovered that the BSP builder produces leaves whose spatial regions can straddle brush boundaries, causing portal generation to emit phantom portals through solid material. The exterior flood-fill then walked from the void through walls into interior air, producing false-positive leak detection on every test map. The symptomatic fix (`filter_portals_through_brushes`) drops portals whose polygon centroids lie inside or on a brush plane.

The sealed-box regression test in `visibility/mod.rs` exercises the specific topology that exposed the bug — an axis-aligned hollow cube — but leaves several adjacent topologies untested. This plan adds focused tests that would catch related failure modes, keep the portal filter honest, and turn manual playability verification into automated guarantees.

---

## Goal

Add test coverage for compile-pipeline correctness across map topologies that aren't currently exercised. Each test is a narrow, high-signal check. No production code changes; all additions land in `#[cfg(test)]` modules inside `postretro-level-compiler`.

---

## Tasks

### Task 1 — Spawn-point regression test for real maps

Parse each of `assets/maps/test.map`, `assets/maps/test-3.map`, and `assets/maps/test-4.map`, find their `info_player_start` entity, run the full compile pipeline, assert the entity's leaf is not in the exterior set.

This is a cheap, high-signal guardrail that catches "the compiler broke playable maps" without requiring any hand-authored synthetic geometry. It would have failed for every map in this repo before the portal brush filter landed and will now pass for all of them. Start with this one — it turns the "I can see around inside test-3.prl" manual verification into an automated assertion.

**Placement.** Probably a new test in `parse.rs` or `visibility/mod.rs` that runs the full parse + CSG + partition + portals + filter + exterior flood pipeline on each real .map file and checks the spawn point's leaf. Use `parse_map_file` to get `brush_volumes` and the spawn entity's origin.

### Task 2 — Genuine-leak detection test

Build a synthetic sealed box (like the one in `sealed_box_center_point_is_not_classified_exterior`), but remove one wall brush (or punch a small hole in one wall), run the pipeline, assert the interior IS now in the exterior set.

This is the inverse-sense test that keeps the portal filter honest. We need to be sure the fix didn't trivially disable leak detection, and the only way to know is to construct a map that genuinely leaks and verify the flood-fill still catches it. If this test ever goes green without adjustment, the portal filter has become too aggressive and is hiding real leaks.

**Placement.** Alongside the existing sealed-box test in `visibility/mod.rs`. Reuse the `sealed_box` / `box_faces` / `box_volume` helpers already in that tests module; just omit one slab.

### Task 3 — Non-axis-aligned geometry test

Build a sealed room with at least one angled wall (45° is a reasonable first case) and assert the interior stays interior.

The existing sealed box is trivially axis-aligned. The portal filter's centroid-in-brush test relies on half-plane math that should work for arbitrary orientations, but no test has verified it on angled geometry. Could surface a case where centroid sampling is too coarse for non-axis-aligned portals, or where the `PORTAL_BRUSH_EPSILON` convention misclassifies a portal on an angled brush face.

**Placement.** New test in `visibility/mod.rs`. Will require a helper to build a tilted wall brush (six arbitrary planes, not just axis-aligned ones).

### Task 4 — Multi-room-with-doorway test

Two sealed rooms connected by an opening in a shared wall. Assert both rooms' interiors are not exterior, and that each room's PVS includes leaves in the other room through the doorway.

This tests the positive case (legitimate portals survive) alongside the negative case the existing sealed-box test covers (phantom portals get dropped). The portal filter could in principle over-filter a legitimate doorway portal whose centroid happens to lie near a brush. If that ever happens, this test catches it and the filter's epsilon needs tightening.

**Placement.** New test in `visibility/mod.rs`. PVS verification requires reading back the encoded `LeafPvsSection` after `encode_vis`, similar to the existing `single_empty_leaf_sees_itself` test.

### Task 5 — Pillar-in-room test

A sealed room with a solid pillar in the middle. Validates PVS occlusion: leaves on opposite sides of the pillar should not see each other.

The existing `floating_cube_air_space_leaves_stay_empty` test in `partition/bsp.rs` checks something adjacent but doesn't exercise portals or visibility — it only verifies that narrow air gaps aren't misclassified as solid. This task covers the complementary case: a solid obstacle correctly blocks visibility through it.

**Placement.** New test in `visibility/mod.rs`. Assert that a leaf on one side of the pillar's PVS bitset does not include a leaf on the opposite side.

---

## Files to modify

| File | Crate | Change |
|------|-------|--------|
| `postretro-level-compiler/src/visibility/mod.rs` | `postretro-level-compiler` | Tasks 2, 3, 4, 5 — new `#[cfg(test)]` tests alongside existing sealed-box test |
| `postretro-level-compiler/src/parse.rs` or `visibility/mod.rs` | `postretro-level-compiler` | Task 1 — real-map spawn-point test (placement TBD) |

---

## Acceptance Criteria

1. `cargo test -p postretro-level-compiler` passes with all five new tests green.
2. No production code changes. Any failure during implementation must be fixed by adjusting the test, not by relaxing the assertion or patching production to make the test pass.
3. If Task 2 (genuine leak) or Task 4 (doorway) exposes a bug in `filter_portals_through_brushes` (false positive or false negative), the plan is paused and the finding surfaced to the user before proceeding. These tests are diagnostic by nature and their failure is a signal, not a nuisance.
4. Each test's failure message is informative enough to distinguish "the map is unsealed" from "the filter is too aggressive" from "the BSP topology changed under us" without re-running with extra debug output.

---

## Out of scope

- Fixing `classify_leaf_solidity` or the BSP leaf-straddling issue. Those are separate plans (see Open Questions below).
- Deleting the now-dead `is_solid` field and its downstream branches.
- The "subdivide BSP leaves to brush boundaries" work that would retire the portal filter entirely.
- Any change to production code in `portals.rs`, `partition/bsp.rs`, `visibility/mod.rs`, or `main.rs`.
- Performance benchmarks of the portal filter.
- Renaming `test.map` (that idea was moot once test.map started producing real interior leaves).

---

## Open Questions

1. Should Task 1 live in `parse.rs` tests (where real .map files are already parsed in test fixtures) or in `visibility/mod.rs` (where the full pipeline is tested)? Either works; the distinction is whether spawn-point verification belongs to parsing or to the visibility stage that interprets its meaning.
2. For Task 4, does the existing `encode_vis` / `LeafPvsSection` test infrastructure support "assert leaf A's PVS bitset contains leaf B" directly, or does the test need a small decode helper? The existing `single_empty_leaf_sees_itself` test decompresses PVS manually; if more tests want this, a tiny `assert_pvs_contains(result, a, b)` helper in the tests module would reduce duplication.
3. Should there also be a test for "overlapping brushes" (two world brushes occupying the same volume)? The `classify_leaf_solidity` doc comment explicitly disclaims support for overlapping brushes, but we don't have a test that either verifies graceful behavior or asserts the known limitation. Marked as an open question because it may belong to a separate plan about brush-authoring guarantees, not compile-pipeline topology.
