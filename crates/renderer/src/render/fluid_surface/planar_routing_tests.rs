use super::*;
use crate::render::fluid_reflection_frame_test_fixtures::{green_probe, pool};

// Runs follow plane identity: a probe-bound up-facing face at one height is
// one run with that height, an unbound or down-facing face a run with none,
// and runs tile every material range exactly, never crossing one.
#[test]
fn planar_routing_splits_ranges_by_bound_horizontal_plane() {
    let mut down = pool(2, 1.0, &[-1.0, 1.0]);
    down.surface_faces[0].vertices.reverse();
    let volumes = [
        pool(0, 0.0, &[-6.0, 0.0, 2.0]),
        pool(1, 0.3, &[3.0, 5.0]),
        down,
    ];
    // Volume 0's second face binds no probe.
    let probes = green_probe(&[(0, 0), (1, 0), (2, 0)]);
    let mesh = pack_fluid_surfaces(&volumes, &[], 1, None);
    let parts = split_planar_ranges(&mesh, &volumes, Some(&probes));
    let heights: Vec<Option<f32>> = parts.iter().map(|part| part.height).collect();
    assert_eq!(heights, [Some(0.0), None, Some(0.3), None], "{parts:?}");
    let mut next = 0;
    for part in &parts {
        assert_eq!(part.start, next, "runs tile the index buffer: {parts:?}");
        assert!(part.count > 0 && part.count % 3 == 0, "{part:?}");
        next = part.start + part.count;
    }
    assert_eq!(next as usize, mesh.indices.len());
    for volume in &mesh.volumes {
        for range in &volume.material_ranges {
            let end = range.index_start + range.index_count;
            assert!(
                parts
                    .iter()
                    .all(|part| part.start + part.count <= range.index_start
                        || part.start >= end
                        || (part.start >= range.index_start && part.start + part.count <= end)),
                "no run crosses a range: {parts:?}"
            );
        }
    }
    // Without a probe section no run has a plane.
    let unbound = split_planar_ranges(&mesh, &volumes, None);
    assert!(unbound.iter().all(|part| part.height.is_none()));
    assert!(same_plane(Some(0.0), Some(PLANE_HEIGHT_TOLERANCE)));
    assert!(!same_plane(Some(0.0), Some(0.3)) && !same_plane(None, Some(0.0)));
}
