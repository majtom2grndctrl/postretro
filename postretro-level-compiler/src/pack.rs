// Pack and write: serialize sections to .prl binary, validate via read-back.
// See: context/lib/build_pipeline.md §PRL

use std::fs;
use std::io::Cursor;
use std::path::Path;

use postretro_level_format::bsp::{BspLeavesSection, BspNodesSection};
use postretro_level_format::geometry::GeometrySection;
use postretro_level_format::leaf_pvs::LeafPvsSection;
use postretro_level_format::portals::{PortalRecord, PortalsSection};
use postretro_level_format::{
    SectionBlob, SectionId, read_container, read_section_data, write_prl,
};

use crate::portals::Portal;

/// Convert compiler portal data into a `PortalsSection` for the format crate.
pub fn encode_portals(portals: &[Portal]) -> PortalsSection {
    let mut vertices = Vec::new();
    let mut records = Vec::new();

    for portal in portals {
        let vertex_start = vertices.len() as u32;
        let vertex_count = portal.polygon.len() as u32;

        for v in &portal.polygon {
            vertices.push([v.x, v.y, v.z]);
        }

        records.push(PortalRecord {
            vertex_start,
            vertex_count,
            front_leaf: portal.front_leaf as u32,
            back_leaf: portal.back_leaf as u32,
        });
    }

    PortalsSection {
        vertices,
        portals: records,
    }
}

/// Write geometry, BSP nodes, BSP leaves, and leaf PVS sections to a .prl file (--pvs mode).
pub fn pack_and_write_pvs(
    output: &Path,
    geometry: &GeometrySection,
    nodes: &BspNodesSection,
    leaves: &BspLeavesSection,
    leaf_pvs: &LeafPvsSection,
) -> anyhow::Result<()> {
    let geometry_bytes = geometry.to_bytes();
    let nodes_bytes = nodes.to_bytes();
    let leaves_bytes = leaves.to_bytes();
    let leaf_pvs_bytes = leaf_pvs.to_bytes();

    let sections = vec![
        SectionBlob {
            section_id: SectionId::Geometry as u32,
            version: 1,
            data: geometry_bytes.clone(),
        },
        SectionBlob {
            section_id: SectionId::BspNodes as u32,
            version: 1,
            data: nodes_bytes.clone(),
        },
        SectionBlob {
            section_id: SectionId::BspLeaves as u32,
            version: 1,
            data: leaves_bytes.clone(),
        },
        SectionBlob {
            section_id: SectionId::LeafPvs as u32,
            version: 1,
            data: leaf_pvs_bytes.clone(),
        },
    ];

    write_and_validate_sections(output, &sections)?;

    log::info!("[Compiler] Sections: {}", sections.len());
    log::info!("[Compiler]   Geometry: {} bytes", geometry_bytes.len());
    log::info!("[Compiler]   BspNodes: {} bytes", nodes_bytes.len());
    log::info!("[Compiler]   BspLeaves: {} bytes", leaves_bytes.len());
    log::info!("[Compiler]   LeafPvs: {} bytes", leaf_pvs_bytes.len());

    Ok(())
}

/// Write geometry, BSP nodes, BSP leaves, and portals sections to a .prl file (default mode).
///
/// Clears pvs_offset and pvs_size in leaf records since no PVS section is written.
pub fn pack_and_write_portals(
    output: &Path,
    geometry: &GeometrySection,
    nodes: &BspNodesSection,
    leaves: &BspLeavesSection,
    portals: &PortalsSection,
) -> anyhow::Result<()> {
    // Zero out PVS references in leaves since no LeafPvs section is written.
    let portal_leaves = BspLeavesSection {
        leaves: leaves
            .leaves
            .iter()
            .map(|l| {
                use postretro_level_format::bsp::BspLeafRecord;
                BspLeafRecord {
                    pvs_offset: 0,
                    pvs_size: 0,
                    ..*l
                }
            })
            .collect(),
    };

    let geometry_bytes = geometry.to_bytes();
    let nodes_bytes = nodes.to_bytes();
    let leaves_bytes = portal_leaves.to_bytes();
    let portals_bytes = portals.to_bytes();

    let sections = vec![
        SectionBlob {
            section_id: SectionId::Geometry as u32,
            version: 1,
            data: geometry_bytes.clone(),
        },
        SectionBlob {
            section_id: SectionId::BspNodes as u32,
            version: 1,
            data: nodes_bytes.clone(),
        },
        SectionBlob {
            section_id: SectionId::BspLeaves as u32,
            version: 1,
            data: leaves_bytes.clone(),
        },
        SectionBlob {
            section_id: SectionId::Portals as u32,
            version: 1,
            data: portals_bytes.clone(),
        },
    ];

    write_and_validate_sections(output, &sections)?;

    log::info!("[Compiler] Sections: {}", sections.len());
    log::info!("[Compiler]   Geometry: {} bytes", geometry_bytes.len());
    log::info!("[Compiler]   BspNodes: {} bytes", nodes_bytes.len());
    log::info!("[Compiler]   BspLeaves: {} bytes", leaves_bytes.len());
    log::info!("[Compiler]   Portals: {} bytes", portals_bytes.len());

    Ok(())
}

/// Write sections to disk and validate via read-back.
fn write_and_validate_sections(output: &Path, sections: &[SectionBlob]) -> anyhow::Result<()> {
    // Validate output directory exists before writing
    if let Some(parent) = output.parent() {
        if !parent.as_os_str().is_empty() && !parent.exists() {
            anyhow::bail!("output directory does not exist: {}", parent.display());
        }
    }

    let mut file_buf = Vec::new();
    write_prl(&mut file_buf, sections)?;
    fs::write(output, &file_buf)?;

    let total_size = file_buf.len();
    log::info!(
        "[Compiler] Wrote {} ({} bytes)",
        output.display(),
        total_size
    );

    // Read-back validation: verify all sections round-trip.
    validate_readback(&file_buf, sections)?;
    log::info!("[Compiler] Read-back validation passed.");

    Ok(())
}

/// Re-read the written bytes and verify all sections match.
fn validate_readback(file_buf: &[u8], expected_sections: &[SectionBlob]) -> anyhow::Result<()> {
    let mut cursor = Cursor::new(file_buf);
    let meta = read_container(&mut cursor)?;

    anyhow::ensure!(
        meta.header.section_count as usize == expected_sections.len(),
        "expected {} sections, got {}",
        expected_sections.len(),
        meta.header.section_count
    );

    // Verify retired sections are absent
    anyhow::ensure!(
        meta.find_section(SectionId::ClusterVisibility as u32)
            .is_none(),
        "retired ClusterVisibility section (ID 2) should not be present"
    );
    anyhow::ensure!(
        meta.find_section(SectionId::VisibilityConfidence as u32)
            .is_none(),
        "retired VisibilityConfidence section (ID 11) should not be present"
    );

    for expected in expected_sections {
        let entry = meta.find_section(expected.section_id).ok_or_else(|| {
            anyhow::anyhow!("section ID {} missing from read-back", expected.section_id)
        })?;
        anyhow::ensure!(
            entry.size > 0,
            "section ID {} has zero size",
            expected.section_id
        );

        let actual =
            read_section_data(&mut cursor, &meta, expected.section_id)?.ok_or_else(|| {
                anyhow::anyhow!(
                    "section ID {} data missing from read-back",
                    expected.section_id
                )
            })?;
        anyhow::ensure!(
            actual == expected.data,
            "section ID {} data mismatch after read-back",
            expected.section_id
        );
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use postretro_level_format::bsp::{BspLeafRecord, BspNodeRecord};
    use postretro_level_format::geometry::FaceMeta;

    fn sample_geometry() -> GeometrySection {
        GeometrySection {
            vertices: vec![[1.0, 2.0, 3.0], [4.0, 5.0, 6.0], [7.0, 8.0, 9.0]],
            indices: vec![0, 1, 2],
            faces: vec![FaceMeta {
                index_offset: 0,
                index_count: 3,
                leaf_index: 0,
            }],
        }
    }

    fn sample_nodes() -> BspNodesSection {
        BspNodesSection {
            nodes: vec![BspNodeRecord {
                plane_normal: [1.0, 0.0, 0.0],
                plane_distance: 32.0,
                front: -1 - 0, // leaf 0
                back: -1 - 1,  // leaf 1
            }],
        }
    }

    fn sample_leaves() -> BspLeavesSection {
        BspLeavesSection {
            leaves: vec![
                BspLeafRecord {
                    face_start: 0,
                    face_count: 1,
                    bounds_min: [0.0, 0.0, 0.0],
                    bounds_max: [32.0, 64.0, 64.0],
                    pvs_offset: 0,
                    pvs_size: 1,
                    is_solid: 0,
                },
                BspLeafRecord {
                    face_start: 0,
                    face_count: 0,
                    bounds_min: [32.0, 0.0, 0.0],
                    bounds_max: [64.0, 64.0, 64.0],
                    pvs_offset: 0,
                    pvs_size: 0,
                    is_solid: 1,
                },
            ],
        }
    }

    fn sample_leaf_pvs() -> LeafPvsSection {
        LeafPvsSection {
            pvs_data: vec![0xFF],
        }
    }

    #[test]
    fn pack_write_pvs_produces_valid_prl_file() {
        let dir = std::env::temp_dir().join("postretro_test_pack");
        let _ = std::fs::create_dir_all(&dir);
        let output = dir.join("test_pack_pvs.prl");

        let geometry = sample_geometry();
        let nodes = sample_nodes();
        let leaves = sample_leaves();
        let leaf_pvs = sample_leaf_pvs();

        pack_and_write_pvs(&output, &geometry, &nodes, &leaves, &leaf_pvs)
            .expect("pack_and_write_pvs should succeed");

        let data = std::fs::read(&output).expect("should read output file");
        assert_eq!(&data[0..4], b"PRL\0");

        let mut cursor = Cursor::new(&data);
        let meta = read_container(&mut cursor).expect("should read container");
        assert_eq!(meta.header.section_count, 4);

        assert!(meta.find_section(SectionId::Geometry as u32).is_some());
        assert!(meta.find_section(SectionId::BspNodes as u32).is_some());
        assert!(meta.find_section(SectionId::BspLeaves as u32).is_some());
        assert!(meta.find_section(SectionId::LeafPvs as u32).is_some());
        assert!(meta.find_section(SectionId::Portals as u32).is_none());

        let _ = std::fs::remove_file(&output);
    }

    #[test]
    fn pack_write_portals_produces_valid_prl_file() {
        let dir = std::env::temp_dir().join("postretro_test_pack");
        let _ = std::fs::create_dir_all(&dir);
        let output = dir.join("test_pack_portals.prl");

        let geometry = sample_geometry();
        let nodes = sample_nodes();
        let leaves = sample_leaves();
        let portals = PortalsSection {
            vertices: vec![[32.0, 0.0, 0.0], [32.0, 64.0, 0.0], [32.0, 64.0, 64.0]],
            portals: vec![PortalRecord {
                vertex_start: 0,
                vertex_count: 3,
                front_leaf: 0,
                back_leaf: 1,
            }],
        };

        pack_and_write_portals(&output, &geometry, &nodes, &leaves, &portals)
            .expect("pack_and_write_portals should succeed");

        let data = std::fs::read(&output).expect("should read output file");
        assert_eq!(&data[0..4], b"PRL\0");

        let mut cursor = Cursor::new(&data);
        let meta = read_container(&mut cursor).expect("should read container");
        assert_eq!(meta.header.section_count, 4);

        assert!(meta.find_section(SectionId::Geometry as u32).is_some());
        assert!(meta.find_section(SectionId::BspNodes as u32).is_some());
        assert!(meta.find_section(SectionId::BspLeaves as u32).is_some());
        assert!(meta.find_section(SectionId::Portals as u32).is_some());
        assert!(meta.find_section(SectionId::LeafPvs as u32).is_none());

        let _ = std::fs::remove_file(&output);
    }

    #[test]
    fn pack_write_rejects_nonexistent_directory() {
        let output = Path::new("/nonexistent/deeply/nested/dir/test.prl");
        let geometry = sample_geometry();
        let nodes = sample_nodes();
        let leaves = sample_leaves();
        let leaf_pvs = sample_leaf_pvs();

        let result = pack_and_write_pvs(output, &geometry, &nodes, &leaves, &leaf_pvs);
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(
            msg.contains("output directory does not exist"),
            "expected directory error, got: {msg}"
        );
    }

    #[test]
    fn full_pipeline_pvs_mode_produces_valid_prl() {
        let map_path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .expect("workspace root")
            .join("assets/maps/test.map");

        let map_data = crate::parse::parse_map_file(&map_path, crate::map_format::MapFormat::IdTech2).expect("test.map should parse");
        let result = crate::partition::partition(map_data.world_faces, &map_data.brush_volumes)
            .expect("partition should succeed");

        let geometry = crate::geometry::extract_geometry(&result.faces, &result.tree);
        let (vis_result, _portals) = crate::visibility::build_portal_pvs(&result.tree);

        let dir = std::env::temp_dir().join("postretro_test_pipeline");
        let _ = std::fs::create_dir_all(&dir);
        let output = dir.join("test_pipeline_pvs.prl");

        pack_and_write_pvs(
            &output,
            &geometry,
            &vis_result.nodes_section,
            &vis_result.leaves_section,
            &vis_result.leaf_pvs_section,
        )
        .expect("full pipeline pvs pack should succeed");

        let data = std::fs::read(&output).expect("should read output file");
        let mut cursor = Cursor::new(&data);
        let meta = read_container(&mut cursor).expect("should read container");

        assert_eq!(meta.header.section_count, 4);
        assert!(meta.find_section(SectionId::LeafPvs as u32).is_some());
        assert!(meta.find_section(SectionId::Portals as u32).is_none());

        let _ = std::fs::remove_file(&output);
    }

    #[test]
    fn full_pipeline_portal_mode_produces_valid_prl() {
        let map_path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .expect("workspace root")
            .join("assets/maps/test.map");

        let map_data = crate::parse::parse_map_file(&map_path, crate::map_format::MapFormat::IdTech2).expect("test.map should parse");
        let result = crate::partition::partition(map_data.world_faces, &map_data.brush_volumes)
            .expect("partition should succeed");

        let geometry = crate::geometry::extract_geometry(&result.faces, &result.tree);
        let (vis_result, generated_portals) = crate::visibility::build_portal_pvs(&result.tree);

        let portals_section = encode_portals(&generated_portals);

        let dir = std::env::temp_dir().join("postretro_test_pipeline");
        let _ = std::fs::create_dir_all(&dir);
        let output = dir.join("test_pipeline_portals.prl");

        pack_and_write_portals(
            &output,
            &geometry,
            &vis_result.nodes_section,
            &vis_result.leaves_section,
            &portals_section,
        )
        .expect("full pipeline portal pack should succeed");

        let data = std::fs::read(&output).expect("should read output file");
        let mut cursor = Cursor::new(&data);
        let meta = read_container(&mut cursor).expect("should read container");

        assert_eq!(meta.header.section_count, 4);
        assert!(meta.find_section(SectionId::Portals as u32).is_some());
        assert!(meta.find_section(SectionId::LeafPvs as u32).is_none());
        assert!(meta.find_section(SectionId::BspNodes as u32).is_some());
        assert!(meta.find_section(SectionId::BspLeaves as u32).is_some());

        let _ = std::fs::remove_file(&output);
    }
}
