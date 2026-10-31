// Content fingerprints several bake stages fold into their cache keys: geometry and bounce materials.
// See: context/lib/build_pipeline.md §Build Cache

use blake3::Hasher;

use crate::geometry::GeometryResult;
use crate::surface_materials::stats::BounceMaterials;

/// Whole-map `GeometryResult` content hash (postcard + blake3): what every
/// stage whose rays trace the whole map folds, so callers and tests derive
/// the identical fingerprint.
pub(crate) fn geometry_content_hash(geometry: &GeometryResult) -> [u8; 32] {
    let encoded = postcard::to_allocvec(geometry).expect("postcard serialize GeometryResult");
    *blake3::hash(&encoded).as_bytes()
}

/// Compute once per stage; no geometry scan or allocation occurs per group/entry.
pub(crate) fn bounce_material_hash(
    materials: Option<&BounceMaterials>,
    geometry: &GeometryResult,
) -> Option<[u8; 32]> {
    materials.map(|materials| {
        let mut hasher = Hasher::new();
        materials.fold_key(&mut hasher, geometry);
        *hasher.finalize().as_bytes()
    })
}

/// The one encoding every SH key that reads bounce albedo folds for the
/// material table. Presence is tagged so a colour-blind bake never shares a
/// key with a coloured one; absence folds no saturation, since nothing reads
/// it then.
pub(crate) fn fold_bounce_hash(hasher: &mut Hasher, bounce_hash: Option<&[u8; 32]>) {
    match bounce_hash {
        Some(bounce_hash) => {
            hasher.update(&[1]);
            hasher.update(bounce_hash);
        }
        None => {
            hasher.update(&[0]);
        }
    }
}

#[cfg(test)]
mod tests {
    use postretro_level_format::geometry::{FaceMeta, GeometrySection, Vertex};
    use postretro_level_format::texture_names::TextureNamesSection;

    use super::*;
    use crate::geometry::FaceIndexRange;
    use crate::surface_materials::stats::MaterialStats;

    fn hex(bytes: &[u8]) -> String {
        bytes.iter().map(|byte| format!("{byte:02x}")).collect()
    }

    /// One textured quad and one untextured triangle.
    fn geometry() -> GeometryResult {
        let vertex = |p: [f32; 3], uv: [f32; 2]| {
            Vertex::new(
                p,
                uv,
                [0.0, 1.0, 0.0],
                [1.0, 0.0, 0.0],
                true,
                [0.25, 0.5],
                3,
            )
        };
        GeometryResult {
            geometry: GeometrySection {
                vertices: vec![
                    vertex([0.0, 0.0, 0.0], [0.0, 0.0]),
                    vertex([0.0, 0.0, 2.0], [0.0, 64.0]),
                    vertex([2.0, 0.0, 2.0], [64.0, 64.0]),
                    vertex([2.0, 0.0, 0.0], [64.0, 0.0]),
                    vertex([1.0, 3.0, 1.0], [8.0, 8.0]),
                ],
                indices: vec![0, 1, 2, 0, 2, 3, 0, 1, 4],
                faces: vec![
                    FaceMeta {
                        leaf_index: 2,
                        texture_index: 1,
                    },
                    FaceMeta {
                        leaf_index: 5,
                        texture_index: 0,
                    },
                ],
            },
            texture_names: TextureNamesSection {
                names: vec!["a/floor".to_owned(), "b/wall".to_owned()],
            },
            face_index_ranges: vec![
                FaceIndexRange {
                    index_offset: 0,
                    index_count: 6,
                },
                FaceIndexRange {
                    index_offset: 6,
                    index_count: 3,
                },
            ],
        }
    }

    fn materials() -> BounceMaterials {
        BounceMaterials::new(
            vec![
                MaterialStats::with_snapped_mean([200, 40, 10]),
                MaterialStats::default(),
            ],
            0.75,
        )
        .unwrap()
    }

    // AC 27: the helpers left `sh_group.rs` unchanged, so every pre-change
    // id-27, id-35, id-47/48 and chunk-light-list key they feed keeps its
    // bytes and a first warm build hits those entries. These are the values
    // the helpers computed before the move.
    #[test]
    fn moved_key_helpers_reproduce_their_pre_move_fingerprints() {
        let geometry = geometry();
        assert_eq!(hex(&geometry_content_hash(&geometry)), PINNED_GEOMETRY);
        let bounce = bounce_material_hash(Some(&materials()), &geometry);
        assert_eq!(hex(bounce.as_ref().unwrap()), PINNED_BOUNCE);
        assert_eq!(bounce_material_hash(None, &geometry), None);
        let folded = |hash: Option<&[u8; 32]>| {
            let mut hasher = Hasher::new();
            fold_bounce_hash(&mut hasher, hash);
            hex(hasher.finalize().as_bytes())
        };
        assert_eq!(folded(bounce.as_ref()), PINNED_FOLD_PRESENT);
        assert_eq!(folded(None), PINNED_FOLD_ABSENT);
    }

    const PINNED_GEOMETRY: &str =
        "3b55a9fd409a17ab7edc5de52b5a6157f78669cc77bd798fe8e1766b6a710d30";
    const PINNED_BOUNCE: &str = "3932ddb029b8d87cc72e0732981be93f49efd679aa89c678de1f827f8fde4064";
    const PINNED_FOLD_PRESENT: &str =
        "f5051c8f61c6bf880e9696cca2e9cfa709811b1c5067774e0f484af1eacabd15";
    const PINNED_FOLD_ABSENT: &str =
        "2d3adedff11b61f14c886e35afa036736dcd87a74d27b5c1510225d0f592e213";
}
