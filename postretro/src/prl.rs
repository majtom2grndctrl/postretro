// PRL level loading: read .prl files, produce cluster-based engine data structures.
// See: context/lib/build_pipeline.md §PRL

use std::path::Path;

use glam::Vec3;
use postretro_level_format::confidence::VisibilityConfidenceSection;
use postretro_level_format::geometry::GeometrySection;
use postretro_level_format::visibility::{ClusterVisibilitySection, decompress_pvs};
use postretro_level_format::{self as prl_format, SectionId};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum PrlLoadError {
    #[error("PRL file not found: {0}")]
    FileNotFound(String),
    #[error("failed to read PRL file: {0}")]
    IoError(#[from] std::io::Error),
    #[error("PRL format error: {0}")]
    FormatError(#[from] prl_format::FormatError),
    #[error("PRL file has no geometry section")]
    NoGeometry,
}

/// Per-cluster runtime data for PRL levels.
#[derive(Debug, Clone)]
pub struct ClusterData {
    pub bounds_min: Vec3,
    pub bounds_max: Vec3,
    /// Starting face index in `LevelWorld::face_meta`.
    pub face_start: u32,
    /// Number of faces in this cluster.
    pub face_count: u32,
    /// Decompressed PVS: one bool per cluster, true = potentially visible.
    pub pvs: Vec<bool>,
}

/// Per-face draw metadata for PRL levels.
#[derive(Debug, Clone)]
pub struct FaceMeta {
    pub index_offset: u32,
    pub index_count: u32,
}

/// Cluster-based level data loaded from a .prl file.
#[derive(Debug)]
pub struct LevelWorld {
    pub vertices: Vec<[f32; 3]>,
    pub indices: Vec<u32>,
    pub face_meta: Vec<FaceMeta>,
    pub clusters: Vec<ClusterData>,
    /// Whether PVS data was present in the file.
    pub has_pvs: bool,
    /// Diagnostic: per-cluster-pair visibility confidence.
    /// Present only when the PRL was compiled with `--diagnostics`.
    pub confidence: Option<VisibilityConfidenceSection>,
}

impl LevelWorld {
    /// Find which cluster contains the given position by scanning bounding volumes.
    /// Falls back to the nearest cluster if the position is outside all bounds.
    pub fn find_cluster(&self, position: Vec3) -> Option<usize> {
        // Exact containment check first.
        for (i, cluster) in self.clusters.iter().enumerate() {
            if position.x >= cluster.bounds_min.x
                && position.x <= cluster.bounds_max.x
                && position.y >= cluster.bounds_min.y
                && position.y <= cluster.bounds_max.y
                && position.z >= cluster.bounds_min.z
                && position.z <= cluster.bounds_max.z
            {
                return Some(i);
            }
        }

        // Fallback: nearest cluster by distance to AABB center.
        self.clusters
            .iter()
            .enumerate()
            .filter(|(_, c)| c.face_count > 0)
            .min_by(|(_, a), (_, b)| {
                let center_a = (a.bounds_min + a.bounds_max) * 0.5;
                let center_b = (b.bounds_min + b.bounds_max) * 0.5;
                let dist_a = position.distance_squared(center_a);
                let dist_b = position.distance_squared(center_b);
                dist_a
                    .partial_cmp(&dist_b)
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
            .map(|(i, _)| i)
    }

    /// Compute a reasonable spawn position: center of the level's geometry bounds.
    pub fn spawn_position(&self) -> Vec3 {
        let mut mins = Vec3::splat(f32::MAX);
        let mut maxs = Vec3::splat(f32::MIN);
        for cluster in &self.clusters {
            if cluster.face_count == 0 {
                continue;
            }
            mins = mins.min(cluster.bounds_min);
            maxs = maxs.max(cluster.bounds_max);
        }
        (mins + maxs) * 0.5
    }
}

/// Build a per-face cluster index mapping from cluster face ranges.
///
/// Returns a Vec where entry `i` is the cluster index that face `i` belongs to.
/// Used by the renderer to assign per-cluster wireframe colors.
pub fn face_cluster_indices(world: &LevelWorld) -> Vec<u32> {
    let mut indices = vec![0u32; world.face_meta.len()];
    for (cluster_idx, cluster) in world.clusters.iter().enumerate() {
        let start = cluster.face_start as usize;
        let count = cluster.face_count as usize;
        for face_idx in start..start + count {
            if let Some(slot) = indices.get_mut(face_idx) {
                *slot = cluster_idx as u32;
            }
        }
    }
    indices
}

pub fn load_prl(path: &str) -> Result<LevelWorld, PrlLoadError> {
    let path_ref = Path::new(path);
    if !path_ref.exists() {
        return Err(PrlLoadError::FileNotFound(path.to_string()));
    }

    let file_data = std::fs::read(path_ref)?;
    let mut cursor = std::io::Cursor::new(&file_data);

    let meta = prl_format::read_container(&mut cursor)?;

    let geom_data = prl_format::read_section_data(&mut cursor, &meta, SectionId::Geometry as u32)?
        .ok_or(PrlLoadError::NoGeometry)?;
    let geom = GeometrySection::from_bytes(&geom_data)?;

    let vis_section = match prl_format::read_section_data(
        &mut cursor,
        &meta,
        SectionId::ClusterVisibility as u32,
    )? {
        Some(data) => {
            let section = ClusterVisibilitySection::from_bytes(&data)?;
            Some(section)
        }
        None => {
            log::warn!(
                "[PRL] No cluster visibility section — PVS culling disabled, drawing all clusters"
            );
            None
        }
    };

    let confidence_section = match prl_format::read_section_data(
        &mut cursor,
        &meta,
        SectionId::VisibilityConfidence as u32,
    )? {
        Some(data) => {
            let section = VisibilityConfidenceSection::from_bytes(&data)?;
            log::info!(
                "[PRL] Loaded visibility confidence section ({} clusters)",
                section.cluster_count,
            );
            Some(section)
        }
        None => None,
    };

    let has_pvs = vis_section.is_some();

    let face_meta: Vec<FaceMeta> = geom
        .faces
        .iter()
        .map(|f| FaceMeta {
            index_offset: f.index_offset,
            index_count: f.index_count,
        })
        .collect();

    let clusters = build_clusters(&geom, vis_section.as_ref());

    log::info!(
        "[PRL] Loaded: {} vertices, {} indices ({} triangles), {} faces, {} clusters, pvs={}",
        geom.vertices.len(),
        geom.indices.len(),
        geom.indices.len() / 3,
        face_meta.len(),
        clusters.len(),
        has_pvs,
    );

    Ok(LevelWorld {
        vertices: geom.vertices,
        indices: geom.indices,
        face_meta,
        clusters,
        has_pvs,
        confidence: confidence_section,
    })
}

/// Build runtime cluster data from geometry and optional visibility sections.
///
/// When a visibility section is present, clusters use its bounding volumes,
/// face ranges, and decompressed PVS. When absent, clusters are derived from
/// the geometry section's face metadata — one cluster per unique cluster_index.
fn build_clusters(
    geom: &GeometrySection,
    vis: Option<&ClusterVisibilitySection>,
) -> Vec<ClusterData> {
    match vis {
        Some(vis_section) => {
            let cluster_count = vis_section.clusters.len();
            let pvs_byte_count = cluster_count.div_ceil(8);

            vis_section
                .clusters
                .iter()
                .map(|ci| {
                    let pvs_slice = if ci.pvs_size > 0 {
                        let start = ci.pvs_offset as usize;
                        let end = start + ci.pvs_size as usize;
                        if end <= vis_section.pvs_data.len() {
                            &vis_section.pvs_data[start..end]
                        } else {
                            &[]
                        }
                    } else {
                        &[]
                    };

                    let decompressed = decompress_pvs(pvs_slice, pvs_byte_count);

                    // Convert byte bitfield to per-cluster bool vec.
                    let mut pvs = Vec::with_capacity(cluster_count);
                    for cluster_idx in 0..cluster_count {
                        let byte_idx = cluster_idx / 8;
                        let bit_idx = cluster_idx % 8;
                        let visible = byte_idx < decompressed.len()
                            && (decompressed[byte_idx] & (1 << bit_idx)) != 0;
                        pvs.push(visible);
                    }

                    ClusterData {
                        bounds_min: Vec3::from(ci.bounds_min),
                        bounds_max: Vec3::from(ci.bounds_max),
                        face_start: ci.face_start,
                        face_count: ci.face_count,
                        pvs,
                    }
                })
                .collect()
        }
        None => {
            // No visibility section: derive clusters from geometry face metadata.
            let max_cluster = geom
                .faces
                .iter()
                .map(|f| f.cluster_index)
                .max()
                .unwrap_or(0);
            let cluster_count = max_cluster as usize + 1;

            (0..cluster_count)
                .map(|ci| {
                    let ci_u32 = ci as u32;
                    // Compute bounding volume from face vertices.
                    let mut mins = Vec3::splat(f32::MAX);
                    let mut maxs = Vec3::splat(f32::MIN);
                    let mut face_start = u32::MAX;
                    let mut face_count = 0u32;

                    for (fi, face) in geom.faces.iter().enumerate() {
                        if face.cluster_index != ci_u32 {
                            continue;
                        }
                        if face_start == u32::MAX {
                            face_start = fi as u32;
                        }
                        face_count += 1;

                        let idx_start = face.index_offset as usize;
                        let idx_end = idx_start + face.index_count as usize;
                        for &idx in &geom.indices[idx_start..idx_end.min(geom.indices.len())] {
                            if let Some(v) = geom.vertices.get(idx as usize) {
                                let pos = Vec3::from(*v);
                                mins = mins.min(pos);
                                maxs = maxs.max(pos);
                            }
                        }
                    }

                    if face_start == u32::MAX {
                        face_start = 0;
                    }

                    // Without PVS, all clusters are visible from all clusters.
                    let pvs = vec![true; cluster_count];

                    ClusterData {
                        bounds_min: mins,
                        bounds_max: maxs,
                        face_start,
                        face_count,
                        pvs,
                    }
                })
                .collect()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use postretro_level_format::geometry::{FaceMeta as FormatFaceMeta, GeometrySection};
    use postretro_level_format::visibility::{ClusterInfo, ClusterVisibilitySection, compress_pvs};

    fn sample_geometry() -> GeometrySection {
        GeometrySection {
            vertices: vec![
                [0.0, 0.0, 0.0],
                [1.0, 0.0, 0.0],
                [1.0, 1.0, 0.0],
                [10.0, 0.0, 0.0],
                [11.0, 0.0, 0.0],
                [11.0, 1.0, 0.0],
            ],
            indices: vec![0, 1, 2, 3, 4, 5],
            faces: vec![
                FormatFaceMeta {
                    index_offset: 0,
                    index_count: 3,
                    cluster_index: 0,
                },
                FormatFaceMeta {
                    index_offset: 3,
                    index_count: 3,
                    cluster_index: 1,
                },
            ],
        }
    }

    fn sample_visibility() -> ClusterVisibilitySection {
        // 2 clusters, each can see both (full visibility).
        // Bitfield: 2 clusters = 1 byte, value 0b11 = both visible.
        let pvs_uncompressed_0 = vec![0b0000_0011u8];
        let pvs_uncompressed_1 = vec![0b0000_0011u8];
        let compressed_0 = compress_pvs(&pvs_uncompressed_0);
        let compressed_1 = compress_pvs(&pvs_uncompressed_1);

        let mut pvs_data = Vec::new();
        let offset_0 = 0u32;
        pvs_data.extend_from_slice(&compressed_0);
        let offset_1 = pvs_data.len() as u32;
        pvs_data.extend_from_slice(&compressed_1);

        ClusterVisibilitySection {
            clusters: vec![
                ClusterInfo {
                    bounds_min: [0.0, 0.0, 0.0],
                    bounds_max: [2.0, 2.0, 2.0],
                    face_start: 0,
                    face_count: 1,
                    pvs_offset: offset_0,
                    pvs_size: compressed_0.len() as u32,
                },
                ClusterInfo {
                    bounds_min: [9.0, 0.0, 0.0],
                    bounds_max: [12.0, 2.0, 2.0],
                    face_start: 1,
                    face_count: 1,
                    pvs_offset: offset_1,
                    pvs_size: compressed_1.len() as u32,
                },
            ],
            pvs_data,
        }
    }

    // -- build_clusters with visibility --

    #[test]
    fn build_clusters_with_visibility_preserves_bounds() {
        let geom = sample_geometry();
        let vis = sample_visibility();
        let clusters = build_clusters(&geom, Some(&vis));

        assert_eq!(clusters.len(), 2);
        assert_eq!(clusters[0].bounds_min, Vec3::new(0.0, 0.0, 0.0));
        assert_eq!(clusters[0].bounds_max, Vec3::new(2.0, 2.0, 2.0));
        assert_eq!(clusters[1].bounds_min, Vec3::new(9.0, 0.0, 0.0));
        assert_eq!(clusters[1].bounds_max, Vec3::new(12.0, 2.0, 2.0));
    }

    #[test]
    fn build_clusters_with_visibility_preserves_face_ranges() {
        let geom = sample_geometry();
        let vis = sample_visibility();
        let clusters = build_clusters(&geom, Some(&vis));

        assert_eq!(clusters[0].face_start, 0);
        assert_eq!(clusters[0].face_count, 1);
        assert_eq!(clusters[1].face_start, 1);
        assert_eq!(clusters[1].face_count, 1);
    }

    #[test]
    fn build_clusters_with_visibility_decompresses_pvs() {
        let geom = sample_geometry();
        let vis = sample_visibility();
        let clusters = build_clusters(&geom, Some(&vis));

        // Both clusters should see both clusters.
        assert_eq!(clusters[0].pvs.len(), 2);
        assert!(clusters[0].pvs[0]);
        assert!(clusters[0].pvs[1]);
        assert_eq!(clusters[1].pvs.len(), 2);
        assert!(clusters[1].pvs[0]);
        assert!(clusters[1].pvs[1]);
    }

    // -- build_clusters without visibility --

    #[test]
    fn build_clusters_without_visibility_derives_from_geometry() {
        let geom = sample_geometry();
        let clusters = build_clusters(&geom, None);

        assert_eq!(clusters.len(), 2);
        assert_eq!(clusters[0].face_count, 1);
        assert_eq!(clusters[1].face_count, 1);
    }

    #[test]
    fn build_clusters_without_visibility_all_visible() {
        let geom = sample_geometry();
        let clusters = build_clusters(&geom, None);

        // Without PVS, all clusters visible from all clusters.
        for cluster in &clusters {
            assert_eq!(cluster.pvs.len(), 2);
            assert!(cluster.pvs.iter().all(|&v| v));
        }
    }

    // -- find_cluster --

    #[test]
    fn find_cluster_returns_containing_cluster() {
        let world = LevelWorld {
            vertices: vec![],
            indices: vec![],
            face_meta: vec![],
            clusters: vec![
                ClusterData {
                    bounds_min: Vec3::new(0.0, 0.0, 0.0),
                    bounds_max: Vec3::new(10.0, 10.0, 10.0),
                    face_start: 0,
                    face_count: 0,
                    pvs: vec![],
                },
                ClusterData {
                    bounds_min: Vec3::new(20.0, 0.0, 0.0),
                    bounds_max: Vec3::new(30.0, 10.0, 10.0),
                    face_start: 0,
                    face_count: 0,
                    pvs: vec![],
                },
            ],
            has_pvs: false,
            confidence: None,
        };

        assert_eq!(world.find_cluster(Vec3::new(5.0, 5.0, 5.0)), Some(0));
        assert_eq!(world.find_cluster(Vec3::new(25.0, 5.0, 5.0)), Some(1));
    }

    #[test]
    fn find_cluster_returns_none_when_outside_all() {
        let world = LevelWorld {
            vertices: vec![],
            indices: vec![],
            face_meta: vec![],
            clusters: vec![ClusterData {
                bounds_min: Vec3::new(0.0, 0.0, 0.0),
                bounds_max: Vec3::new(10.0, 10.0, 10.0),
                face_start: 0,
                face_count: 0,
                pvs: vec![],
            }],
            has_pvs: false,
            confidence: None,
        };

        assert_eq!(world.find_cluster(Vec3::new(50.0, 50.0, 50.0)), None);
    }

    #[test]
    fn find_cluster_returns_first_match_for_overlapping() {
        let world = LevelWorld {
            vertices: vec![],
            indices: vec![],
            face_meta: vec![],
            clusters: vec![
                ClusterData {
                    bounds_min: Vec3::new(0.0, 0.0, 0.0),
                    bounds_max: Vec3::new(10.0, 10.0, 10.0),
                    face_start: 0,
                    face_count: 0,
                    pvs: vec![],
                },
                ClusterData {
                    bounds_min: Vec3::new(5.0, 0.0, 0.0),
                    bounds_max: Vec3::new(15.0, 10.0, 10.0),
                    face_start: 0,
                    face_count: 0,
                    pvs: vec![],
                },
            ],
            has_pvs: false,
            confidence: None,
        };

        // Point at (7, 5, 5) is in both clusters; first match wins.
        assert_eq!(world.find_cluster(Vec3::new(7.0, 5.0, 5.0)), Some(0));
    }

    #[test]
    fn find_cluster_boundary_point_is_inside() {
        let world = LevelWorld {
            vertices: vec![],
            indices: vec![],
            face_meta: vec![],
            clusters: vec![ClusterData {
                bounds_min: Vec3::new(0.0, 0.0, 0.0),
                bounds_max: Vec3::new(10.0, 10.0, 10.0),
                face_start: 0,
                face_count: 0,
                pvs: vec![],
            }],
            has_pvs: false,
            confidence: None,
        };

        // Points on the boundary are inside (inclusive bounds).
        assert_eq!(world.find_cluster(Vec3::new(0.0, 0.0, 0.0)), Some(0));
        assert_eq!(world.find_cluster(Vec3::new(10.0, 10.0, 10.0)), Some(0));
    }

    // -- load_prl error cases --

    #[test]
    fn load_prl_missing_file_returns_file_not_found() {
        let result = load_prl("nonexistent/path/to/map.prl");
        assert!(result.is_err());
        assert!(
            matches!(result.unwrap_err(), PrlLoadError::FileNotFound(_)),
            "expected FileNotFound"
        );
    }

    // -- Round-trip: write a PRL file, load it --

    #[test]
    fn load_prl_round_trip_with_visibility() {
        let geom = sample_geometry();
        let vis = sample_visibility();

        let sections = vec![
            prl_format::SectionBlob {
                section_id: SectionId::Geometry as u32,
                version: 1,
                data: geom.to_bytes(),
            },
            prl_format::SectionBlob {
                section_id: SectionId::ClusterVisibility as u32,
                version: 1,
                data: vis.to_bytes(),
            },
        ];

        let tmp = std::env::temp_dir().join("postretro_test_round_trip.prl");
        let mut file = std::fs::File::create(&tmp).unwrap();
        prl_format::write_prl(&mut file, &sections).unwrap();

        let world = load_prl(tmp.to_str().unwrap()).unwrap();
        assert_eq!(world.vertices.len(), 6);
        assert_eq!(world.indices.len(), 6);
        assert_eq!(world.face_meta.len(), 2);
        assert_eq!(world.clusters.len(), 2);
        assert!(world.has_pvs);

        std::fs::remove_file(&tmp).ok();
    }

    #[test]
    fn load_prl_round_trip_without_visibility() {
        let geom = sample_geometry();

        let sections = vec![prl_format::SectionBlob {
            section_id: SectionId::Geometry as u32,
            version: 1,
            data: geom.to_bytes(),
        }];

        let tmp = std::env::temp_dir().join("postretro_test_no_vis.prl");
        let mut file = std::fs::File::create(&tmp).unwrap();
        prl_format::write_prl(&mut file, &sections).unwrap();

        let world = load_prl(tmp.to_str().unwrap()).unwrap();
        assert_eq!(world.vertices.len(), 6);
        assert_eq!(world.clusters.len(), 2);
        assert!(!world.has_pvs);

        // Without PVS, all clusters should be mutually visible.
        for cluster in &world.clusters {
            assert!(cluster.pvs.iter().all(|&v| v));
        }

        std::fs::remove_file(&tmp).ok();
    }

    #[test]
    fn load_prl_invalid_magic_produces_clear_error() {
        let tmp = std::env::temp_dir().join("postretro_test_bad_magic.prl");
        std::fs::write(&tmp, b"NOPE extra data for length").unwrap();

        let result = load_prl(tmp.to_str().unwrap());
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(
            err_msg.contains("magic"),
            "error should mention magic: {err_msg}"
        );

        std::fs::remove_file(&tmp).ok();
    }

    #[test]
    fn load_prl_truncated_file_produces_clear_error() {
        let tmp = std::env::temp_dir().join("postretro_test_truncated.prl");
        std::fs::write(&tmp, &[0x50, 0x52, 0x4C]).unwrap(); // "PRL" only

        let result = load_prl(tmp.to_str().unwrap());
        assert!(result.is_err());

        std::fs::remove_file(&tmp).ok();
    }
}
