// Pack and write: serialize sections to .prl binary, validate via read-back.
// See: context/plans/in-progress/prl-phase-1-minimum-viable-compiler/task-06-pack-write.md

use std::fs;
use std::io::Cursor;
use std::path::Path;

use postretro_level_format::confidence::VisibilityConfidenceSection;
use postretro_level_format::geometry::GeometrySection;
use postretro_level_format::visibility::ClusterVisibilitySection;
use postretro_level_format::{
    SectionBlob, SectionId, read_container, read_section_data, write_prl,
};

/// Write geometry, visibility, and optional confidence sections to a .prl file,
/// then validate via read-back.
///
/// `confidence` is the per-cluster-pair confidence matrix from diagnostic mode.
/// When `None`, the confidence section is omitted (normal compilation).
pub fn pack_and_write(
    output: &Path,
    geometry: &GeometrySection,
    visibility: &ClusterVisibilitySection,
    confidence: Option<&[Vec<f32>]>,
) -> anyhow::Result<()> {
    let geometry_bytes = geometry.to_bytes();
    let visibility_bytes = visibility.to_bytes();

    let mut sections = vec![
        SectionBlob {
            section_id: SectionId::Geometry as u32,
            version: 1,
            data: geometry_bytes.clone(),
        },
        SectionBlob {
            section_id: SectionId::ClusterVisibility as u32,
            version: 1,
            data: visibility_bytes.clone(),
        },
    ];

    let confidence_bytes = if let Some(conf_matrix) = confidence {
        let cluster_count = conf_matrix.len() as u32;
        let flat: Vec<f32> = conf_matrix.iter().flat_map(|row| row.iter().copied()).collect();
        let section = VisibilityConfidenceSection {
            cluster_count,
            data: flat,
        };
        let bytes = section.to_bytes();
        sections.push(SectionBlob {
            section_id: SectionId::VisibilityConfidence as u32,
            version: 1,
            data: bytes.clone(),
        });
        Some(bytes)
    } else {
        None
    };

    // Validate output directory exists before writing
    if let Some(parent) = output.parent() {
        if !parent.as_os_str().is_empty() && !parent.exists() {
            anyhow::bail!("output directory does not exist: {}", parent.display());
        }
    }

    let mut file_buf = Vec::new();
    write_prl(&mut file_buf, &sections)?;
    fs::write(output, &file_buf)?;

    let total_size = file_buf.len();
    log::info!(
        "[Compiler] Wrote {} ({} bytes)",
        output.display(),
        total_size
    );
    log::info!("[Compiler] Sections: {}", sections.len());
    log::info!("[Compiler]   Geometry: {} bytes", geometry_bytes.len());
    log::info!(
        "[Compiler]   ClusterVisibility: {} bytes",
        visibility_bytes.len()
    );
    if let Some(ref conf_bytes) = confidence_bytes {
        log::info!(
            "[Compiler]   VisibilityConfidence: {} bytes",
            conf_bytes.len()
        );
    }

    // Read-back validation
    validate_readback(
        &file_buf,
        &geometry_bytes,
        &visibility_bytes,
        confidence_bytes.as_deref(),
    )?;
    log::info!("[Compiler] Read-back validation passed.");

    Ok(())
}

/// Re-read the written bytes and verify all sections deserialize to matching data.
fn validate_readback(
    file_buf: &[u8],
    expected_geometry: &[u8],
    expected_visibility: &[u8],
    expected_confidence: Option<&[u8]>,
) -> anyhow::Result<()> {
    let mut cursor = Cursor::new(file_buf);
    let meta = read_container(&mut cursor)?;

    let expected_section_count = if expected_confidence.is_some() { 3 } else { 2 };
    anyhow::ensure!(
        meta.header.section_count == expected_section_count,
        "expected {} sections, got {}",
        expected_section_count,
        meta.header.section_count
    );

    // Check required sections are present with non-zero sizes
    let geom_entry = meta
        .find_section(SectionId::Geometry as u32)
        .ok_or_else(|| anyhow::anyhow!("Geometry section missing from read-back"))?;
    anyhow::ensure!(geom_entry.size > 0, "Geometry section has zero size");

    let vis_entry = meta
        .find_section(SectionId::ClusterVisibility as u32)
        .ok_or_else(|| anyhow::anyhow!("ClusterVisibility section missing from read-back"))?;
    anyhow::ensure!(
        vis_entry.size > 0,
        "ClusterVisibility section has zero size"
    );

    // Read back and compare raw bytes
    let geom_data = read_section_data(&mut cursor, &meta, SectionId::Geometry as u32)?
        .ok_or_else(|| anyhow::anyhow!("Geometry section data missing"))?;
    anyhow::ensure!(
        geom_data == expected_geometry,
        "Geometry section data mismatch after read-back"
    );

    let vis_data = read_section_data(&mut cursor, &meta, SectionId::ClusterVisibility as u32)?
        .ok_or_else(|| anyhow::anyhow!("ClusterVisibility section data missing"))?;
    anyhow::ensure!(
        vis_data == expected_visibility,
        "ClusterVisibility section data mismatch after read-back"
    );

    // Verify required sections deserialize without error
    GeometrySection::from_bytes(&geom_data)?;
    ClusterVisibilitySection::from_bytes(&vis_data)?;

    // Validate confidence section if expected
    if let Some(expected_conf) = expected_confidence {
        let conf_data =
            read_section_data(&mut cursor, &meta, SectionId::VisibilityConfidence as u32)?
                .ok_or_else(|| {
                    anyhow::anyhow!("VisibilityConfidence section data missing from read-back")
                })?;
        anyhow::ensure!(
            conf_data == expected_conf,
            "VisibilityConfidence section data mismatch after read-back"
        );
        VisibilityConfidenceSection::from_bytes(&conf_data)?;
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use glam::Vec3;
    use postretro_level_format::geometry::FaceMeta;
    use postretro_level_format::visibility::ClusterInfo;

    /// Build a VoxelGrid covering clusters and faces, matching the main pipeline logic.
    fn build_test_voxel_grid(
        clusters: &[crate::partition::Cluster],
        faces: &[crate::map_data::Face],
        brush_volumes: &[crate::map_data::BrushVolume],
    ) -> crate::voxel_grid::VoxelGrid {
        let mut world_bounds = crate::partition::Aabb::empty();
        for c in clusters {
            world_bounds.expand_aabb(&c.bounds);
        }
        for face in faces {
            for &v in &face.vertices {
                world_bounds.expand_point(v);
            }
        }
        if !world_bounds.is_valid() {
            world_bounds = crate::partition::Aabb {
                min: Vec3::ZERO,
                max: Vec3::splat(1.0),
            };
        }
        let pad = Vec3::splat(crate::voxel_grid::DEFAULT_VOXEL_SIZE);
        world_bounds.min -= pad;
        world_bounds.max += pad;
        crate::voxel_grid::VoxelGrid::from_brushes(
            brush_volumes,
            &world_bounds,
            crate::voxel_grid::DEFAULT_VOXEL_SIZE,
        )
    }

    fn sample_geometry() -> GeometrySection {
        GeometrySection {
            vertices: vec![[1.0, 2.0, 3.0], [4.0, 5.0, 6.0], [7.0, 8.0, 9.0]],
            indices: vec![0, 1, 2],
            faces: vec![FaceMeta {
                index_offset: 0,
                index_count: 3,
                cluster_index: 0,
            }],
        }
    }

    fn sample_visibility() -> ClusterVisibilitySection {
        ClusterVisibilitySection {
            clusters: vec![ClusterInfo {
                bounds_min: [0.0, 0.0, 0.0],
                bounds_max: [10.0, 10.0, 10.0],
                face_start: 0,
                face_count: 1,
                pvs_offset: 0,
                pvs_size: 1,
            }],
            pvs_data: vec![0xFF],
        }
    }

    #[test]
    fn pack_write_produces_valid_prl_file() {
        let dir = std::env::temp_dir().join("postretro_test_pack");
        let _ = std::fs::create_dir_all(&dir);
        let output = dir.join("test_pack.prl");

        let geometry = sample_geometry();
        let visibility = sample_visibility();

        pack_and_write(&output, &geometry, &visibility, None)
            .expect("pack_and_write should succeed");

        // Verify file exists and starts with magic bytes
        let data = std::fs::read(&output).expect("should read output file");
        assert_eq!(&data[0..4], b"PRL\0");

        // Verify section table lists both sections with non-zero sizes
        let mut cursor = Cursor::new(&data);
        let meta = read_container(&mut cursor).expect("should read container");
        assert_eq!(meta.header.section_count, 2);

        let geom_entry = meta
            .find_section(SectionId::Geometry as u32)
            .expect("Geometry section should exist");
        assert!(geom_entry.size > 0);

        let vis_entry = meta
            .find_section(SectionId::ClusterVisibility as u32)
            .expect("ClusterVisibility section should exist");
        assert!(vis_entry.size > 0);

        // Verify round-trip: deserialized data matches original
        let geom_data = read_section_data(&mut cursor, &meta, SectionId::Geometry as u32)
            .unwrap()
            .unwrap();
        let restored_geom = GeometrySection::from_bytes(&geom_data).unwrap();
        assert_eq!(geometry, restored_geom);

        let vis_data = read_section_data(&mut cursor, &meta, SectionId::ClusterVisibility as u32)
            .unwrap()
            .unwrap();
        let restored_vis = ClusterVisibilitySection::from_bytes(&vis_data).unwrap();
        assert_eq!(visibility, restored_vis);

        // Cleanup
        let _ = std::fs::remove_file(&output);
    }

    #[test]
    fn pack_write_rejects_nonexistent_directory() {
        let output = Path::new("/nonexistent/deeply/nested/dir/test.prl");
        let geometry = sample_geometry();
        let visibility = sample_visibility();

        let result = pack_and_write(output, &geometry, &visibility, None);
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(
            msg.contains("output directory does not exist"),
            "expected directory error, got: {msg}"
        );
    }

    /// Helper: convert grid cells to Cluster type for test compatibility.
    fn grid_cells_to_clusters(
        cells: &[crate::spatial_grid::GridCell],
    ) -> Vec<crate::partition::Cluster> {
        cells
            .iter()
            .filter(|c| {
                !c.face_indices.is_empty()
                    || c.cell_type.map_or(false, |t| {
                        t != crate::spatial_grid::CellType::Solid
                    })
            })
            .enumerate()
            .map(|(new_id, cell)| crate::partition::Cluster {
                id: new_id,
                bounds: cell.bounds.clone(),
                face_indices: cell.face_indices.clone(),
            })
            .collect()
    }

    #[test]
    fn full_pipeline_produces_valid_prl() {
        let map_path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .expect("workspace root")
            .join("assets/maps/test.map");

        let map_data = crate::parse::parse_map_file(&map_path).expect("test.map should parse");
        let grid_result = crate::spatial_grid::assign_to_grid(map_data.world_faces, None);
        let clusters = grid_cells_to_clusters(&grid_result.cells);

        let geometry = crate::geometry::extract_geometry(&grid_result.faces, &clusters);
        let min_cell_dim = grid_result
            .cell_size
            .x
            .min(grid_result.cell_size.y)
            .min(grid_result.cell_size.z)
            .max(1.0);
        let vg = build_test_voxel_grid(&clusters, &grid_result.faces, &map_data.brush_volumes);
        let vis_result = crate::visibility::compute_visibility(
            &clusters,
            &map_data.entities,
            &vg,
            min_cell_dim,
            &grid_result.faces,
            false,
        );

        let dir = std::env::temp_dir().join("postretro_test_pipeline");
        let _ = std::fs::create_dir_all(&dir);
        let output = dir.join("test_pipeline.prl");

        pack_and_write(&output, &geometry, &vis_result.section, None)
            .expect("full pipeline pack should succeed");

        // Verify full round-trip from file
        let data = std::fs::read(&output).expect("should read output file");
        let mut cursor = Cursor::new(&data);
        let meta = read_container(&mut cursor).expect("should read container");

        let geom_data = read_section_data(&mut cursor, &meta, SectionId::Geometry as u32)
            .unwrap()
            .unwrap();
        let restored_geom = GeometrySection::from_bytes(&geom_data).unwrap();
        assert_eq!(geometry.vertices.len(), restored_geom.vertices.len());
        assert_eq!(geometry.indices.len(), restored_geom.indices.len());
        assert_eq!(geometry.faces.len(), restored_geom.faces.len());
        assert_eq!(geometry, restored_geom);

        let vis_data = read_section_data(&mut cursor, &meta, SectionId::ClusterVisibility as u32)
            .unwrap()
            .unwrap();
        let restored_vis = ClusterVisibilitySection::from_bytes(&vis_data).unwrap();
        assert_eq!(vis_result.section, restored_vis);

        // Cleanup
        let _ = std::fs::remove_file(&output);
    }
}
