// Pack and write: serialize sections to .prl binary, validate via read-back.
// See: context/lib/build_pipeline.md §PRL

use std::fs;
use std::io::Cursor;
use std::path::Path;

use postretro_level_format::alpha_lights::{
    ALPHA_LIGHT_LEAF_UNASSIGNED, AlphaFalloffModel, AlphaLightRecord, AlphaLightType,
    AlphaLightsSection,
};
use postretro_level_format::animated_light_chunks::AnimatedLightChunksSection;
use postretro_level_format::animated_light_weight_maps::AnimatedLightWeightMapsSection;
use postretro_level_format::bsp::{BspLeavesSection, BspNodesSection};
use postretro_level_format::bvh::BvhSection;
use postretro_level_format::chunk_light_list::ChunkLightListSection;
use postretro_level_format::delta_sh_volumes::DeltaShVolumesSection;
use postretro_level_format::light_influence::{InfluenceRecord, LightInfluenceSection};
use postretro_level_format::light_tags::LightTagsSection;
use postretro_level_format::lightmap::LightmapSection;
use postretro_level_format::portals::{PortalRecord, PortalsSection};
use postretro_level_format::sh_volume::ShVolumeSection;
use postretro_level_format::{
    SectionBlob, SectionId, read_container, read_section_data, write_prl,
};

use crate::geometry::GeometryResult;
use crate::light_namespaces::AlphaLightsNs;
use crate::map_data::{FalloffModel, LightType};
use crate::partition::{BspTree, find_leaf_for_point};
use crate::portals::Portal;

/// Serialize a `BvhSection` with per-leaf animated-light chunk ranges stamped
/// into the on-disk `BvhLeaf` records.
///
/// `chunk_ranges` is the parallel `(chunk_range_start, chunk_range_count)`
/// table returned by `animated_light_chunks::build_animated_light_chunks`,
/// indexed by BVH leaf slot. Pass an empty slice when no animated-light chunk
/// section is being emitted — every leaf then carries `(0, 0)` (the default).
///
/// This is the only sanctioned site that writes the chunk-range fields of
/// `BvhLeaf` to disk: keeping the application here, immediately adjacent to
/// `to_bytes()`, makes the "animated-light chunks must run before BVH
/// serialization" ordering an explicit data dependency rather than a hidden
/// side effect on `BvhSection`.
fn serialize_bvh_with_chunk_ranges(bvh: &BvhSection, chunk_ranges: &[(u32, u32)]) -> Vec<u8> {
    if chunk_ranges.is_empty() {
        // No chunk ranges to stamp — leaves keep their (0, 0) default.
        return bvh.to_bytes();
    }
    debug_assert_eq!(
        chunk_ranges.len(),
        bvh.leaves.len(),
        "chunk_ranges must be parallel to bvh.leaves",
    );
    let mut stamped = bvh.clone();
    for (leaf, &(start, count)) in stamped.leaves.iter_mut().zip(chunk_ranges.iter()) {
        leaf.chunk_range_start = start;
        leaf.chunk_range_count = count;
    }
    stamped.to_bytes()
}

/// Convert translated map lights into an `AlphaLightsSection` for the format
/// crate. Strips animation curves (direct lighting path uses static base
/// properties only — sub-plan 3 of the Lighting Foundation plan).
pub fn encode_alpha_lights(lights: &AlphaLightsNs<'_>, tree: &BspTree) -> AlphaLightsSection {
    let records: Vec<AlphaLightRecord> = lights
        .entries()
        .iter()
        .map(|entry| {
            let src_index = entry.source_index;
            let l = entry.light;
            let light_type = match l.light_type {
                LightType::Point => AlphaLightType::Point,
                LightType::Spot => AlphaLightType::Spot,
                LightType::Directional => AlphaLightType::Directional,
            };
            let falloff_model = match l.falloff_model {
                FalloffModel::Linear => AlphaFalloffModel::Linear,
                FalloffModel::InverseDistance => AlphaFalloffModel::InverseDistance,
                FalloffModel::InverseSquared => AlphaFalloffModel::InverseSquared,
            };

            let leaf_index = if tree.leaves.is_empty() {
                ALPHA_LIGHT_LEAF_UNASSIGNED
            } else {
                let idx = find_leaf_for_point(tree, l.origin);
                if tree.leaves[idx].is_solid {
                    log::warn!(
                        "[Compiler] AlphaLights: light {src_index} at origin ({:.3}, {:.3}, {:.3}) is inside a solid leaf; marking unassigned",
                        l.origin.x,
                        l.origin.y,
                        l.origin.z,
                    );
                    ALPHA_LIGHT_LEAF_UNASSIGNED
                } else {
                    idx as u32
                }
            };

            AlphaLightRecord {
                origin: [l.origin.x, l.origin.y, l.origin.z],
                light_type,
                intensity: l.intensity,
                color: l.color,
                falloff_model,
                falloff_range: l.falloff_range,
                cone_angle_inner: l.cone_angle_inner.unwrap_or(0.0),
                cone_angle_outer: l.cone_angle_outer.unwrap_or(0.0),
                cone_direction: l.cone_direction.unwrap_or([0.0, 0.0, 0.0]),
                cast_shadows: l.cast_shadows,
                is_dynamic: l.is_dynamic,
                leaf_index,
            }
        })
        .collect();

    AlphaLightsSection { lights: records }
}

/// Encode per-light script tags, aligned with the AlphaLights record order.
/// Returns `None` when no light in the AlphaLights namespace carries a tag —
/// the caller omits the section entirely in that case so tag-less maps add
/// zero bytes.
pub fn encode_light_tags(lights: &AlphaLightsNs<'_>) -> Option<LightTagsSection> {
    if lights.entries().iter().all(|e| e.light.tags.is_empty()) {
        return None;
    }
    let tags = lights
        .entries()
        .iter()
        .map(|e| e.light.tags.join(" "))
        .collect();
    Some(LightTagsSection { tags })
}

/// Derive influence records from the AlphaLights namespace. Iteration order
/// matches AlphaLights — record `i` here corresponds to light `i` there.
pub fn encode_light_influence(lights: &AlphaLightsNs<'_>) -> LightInfluenceSection {
    let records = lights
        .entries()
        .iter()
        .map(|e| {
            let l = e.light;
            let (center, radius) = match l.light_type {
                LightType::Directional => ([0.0f32, 0.0, 0.0], f32::MAX),
                LightType::Point | LightType::Spot => {
                    let cx = l.origin.x as f32;
                    let cy = l.origin.y as f32;
                    let cz = l.origin.z as f32;
                    ([cx, cy, cz], l.falloff_range)
                }
            };
            InfluenceRecord { center, radius }
        })
        .collect();

    LightInfluenceSection { records }
}

/// Convert compiler portal data into a `PortalsSection` for the format crate.
pub fn encode_portals(portals: &[Portal]) -> PortalsSection {
    let mut vertices = Vec::new();
    let mut records = Vec::new();

    for portal in portals {
        let vertex_start = vertices.len() as u32;
        let vertex_count = portal.polygon.len() as u32;

        // Output precision boundary: narrow portal vertices from f64 to f32
        // at the PRL format write site.
        for v in &portal.polygon {
            vertices.push([v.x as f32, v.y as f32, v.z as f32]);
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

/// Write geometry, texture names, BSP nodes, BSP leaves, portals, BVH,
/// alpha lights, light influence, and SH volume sections to a .prl file.
#[allow(clippy::too_many_arguments)]
pub fn pack_and_write_portals(
    output: &Path,
    geo_result: &GeometryResult,
    nodes: &BspNodesSection,
    leaves: &BspLeavesSection,
    portals: &PortalsSection,
    bvh: &BvhSection,
    bvh_chunk_ranges: &[(u32, u32)],
    alpha_lights: &AlphaLightsSection,
    light_influence: &LightInfluenceSection,
    sh_volume: &ShVolumeSection,
    lightmap: &LightmapSection,
    chunk_light_list: &ChunkLightListSection,
    animated_light_chunks: Option<&AnimatedLightChunksSection>,
    animated_light_weight_maps: Option<&AnimatedLightWeightMapsSection>,
    light_tags: Option<&LightTagsSection>,
    delta_sh_volumes: Option<&DeltaShVolumesSection>,
) -> anyhow::Result<()> {
    let geometry_bytes = geo_result.geometry.to_bytes();
    let texture_names_bytes = geo_result.texture_names.to_bytes();
    let nodes_bytes = nodes.to_bytes();
    let leaves_bytes = leaves.to_bytes();
    let portals_bytes = portals.to_bytes();
    let bvh_bytes = serialize_bvh_with_chunk_ranges(bvh, bvh_chunk_ranges);
    let alpha_lights_bytes = alpha_lights.to_bytes();
    let light_influence_bytes = light_influence.to_bytes();
    let sh_volume_bytes = sh_volume.to_bytes();
    let lightmap_bytes = lightmap.to_bytes();
    let chunk_light_list_bytes = chunk_light_list.to_bytes();
    let animated_light_chunks_bytes = animated_light_chunks.map(|s| s.to_bytes());
    let animated_light_weight_maps_bytes = animated_light_weight_maps.map(|s| s.to_bytes());
    let light_tags_bytes = light_tags.map(|s| s.to_bytes());
    let delta_sh_volumes_bytes = delta_sh_volumes.map(|s| s.to_bytes());

    let mut sections = vec![
        SectionBlob {
            section_id: SectionId::Geometry as u32,
            version: 1,
            data: geometry_bytes.clone(),
        },
        SectionBlob {
            section_id: SectionId::TextureNames as u32,
            version: 1,
            data: texture_names_bytes.clone(),
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
        SectionBlob {
            section_id: SectionId::ChunkLightList as u32,
            version: 1,
            data: chunk_light_list_bytes.clone(),
        },
        SectionBlob {
            section_id: SectionId::Bvh as u32,
            version: 1,
            data: bvh_bytes.clone(),
        },
        SectionBlob {
            section_id: SectionId::AlphaLights as u32,
            version: 1,
            data: alpha_lights_bytes.clone(),
        },
        SectionBlob {
            section_id: SectionId::LightInfluence as u32,
            version: 1,
            data: light_influence_bytes.clone(),
        },
        SectionBlob {
            section_id: SectionId::ShVolume as u32,
            version: 1,
            data: sh_volume_bytes.clone(),
        },
        SectionBlob {
            section_id: SectionId::Lightmap as u32,
            version: 1,
            data: lightmap_bytes.clone(),
        },
    ];
    if let Some(ref bytes) = animated_light_chunks_bytes {
        sections.push(SectionBlob {
            section_id: SectionId::AnimatedLightChunks as u32,
            version: 1,
            data: bytes.clone(),
        });
    }
    if let Some(ref bytes) = animated_light_weight_maps_bytes {
        sections.push(SectionBlob {
            section_id: SectionId::AnimatedLightWeightMaps as u32,
            version: 1,
            data: bytes.clone(),
        });
    }
    if let Some(ref bytes) = light_tags_bytes {
        sections.push(SectionBlob {
            section_id: SectionId::LightTags as u32,
            version: 1,
            data: bytes.clone(),
        });
    }
    if let Some(ref bytes) = delta_sh_volumes_bytes {
        sections.push(SectionBlob {
            section_id: SectionId::DeltaShVolumes as u32,
            version: 1,
            data: bytes.clone(),
        });
    }

    write_and_validate_sections(output, &sections)?;

    log::info!("Sections: {}", sections.len());
    log::info!("  Geometry: {} bytes", geometry_bytes.len());
    log::info!("  TextureNames: {} bytes", texture_names_bytes.len());
    log::info!("  BspNodes: {} bytes", nodes_bytes.len());
    log::info!("  BspLeaves: {} bytes", leaves_bytes.len());
    log::info!("  Portals: {} bytes", portals_bytes.len());
    log::info!("  Bvh: {} bytes", bvh_bytes.len());
    let assigned_count = alpha_lights
        .lights
        .iter()
        .filter(|r| r.leaf_index != ALPHA_LIGHT_LEAF_UNASSIGNED)
        .count();
    let unassigned_count = alpha_lights.lights.len() - assigned_count;
    log::info!(
        "  AlphaLights: {} bytes ({} lights, {} assigned to leaves, {} unassigned)",
        alpha_lights_bytes.len(),
        alpha_lights.lights.len(),
        assigned_count,
        unassigned_count,
    );
    log::info!(
        "  LightInfluence: {} bytes ({} records)",
        light_influence_bytes.len(),
        light_influence.records.len()
    );
    log::info!(
        "  ShVolume: {} bytes ({} probes)",
        sh_volume_bytes.len(),
        sh_volume.probes.len()
    );
    log::info!(
        "  Lightmap: {} bytes ({}x{})",
        lightmap_bytes.len(),
        lightmap.width,
        lightmap.height,
    );
    log::info!(
        "  ChunkLightList: {} bytes (has_grid={}, {} chunks, {} indices)",
        chunk_light_list_bytes.len(),
        chunk_light_list.has_grid,
        chunk_light_list.chunk_count(),
        chunk_light_list.light_indices.len(),
    );
    if let (Some(section), Some(bytes)) = (animated_light_chunks, &animated_light_chunks_bytes) {
        log::info!(
            "  AnimatedLightChunks: {} bytes ({} chunks, {} indices)",
            bytes.len(),
            section.chunks.len(),
            section.light_indices.len(),
        );
    }
    if let (Some(section), Some(bytes)) = (
        animated_light_weight_maps,
        &animated_light_weight_maps_bytes,
    ) {
        log::info!(
            "  AnimatedLightWeightMaps: {} bytes ({} chunks, {} offset entries, {} texel lights)",
            bytes.len(),
            section.chunk_rects.len(),
            section.offset_counts.len(),
            section.texel_lights.len(),
        );
    }

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
    log::info!("Wrote {} ({} bytes)", output.display(), total_size);

    // Read-back validation: verify all sections round-trip.
    validate_readback(&file_buf, sections)?;
    log::info!("Read-back validation passed.");

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
    use postretro_level_format::bvh::{BVH_NODE_FLAG_LEAF, BvhLeaf, BvhNode as FlatBvhNode};
    use postretro_level_format::geometry::{FaceMeta, GeometrySection, Vertex};
    use postretro_level_format::texture_names::TextureNamesSection;

    fn sample_geo_result() -> GeometryResult {
        GeometryResult {
            geometry: GeometrySection {
                vertices: vec![
                    Vertex::new(
                        [1.0, 2.0, 3.0],
                        [0.25, 0.75],
                        [0.0, 1.0, 0.0],
                        [1.0, 0.0, 0.0],
                        true,
                        [0.0, 0.0],
                    ),
                    Vertex::new(
                        [4.0, 5.0, 6.0],
                        [0.5, 0.0],
                        [0.0, 1.0, 0.0],
                        [1.0, 0.0, 0.0],
                        true,
                        [0.0, 0.0],
                    ),
                    Vertex::new(
                        [7.0, 8.0, 9.0],
                        [1.0, 1.0],
                        [0.0, 1.0, 0.0],
                        [1.0, 0.0, 0.0],
                        true,
                        [0.0, 0.0],
                    ),
                ],
                indices: vec![0, 1, 2],
                faces: vec![FaceMeta {
                    leaf_index: 0,
                    texture_index: 0,
                }],
            },
            texture_names: TextureNamesSection {
                names: vec!["test_texture".to_string()],
            },
            face_index_ranges: vec![crate::geometry::FaceIndexRange {
                index_offset: 0,
                index_count: 3,
            }],
        }
    }

    fn sample_nodes() -> BspNodesSection {
        BspNodesSection {
            nodes: vec![BspNodeRecord {
                plane_normal: [1.0, 0.0, 0.0],
                plane_distance: 32.0,
                front: -1,    // leaf 0
                back: -1 - 1, // leaf 1
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
                    is_solid: 0,
                },
                BspLeafRecord {
                    face_start: 0,
                    face_count: 0,
                    bounds_min: [32.0, 0.0, 0.0],
                    bounds_max: [64.0, 64.0, 64.0],
                    is_solid: 1,
                },
            ],
        }
    }

    fn sample_bvh() -> BvhSection {
        BvhSection {
            nodes: vec![FlatBvhNode {
                aabb_min: [0.0, 0.0, 0.0],
                skip_index: 1,
                aabb_max: [1.0, 1.0, 1.0],
                left_child_or_leaf_index: 0,
                flags: BVH_NODE_FLAG_LEAF,
                _padding: 0,
            }],
            leaves: vec![BvhLeaf {
                aabb_min: [0.0, 0.0, 0.0],
                material_bucket_id: 0,
                aabb_max: [1.0, 1.0, 1.0],
                index_offset: 0,
                index_count: 3,
                cell_id: 0,
                chunk_range_start: 0,
                chunk_range_count: 0,
            }],
            root_node_index: 0,
        }
    }

    fn empty_alpha_lights() -> AlphaLightsSection {
        AlphaLightsSection::default()
    }

    fn empty_light_influence() -> LightInfluenceSection {
        LightInfluenceSection::default()
    }

    fn empty_sh_volume() -> ShVolumeSection {
        ShVolumeSection {
            grid_origin: [0.0, 0.0, 0.0],
            cell_size: [1.0, 1.0, 1.0],
            grid_dimensions: [0, 0, 0],
            probe_stride: postretro_level_format::sh_volume::PROBE_STRIDE,
            probes: Vec::new(),
            animation_descriptors: Vec::new(),
        }
    }

    fn placeholder_lightmap() -> LightmapSection {
        LightmapSection::placeholder()
    }

    fn placeholder_chunk_light_list() -> ChunkLightListSection {
        ChunkLightListSection::placeholder()
    }

    #[test]
    fn pack_write_portals_produces_valid_prl_file() {
        let dir = std::env::temp_dir().join("postretro_test_pack");
        let _ = std::fs::create_dir_all(&dir);
        let output = dir.join("test_pack_portals.prl");

        let geo_result = sample_geo_result();
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
        let bvh = sample_bvh();

        let alpha_lights = empty_alpha_lights();
        pack_and_write_portals(
            &output,
            &geo_result,
            &nodes,
            &leaves,
            &portals,
            &bvh,
            &[],
            &alpha_lights,
            &empty_light_influence(),
            &empty_sh_volume(),
            &placeholder_lightmap(),
            &placeholder_chunk_light_list(),
            None,
            None,
            None,
            None,
        )
        .expect("pack_and_write_portals should succeed");

        let data = std::fs::read(&output).expect("should read output file");
        assert_eq!(&data[0..4], b"PRL\0");

        let mut cursor = Cursor::new(&data);
        let meta = read_container(&mut cursor).expect("should read container");
        assert_eq!(meta.header.section_count, 11);

        assert!(meta.find_section(SectionId::Geometry as u32).is_some());
        assert!(meta.find_section(SectionId::TextureNames as u32).is_some());
        assert!(meta.find_section(SectionId::BspNodes as u32).is_some());
        assert!(meta.find_section(SectionId::BspLeaves as u32).is_some());
        assert!(meta.find_section(SectionId::Portals as u32).is_some());
        assert!(meta.find_section(SectionId::Bvh as u32).is_some());
        assert!(meta.find_section(SectionId::AlphaLights as u32).is_some());
        assert!(
            meta.find_section(SectionId::LightInfluence as u32)
                .is_some()
        );
        assert!(meta.find_section(SectionId::ShVolume as u32).is_some());
        assert!(meta.find_section(SectionId::Lightmap as u32).is_some());

        let _ = std::fs::remove_file(&output);
    }

    #[test]
    fn pack_write_rejects_nonexistent_directory() {
        let output = Path::new("/nonexistent/deeply/nested/dir/test.prl");
        let geo_result = sample_geo_result();
        let nodes = sample_nodes();
        let leaves = sample_leaves();
        let portals = PortalsSection {
            vertices: vec![],
            portals: vec![],
        };
        let bvh = sample_bvh();
        let alpha_lights = empty_alpha_lights();

        let result = pack_and_write_portals(
            output,
            &geo_result,
            &nodes,
            &leaves,
            &portals,
            &bvh,
            &[],
            &alpha_lights,
            &empty_light_influence(),
            &empty_sh_volume(),
            &placeholder_lightmap(),
            &placeholder_chunk_light_list(),
            None,
            None,
            None,
            None,
        );
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(
            msg.contains("output directory does not exist"),
            "expected directory error, got: {msg}"
        );
    }

    #[test]
    fn full_pipeline_portal_mode_produces_valid_prl() {
        let map_path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .and_then(|p| p.parent())
            .expect("workspace root")
            .join("content/tests/maps/test.map");

        let map_data =
            crate::parse::parse_map_file(&map_path, crate::map_format::MapFormat::IdTech2)
                .expect("test.map should parse");
        let result =
            crate::partition::partition(&map_data.brush_volumes).expect("partition should succeed");

        let exterior = std::collections::HashSet::new();
        let geo_result = crate::geometry::extract_geometry(&result.faces, &result.tree, &exterior);
        let generated_portals = crate::portals::generate_portals(&result.tree);
        let vis_result = crate::visibility::encode_vis(&result.tree, &exterior);

        let (bvh, primitives, bvh_section) =
            crate::bvh_build::build_bvh(&geo_result).expect("bvh build should succeed");

        let static_lights =
            crate::light_namespaces::StaticBakedLights::from_lights(&map_data.lights);
        let animated_lights =
            crate::light_namespaces::AnimatedBakedLights::from_lights(&map_data.lights);
        let alpha_ns = crate::light_namespaces::AlphaLightsNs::from_lights(&map_data.lights);
        let sh_inputs = crate::sh_bake::BakeInputs {
            bvh: &bvh,
            primitives: &primitives,
            geometry: &geo_result,
            tree: &result.tree,
            exterior_leaves: &exterior,
            static_lights: &static_lights,
            animated_lights: &animated_lights,
        };
        let sh_volume = crate::sh_bake::bake_sh_volume(&sh_inputs, 4.0);

        let portals_section = encode_portals(&generated_portals);

        let dir = std::env::temp_dir().join("postretro_test_pipeline");
        let _ = std::fs::create_dir_all(&dir);
        let output = dir.join("test_pipeline_portals.prl");

        let alpha_lights = encode_alpha_lights(&alpha_ns, &result.tree);
        let light_influence = encode_light_influence(&alpha_ns);
        pack_and_write_portals(
            &output,
            &geo_result,
            &vis_result.nodes_section,
            &vis_result.leaves_section,
            &portals_section,
            &bvh_section,
            &[],
            &alpha_lights,
            &light_influence,
            &sh_volume,
            &placeholder_lightmap(),
            &placeholder_chunk_light_list(),
            None,
            None,
            None,
            None,
        )
        .expect("full pipeline portal pack should succeed");

        let data = std::fs::read(&output).expect("should read output file");
        let mut cursor = Cursor::new(&data);
        let meta = read_container(&mut cursor).expect("should read container");

        assert_eq!(meta.header.section_count, 11);
        assert!(meta.find_section(SectionId::Geometry as u32).is_some());
        assert!(meta.find_section(SectionId::TextureNames as u32).is_some());
        assert!(meta.find_section(SectionId::Portals as u32).is_some());
        assert!(meta.find_section(SectionId::Bvh as u32).is_some());
        assert!(meta.find_section(SectionId::AlphaLights as u32).is_some());
        assert!(
            meta.find_section(SectionId::LightInfluence as u32)
                .is_some()
        );
        assert!(meta.find_section(SectionId::ShVolume as u32).is_some());
        assert!(meta.find_section(SectionId::Lightmap as u32).is_some());
        assert!(meta.find_section(SectionId::BspNodes as u32).is_some());
        assert!(meta.find_section(SectionId::BspLeaves as u32).is_some());

        let _ = std::fs::remove_file(&output);
    }

    /// Every test map in `content/tests/maps/` must compile end-to-end and emit an
    /// SH volume section. The bake uses a coarse spacing (4 m) to keep test
    /// time bounded — the probe count is a design parameter, not what this
    /// test is exercising.
    #[test]
    fn every_test_map_compiles_with_sh_section() {
        let maps_dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .and_then(|p| p.parent())
            .expect("workspace root")
            .join("content/tests/maps");

        let mut map_count = 0;
        for entry in std::fs::read_dir(&maps_dir).expect("maps dir should exist") {
            let entry = entry.expect("dir entry");
            let path = entry.path();
            if path.extension().and_then(|s| s.to_str()) != Some("map") {
                continue;
            }
            map_count += 1;
            let map_data =
                crate::parse::parse_map_file(&path, crate::map_format::MapFormat::IdTech2)
                    .unwrap_or_else(|e| panic!("failed to parse {}: {e}", path.display()));
            let result = crate::partition::partition(&map_data.brush_volumes)
                .unwrap_or_else(|e| panic!("failed to partition {}: {e}", path.display()));
            let exterior = std::collections::HashSet::new();
            let geo_result =
                crate::geometry::extract_geometry(&result.faces, &result.tree, &exterior);
            let (bvh, primitives, _) = crate::bvh_build::build_bvh(&geo_result)
                .unwrap_or_else(|e| panic!("bvh build failed on {}: {e}", path.display()));

            let static_lights =
                crate::light_namespaces::StaticBakedLights::from_lights(&map_data.lights);
            let animated_lights =
                crate::light_namespaces::AnimatedBakedLights::from_lights(&map_data.lights);
            let sh_inputs = crate::sh_bake::BakeInputs {
                bvh: &bvh,
                primitives: &primitives,
                geometry: &geo_result,
                tree: &result.tree,
                exterior_leaves: &exterior,
                static_lights: &static_lights,
                animated_lights: &animated_lights,
            };
            let section = crate::sh_bake::bake_sh_volume(&sh_inputs, 4.0);

            // Every real test map has geometry, so the grid must have at
            // least 1 probe along each axis, and the section must round-trip.
            let dims = section.grid_dimensions;
            assert!(
                dims[0] > 0 && dims[1] > 0 && dims[2] > 0,
                "{} produced an empty SH grid",
                path.display()
            );
            let bytes = section.to_bytes();
            let restored = postretro_level_format::sh_volume::ShVolumeSection::from_bytes(&bytes)
                .unwrap_or_else(|e| {
                    panic!("sh volume round-trip failed for {}: {e}", path.display())
                });
            assert_eq!(section, restored);
        }
        assert!(
            map_count > 0,
            "no .map files found in {}",
            maps_dir.display()
        );
    }

    #[test]
    fn encode_light_influence_derives_correct_bounds() {
        use crate::map_data::{FalloffModel, LightType, MapLight};
        use glam::DVec3;

        let lights = vec![
            MapLight {
                origin: DVec3::new(10.0, 20.0, 30.0),
                light_type: LightType::Point,
                intensity: 1.0,
                color: [1.0, 1.0, 1.0],
                falloff_model: FalloffModel::InverseSquared,
                falloff_range: 50.0,
                cone_angle_inner: None,
                cone_angle_outer: None,
                cone_direction: None,
                animation: None,
                cast_shadows: false,
                bake_only: false,
                is_dynamic: false,
                tags: vec![],
            },
            MapLight {
                origin: DVec3::new(-4.0, 1.0, 0.5),
                light_type: LightType::Spot,
                intensity: 1.5,
                color: [1.0, 0.8, 0.6],
                falloff_model: FalloffModel::Linear,
                falloff_range: 25.0,
                cone_angle_inner: Some(0.5),
                cone_angle_outer: Some(0.8),
                cone_direction: Some([0.0, -1.0, 0.0]),
                animation: None,
                cast_shadows: true,
                bake_only: false,
                is_dynamic: false,
                tags: vec![],
            },
            MapLight {
                origin: DVec3::new(0.0, 100.0, 0.0),
                light_type: LightType::Directional,
                intensity: 0.9,
                color: [0.9, 0.95, 1.0],
                falloff_model: FalloffModel::Linear,
                falloff_range: 0.0,
                cone_angle_inner: None,
                cone_angle_outer: None,
                cone_direction: Some([0.0, -1.0, 0.0]),
                animation: None,
                cast_shadows: false,
                bake_only: false,
                is_dynamic: false,
                tags: vec![],
            },
        ];

        let alpha_ns = crate::light_namespaces::AlphaLightsNs::from_lights(&lights);
        let section = encode_light_influence(&alpha_ns);
        assert_eq!(section.records.len(), 3);

        // Point: center = position (f64→f32), radius = falloff_range.
        assert_eq!(section.records[0].center, [10.0, 20.0, 30.0]);
        assert_eq!(section.records[0].radius, 50.0);

        // Spot: same derivation as Point.
        assert_eq!(section.records[1].center, [-4.0, 1.0, 0.5]);
        assert_eq!(section.records[1].radius, 25.0);

        // Directional: center zeroed, radius = f32::MAX sentinel.
        assert_eq!(section.records[2].center, [0.0, 0.0, 0.0]);
        assert_eq!(section.records[2].radius, f32::MAX);
    }

    #[test]
    fn encode_alpha_lights_assigns_leaf_indices_and_flags_solid_leaf_lights() {
        use crate::map_data::{FalloffModel, LightType, MapLight};
        use crate::partition::{Aabb, BspChild, BspLeaf, BspNode, BspTree};
        use glam::DVec3;

        // Trivial tree: split on X = 0; back leaf (0) is empty, front leaf (1)
        // is solid. A light at +X lands in the solid leaf (sentinel); a light
        // at -X lands in the empty leaf (real index).
        let tree = BspTree {
            nodes: vec![BspNode {
                plane_normal: DVec3::X,
                plane_distance: 0.0,
                front: BspChild::Leaf(1),
                back: BspChild::Leaf(0),
                parent: None,
            }],
            leaves: vec![
                BspLeaf {
                    face_indices: vec![],
                    bounds: Aabb::empty(),
                    is_solid: false,
                },
                BspLeaf {
                    face_indices: vec![],
                    bounds: Aabb::empty(),
                    is_solid: true,
                },
            ],
        };

        let mk = |origin: DVec3| MapLight {
            origin,
            light_type: LightType::Point,
            intensity: 1.0,
            color: [1.0, 1.0, 1.0],
            falloff_model: FalloffModel::Linear,
            falloff_range: 10.0,
            cone_angle_inner: None,
            cone_angle_outer: None,
            cone_direction: None,
            animation: None,
            cast_shadows: false,
            bake_only: false,
            is_dynamic: false,
            tags: vec![],
        };

        let lights = vec![
            mk(DVec3::new(-5.0, 0.0, 0.0)),
            mk(DVec3::new(5.0, 0.0, 0.0)),
        ];

        let alpha_ns = crate::light_namespaces::AlphaLightsNs::from_lights(&lights);
        let section = encode_alpha_lights(&alpha_ns, &tree);
        assert_eq!(section.lights.len(), 2);
        assert_eq!(section.lights[0].leaf_index, 0);
        assert_eq!(
            section.lights[1].leaf_index,
            postretro_level_format::alpha_lights::ALPHA_LIGHT_LEAF_UNASSIGNED
        );
    }
}
