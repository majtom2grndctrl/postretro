// The forward pass's per-vertex emissive volume id stream, stamped from id 60's
// per-volume face lists.
// See: context/lib/rendering_pipeline.md §7.3

use postretro_level_format::emissive::EmissiveSection;
use postretro_render_data::geometry::BvhLeaf;

use crate::prl::PrlLoadError;
use crate::prl_face_ranges::face_index_ranges;

/// The stream value of a vertex whose face belongs to no volume.
pub(crate) const NO_EMISSIVE_VOLUME: u32 = 0;

/// One `u32` per world vertex: its face's emissive volume index + 1, or
/// [`NO_EMISSIVE_VOLUME`]. Volume 255 stamps 256, so no index aliases "none".
///
/// A level where no volume lists a face returns an empty stream: the renderer
/// binds a zero-filled one, and the BVH leaves are never consulted.
///
/// Each face's indices come from [`face_index_ranges`]. The compiler gives
/// every face its own vertices, so a vertex never belongs to two faces. A
/// vertex two faces of differing membership share could carry only one id, and
/// the other face would glow with its neighbour's volume; the load is refused
/// rather than let the id bleed across the edge. Faces that share a vertex and
/// agree on their volume are harmless and allowed.
pub(crate) fn build_vertex_emissive_volumes(
    vertex_count: usize,
    indices: &[u32],
    leaves: &[BvhLeaf],
    face_count: usize,
    emissive: &EmissiveSection,
) -> Result<Vec<u32>, PrlLoadError> {
    if emissive
        .volumes
        .iter()
        .all(|volume| volume.faces.is_empty())
    {
        return Ok(Vec::new());
    }
    stamp(vertex_count, indices, leaves, face_count, emissive).map_err(|reason| {
        crate::prl_loader::section_validation(
            "Emissive",
            format!("volume membership cannot reach the world vertices: {reason}"),
        )
    })
}

fn stamp(
    vertex_count: usize,
    indices: &[u32],
    leaves: &[BvhLeaf],
    face_count: usize,
    emissive: &EmissiveSection,
) -> Result<Vec<u32>, String> {
    let mut face_volume = vec![NO_EMISSIVE_VOLUME; face_count];
    for (volume, record) in emissive.volumes.iter().enumerate() {
        let id = volume as u32 + 1;
        for &face in &record.faces {
            let slot = face_volume.get_mut(face as usize).ok_or_else(|| {
                format!("volume {volume} face {face} is outside the {face_count} faces")
            })?;
            *slot = id;
        }
    }
    let ranges = face_index_ranges(leaves, face_count, indices.len())?;

    let mut stream = vec![NO_EMISSIVE_VOLUME; vertex_count];
    // The face that stamped each vertex, so a second face can be compared
    // against the id already there.
    const UNSTAMPED: usize = usize::MAX;
    let mut stamped_by = vec![UNSTAMPED; vertex_count];
    for (face, range) in ranges.into_iter().enumerate() {
        let id = face_volume[face];
        for &index in &indices[range] {
            let vertex = index as usize;
            let (Some(slot), Some(first)) = (stream.get_mut(vertex), stamped_by.get_mut(vertex))
            else {
                return Err(format!(
                    "face {face} names vertex {index} past the {vertex_count}-vertex Geometry"
                ));
            };
            if *first != UNSTAMPED && *first != face && *slot != id {
                return Err(format!(
                    "faces {} and {face} share vertex {index} but belong to different volumes",
                    *first
                ));
            }
            *slot = id;
            *first = face;
        }
    }
    Ok(stream)
}

#[cfg(test)]
mod tests {
    use super::*;
    use postretro_level_format::emissive::{EmissiveVolumeRecord, MAX_EMISSIVE_VOLUMES};

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

    fn volume(faces: Vec<u32>) -> EmissiveVolumeRecord {
        EmissiveVolumeRecord {
            min: 0.0,
            max: 1.0,
            color: [1.0; 3],
            origin: [0.0; 3],
            faces,
            tags: Vec::new(),
        }
    }

    fn section(volumes: Vec<EmissiveVolumeRecord>) -> EmissiveSection {
        EmissiveSection {
            textures: Vec::new(),
            volumes,
        }
    }

    /// Three quads as the compiler writes them, each with its own four
    /// vertices; leaves arrive in material-bucket order, not face order.
    fn three_quads() -> (usize, Vec<u32>, Vec<BvhLeaf>) {
        let mut indices = Vec::new();
        for face in 0..3u32 {
            let base = face * 4;
            indices.extend([base, base + 1, base + 2, base, base + 2, base + 3]);
        }
        (12, indices, vec![leaf(12, 6), leaf(0, 6), leaf(6, 6)])
    }

    fn message(error: PrlLoadError) -> String {
        error.to_string()
    }

    // A8 first slice: a member face's vertices carry its id; the adjacent
    // non-member face, whose own vertices sit at the same positions along the
    // shared edge, carries 0, so the id never bleeds across the edge.
    #[test]
    fn member_face_vertices_carry_the_volume_id_and_the_neighbour_carries_none() {
        let (vertices, indices, leaves) = three_quads();
        let emissive = section(vec![volume(vec![1])]);
        let stream =
            build_vertex_emissive_volumes(vertices, &indices, &leaves, 3, &emissive).unwrap();
        assert_eq!(stream.len(), vertices);
        assert_eq!(&stream[0..4], &[0; 4], "face 0 is no member");
        assert_eq!(&stream[4..8], &[1; 4], "face 1 is volume 0's");
        assert_eq!(&stream[8..12], &[0; 4], "face 2 is no member");
    }

    // P36: the last volume a map may carry stamps 256, never 0.
    #[test]
    fn volume_255_stamps_256_and_never_aliases_no_volume() {
        let (vertices, indices, leaves) = three_quads();
        let mut volumes: Vec<_> = (0..MAX_EMISSIVE_VOLUMES)
            .map(|_| volume(Vec::new()))
            .collect();
        volumes[MAX_EMISSIVE_VOLUMES - 1] = volume(vec![2]);
        let stream =
            build_vertex_emissive_volumes(vertices, &indices, &leaves, 3, &section(volumes))
                .unwrap();
        assert_eq!(&stream[8..12], &[256; 4]);
        assert_eq!(&stream[0..8], &[NO_EMISSIVE_VOLUME; 8]);
    }

    // No member face: no stream and no range recovery, so even leaves that
    // could not tile the faces are never read.
    #[test]
    fn a_level_without_member_faces_builds_no_stream() {
        let (vertices, indices, _) = three_quads();
        for emissive in [section(Vec::new()), section(vec![volume(Vec::new())])] {
            let stream =
                build_vertex_emissive_volumes(vertices, &indices, &[], 3, &emissive).unwrap();
            assert!(stream.is_empty());
        }
    }

    // A value the compiler never writes: one vertex claimed by faces of
    // differing membership. The load is refused by name.
    #[test]
    fn a_vertex_shared_by_faces_of_differing_membership_refuses_the_load() {
        let (vertices, mut indices, leaves) = three_quads();
        // Face 1's first triangle reuses face 0's vertex 3.
        indices[6] = 3;
        let emissive = section(vec![volume(vec![1])]);
        let error =
            build_vertex_emissive_volumes(vertices, &indices, &leaves, 3, &emissive).unwrap_err();
        let text = message(error);
        assert!(text.contains("Emissive"), "{text}");
        assert!(
            text.contains("faces 0 and 1 share vertex 3 but belong to different volumes"),
            "{text}"
        );

        // Faces that agree on their volume may share a vertex.
        let agreeing = section(vec![volume(vec![0, 1])]);
        let stream =
            build_vertex_emissive_volumes(vertices, &indices, &leaves, 3, &agreeing).unwrap();
        assert_eq!(stream[3], 1);
    }

    // Membership with no way to find each face's vertices refuses rather than
    // stamp a guess.
    #[test]
    fn leaves_that_do_not_tile_the_faces_refuse_a_level_with_members() {
        let (vertices, indices, _) = three_quads();
        let emissive = section(vec![volume(vec![0])]);
        let text = message(
            build_vertex_emissive_volumes(vertices, &indices, &[leaf(0, 6)], 3, &emissive)
                .unwrap_err(),
        );
        assert!(text.contains("1 BVH leaves for 3 Geometry faces"), "{text}");
    }
}
