// The forward pass's per-vertex static specular owner stream, stamped from id 42's owner table.
// See: context/lib/rendering_pipeline.md §4 (Static specular ownership)

use postretro_level_format::shadowmask_atlas::{FaceOwners, MaskOffset, SpecOwnerVertex};
use postretro_render_data::geometry::BvhLeaf;

use crate::prl_face_ranges::face_index_ranges;

/// One owner entry per world vertex: its face's id-42 owners and mask offset,
/// or [`SpecOwnerVertex::EMPTY`]. Always `vertex_count` long, so the forward
/// pass never binds a short buffer. `face_owners` holds id 42's owners and
/// mask offsets, one of each per face.
///
/// Each face's indices come from [`face_index_ranges`]. One non-empty leaf
/// per face holds because the compiler refuses to pack id 42 when a face has
/// no triangle.
///
/// Vertices must not be shared between faces whose entries differ, in owners
/// or in mask offset: the stamp is per vertex, so a shared vertex could carry
/// only one of them, and the other face would read another's mask texels. The
/// compiler un-shares vertices per face, but this is checked here rather than
/// trusted. Faces that share a vertex and agree on their whole entry are
/// harmless and allowed.
///
/// When any of that fails (leaf count differs from the face count, ranges
/// leave a gap or overlap, an index names no vertex, or a shared vertex is
/// claimed by conflicting entries), one `[PRL]` error is logged and the stream
/// stays empty: every face then gets no static non-SDF world specular, never
/// another face's owners or mask.
pub(crate) fn build_vertex_spec_owners(
    vertex_count: usize,
    indices: &[u32],
    leaves: &[BvhLeaf],
    face_owners: Option<(&[FaceOwners], &[MaskOffset])>,
) -> Vec<SpecOwnerVertex> {
    let mut stream = vec![SpecOwnerVertex::EMPTY; vertex_count];
    let Some((face_owners, mask_offsets)) = face_owners else {
        return stream;
    };
    // The face that stamped each vertex (`UNSTAMPED` until one does), so a
    // second face can be compared against the entry already there. Id 42's face
    // count is a u32, so every face index fits.
    const UNSTAMPED: u32 = u32::MAX;
    let mut stamped_by = vec![UNSTAMPED; vertex_count];
    // Id 42 parses one offset per owner entry; zipping lists of two lengths
    // would hand faces past the shorter one nothing, silently.
    let lengths_match = if face_owners.len() == mask_offsets.len() {
        Ok(())
    } else {
        Err(format!(
            "{} owner entries but {} mask offsets",
            face_owners.len(),
            mask_offsets.len()
        ))
    };
    let stamped =
        lengths_match.and_then(|()| face_index_ranges(leaves, face_owners.len(), indices.len()));
    let stamped = stamped.and_then(|ranges| {
        let entries = face_owners
            .iter()
            .zip(mask_offsets)
            .map(|(&owners, &mask_offset)| SpecOwnerVertex {
                owners,
                mask_offset,
            });
        for (face, (range, entry)) in ranges.into_iter().zip(entries).enumerate() {
            for &index in &indices[range] {
                let vertex = index as usize;
                let (Some(slot), Some(first)) =
                    (stream.get_mut(vertex), stamped_by.get_mut(vertex))
                else {
                    return Err(format!(
                        "face {face} names vertex {index} past the {vertex_count}-vertex Geometry"
                    ));
                };
                let face_id = face as u32;
                if *first != UNSTAMPED && *first != face_id && *slot != entry {
                    let what = match slot.owners == entry.owners {
                        true => "mask offsets",
                        false => "owner entries",
                    };
                    return Err(format!(
                        "faces {} and {face} share vertex {index} but their {what} differ",
                        *first
                    ));
                }
                *slot = entry;
                *first = face_id;
            }
        }
        Ok(())
    });
    if let Err(reason) = stamped {
        log::error!(
            "[PRL] ShadowmaskAtlas face owners cannot reach the world vertices: {reason}; \
             this level gets no static non-SDF world specular"
        );
        stream.fill(SpecOwnerVertex::EMPTY);
    }
    stream
}

#[cfg(test)]
mod tests {
    use super::*;
    use log::Level;
    use postretro_level_format::shadowmask_atlas::{NO_FACE_OWNERS, SHADOWMASK_OWNER_EMPTY};
    use postretro_test_log_capture::LogCapture;

    const E: u16 = SHADOWMASK_OWNER_EMPTY;
    const EMPTY: SpecOwnerVertex = SpecOwnerVertex::EMPTY;

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
    /// Each owned face's mask chart sits at its own offset; the unowned face's
    /// is zero.
    const OFFSETS: [MaskOffset; 3] = [[-8, 4], [0, 0], [12, -20]];

    fn stamp(
        vertex_count: usize,
        indices: &[u32],
        leaves: &[BvhLeaf],
        owners: &[FaceOwners],
        offsets: &[MaskOffset],
    ) -> Vec<SpecOwnerVertex> {
        build_vertex_spec_owners(vertex_count, indices, leaves, Some((owners, offsets)))
    }

    fn entry(face: usize) -> SpecOwnerVertex {
        SpecOwnerVertex {
            owners: OWNERS[face],
            mask_offset: OFFSETS[face],
        }
    }

    fn loader_errors(capture: &LogCapture) -> Vec<String> {
        capture
            .records()
            .into_iter()
            .filter(|record| record.level == Level::Error && record.message.starts_with("[PRL]"))
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
        let stream = stamp(vertex_count, &indices, &leaves, &OWNERS, &OFFSETS);
        assert!(loader_errors(&capture).is_empty());
        assert_eq!(stream.len(), vertex_count);
        assert_eq!(&stream[0..4], &[entry(0); 4], "face 0's vertices");
        assert_eq!(&stream[4..7], &[EMPTY; 3], "face 1's vertices");
        assert_eq!(&stream[7..11], &[entry(2); 4], "face 2's vertices");
    }

    // Each owned face's vertices carry its own mask offset beside its owners;
    // an unowned face's carry zero and sample no mask.
    #[test]
    fn the_stamp_carries_each_faces_mask_offset_and_zero_for_unowned() {
        let (vertex_count, indices) = three_faces();
        let leaves = [leaf(0, 6), leaf(6, 3), leaf(9, 6)];
        let stream = stamp(vertex_count, &indices, &leaves, &OWNERS, &OFFSETS);
        let offsets: Vec<MaskOffset> = stream.iter().map(|v| v.mask_offset).collect();
        assert_eq!(&offsets[0..4], &[[-8, 4]; 4]);
        assert_eq!(&offsets[4..7], &[[0, 0]; 3]);
        assert_eq!(stream[4].owners, NO_FACE_OWNERS);
        assert_eq!(&offsets[7..11], &[[12, -20]; 4]);
    }

    #[test]
    fn owner_stream_without_id42_is_all_empty_at_vertex_count() {
        let (vertex_count, indices) = three_faces();
        let leaves = [leaf(0, 6), leaf(6, 3), leaf(9, 6)];
        let capture = LogCapture::start();
        let stream = build_vertex_spec_owners(vertex_count, &indices, &leaves, None);
        assert_eq!(stream, vec![EMPTY; vertex_count]);
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
            let stream = stamp(vertex_count, &indices, &leaves, &OWNERS, &OFFSETS);
            assert_eq!(stream, vec![EMPTY; vertex_count], "{what}");
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
        let stream = stamp(8, &indices, &leaves, &OWNERS, &OFFSETS);
        assert_eq!(stream, vec![EMPTY; 8]);
        let errors = loader_errors(&capture);
        assert_eq!(errors.len(), 1, "{errors:?}");
        assert!(
            errors[0].contains("past the 8-vertex Geometry"),
            "{errors:?}"
        );
    }

    // Face 1's one triangle, [4, 5, 3], reuses vertex 3, which face 0 already
    // stamped.
    fn faces_sharing_vertex_3() -> Vec<u32> {
        vec![0, 1, 2, 0, 2, 3, 4, 5, 3, 7, 8, 9, 7, 9, 10]
    }

    // The stamp is per vertex, so a vertex two faces share cannot carry both
    // faces' differing entries: the whole stream stays empty rather than let
    // the later face overwrite the earlier one.
    #[test]
    fn a_vertex_shared_by_faces_with_different_owners_empties_the_stream() {
        let (vertex_count, _) = three_faces();
        let leaves = [leaf(0, 6), leaf(6, 3), leaf(9, 6)];
        assert_ne!(OWNERS[0], OWNERS[1]);
        let capture = LogCapture::start();
        let stream = stamp(
            vertex_count,
            &faces_sharing_vertex_3(),
            &leaves,
            &OWNERS,
            &OFFSETS,
        );
        assert_eq!(stream, vec![EMPTY; vertex_count]);
        let errors = loader_errors(&capture);
        assert_eq!(errors.len(), 1, "{errors:?}");
        assert!(
            errors[0].contains("faces 0 and 1 share vertex 3 but their owner entries differ"),
            "{errors:?}"
        );
        assert!(
            errors[0].contains("no static non-SDF world specular"),
            "{errors:?}"
        );
    }

    // Faces that agree on their entry lose nothing at a shared vertex.
    #[test]
    fn a_vertex_shared_by_faces_with_the_same_owners_keeps_the_stream() {
        let (vertex_count, _) = three_faces();
        let leaves = [leaf(0, 6), leaf(6, 3), leaf(9, 6)];
        let owners = [OWNERS[0], OWNERS[0], OWNERS[2]];
        let offsets = [OFFSETS[0], OFFSETS[0], OFFSETS[2]];
        let capture = LogCapture::start();
        let stream = stamp(
            vertex_count,
            &faces_sharing_vertex_3(),
            &leaves,
            &owners,
            &offsets,
        );
        assert!(loader_errors(&capture).is_empty());
        assert_eq!(&stream[0..6], &[entry(0); 6], "faces 0 and 1");
        assert_eq!(stream[6], EMPTY, "no face references vertex 6");
        assert_eq!(&stream[7..11], &[entry(2); 4], "face 2's vertices");
    }

    // Equal owners are not enough: a vertex shared by two faces whose mask
    // charts sit at different offsets could send one face to the other's mask
    // texels, so the stamp fails by name, as conflicting owners do.
    #[test]
    fn a_vertex_shared_by_equal_owners_with_different_mask_offsets_empties_the_stream() {
        let (vertex_count, _) = three_faces();
        let leaves = [leaf(0, 6), leaf(6, 3), leaf(9, 6)];
        let owners = [OWNERS[0], OWNERS[0], OWNERS[2]];
        let offsets = [OFFSETS[0], [OFFSETS[0][0] + 4, OFFSETS[0][1]], OFFSETS[2]];
        let capture = LogCapture::start();
        let stream = stamp(
            vertex_count,
            &faces_sharing_vertex_3(),
            &leaves,
            &owners,
            &offsets,
        );
        assert_eq!(stream, vec![EMPTY; vertex_count]);
        let errors = loader_errors(&capture);
        assert_eq!(errors.len(), 1, "{errors:?}");
        assert!(
            errors[0].contains("faces 0 and 1 share vertex 3 but their mask offsets differ"),
            "{errors:?}"
        );
        assert!(
            errors[0].contains("no static non-SDF world specular"),
            "{errors:?}"
        );
    }

    // Owner entries and mask offsets of different counts never stamp: the
    // stream empties by name, as any other mismatch does.
    #[test]
    fn owner_entries_and_mask_offsets_of_different_counts_empty_the_stream() {
        let (vertex_count, indices) = three_faces();
        let leaves = [leaf(0, 6), leaf(6, 3), leaf(9, 6)];
        let capture = LogCapture::start();
        let stream = stamp(vertex_count, &indices, &leaves, &OWNERS, &OFFSETS[..2]);
        assert_eq!(stream, vec![EMPTY; vertex_count]);
        let errors = loader_errors(&capture);
        assert_eq!(errors.len(), 1, "{errors:?}");
        assert!(
            errors[0].contains("3 owner entries but 2 mask offsets"),
            "{errors:?}"
        );
    }
}
