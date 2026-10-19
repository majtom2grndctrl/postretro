//! Closure's candidate lookup is output-sensitive: on a synthetic ocean its
//! vertex-to-vertex weld tests and vertex-to-edge insert tests grow no faster
//! than n log n in the level's vertex count.

use super::*;
use crate::environment_volumes::closure::{ClosureStats, close_seams};
use crate::environment_volumes::surface_pieces;

/// What closure did on `volumes` (each one entity's brushes), and the
/// vertex count of the faces it started from.
fn closure_work(world: &[String], volumes: &[String]) -> (usize, ClosureStats) {
    let (map, _) = compile(world, volumes);
    let mut pieces = surface_pieces(&map.environment_volumes, &map.brush_volumes);
    let vertices = pieces
        .iter()
        .flatten()
        .map(|piece| piece.vertices.len())
        .sum();
    let stats = close_seams(&mut pieces);
    (vertices, stats)
}

/// A `columns` × `columns` brick pattern of fluid brushes: columns alternate
/// 32 and 48 units wide, and each column's 64-unit rows start 32 units
/// further on than its neighbour's, so every top meets its neighbours'
/// corners mid-edge.
fn brick_ocean(columns: i32) -> Vec<String> {
    let mut brushes = Vec::new();
    let mut x = 0;
    for column in 0..columns {
        let width = if column % 2 == 0 { 32 } else { 48 };
        let offset = if column % 2 == 0 { 0 } else { 32 };
        for row in 0..columns {
            let y0 = row * 64 - offset;
            brushes.push(box_brush([x, y0, 0], [x + width, y0 + 64, 32], "water_tex"));
        }
        x += width;
    }
    brushes
}

/// Asserts closure's weld and insert tests grow no faster than n log n in
/// the level's vertex count from `small` to `large`, and that the larger
/// level welds or inserts at all.
#[track_caller]
fn assert_grows_within_n_log_n(
    what: &str,
    small: (usize, ClosureStats),
    large: (usize, ClosureStats),
) {
    let n_log_n = |n: usize| n as f64 * (n as f64).log2();
    let bound = n_log_n(large.0) / n_log_n(small.0);
    assert!(
        large.0 >= 3 * small.0,
        "{what}: sizes {} and {}",
        small.0,
        large.0
    );
    assert!(large.1.inserted > 0, "{what}: closure inserts nothing");
    for (tests, small_tests, large_tests) in [
        ("weld", small.1.weld_tests, large.1.weld_tests),
        ("insert", small.1.insert_tests, large.1.insert_tests),
    ] {
        let ratio = large_tests as f64 / small_tests as f64;
        assert!(
            ratio <= bound,
            "{what}: {tests} tests {small_tests} at {} vertices, {large_tests} at {}: \
             ratio {ratio}, n log n allows {bound}",
            small.0,
            large.0
        );
    }
}

// Ignored for its fixtures' cost, not closure's: subtracting a long row of
// sibling brushes takes seconds, while closure takes milliseconds.
#[test]
#[ignore = "compiles thousands of brushes; run on demand"]
fn closure_candidate_tests_grow_within_n_log_n_on_a_synthetic_ocean() {
    // One fluid entity of many brushes, then one entity per brush.
    for per_brush in [false, true] {
        let work = |columns: i32| {
            let brushes = brick_ocean(columns);
            let volumes = if per_brush {
                brushes
                    .iter()
                    .map(|brush| fluid(std::slice::from_ref(brush)))
                    .collect()
            } else {
                vec![fluid(&brushes)]
            };
            closure_work(&distant_world(), &volumes)
        };
        assert_grows_within_n_log_n(
            if per_brush {
                "entity per brush"
            } else {
                "one entity"
            },
            // Past 8 columns, where the edge of the ocean still dominates.
            work(16),
            work(32),
        );
    }

    // One long top bordered along one edge by a row of small brushes, a
    // separate entity, whose corners all land on it.
    let work = |pieces: i32| {
        let length = 2 * pieces;
        let long = fluid(&[box_brush([0, -64, 0], [length, 0, 32], "water_tex")]);
        let row: Vec<String> = (0..pieces)
            .map(|i| box_brush([2 * i, 0, 0], [2 * i + 2, 8, 32], "water_tex"))
            .collect();
        closure_work(&distant_world(), &[long, fluid(&row)])
    };
    assert_grows_within_n_log_n("one face beside thousands", work(1000), work(4000));
}
