// The glossy overlay's per-vertex world probe index stream, stamped from id
// 54's world binding list (id-17 face numbering).
// See: context/lib/rendering_pipeline.md §6, §7.3a (Glossy world faces)

use postretro_level_format::reflection_probes::ReflectionProbeWorldBinding;
use postretro_render_data::geometry::BvhLeaf;

use crate::prl_face_ranges::face_index_ranges;

/// The stream value of a vertex whose face binds no probe: the fluid probe
/// index's no-probe value, so both reflectors read one sentinel.
pub const NO_WORLD_PROBE: u32 = u32::MAX;

/// One probe index per world vertex: its face's bound probe, or
/// [`NO_WORLD_PROBE`]. `None` (no allocation) when nothing binds: an empty
/// list, or faces the BVH leaves cannot reach. `face_count` is Geometry (id
/// 17)'s face count, which the bindings index; `probe_count` bounds the probe
/// indices the stream may hold, and a binding past it binds nothing.
///
/// Each face's indices come from [`face_index_ranges`], with one leaf per
/// face: a face with no leaf leaves the leaves short of `face_count`, and the
/// stream is refused rather than let a face read the next face's range. The
/// compiler gives every face its own vertices; a vertex two faces
/// share whose bindings differ could carry only one probe, so the stream is
/// refused whole (logged once) rather than let one face read another's probe.
pub fn build_vertex_world_probes(
    vertex_count: usize,
    indices: &[u32],
    leaves: &[BvhLeaf],
    face_count: usize,
    bindings: &[ReflectionProbeWorldBinding],
    probe_count: usize,
) -> Option<Vec<u32>> {
    if bindings.is_empty() {
        return None;
    }
    match stamp(
        vertex_count,
        indices,
        leaves,
        face_count,
        bindings,
        probe_count,
    ) {
        Ok(stream) => Some(stream),
        Err(reason) => {
            log::error!(
                "[PRL] ReflectionProbes world bindings cannot reach the world vertices: \
                 {reason}; glossy world faces draw no reflection"
            );
            None
        }
    }
}

fn stamp(
    vertex_count: usize,
    indices: &[u32],
    leaves: &[BvhLeaf],
    face_count: usize,
    bindings: &[ReflectionProbeWorldBinding],
    probe_count: usize,
) -> Result<Vec<u32>, String> {
    let ranges = face_index_ranges(leaves, face_count, indices.len())?;
    let mut stream = vec![NO_WORLD_PROBE; vertex_count];
    const UNSTAMPED: usize = usize::MAX;
    let mut stamped_by = vec![UNSTAMPED; vertex_count];
    for binding in bindings {
        let face = binding.face as usize;
        let Some(range) = ranges.get(face) else {
            return Err(format!("face {face} is outside the {} faces", ranges.len()));
        };
        if binding.probe as usize >= probe_count {
            continue;
        }
        for &index in &indices[range.clone()] {
            let vertex = index as usize;
            let (Some(slot), Some(first)) = (stream.get_mut(vertex), stamped_by.get_mut(vertex))
            else {
                return Err(format!(
                    "face {face} names vertex {index} past the {vertex_count}-vertex Geometry"
                ));
            };
            if *first != UNSTAMPED && *first != face && *slot != binding.probe {
                return Err(format!(
                    "faces {} and {face} share vertex {index} but bind different probes",
                    *first
                ));
            }
            *slot = binding.probe;
            *first = face;
        }
    }
    // A shared vertex an unbound face also draws keeps the bound face's
    // probe; the overlay redraws the unbound face from the same vertex, so
    // refuse that too.
    for (face, range) in ranges.iter().enumerate() {
        // The loader refuses a list out of order, so this is one search.
        if bindings
            .binary_search_by_key(&face, |binding| binding.face as usize)
            .is_ok()
        {
            continue;
        }
        if let Some(&index) = indices[range.clone()].iter().find(|&&index| {
            stamped_by
                .get(index as usize)
                .is_some_and(|&by| by != UNSTAMPED)
        }) {
            return Err(format!(
                "unbound face {face} shares vertex {index} with a bound face"
            ));
        }
    }
    Ok(stream)
}

#[cfg(test)]
mod tests {
    use super::*;
    use log::Level;
    use postretro_test_log_capture::LogCapture;

    fn leaf(index_offset: u32, index_count: u32) -> BvhLeaf {
        BvhLeaf {
            aabb_min: [0.0; 3],
            material_bucket_id: 0,
            aabb_max: [1.0; 3],
            index_offset,
            index_count,
            cell_id: 0,
            chunk_range_start: 0,
            chunk_range_count: 0,
        }
    }

    fn bind(face: u32, probe: u32) -> ReflectionProbeWorldBinding {
        ReflectionProbeWorldBinding { face, probe }
    }

    /// Face 0 a quad (vertices 0..4), face 1 a triangle (4..7), face 2 a
    /// quad (7..11), leaves in bucket order rather than face order.
    fn three_faces() -> (usize, Vec<u32>, [BvhLeaf; 3]) {
        let indices = vec![0, 1, 2, 0, 2, 3, 4, 5, 6, 7, 8, 9, 7, 9, 10];
        (11, indices, [leaf(9, 6), leaf(0, 6), leaf(6, 3)])
    }

    #[test]
    fn world_probe_stream_stamps_each_bound_face_and_leaves_the_rest_unbound() {
        let (vertices, indices, leaves) = three_faces();
        let stream =
            build_vertex_world_probes(vertices, &indices, &leaves, 3, &[bind(0, 3), bind(2, 1)], 4)
                .expect("bound faces reach their vertices");
        let n = NO_WORLD_PROBE;
        assert_eq!(stream, vec![3, 3, 3, 3, n, n, n, 1, 1, 1, 1]);
    }

    #[test]
    fn world_probe_stream_allocates_nothing_without_world_bindings() {
        let (vertices, indices, leaves) = three_faces();
        assert_eq!(
            build_vertex_world_probes(vertices, &indices, &leaves, 3, &[], 4),
            None
        );
    }

    #[test]
    fn world_probe_stream_skips_a_binding_past_the_installed_probes() {
        let (vertices, indices, leaves) = three_faces();
        let stream =
            build_vertex_world_probes(vertices, &indices, &leaves, 3, &[bind(1, 4)], 4).unwrap();
        assert!(stream.iter().all(|&probe| probe == NO_WORLD_PROBE));
    }

    /// The `[PRL]` errors `capture` holds.
    fn prl_errors(capture: &LogCapture) -> usize {
        capture
            .records()
            .into_iter()
            .filter(|record| record.level == Level::Error && record.message.starts_with("[PRL]"))
            .count()
    }

    // A binding is checked against id 17's face count, not the leaf count: a
    // face with no triangle has no leaf, so the leaves fall short of the faces
    // and the stream is refused, rather than face 2's binding stamping the
    // leaf range that belongs to another face.
    #[test]
    fn world_probe_stream_refuses_leaves_short_of_the_geometry_face_count() {
        let capture = LogCapture::start();
        let (vertices, indices, leaves) = three_faces();
        assert_eq!(
            build_vertex_world_probes(vertices, &indices, &leaves, 4, &[bind(2, 0)], 1),
            None,
            "four faces, three leaves"
        );
        assert_eq!(prl_errors(&capture), 1, "the refused stream logs once");
    }

    #[test]
    fn world_probe_stream_refuses_a_binding_past_the_geometry_face_count() {
        let capture = LogCapture::start();
        let (vertices, indices, leaves) = three_faces();
        assert_eq!(
            build_vertex_world_probes(vertices, &indices, &leaves, 3, &[bind(3, 0)], 1),
            None
        );
        assert_eq!(prl_errors(&capture), 1, "the refused stream logs once");
    }

    #[test]
    fn world_probe_stream_refuses_a_vertex_shared_by_faces_binding_differently() {
        let capture = LogCapture::start();
        // Faces 0 and 1 share vertex 2.
        let indices = vec![0, 1, 2, 2, 3, 4];
        let leaves = [leaf(0, 3), leaf(3, 3)];
        for bindings in [[bind(0, 0), bind(1, 1)].as_slice(), &[bind(0, 0)]] {
            assert_eq!(
                build_vertex_world_probes(5, &indices, &leaves, 2, bindings, 2),
                None
            );
        }
        assert_eq!(prl_errors(&capture), 2, "each refused stream logs once");
    }
}
