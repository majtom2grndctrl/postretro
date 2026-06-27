// Renderer unit tests (split from the original `mod tests`).
// See: context/lib/testing_guide.md

use super::super::*;

#[test]
fn compute_fog_cell_mask_culled_unions_visible_cell_masks() {
    let masks = vec![0b001u32, 0b010, 0b101, 0b000]; // 4 cells, 3 fog volumes
    let fog_reachable = [1u32, 2];
    let active = compute_fog_cell_mask(&fog_reachable, Some(&masks), 3, Some(1));
    // cell1→0b010, cell2→0b101 → OR 0b111; camera-cell union (camera_cell=1,
    // already in reachable set) is idempotent here — see
    // compute_fog_cell_mask_camera_cell_union_is_idempotent_when_already_reachable
    assert_eq!(active, 0b111);
}

#[test]
fn compute_fog_cell_mask_drawall_returns_all_canonical_slots() {
    let masks = vec![0u32; 4]; // present but ignored on DrawAll path
    // Empty fog_reachable signals all-active fog fallback sentinel.
    assert_eq!(compute_fog_cell_mask(&[], Some(&masks), 3, Some(0)), 0b111);
    assert_eq!(compute_fog_cell_mask(&[], None, 3, Some(0)), 0b111);
}

#[test]
fn compute_fog_cell_mask_caller_without_mask_table_gets_all_slots() {
    // Helper-level contract: a caller that passes no mask table gets a
    // conservative all-slots result. Modern PRL load rejects canonical fog
    // volumes without FogCellMasks before reaching renderer fog culling.
    let fog_reachable = [0u32, 1, 2];
    assert_eq!(
        compute_fog_cell_mask(&fog_reachable, None, 4, Some(0)),
        0b1111
    );
}

#[test]
fn compute_fog_cell_mask_zero_canonical_volumes_returns_zero() {
    assert_eq!(compute_fog_cell_mask(&[], None, 0, Some(0)), 0);
    assert_eq!(
        compute_fog_cell_mask(&[0u32], Some(&[0xFFu32]), 0, Some(0)),
        0
    );
}

#[test]
fn compute_fog_cell_mask_unions_camera_cell_when_absent_from_fog_reachable() {
    // Camera in cell 3 (not in fog_reachable). Its 0b100 bit must still appear.
    // Regression: portal traversal can transiently omit the camera cell,
    // causing fog the camera is inside to flicker off.
    let masks = vec![0b001u32, 0b010, 0b000, 0b100];
    let fog_reachable = [0u32, 1];
    let active = compute_fog_cell_mask(&fog_reachable, Some(&masks), 3, Some(3));
    // 0b001 | 0b010 (union) | 0b100 (camera cell) = 0b111
    assert_eq!(active, 0b111);
}

#[test]
fn compute_fog_cell_mask_camera_cell_union_is_idempotent_when_already_reachable() {
    let masks = vec![0b001u32, 0b010, 0b100];
    let fog_reachable = [0u32, 2];
    let with_cam = compute_fog_cell_mask(&fog_reachable, Some(&masks), 3, Some(2));
    let without_cam = compute_fog_cell_mask(&fog_reachable, Some(&masks), 3, None);
    assert_eq!(with_cam, without_cam);
    assert_eq!(with_cam, 0b101);
}

#[test]
fn sphere_intersects_any_fog_aabb_inside_passes() {
    let aabbs = vec![(Vec3::new(-1.0, -1.0, -1.0), Vec3::new(1.0, 1.0, 1.0))];
    assert!(sphere_intersects_any_fog_aabb(
        Vec3::new(0.0, 0.0, 0.0),
        0.1,
        &aabbs,
    ));
}

#[test]
fn sphere_intersects_any_fog_aabb_outside_all_drops() {
    let aabbs = vec![
        (Vec3::new(-1.0, -1.0, -1.0), Vec3::new(1.0, 1.0, 1.0)),
        (Vec3::new(50.0, 50.0, 50.0), Vec3::new(52.0, 52.0, 52.0)),
    ];
    assert!(!sphere_intersects_any_fog_aabb(
        Vec3::new(100.0, 100.0, 100.0),
        5.0,
        &aabbs,
    ));
}

#[test]
fn sphere_intersects_any_fog_aabb_empty_list_passes_everything() {
    assert!(sphere_intersects_any_fog_aabb(
        Vec3::new(0.0, 0.0, 0.0),
        1.0,
        &[],
    ));
}

#[test]
fn sphere_intersects_any_fog_aabb_grazing_edge_passes() {
    // distance == radius counts as intersecting (matches sphere_intersects_any_aabb).
    let aabbs = vec![(Vec3::new(0.0, 0.0, 0.0), Vec3::new(1.0, 1.0, 1.0))];
    assert!(sphere_intersects_any_fog_aabb(
        Vec3::new(2.0, 0.5, 0.5),
        1.0,
        &aabbs,
    ));
}
