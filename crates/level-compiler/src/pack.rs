// Pack and write: serialize sections to .prl binary, validate via read-back.
// See: context/lib/build_pipeline.md §PRL Compilation

use std::fs;
use std::io::Cursor;
use std::path::Path;

use glam::Vec3;
use postretro_level_format::alpha_lights::{
    ALPHA_LIGHT_LEAF_UNASSIGNED, AlphaFalloffModel, AlphaLightRecord, AlphaLightType,
    AlphaLightsSection, AlphaShadowType,
};
use postretro_level_format::animated_light_chunks::AnimatedLightChunksSection;
use postretro_level_format::animated_light_weight_maps::AnimatedLightWeightMapsSection;
use postretro_level_format::bsp::{BspLeavesSection, BspNodesSection};
use postretro_level_format::bvh::BvhSection;
use postretro_level_format::cell_locator::{
    CellLocatorChild, CellLocatorNodeRecord, CellLocatorSection,
};
use postretro_level_format::cells::{
    CELL_FLAG_DRAWABLE, CELL_FLAG_EXTERIOR, CELL_FLAG_SOLID, CellRecord, CellsSection,
};
use postretro_level_format::chunk_light_list::ChunkLightListSection;
use postretro_level_format::data_script::DataScriptSection;
use postretro_level_format::delta_sh_volumes::DeltaShVolumesSection;
use postretro_level_format::direct_sh_volume::DirectShVolumeSection;
use postretro_level_format::fog_cell_masks::FogCellMasksSection;
use postretro_level_format::fog_volumes::{FogVolumeRecord, FogVolumesSection};
use postretro_level_format::light_influence::{InfluenceRecord, LightInfluenceSection};
use postretro_level_format::light_tags::LightTagsSection;
use postretro_level_format::lightmap::LightmapSection;
use postretro_level_format::map_entity::{MapEntityRecord, MapEntitySection};
use postretro_level_format::navmesh::NavMeshSection;
use postretro_level_format::portals::{PortalRecord, PortalsSection};
use postretro_level_format::sdf_atlas::SdfAtlasSection;
use postretro_level_format::sh_volume::OctahedralShVolumeSection;
use postretro_level_format::texture_cache_keys::TextureCacheKeysSection;
use postretro_level_format::{
    SectionBlob, SectionId, read_container, read_section_data, write_prl,
};

use std::collections::{HashMap, HashSet};

use crate::geometry::GeometryResult;
use crate::light_namespaces::AlphaLightsNs;
use crate::map_data::{FalloffModel, LightType, ShadowType};
use crate::partition::{BspChild, BspTree, find_leaf_for_point};
use crate::portals::Portal;

#[path = "pack_sections.rs"]
mod pack_sections;

use pack_sections::{append_optional_section, serialize_bvh_with_chunk_ranges};

/// Convert translated map lights into an `AlphaLightsSection` for the format
/// crate. Strips animation curves; the direct lighting path uses the static
/// base properties only.
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
                is_dynamic: l.is_dynamic,
                casts_entity_shadows: l.casts_entity_shadows,
                leaf_index,
                shadow_type: match l.shadow_type {
                    ShadowType::StaticLightMap => AlphaShadowType::StaticLightMap,
                    ShadowType::Sdf => AlphaShadowType::Sdf,
                },
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

/// Encode the collected non-light, non-worldspawn map entities into a
/// `MapEntitySection` for the runtime classname dispatch. Returns `None` when
/// the map carries no such entities — the caller omits the section so empty
/// maps add zero bytes.
///
/// Origin is narrowed from `f64` (compiler precision) to `f32` (engine /
/// runtime precision) at this boundary; angles are already engine-convention
/// `f32` from the format adapter.
pub fn encode_map_entities(
    entities: &[crate::map_data::MapEntityRecord],
) -> Option<MapEntitySection> {
    if entities.is_empty() {
        return None;
    }
    let entries = entities
        .iter()
        .map(|e| MapEntityRecord {
            classname: e.classname.clone(),
            origin: [e.origin.x as f32, e.origin.y as f32, e.origin.z as f32],
            angles: e.angles,
            key_values: e.key_values.clone(),
            tags: e.tags.clone(),
        })
        .collect();
    Some(MapEntitySection { entries })
}

/// Encode the resolved fog volume entities and the worldspawn-scoped scalars
/// (`fog_pixel_scale` and `initial_gravity`) into a `FogVolumesSection`.
/// Always produces a section so the worldspawn data is honoured at runtime,
/// even when the map carries no fog brushes.
pub fn encode_fog_volumes(
    fog_volumes: &[crate::map_data::MapFogVolume],
    fog_pixel_scale: u32,
    initial_gravity: f32,
) -> FogVolumesSection {
    let volumes = fog_volumes
        .iter()
        .map(|v| {
            // Bake derived AABB metrics at compile time so the raymarch shader
            // can skip recomputing them per ray step. `half_ext` is clamped
            // away from zero to avoid infinities for degenerate volumes.
            let min = Vec3::from(v.min);
            let max = Vec3::from(v.max);
            let center = (min + max) * 0.5;
            let half_ext = ((max - min) * 0.5).max(Vec3::splat(1.0e-6));
            let inv_half_ext = Vec3::ONE / half_ext;
            // Semantic point entities (fog_lamp sphere, fog_tube capsule) have
            // no planes and use `radial_falloff` for their fade shape. For a
            // sphere (isotropic AABB), normalise by the sphere radius so
            // `radial_t == 1` at the actual sphere surface and the shader's
            // `pow(1 - radial_t, radial_falloff)` reaches 0 exactly there.
            // Using the AABB half-diagonal (= R*sqrt(3) for a sphere) would
            // push the zero-density point to the AABB corners, leaving visible
            // density at the sphere boundary and making the volume look boxy.
            // For anisotropic volumes (fog_tube) keep the half-diagonal as
            // before; improving capsule shaping requires a proper capsule SDF
            // and is left for a future pass.
            let is_sphere_semantic = v.planes.is_empty()
                && (half_ext.x - half_ext.y).abs() < 1.0e-3
                && (half_ext.y - half_ext.z).abs() < 1.0e-3;
            let half_diag = if is_sphere_semantic {
                half_ext.x
            } else {
                half_ext.length()
            };
            // Typed bool in IR → float discriminant at write time: radial-fade
            // producers (plane-bounded `fog_volume`, `fog_lamp`, `fog_tube`) → 0.0;
            // axis-aligned `fog_volume` (ellipsoid path) → 1.0.
            let shape_mode = if v.is_ellipsoid { 1.0 } else { 0.0 };

            FogVolumeRecord {
                min: v.min,
                density: v.density,
                max: v.max,
                edge_softness: v.edge_softness,
                glow: v.glow,
                radial_falloff: v.radial_falloff,
                center: center.to_array(),
                inv_half_ext: inv_half_ext.to_array(),
                half_diag,
                shape_mode,
                tint: v.tint,
                saturation: v.saturation,
                min_brightness: v.min_brightness,
                light_range: v.light_range,
                anisotropy: v.anisotropy,
                ambient_scatter: v.ambient_scatter,
                plane_count: v.planes.len() as u32,
                planes: v.planes.clone(),
                tags: v.tags.clone(),
            }
        })
        .collect();
    FogVolumesSection {
        pixel_scale: fog_pixel_scale,
        initial_gravity,
        volumes,
    }
}

/// Build a `DataScriptSection` from already-compiled bytes and the resolved
/// source path. The compiler reads the source, runs `scripts-build` for `.ts`
/// inputs (or passes Luau through unchanged), then hands the result here for
/// embedding in the PRL.
pub fn encode_data_script(compiled_bytes: Vec<u8>, source_path: String) -> DataScriptSection {
    DataScriptSection {
        compiled_bytes,
        source_path,
    }
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

/// Encode runtime cells from BSP leaf records plus explicit exterior
/// classification. Cell ids stay one-to-one with BSP leaf ids.
pub fn encode_cells(
    leaves: &BspLeavesSection,
    portals: &PortalsSection,
    exterior_leaves: &HashSet<usize>,
) -> anyhow::Result<CellsSection> {
    if leaves.leaves.is_empty() {
        anyhow::bail!("Cells section requires at least one BSP leaf");
    }

    let mut portal_refs_by_cell: Vec<Vec<u32>> = vec![Vec::new(); leaves.leaves.len()];
    for (portal_idx, portal) in portals.portals.iter().enumerate() {
        let portal_idx = portal_idx as u32;
        let front = portal.front_leaf as usize;
        let back = portal.back_leaf as usize;
        if front >= leaves.leaves.len() || back >= leaves.leaves.len() {
            anyhow::bail!(
                "Cells portal adjacency references leaf out of range: portal {portal_idx} \
                 front={} back={} leaf_count={}",
                portal.front_leaf,
                portal.back_leaf,
                leaves.leaves.len()
            );
        }
        portal_refs_by_cell[front].push(portal_idx);
        portal_refs_by_cell[back].push(portal_idx);
    }
    for refs in &mut portal_refs_by_cell {
        refs.sort_unstable();
        refs.dedup();
    }

    let mut portal_refs = Vec::new();
    let mut cells = Vec::with_capacity(leaves.leaves.len());
    for (cell_idx, leaf) in leaves.leaves.iter().enumerate() {
        validate_cell_bounds(cell_idx, leaf)?;

        let solid = leaf.is_solid != 0;
        let exterior = exterior_leaves.contains(&cell_idx);
        if solid && exterior {
            anyhow::bail!("Cells cell {cell_idx} cannot be both solid and exterior");
        }
        if (solid || exterior) && leaf.face_count != 0 {
            anyhow::bail!(
                "Cells cell {cell_idx} is solid/exterior but has face_count {}",
                leaf.face_count
            );
        }

        let drawable = !solid && !exterior && leaf.face_count > 0;
        let flags = (u32::from(solid) * CELL_FLAG_SOLID)
            | (u32::from(exterior) * CELL_FLAG_EXTERIOR)
            | (u32::from(drawable) * CELL_FLAG_DRAWABLE);

        let refs = &portal_refs_by_cell[cell_idx];
        let (portal_ref_start, portal_ref_count) = if refs.is_empty() {
            (0, 0)
        } else {
            let start = portal_refs.len() as u32;
            portal_refs.extend_from_slice(refs);
            (start, refs.len() as u32)
        };

        cells.push(CellRecord {
            bounds_min: leaf.bounds_min,
            bounds_max: leaf.bounds_max,
            flags,
            face_start: if leaf.face_count == 0 {
                0
            } else {
                leaf.face_start
            },
            face_count: leaf.face_count,
            portal_ref_start,
            portal_ref_count,
        });
    }

    let section = CellsSection { cells, portal_refs };
    CellsSection::from_bytes(&section.to_bytes())?;
    Ok(section)
}

fn validate_cell_bounds(
    cell_idx: usize,
    leaf: &postretro_level_format::bsp::BspLeafRecord,
) -> anyhow::Result<()> {
    for axis in 0..3 {
        let min = leaf.bounds_min[axis];
        let max = leaf.bounds_max[axis];
        if !min.is_finite() || !max.is_finite() {
            anyhow::bail!(
                "Cells cell {cell_idx} has non-finite bounds on axis {axis}: min {min}, max {max}"
            );
        }
        if min > max {
            anyhow::bail!(
                "Cells cell {cell_idx} has inverted bounds on axis {axis}: min {min} > max {max}"
            );
        }
    }
    Ok(())
}

/// Encode the point-to-cell locator from the final BSP tree. Cell ids preserve
/// the BSP leaf id space, but the wire format names them as cells rather than
/// using the legacy negative leaf sentinel.
pub fn encode_cell_locator(tree: &BspTree) -> anyhow::Result<CellLocatorSection> {
    if tree.leaves.is_empty() {
        anyhow::bail!("CellLocator section requires at least one BSP leaf");
    }

    let root = if tree.nodes.is_empty() {
        CellLocatorChild::Cell(0)
    } else {
        CellLocatorChild::Node(0)
    };
    let mut nodes = Vec::with_capacity(tree.nodes.len());
    for node in &tree.nodes {
        nodes.push(CellLocatorNodeRecord {
            plane_normal: [
                node.plane_normal.x as f32,
                node.plane_normal.y as f32,
                node.plane_normal.z as f32,
            ],
            plane_distance: node.plane_distance as f32,
            front: locator_child(&node.front),
            back: locator_child(&node.back),
        });
    }

    let section = CellLocatorSection { root, nodes };
    CellLocatorSection::from_bytes(&section.to_bytes(), tree.leaves.len() as u32)?;
    Ok(section)
}

fn locator_child(child: &BspChild) -> CellLocatorChild {
    match child {
        BspChild::Node(index) => CellLocatorChild::Node(*index as u32),
        BspChild::Leaf(index) => CellLocatorChild::Cell(*index as u32),
    }
}

/// Write all required sections (geometry, texture names, texture cache keys,
/// cells, cell locator, portals, BVH, alpha lights, light influence,
/// lightmap, chunk light list, SH volume, and FogVolumes) and conditionally
/// write optional sections (direct SH volume, animated-light chunks and weight
/// maps, light tags, delta SH volumes, data script, map entities, and fog cell
/// masks) when their arguments are non-`None`. The direct SH volume is `None`
/// only when the map has no static (baked) lights at all — the loader treats
/// absence as direct = 0, so animated-only maps emit no direct section. A map
/// whose static-baked lights are all `ShadowType::Sdf` still emits a PRESENT
/// all-zero section: `Sdf` lights are dropped by `static_direct_lights` (their
/// direct term is runtime-traced), but the section itself is not omitted.
///
/// `texture_cache_keys` maps each texture name (as it appears in
/// `geo_result.texture_names.names`) to the 32-byte `.prm` filename key
/// produced by the texture-mip baker. Names absent from the map (no
/// authored PNG slots found) get an all-zero key, matching the baker's
/// "nothing to bake" sentinel.
#[allow(clippy::too_many_arguments)]
pub fn pack_and_write_portals(
    output: &Path,
    geo_result: &GeometryResult,
    texture_cache_keys: &HashMap<String, [u8; 32]>,
    _nodes: &BspNodesSection,
    leaves: &BspLeavesSection,
    tree: &BspTree,
    portals: &PortalsSection,
    exterior_leaves: &HashSet<usize>,
    bvh: &BvhSection,
    bvh_chunk_ranges: &[(u32, u32)],
    alpha_lights: &AlphaLightsSection,
    light_influence: &LightInfluenceSection,
    sh_volume: &OctahedralShVolumeSection,
    direct_sh_volume: Option<&DirectShVolumeSection>,
    lightmap: &LightmapSection,
    chunk_light_list: &ChunkLightListSection,
    animated_light_chunks: Option<&AnimatedLightChunksSection>,
    animated_light_weight_maps: Option<&AnimatedLightWeightMapsSection>,
    light_tags: Option<&LightTagsSection>,
    delta_sh_volumes: Option<&DeltaShVolumesSection>,
    data_script: Option<&DataScriptSection>,
    map_entities: Option<&MapEntitySection>,
    fog_volumes: &FogVolumesSection,
    fog_cell_masks: Option<&FogCellMasksSection>,
    sdf_atlas: Option<&SdfAtlasSection>,
    navmesh: Option<&NavMeshSection>,
    // Pre-serialized CellDrawIndex (id 37) bytes, or `None` for zero-leaf maps.
    // Already-encoded because the bake is gated on non-empty BVH leaves upstream;
    // emission is independent of portal presence.
    cell_draw_index_bytes: Option<Vec<u8>>,
) -> anyhow::Result<()> {
    let geometry_bytes = geo_result.geometry.to_bytes();
    let texture_names_bytes = geo_result.texture_names.to_bytes();
    let texture_cache_keys_section = TextureCacheKeysSection {
        keys: geo_result
            .texture_names
            .names
            .iter()
            .map(|name| texture_cache_keys.get(name).copied().unwrap_or([0u8; 32]))
            .collect(),
    };
    let texture_cache_keys_bytes = texture_cache_keys_section.to_bytes();
    let portals_bytes = portals.to_bytes();
    let cells_section = encode_cells(leaves, portals, exterior_leaves)?;
    let cells_bytes = cells_section.to_bytes();
    let locator_section = encode_cell_locator(tree)?;
    let locator_bytes = locator_section.to_bytes();
    let bvh_bytes = serialize_bvh_with_chunk_ranges(bvh, bvh_chunk_ranges);
    anyhow::ensure!(
        bvh.leaves.is_empty() || cell_draw_index_bytes.is_some(),
        "CellDrawIndex section is required when Bvh contains {} leaf/leaves",
        bvh.leaves.len()
    );
    anyhow::ensure!(
        !bvh.leaves.is_empty() || cell_draw_index_bytes.is_none(),
        "CellDrawIndex section must be omitted when Bvh has no leaves"
    );
    let alpha_lights_bytes = alpha_lights.to_bytes();
    let light_influence_bytes = light_influence.to_bytes();
    let sh_volume_bytes = sh_volume.to_bytes();
    let direct_sh_volume_bytes = direct_sh_volume.map(|s| s.to_bytes());
    let lightmap_bytes = lightmap.to_bytes();
    let chunk_light_list_bytes = chunk_light_list.to_bytes();
    let animated_light_chunks_bytes = animated_light_chunks.map(|s| s.to_bytes());
    let animated_light_weight_maps_bytes = animated_light_weight_maps.map(|s| s.to_bytes());
    let light_tags_bytes = light_tags.map(|s| s.to_bytes());
    let delta_sh_volumes_bytes = delta_sh_volumes.map(|s| s.to_bytes());
    let data_script_bytes = data_script.map(|s| s.to_bytes());
    let map_entities_bytes = map_entities.map(|s| s.to_bytes());
    let fog_volumes_bytes = fog_volumes.to_bytes();
    let fog_cell_masks_bytes = fog_cell_masks.map(|s| s.to_bytes());
    let sdf_atlas_bytes = sdf_atlas.map(|s| s.to_bytes());
    let navmesh_bytes = navmesh.map(|s| s.to_bytes());

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
            section_id: SectionId::TextureCacheKeys as u32,
            version: 1,
            data: texture_cache_keys_bytes.clone(),
        },
        SectionBlob {
            section_id: SectionId::Cells as u32,
            version: 1,
            data: cells_bytes.clone(),
        },
        SectionBlob {
            section_id: SectionId::CellLocator as u32,
            version: 1,
            data: locator_bytes.clone(),
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
            section_id: SectionId::OctahedralShVolume as u32,
            version: 1,
            data: sh_volume_bytes.clone(),
        },
        SectionBlob {
            section_id: SectionId::Lightmap as u32,
            version: 1,
            data: lightmap_bytes.clone(),
        },
    ];
    append_optional_section(
        &mut sections,
        SectionId::DirectShVolume as u32,
        direct_sh_volume_bytes.clone(),
    );
    append_optional_section(
        &mut sections,
        SectionId::AnimatedLightChunks as u32,
        animated_light_chunks_bytes.clone(),
    );
    append_optional_section(
        &mut sections,
        SectionId::AnimatedLightWeightMaps as u32,
        animated_light_weight_maps_bytes.clone(),
    );
    append_optional_section(
        &mut sections,
        SectionId::LightTags as u32,
        light_tags_bytes.clone(),
    );
    append_optional_section(
        &mut sections,
        SectionId::DeltaShVolumes as u32,
        delta_sh_volumes_bytes.clone(),
    );
    append_optional_section(
        &mut sections,
        SectionId::DataScript as u32,
        data_script_bytes.clone(),
    );
    append_optional_section(
        &mut sections,
        SectionId::MapEntity as u32,
        map_entities_bytes,
    );
    sections.push(SectionBlob {
        section_id: SectionId::FogVolumes as u32,
        version: 1,
        data: fog_volumes_bytes.clone(),
    });
    append_optional_section(
        &mut sections,
        SectionId::FogCellMasks as u32,
        fog_cell_masks_bytes.clone(),
    );
    if let Some(ref bytes) = sdf_atlas_bytes {
        sections.push(SectionBlob {
            section_id: SectionId::SdfAtlas as u32,
            version: postretro_level_format::sdf_atlas::SDF_ATLAS_VERSION as u16,
            data: bytes.clone(),
        });
    }
    if let Some(ref bytes) = navmesh_bytes {
        sections.push(SectionBlob {
            section_id: SectionId::NavMesh as u32,
            // Container SectionEntry.version reuses NAVMESH_VERSION (body
            // constant). Conceptually distinct — container vs. body version —
            // but coupled at 1 for now.
            version: postretro_level_format::navmesh::NAVMESH_VERSION,
            data: bytes.clone(),
        });
    }
    append_optional_section(
        &mut sections,
        SectionId::CellDrawIndex as u32,
        cell_draw_index_bytes,
    );

    write_and_validate_sections(output, &sections)?;

    log::info!("Sections: {}", sections.len());
    log::info!("  Geometry: {} bytes", geometry_bytes.len());
    log::info!("  TextureNames: {} bytes", texture_names_bytes.len());
    log::info!(
        "  TextureCacheKeys: {} bytes ({} keys)",
        texture_cache_keys_bytes.len(),
        texture_cache_keys_section.keys.len(),
    );
    log::info!(
        "  Cells: {} bytes ({} cells, {} portal refs)",
        cells_bytes.len(),
        cells_section.cells.len(),
        cells_section.portal_refs.len(),
    );
    log::info!(
        "  CellLocator: {} bytes ({} nodes)",
        locator_bytes.len(),
        locator_section.nodes.len(),
    );
    log::info!("  Portals: {} bytes", portals_bytes.len());
    log::info!("  Bvh: {} bytes", bvh_bytes.len());
    let assigned_count = alpha_lights
        .lights
        .iter()
        .filter(|r| r.leaf_index != ALPHA_LIGHT_LEAF_UNASSIGNED)
        .count();
    let unassigned_count = alpha_lights.lights.len() - assigned_count;
    log::info!(
        "  AlphaLights: {} bytes ({} lights, {} assigned to cells, {} unassigned)",
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
        "  OctahedralShVolume: {} bytes ({} probes)",
        sh_volume_bytes.len(),
        sh_volume.probes.len()
    );
    if let (Some(section), Some(bytes)) = (direct_sh_volume, &direct_sh_volume_bytes) {
        log::info!(
            "  DirectShVolume: {} bytes ({} probes, format {})",
            bytes.len(),
            section.total_probes(),
            section.irradiance_format,
        );
    }
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
    if let (Some(section), Some(bytes)) = (data_script, &data_script_bytes) {
        log::info!(
            "  DataScript: {} bytes ({} compiled bytes, source: {})",
            bytes.len(),
            section.compiled_bytes.len(),
            section.source_path,
        );
    }
    log::info!(
        "  FogVolumes: {} bytes ({} volumes, pixel_scale={})",
        fog_volumes_bytes.len(),
        fog_volumes.volumes.len(),
        fog_volumes.pixel_scale,
    );
    if let (Some(section), Some(bytes)) = (fog_cell_masks, &fog_cell_masks_bytes) {
        log::info!(
            "  FogCellMasks: {} bytes ({} cells)",
            bytes.len(),
            section.masks.len(),
        );
    }
    if let (Some(section), Some(bytes)) = (navmesh, &navmesh_bytes) {
        log::info!(
            "  NavMesh: {} bytes ({} regions, {} portals)",
            bytes.len(),
            section.regions.len(),
            section.portals.len(),
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
    use postretro_level_format::cell_draw_index::{CellDrawIndexSection, Span};
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

    fn empty_geo_result() -> GeometryResult {
        GeometryResult {
            geometry: GeometrySection {
                vertices: Vec::new(),
                indices: Vec::new(),
                faces: Vec::new(),
            },
            texture_names: TextureNamesSection { names: Vec::new() },
            face_index_ranges: Vec::new(),
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

    fn empty_draw_leaves() -> BspLeavesSection {
        let mut leaves = sample_leaves();
        leaves.leaves[0].face_count = 0;
        leaves
    }

    fn sample_tree() -> BspTree {
        BspTree {
            nodes: vec![crate::partition::BspNode {
                plane_normal: glam::DVec3::X,
                plane_distance: 32.0,
                front: crate::partition::BspChild::Leaf(0),
                back: crate::partition::BspChild::Leaf(1),
                parent: None,
            }],
            leaves: vec![
                crate::partition::BspLeaf {
                    face_indices: vec![0],
                    bounds: crate::partition::Aabb {
                        min: glam::DVec3::new(0.0, 0.0, 0.0),
                        max: glam::DVec3::new(32.0, 64.0, 64.0),
                    },
                    is_solid: false,
                    defining_planes: Vec::new(),
                },
                crate::partition::BspLeaf {
                    face_indices: Vec::new(),
                    bounds: crate::partition::Aabb {
                        min: glam::DVec3::new(32.0, 0.0, 0.0),
                        max: glam::DVec3::new(64.0, 64.0, 64.0),
                    },
                    is_solid: true,
                    defining_planes: Vec::new(),
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

    fn empty_bvh() -> BvhSection {
        BvhSection {
            nodes: Vec::new(),
            leaves: Vec::new(),
            root_node_index: 0,
        }
    }

    fn sample_cell_draw_index_bytes() -> Vec<u8> {
        CellDrawIndexSection {
            cell_count: 2,
            span_count: 1,
            cell_span_offset: vec![0, 1, 1],
            spans: vec![Span {
                leaf_start: 0,
                leaf_count: 1,
            }],
        }
        .to_bytes()
    }

    fn empty_alpha_lights() -> AlphaLightsSection {
        AlphaLightsSection::default()
    }

    fn empty_light_influence() -> LightInfluenceSection {
        LightInfluenceSection::default()
    }

    fn empty_sh_volume() -> OctahedralShVolumeSection {
        OctahedralShVolumeSection {
            grid_origin: [0.0, 0.0, 0.0],
            cell_size: [1.0, 1.0, 1.0],
            grid_dimensions: [0, 0, 0],
            probe_stride: postretro_level_format::sh_volume::OCTAHEDRAL_PROBE_STRIDE,
            tile_dimension: postretro_level_format::octahedral::DEFAULT_IRRADIANCE_TILE_DIMENSION,
            tile_border: postretro_level_format::octahedral::DEFAULT_IRRADIANCE_TILE_BORDER,
            atlas_dimensions: [0, 0],
            atlas_tiles_per_row: 0,
            probes: Vec::new(),
            atlas_texels: Vec::new(),
            animation_descriptors: Vec::new(),
            slot_for_map_light: Vec::new(),
        }
    }

    fn placeholder_lightmap() -> LightmapSection {
        LightmapSection::placeholder()
    }

    fn placeholder_chunk_light_list() -> ChunkLightListSection {
        ChunkLightListSection::placeholder()
    }

    #[test]
    fn encode_cells_preserves_leaf_ids_and_derives_sorted_unique_portal_refs() {
        let leaves = BspLeavesSection {
            leaves: vec![
                BspLeafRecord {
                    face_start: 5,
                    face_count: 2,
                    bounds_min: [0.0, 0.0, 0.0],
                    bounds_max: [1.0, 1.0, 1.0],
                    is_solid: 0,
                },
                BspLeafRecord {
                    face_start: 0,
                    face_count: 0,
                    bounds_min: [1.0, 0.0, 0.0],
                    bounds_max: [2.0, 1.0, 1.0],
                    is_solid: 0,
                },
                BspLeafRecord {
                    face_start: 99,
                    face_count: 0,
                    bounds_min: [2.0, 0.0, 0.0],
                    bounds_max: [3.0, 1.0, 1.0],
                    is_solid: 0,
                },
                BspLeafRecord {
                    face_start: 42,
                    face_count: 0,
                    bounds_min: [3.0, 0.0, 0.0],
                    bounds_max: [4.0, 1.0, 1.0],
                    is_solid: 1,
                },
            ],
        };
        let portals = PortalsSection {
            vertices: Vec::new(),
            portals: vec![
                PortalRecord {
                    vertex_start: 0,
                    vertex_count: 0,
                    front_leaf: 0,
                    back_leaf: 1,
                },
                PortalRecord {
                    vertex_start: 0,
                    vertex_count: 0,
                    front_leaf: 1,
                    back_leaf: 2,
                },
                PortalRecord {
                    vertex_start: 0,
                    vertex_count: 0,
                    front_leaf: 1,
                    back_leaf: 1,
                },
            ],
        };
        let exterior = HashSet::from([2usize]);

        let section = encode_cells(&leaves, &portals, &exterior).unwrap();

        assert_eq!(section.cells.len(), 4);
        assert_eq!(section.cells[0].flags, CELL_FLAG_DRAWABLE);
        assert_eq!(section.cells[0].face_start, 5);
        assert_eq!(section.cells[1].flags, 0, "empty interior is not exterior");
        assert_eq!(section.cells[2].flags, CELL_FLAG_EXTERIOR);
        assert_eq!(section.cells[2].face_start, 0);
        assert_eq!(section.cells[3].flags, CELL_FLAG_SOLID);
        assert_eq!(section.cells[3].face_start, 0);

        let cell_1_refs = &section.portal_refs[section.cells[1].portal_ref_start as usize
            ..(section.cells[1].portal_ref_start + section.cells[1].portal_ref_count) as usize];
        assert_eq!(cell_1_refs, &[0, 1, 2]);
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
        let texture_cache_keys: HashMap<String, [u8; 32]> = HashMap::new();
        pack_and_write_portals(
            &output,
            &geo_result,
            &texture_cache_keys,
            &nodes,
            &leaves,
            &sample_tree(),
            &portals,
            &HashSet::new(),
            &bvh,
            &[],
            &alpha_lights,
            &empty_light_influence(),
            &empty_sh_volume(),
            None,
            &placeholder_lightmap(),
            &placeholder_chunk_light_list(),
            None,
            None,
            None,
            None,
            None,
            None,
            &FogVolumesSection::default(),
            None,
            None,
            None,
            Some(sample_cell_draw_index_bytes()),
        )
        .expect("pack_and_write_portals should succeed");

        let data = std::fs::read(&output).expect("should read output file");
        assert_eq!(&data[0..4], b"PRL\0");

        let mut cursor = Cursor::new(&data);
        let meta = read_container(&mut cursor).expect("should read container");
        // Baseline modern sections plus always-emitted FogVolumes and required CellDrawIndex.
        assert_eq!(meta.header.section_count, 14);

        assert!(meta.find_section(SectionId::Geometry as u32).is_some());
        assert!(meta.find_section(SectionId::TextureNames as u32).is_some());
        assert!(
            meta.find_section(SectionId::TextureCacheKeys as u32)
                .is_some()
        );
        assert!(meta.find_section(SectionId::BspNodes as u32).is_none());
        assert!(meta.find_section(SectionId::BspLeaves as u32).is_none());
        assert!(meta.find_section(SectionId::Cells as u32).is_some());
        assert!(meta.find_section(SectionId::CellLocator as u32).is_some());
        assert!(meta.find_section(SectionId::Portals as u32).is_some());
        assert!(meta.find_section(SectionId::Bvh as u32).is_some());
        assert!(meta.find_section(SectionId::CellDrawIndex as u32).is_some());
        assert!(meta.find_section(SectionId::AlphaLights as u32).is_some());
        assert!(
            meta.find_section(SectionId::LightInfluence as u32)
                .is_some()
        );
        assert!(
            meta.find_section(SectionId::OctahedralShVolume as u32)
                .is_some()
        );
        assert!(meta.find_section(SectionId::Lightmap as u32).is_some());

        let _ = std::fs::remove_file(&output);
    }

    #[test]
    fn pack_write_rejects_missing_cell_draw_index_for_non_empty_bvh() {
        let dir = std::env::temp_dir().join("postretro_test_pack");
        let _ = std::fs::create_dir_all(&dir);
        let output = dir.join("test_pack_missing_cell_draw_index.prl");

        let geo_result = sample_geo_result();
        let nodes = sample_nodes();
        let leaves = sample_leaves();
        let portals = PortalsSection {
            vertices: vec![],
            portals: vec![],
        };
        let bvh = sample_bvh();
        let alpha_lights = empty_alpha_lights();
        let texture_cache_keys: HashMap<String, [u8; 32]> = HashMap::new();

        let result = pack_and_write_portals(
            &output,
            &geo_result,
            &texture_cache_keys,
            &nodes,
            &leaves,
            &sample_tree(),
            &portals,
            &HashSet::new(),
            &bvh,
            &[],
            &alpha_lights,
            &empty_light_influence(),
            &empty_sh_volume(),
            None,
            &placeholder_lightmap(),
            &placeholder_chunk_light_list(),
            None,
            None,
            None,
            None,
            None,
            None,
            &FogVolumesSection::default(),
            None,
            None,
            None,
            None,
        );

        let msg = result.expect_err("non-empty BVH without CellDrawIndex must fail");
        assert!(
            msg.to_string()
                .contains("CellDrawIndex section is required"),
            "got: {msg}"
        );
        let _ = std::fs::remove_file(&output);
    }

    #[test]
    fn pack_write_allows_empty_bvh_without_cell_draw_index() {
        let dir = std::env::temp_dir().join("postretro_test_pack");
        let _ = std::fs::create_dir_all(&dir);
        let output = dir.join("test_pack_empty_bvh_no_cell_draw_index.prl");

        let geo_result = empty_geo_result();
        let nodes = sample_nodes();
        let leaves = empty_draw_leaves();
        let portals = PortalsSection {
            vertices: vec![],
            portals: vec![],
        };
        let bvh = empty_bvh();
        let alpha_lights = empty_alpha_lights();
        let texture_cache_keys: HashMap<String, [u8; 32]> = HashMap::new();

        pack_and_write_portals(
            &output,
            &geo_result,
            &texture_cache_keys,
            &nodes,
            &leaves,
            &sample_tree(),
            &portals,
            &HashSet::new(),
            &bvh,
            &[],
            &alpha_lights,
            &empty_light_influence(),
            &empty_sh_volume(),
            None,
            &placeholder_lightmap(),
            &placeholder_chunk_light_list(),
            None,
            None,
            None,
            None,
            None,
            None,
            &FogVolumesSection::default(),
            None,
            None,
            None,
            None,
        )
        .expect("empty BVH may omit CellDrawIndex");

        let data = std::fs::read(&output).expect("should read output file");
        let mut cursor = Cursor::new(&data);
        let meta = read_container(&mut cursor).expect("should read container");
        assert!(meta.find_section(SectionId::Bvh as u32).is_some());
        assert!(meta.find_section(SectionId::CellDrawIndex as u32).is_none());
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
        let texture_cache_keys: HashMap<String, [u8; 32]> = HashMap::new();

        let result = pack_and_write_portals(
            output,
            &geo_result,
            &texture_cache_keys,
            &nodes,
            &leaves,
            &sample_tree(),
            &portals,
            &HashSet::new(),
            &bvh,
            &[],
            &alpha_lights,
            &empty_light_influence(),
            &empty_sh_volume(),
            None,
            &placeholder_lightmap(),
            &placeholder_chunk_light_list(),
            None,
            None,
            None,
            None,
            None,
            None,
            &FogVolumesSection::default(),
            None,
            None,
            None,
            Some(sample_cell_draw_index_bytes()),
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
            .join("content/dev/maps/campaign-test.map");

        let map_data =
            crate::parse::parse_map_file(&map_path, crate::map_format::MapFormat::IdTech2)
                .expect("campaign-test.map should parse");
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
        let sh_inputs = crate::sh_bake::ShBakeCtx {
            bvh: &bvh,
            primitives: &primitives,
            geometry: &geo_result,
            tree: &result.tree,
            exterior_leaves: &exterior,
            static_lights: &static_lights,
            animated_lights: &animated_lights,
            total_light_count: map_data.lights.len(),
        };
        let sh_volume = crate::sh_bake::bake_sh_volume(
            &sh_inputs,
            &crate::sh_bake::ShConfig { probe_spacing: 4.0 },
        );

        let portals_section = encode_portals(&generated_portals);

        let dir = std::env::temp_dir().join("postretro_test_pipeline");
        let _ = std::fs::create_dir_all(&dir);
        let output = dir.join("test_pipeline_portals.prl");

        let alpha_lights = encode_alpha_lights(&alpha_ns, &result.tree);
        let light_influence = encode_light_influence(&alpha_ns);
        let texture_cache_keys: HashMap<String, [u8; 32]> = HashMap::new();
        let cell_draw_index_bytes = crate::cell_draw_index_bake::bake_cell_draw_index(
            &bvh_section.leaves,
            &vis_result.leaves_section.leaves,
        )
        .map(|section| section.to_bytes());
        pack_and_write_portals(
            &output,
            &geo_result,
            &texture_cache_keys,
            &vis_result.nodes_section,
            &vis_result.leaves_section,
            &result.tree,
            &portals_section,
            &exterior,
            &bvh_section,
            &[],
            &alpha_lights,
            &light_influence,
            &sh_volume,
            None,
            &placeholder_lightmap(),
            &placeholder_chunk_light_list(),
            None,
            None,
            None,
            None,
            None,
            None,
            &FogVolumesSection::default(),
            None,
            None,
            None,
            cell_draw_index_bytes,
        )
        .expect("full pipeline portal pack should succeed");

        let data = std::fs::read(&output).expect("should read output file");
        let mut cursor = Cursor::new(&data);
        let meta = read_container(&mut cursor).expect("should read container");

        // Baseline modern sections plus always-emitted FogVolumes and required CellDrawIndex.
        assert_eq!(meta.header.section_count, 14);
        assert!(meta.find_section(SectionId::Geometry as u32).is_some());
        assert!(meta.find_section(SectionId::TextureNames as u32).is_some());
        assert!(
            meta.find_section(SectionId::TextureCacheKeys as u32)
                .is_some()
        );
        assert!(meta.find_section(SectionId::Portals as u32).is_some());
        assert!(meta.find_section(SectionId::Bvh as u32).is_some());
        assert!(meta.find_section(SectionId::CellDrawIndex as u32).is_some());
        assert!(meta.find_section(SectionId::AlphaLights as u32).is_some());
        assert!(
            meta.find_section(SectionId::LightInfluence as u32)
                .is_some()
        );
        assert!(
            meta.find_section(SectionId::OctahedralShVolume as u32)
                .is_some()
        );
        assert!(meta.find_section(SectionId::Lightmap as u32).is_some());
        assert!(meta.find_section(SectionId::BspNodes as u32).is_none());
        assert!(meta.find_section(SectionId::BspLeaves as u32).is_none());
        assert!(meta.find_section(SectionId::Cells as u32).is_some());
        assert!(meta.find_section(SectionId::CellLocator as u32).is_some());
        assert!(meta.find_section(SectionId::FogVolumes as u32).is_some());

        let _ = std::fs::remove_file(&output);
    }

    /// Every test map in `content/dev/maps/` must compile end-to-end and emit an
    /// SH volume section. The bake uses a coarse spacing (4 m) to keep test
    /// time bounded — the probe count is a design parameter, not what this
    /// test is exercising.
    #[test]
    fn every_test_map_compiles_with_sh_section() {
        let maps_dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .and_then(|p| p.parent())
            .expect("workspace root")
            .join("content/dev/maps");

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
            let sh_inputs = crate::sh_bake::ShBakeCtx {
                bvh: &bvh,
                primitives: &primitives,
                geometry: &geo_result,
                tree: &result.tree,
                exterior_leaves: &exterior,
                static_lights: &static_lights,
                animated_lights: &animated_lights,
                total_light_count: map_data.lights.len(),
            };
            let section = crate::sh_bake::bake_sh_volume(
                &sh_inputs,
                &crate::sh_bake::ShConfig { probe_spacing: 4.0 },
            );

            // Every real test map has geometry, so the grid must have at
            // least 1 probe along each axis, and the section must round-trip.
            let dims = section.grid_dimensions;
            assert!(
                dims[0] > 0 && dims[1] > 0 && dims[2] > 0,
                "{} produced an empty SH grid",
                path.display()
            );
            let bytes = section.to_bytes();
            let restored =
                postretro_level_format::sh_volume::OctahedralShVolumeSection::from_bytes(&bytes)
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
                light_size: 0.0,
                angular_diameter: 0.0,
                cone_angle_inner: None,
                cone_angle_outer: None,
                cone_direction: None,
                animation: None,
                bake_only: false,
                is_dynamic: false,
                casts_entity_shadows: false,
                is_animated: false,
                tags: vec![],
                shadow_type: crate::map_data::ShadowType::StaticLightMap,
            },
            MapLight {
                origin: DVec3::new(-4.0, 1.0, 0.5),
                light_type: LightType::Spot,
                intensity: 1.5,
                color: [1.0, 0.8, 0.6],
                falloff_model: FalloffModel::Linear,
                falloff_range: 25.0,
                light_size: 0.0,
                angular_diameter: 0.0,
                cone_angle_inner: Some(0.5),
                cone_angle_outer: Some(0.8),
                cone_direction: Some([0.0, -1.0, 0.0]),
                animation: None,
                bake_only: false,
                is_dynamic: false,
                casts_entity_shadows: false,
                is_animated: false,
                tags: vec![],
                shadow_type: crate::map_data::ShadowType::StaticLightMap,
            },
            MapLight {
                origin: DVec3::new(0.0, 100.0, 0.0),
                light_type: LightType::Directional,
                intensity: 0.9,
                color: [0.9, 0.95, 1.0],
                falloff_model: FalloffModel::Linear,
                falloff_range: 0.0,
                light_size: 0.0,
                angular_diameter: 0.0,
                cone_angle_inner: None,
                cone_angle_outer: None,
                cone_direction: Some([0.0, -1.0, 0.0]),
                animation: None,
                bake_only: false,
                is_dynamic: false,
                casts_entity_shadows: false,
                is_animated: false,
                tags: vec![],
                shadow_type: crate::map_data::ShadowType::StaticLightMap,
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
                    defining_planes: Vec::new(),
                },
                BspLeaf {
                    face_indices: vec![],
                    bounds: Aabb::empty(),
                    is_solid: true,
                    defining_planes: Vec::new(),
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
            light_size: 0.0,
            angular_diameter: 0.0,
            cone_angle_inner: None,
            cone_angle_outer: None,
            cone_direction: None,
            animation: None,
            bake_only: false,
            is_dynamic: false,
            casts_entity_shadows: false,
            is_animated: false,
            tags: vec![],
            shadow_type: crate::map_data::ShadowType::StaticLightMap,
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
