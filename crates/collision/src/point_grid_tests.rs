use super::*;

fn boxes() -> Vec<TreeEntry<u32>> {
    let mut entries = Vec::new();
    for z in 0..8 {
        for y in 0..4 {
            for x in 0..8 {
                let min = Vec3::new(x as f32, y as f32, z as f32);
                entries.push(TreeEntry {
                    min,
                    max: min + Vec3::ONE,
                    payload: entries.len() as u32,
                });
            }
        }
    }
    entries
}

fn assert_matches_tree(grid: &PointGrid, entries: &[TreeEntry<u32>], points: &[Vec3]) {
    let tree = AabbTree::build(entries.to_vec());
    for &point in points {
        let mut expected = Vec::new();
        tree.visit_point(point, |index| expected.push(*index));
        let mut actual = Vec::new();
        grid.visit_point(point, |index| actual.push(*index));
        expected.sort_unstable();
        actual.sort_unstable();
        assert_eq!(actual, expected, "point {point:?}");
    }
}

#[test]
fn grid_cell_faces_and_brush_faces_keep_exact_candidates() {
    let entries = boxes();
    let grid = PointGrid::build(entries.clone());
    assert!(!grid.offsets.is_empty());
    let mut points = vec![
        grid.min,
        grid.max,
        grid.min - Vec3::ONE,
        grid.max + Vec3::ONE,
    ];
    for axis in 0..3 {
        for boundary in 0..=grid.dims[axis] {
            let mut point = (grid.min + grid.max) * 0.5;
            point[axis] = grid.min[axis] + boundary as f32 / grid.scale[axis];
            points.push(point);
            let mut below = point;
            below[axis] = below[axis].next_down();
            points.push(below);
            point[axis] = point[axis].next_up();
            points.push(point);
        }
    }
    for entry in &entries {
        points.extend([entry.min, entry.max, (entry.min + entry.max) * 0.5]);
    }
    assert_matches_tree(&grid, &entries, &points);
}

#[test]
fn large_brushes_and_reference_overflow_keep_exact_fallback_candidates() {
    let mut entries = boxes();
    entries.push(TreeEntry {
        min: Vec3::ZERO,
        max: Vec3::new(8.0, 4.0, 8.0),
        payload: 1000,
    });
    let grid = PointGrid::build(entries.clone());
    assert!(
        grid.fallback.len() > 0,
        "large spanning brush uses fallback"
    );
    let capped = PointGrid::build_with_reference_limit(entries.clone(), 3);
    assert!(capped.entries.len() <= 3);
    assert!(capped.fallback.len() > grid.fallback.len());
    let points: Vec<_> = entries
        .iter()
        .flat_map(|entry| [entry.min, entry.max, (entry.min + entry.max) * 0.5])
        .collect();
    assert_matches_tree(&grid, &entries, &points);
    assert_matches_tree(&capped, &entries, &points);
}

#[test]
fn degenerate_and_nonfinite_grid_setup_preserves_tree_queries() {
    for (min, max) in [
        (Vec3::ZERO, Vec3::new(1.0, 0.0, 1.0)),
        (Vec3::ZERO, Vec3::splat(f32::INFINITY)),
        (Vec3::splat(f32::NAN), Vec3::ONE),
        (Vec3::ONE, Vec3::ZERO),
        (Vec3::splat(-1.0e30), Vec3::splat(1.0e30)),
    ] {
        let entries = vec![TreeEntry {
            min,
            max,
            payload: 0,
        }];
        let grid = PointGrid::build(entries.clone());
        assert!(grid.offsets.is_empty());
        assert_matches_tree(
            &grid,
            &entries,
            &[
                Vec3::ZERO,
                Vec3::ONE,
                Vec3::splat(-1.0),
                Vec3::splat(1.0e20),
            ],
        );
    }
}
