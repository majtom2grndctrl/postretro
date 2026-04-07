// Pack and write: serialize sections to .prl binary, validate via read-back.
// See: context/lib/build_pipeline.md §PRL

use std::fs;
use std::io::Cursor;
use std::path::Path;

use postretro_level_format::geometry::GeometrySection;
use postretro_level_format::visibility::ClusterVisibilitySection;
use postretro_level_format::{
    SectionBlob, SectionId, read_container, read_section_data, write_prl,
};

/// Write geometry and visibility sections to a .prl file, then validate via
/// read-back.
pub fn pack_and_write(
    output: &Path,
    geometry: &GeometrySection,
    visibility: &ClusterVisibilitySection,
) -> anyhow::Result<()> {
    let geometry_bytes = geometry.to_bytes();
    let visibility_bytes = visibility.to_bytes();

    let sections = vec![
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

    // Read-back validation
    validate_readback(&file_buf, &geometry_bytes, &visibility_bytes)?;
    log::info!("[Compiler] Read-back validation passed.");

    Ok(())
}

/// Re-read the written bytes and verify all sections deserialize to matching data.
fn validate_readback(
    file_buf: &[u8],
    expected_geometry: &[u8],
    expected_visibility: &[u8],
) -> anyhow::Result<()> {
    let mut cursor = Cursor::new(file_buf);
    let meta = read_container(&mut cursor)?;

    anyhow::ensure!(
        meta.header.section_count == 2,
        "expected 2 sections, got {}",
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

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use postretro_level_format::geometry::FaceMeta;
    use postretro_level_format::visibility::ClusterInfo;

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

        pack_and_write(&output, &geometry, &visibility).expect("pack_and_write should succeed");

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

        let result = pack_and_write(output, &geometry, &visibility);
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(
            msg.contains("output directory does not exist"),
            "expected directory error, got: {msg}"
        );
    }

    #[test]
    fn full_pipeline_produces_valid_prl() {
        let map_path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .expect("workspace root")
            .join("assets/maps/test.map");

        let map_data = crate::parse::parse_map_file(&map_path).expect("test.map should parse");
        let result = crate::partition::partition(map_data.world_faces, &map_data.brush_volumes)
            .expect("partition should succeed");

        let geometry = crate::geometry::extract_geometry(&result.faces, &result.tree);
        let vis_result = crate::visibility::build_passthrough_pvs(&result.tree);

        let dir = std::env::temp_dir().join("postretro_test_pipeline");
        let _ = std::fs::create_dir_all(&dir);
        let output = dir.join("test_pipeline.prl");

        pack_and_write(&output, &geometry, &vis_result.section)
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
