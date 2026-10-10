// The forward pass's per-vertex static specular owner stream, stamped from id 42's owner table.
// See: context/lib/rendering_pipeline.md §4 (Static specular ownership)

use std::ops::Range;

use postretro_level_format::shadowmask_atlas::{FaceOwners, NO_FACE_OWNERS};
use postretro_render_data::geometry::BvhLeaf;

/// One owner entry per world vertex: its face's id-42 entry, or
/// [`NO_FACE_OWNERS`]. Always `vertex_count` long, so the forward pass never
/// binds a short buffer.
///
/// Id 17 keeps no face index ranges, but the compiler emits exactly one BVH
/// leaf per face, in face order, each covering that face's contiguous index
/// range. Sorting the leaves by `index_offset` recovers face `i`'s range as
/// the `i`-th. When that identity fails (leaf count differs from the face
/// count, ranges leave a gap or overlap, or an index names no vertex), one
/// `[Loader]` error is logged and the stream stays empty: every face then gets
/// no static non-SDF world specular, never another face's owners.
pub(crate) fn build_vertex_spec_owners(
    vertex_count: usize,
    indices: &[u32],
    leaves: &[BvhLeaf],
    face_owners: Option<&[FaceOwners]>,
) -> Vec<FaceOwners> {
    let mut stream = vec![NO_FACE_OWNERS; vertex_count];
    let Some(face_owners) = face_owners else {
        return stream;
    };
    let stamped = face_index_ranges(leaves, face_owners.len(), indices.len()).and_then(|ranges| {
        for (face, (range, owners)) in ranges.into_iter().zip(face_owners).enumerate() {
            for &index in &indices[range] {
                let slot = stream.get_mut(index as usize).ok_or_else(|| {
                    format!(
                        "face {face} names vertex {index} past the {vertex_count}-vertex Geometry"
                    )
                })?;
                *slot = *owners;
            }
        }
        Ok(())
    });
    if let Err(reason) = stamped {
        log::error!(
            "[Loader] ShadowmaskAtlas face owners cannot reach the world vertices: {reason}; \
             this level gets no static non-SDF world specular"
        );
        stream.fill(NO_FACE_OWNERS);
    }
    stream
}

/// Face `i`'s index range is the `i`-th BVH leaf range by `index_offset`.
/// The ranges must tile `0..index_count` exactly, one non-empty leaf per face.
fn face_index_ranges(
    leaves: &[BvhLeaf],
    face_count: usize,
    index_count: usize,
) -> Result<Vec<Range<usize>>, String> {
    if leaves.len() != face_count {
        return Err(format!(
            "{} BVH leaves for {face_count} Geometry faces",
            leaves.len()
        ));
    }
    let mut ranges: Vec<Range<usize>> = leaves
        .iter()
        .map(|leaf| {
            let start = leaf.index_offset as usize;
            start..start + leaf.index_count as usize
        })
        .collect();
    ranges.sort_unstable_by_key(|range| range.start);
    let mut expected = 0;
    for (face, range) in ranges.iter().enumerate() {
        if range.start != expected || range.is_empty() {
            return Err(format!(
                "face {face}'s leaf covers indices {range:?}, expected a non-empty range \
                 starting at {expected}"
            ));
        }
        expected = range.end;
    }
    if expected != index_count {
        return Err(format!(
            "BVH leaves cover {expected} of the {index_count} Geometry indices"
        ));
    }
    Ok(ranges)
}

#[cfg(test)]
mod tests {
    use super::*;
    use log::Level;
    use postretro_level_format::shadowmask_atlas::SHADOWMASK_OWNER_EMPTY;
    use postretro_test_log_capture::LogCapture;

    const E: u16 = SHADOWMASK_OWNER_EMPTY;

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

    /// Three faces as the compiler writes them: face 0 a quad (vertices
    /// 0..4), face 1 a triangle (4..7), face 2 a quad (7..11), each with its
    /// own vertices, indices contiguous in face order.
    fn three_faces() -> (usize, Vec<u32>) {
        let indices = vec![0, 1, 2, 0, 2, 3, 4, 5, 6, 7, 8, 9, 7, 9, 10];
        (11, indices)
    }

    const OWNERS: [FaceOwners; 3] = [[0, 2, E, E], [E, E, E, E], [1, 2, 3, 5]];

    fn loader_errors(capture: &LogCapture) -> Vec<String> {
        capture
            .records()
            .into_iter()
            .filter(|record| record.level == Level::Error && record.message.starts_with("[Loader]"))
            .map(|record| record.message)
            .collect()
    }

    // Leaves arrive in material-bucket order, not face order; sorting by
    // `index_offset` still finds each face's range.
    #[test]
    fn owner_stream_stamps_each_face_entry_onto_its_own_vertices() {
        let (vertex_count, indices) = three_faces();
        let leaves = [leaf(9, 6), leaf(0, 6), leaf(6, 3)];
        let capture = LogCapture::start();
        let stream = build_vertex_spec_owners(vertex_count, &indices, &leaves, Some(&OWNERS));
        assert!(loader_errors(&capture).is_empty());
        assert_eq!(stream.len(), vertex_count);
        assert_eq!(&stream[0..4], &[OWNERS[0]; 4], "face 0's vertices");
        assert_eq!(&stream[4..7], &[NO_FACE_OWNERS; 3], "face 1's vertices");
        assert_eq!(&stream[7..11], &[OWNERS[2]; 4], "face 2's vertices");
    }

    #[test]
    fn owner_stream_without_id42_is_all_empty_at_vertex_count() {
        let (vertex_count, indices) = three_faces();
        let leaves = [leaf(0, 6), leaf(6, 3), leaf(9, 6)];
        let capture = LogCapture::start();
        let stream = build_vertex_spec_owners(vertex_count, &indices, &leaves, None);
        assert_eq!(stream, vec![NO_FACE_OWNERS; vertex_count]);
        assert!(loader_errors(&capture).is_empty(), "absence is not a fault");
    }

    // A face-range mismatch must never hand one face another's owners: the
    // whole stream stays empty, at full length, with one error.
    #[test]
    fn owner_stream_rejects_leaves_that_do_not_tile_the_faces() {
        let (vertex_count, indices) = three_faces();
        let cases: [(&str, Vec<BvhLeaf>, &str); 5] = [
            (
                "a face without a leaf",
                vec![leaf(0, 6), leaf(6, 9)],
                "2 BVH leaves for 3 Geometry faces",
            ),
            (
                "a gap",
                vec![leaf(0, 6), leaf(9, 3), leaf(12, 3)],
                "expected a non-empty range starting at 6",
            ),
            (
                "an overlap",
                vec![leaf(0, 6), leaf(3, 6), leaf(9, 6)],
                "expected a non-empty range starting at 6",
            ),
            (
                "an empty leaf",
                vec![leaf(0, 6), leaf(6, 0), leaf(6, 9)],
                "expected a non-empty range",
            ),
            (
                "indices past the last leaf",
                vec![leaf(0, 6), leaf(6, 3), leaf(9, 3)],
                "cover 12 of the 15 Geometry indices",
            ),
        ];
        for (what, leaves, reason) in cases {
            let capture = LogCapture::start();
            let stream = build_vertex_spec_owners(vertex_count, &indices, &leaves, Some(&OWNERS));
            assert_eq!(stream, vec![NO_FACE_OWNERS; vertex_count], "{what}");
            let errors = loader_errors(&capture);
            assert_eq!(errors.len(), 1, "{what}: {errors:?}");
            assert!(errors[0].contains(reason), "{what}: {errors:?}");
            assert!(
                errors[0].contains("no static non-SDF world specular"),
                "{what}: {errors:?}"
            );
        }

        // An index past the vertex buffer fails the same way, even after
        // earlier faces stamped.
        let capture = LogCapture::start();
        let leaves = [leaf(0, 6), leaf(6, 3), leaf(9, 6)];
        let stream = build_vertex_spec_owners(8, &indices, &leaves, Some(&OWNERS));
        assert_eq!(stream, vec![NO_FACE_OWNERS; 8]);
        let errors = loader_errors(&capture);
        assert_eq!(errors.len(), 1, "{errors:?}");
        assert!(
            errors[0].contains("past the 8-vertex Geometry"),
            "{errors:?}"
        );
    }
}
