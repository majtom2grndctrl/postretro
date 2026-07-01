// PRL section decoding and cross-validation for runtime level data.
// See: context/lib/build_pipeline.md §PRL Compilation

use std::collections::HashSet;
use std::path::Path;

use glam::Vec3;
use postretro_level_format::alpha_lights::ALPHA_LIGHT_LEAF_UNASSIGNED;
use postretro_level_format::alpha_lights::{
    AlphaFalloffModel, AlphaLightType, AlphaLightsSection, AlphaShadowType,
};
use postretro_level_format::animated_light_chunks::AnimatedLightChunksSection;
use postretro_level_format::animated_light_weight_maps::AnimatedLightWeightMapsSection;
use postretro_level_format::bvh::{BVH_NODE_FLAG_LEAF, BvhSection};
use postretro_level_format::cell_draw_index::{CELL_DRAW_INDEX_VERSION, CellDrawIndexSection};
use postretro_level_format::cell_locator::CellLocatorSection;
use postretro_level_format::cells::CellsSection;
use postretro_level_format::chunk_light_list::ChunkLightListSection;
use postretro_level_format::data_script::DataScriptSection;
use postretro_level_format::delta_sh_volumes::{AFFINITY_FACTOR, DeltaShVolumesSection};
use postretro_level_format::direct_sh_volume::DirectShVolumeSection;
use postretro_level_format::fog_cell_masks::FogCellMasksSection;
use postretro_level_format::fog_volumes::{FogVolumeRecord, FogVolumesSection, MAX_FOG_VOLUMES};
use postretro_level_format::geometry::{GeometrySection, NO_TEXTURE};
use postretro_level_format::light_influence::LightInfluenceSection;
use postretro_level_format::light_tags::LightTagsSection;
use postretro_level_format::lightmap::LightmapSection;
use postretro_level_format::map_entity::{MapEntityRecord, MapEntitySection};
use postretro_level_format::navmesh::NavMeshSection;
use postretro_level_format::portals::PortalsSection;
use postretro_level_format::sdf_atlas::SdfAtlasSection;
use postretro_level_format::sh_volume::OctahedralShVolumeSection;
use postretro_level_format::texture_cache_keys::TextureCacheKeysSection;
use postretro_level_format::texture_names::TextureNamesSection;
use postretro_level_format::{self as prl_format, SectionId};

use postretro_render_data::geometry::{BvhLeaf, BvhNode, BvhTree, WorldVertex};
use postretro_render_data::influence::LightInfluence;
use postretro_render_data::material;

use super::{
    CellData, CellDrawIndex, CellLocatorChild, CellLocatorNodeData, FaceMeta, FalloffModel,
    LevelWorld, LightType, LightmapMode, MapLight, PortalData, PrlLoadError, ShadowType,
};

fn derive_material_with_warning(
    texture_name: &str,
    warned_prefixes: &mut HashSet<String>,
) -> material::Material {
    let warned_count = warned_prefixes.len();
    let mat = material::derive_material(texture_name, warned_prefixes);
    let prefix = material::parse_prefix(texture_name);
    if mat == material::Material::Default
        && !prefix.is_empty()
        && warned_prefixes.len() > warned_count
    {
        log::warn!(
            "[Material] Unknown prefix '{}' in texture '{}' — using default material",
            prefix,
            texture_name,
        );
    }
    mat
}

pub(crate) fn convert_alpha_lights(section: AlphaLightsSection) -> Vec<MapLight> {
    section
        .lights
        .into_iter()
        .map(|r| {
            let light_type = match r.light_type {
                AlphaLightType::Point => LightType::Point,
                AlphaLightType::Spot => LightType::Spot,
                AlphaLightType::Directional => LightType::Directional,
            };
            let falloff_model = match r.falloff_model {
                AlphaFalloffModel::Linear => FalloffModel::Linear,
                AlphaFalloffModel::InverseDistance => FalloffModel::InverseDistance,
                AlphaFalloffModel::InverseSquared => FalloffModel::InverseSquared,
            };
            let shadow_type = match r.shadow_type {
                AlphaShadowType::StaticLightMap => ShadowType::StaticLightMap,
                AlphaShadowType::Sdf => ShadowType::Sdf,
            };
            MapLight {
                origin: r.origin,
                light_type,
                intensity: r.intensity,
                color: r.color,
                falloff_model,
                falloff_range: r.falloff_range,
                cone_angle_inner: r.cone_angle_inner,
                cone_angle_outer: r.cone_angle_outer,
                cone_direction: r.cone_direction,
                is_dynamic: r.is_dynamic,
                casts_entity_shadows: r.casts_entity_shadows,
                animated_slot: None, // populated from ShVolume slot table later in load
                tags: vec![],        // populated by LightTags section pass below
                cell_index: r.leaf_index,
                shadow_type,
            }
        })
        .collect()
}

pub(crate) fn convert_bvh_section(section: BvhSection) -> BvhTree {
    let nodes = section
        .nodes
        .into_iter()
        .map(|n| BvhNode {
            aabb_min: n.aabb_min,
            skip_index: n.skip_index,
            aabb_max: n.aabb_max,
            left_child_or_leaf_index: n.left_child_or_leaf_index,
            flags: n.flags,
        })
        .collect();

    let leaves = section
        .leaves
        .into_iter()
        .map(|l| BvhLeaf {
            aabb_min: l.aabb_min,
            material_bucket_id: l.material_bucket_id,
            aabb_max: l.aabb_max,
            index_offset: l.index_offset,
            index_count: l.index_count,
            cell_id: l.cell_id,
            chunk_range_start: l.chunk_range_start,
            chunk_range_count: l.chunk_range_count,
        })
        .collect();

    BvhTree {
        nodes,
        leaves,
        root_node_index: section.root_node_index,
    }
}

pub(crate) fn convert_cells_section(section: CellsSection) -> (Vec<CellData>, Vec<u32>) {
    let cells = section
        .cells
        .into_iter()
        .map(|c| CellData {
            bounds_min: Vec3::from(c.bounds_min),
            bounds_max: Vec3::from(c.bounds_max),
            face_start: c.face_start,
            face_count: c.face_count,
            portal_ref_start: c.portal_ref_start,
            portal_ref_count: c.portal_ref_count,
            is_solid: c.is_solid(),
            is_exterior: c.is_exterior(),
            is_drawable: c.is_drawable(),
        })
        .collect();
    (cells, section.portal_refs)
}

pub(crate) fn convert_cell_locator_section(
    section: CellLocatorSection,
) -> (CellLocatorChild, Vec<CellLocatorNodeData>) {
    fn convert_child(
        child: postretro_level_format::cell_locator::CellLocatorChild,
    ) -> CellLocatorChild {
        match child {
            postretro_level_format::cell_locator::CellLocatorChild::Cell(index) => {
                CellLocatorChild::Cell(index as usize)
            }
            postretro_level_format::cell_locator::CellLocatorChild::Node(index) => {
                CellLocatorChild::Node(index as usize)
            }
        }
    }

    let root = convert_child(section.root);
    let nodes = section
        .nodes
        .into_iter()
        .map(|node| CellLocatorNodeData {
            plane_normal: Vec3::from(node.plane_normal),
            plane_distance: node.plane_distance,
            front: convert_child(node.front),
            back: convert_child(node.back),
        })
        .collect();
    (root, nodes)
}

/// Expected DeltaShVolumes affinity grid dims for a given base SH grid:
/// `ceil(base_dims / factor)` along each axis. The compiler bakes the affinity
/// grid this way; the loader rejects any section whose stored dims disagree.
/// Pure so the validation rule is unit-testable without a `.prl` file.
pub(crate) fn expected_affinity_dims(base_dims: [u32; 3], factor: u8) -> [u32; 3] {
    let f = factor as u32;
    [
        base_dims[0].div_ceil(f),
        base_dims[1].div_ceil(f),
        base_dims[2].div_ceil(f),
    ]
}

/// Validate a loaded DeltaShVolumes section against the engine's invariants.
/// `base` is the base OctahedralShVolume (id 34), or `None` if that section was
/// absent. Pure so the reject paths are unit-testable.
///
/// Rejects (clear typed error, no panic):
/// - `affinity_factor` != the engine's compiled-in `AFFINITY_FACTOR`,
/// - base ShVolume absent while a delta section is present,
/// - `affinity_dims` != `ceil(base_dims / affinity_factor)`,
/// - delta tile geometry differs from the base atlas tile geometry.
pub(crate) fn validate_delta_sh(
    section: &DeltaShVolumesSection,
    base: Option<&OctahedralShVolumeSection>,
) -> Result<(), PrlLoadError> {
    // affinity_factor is locked to the compose pass `@workgroup_size(4,4,4)`.
    if section.affinity_factor != AFFINITY_FACTOR {
        return Err(PrlLoadError::DeltaShAffinityFactorMismatch {
            found: section.affinity_factor,
            expected: AFFINITY_FACTOR,
        });
    }

    // The base grid's dims derive the expected affinity dims; the compose pass
    // cannot run without it.
    let Some(base) = base else {
        return Err(PrlLoadError::DeltaShMissingBaseVolume);
    };
    let base_dims = base.grid_dimensions;

    let expected = expected_affinity_dims(base_dims, AFFINITY_FACTOR);
    if section.affinity_dims != expected {
        return Err(PrlLoadError::DeltaShAffinityDimsMismatch {
            found: section.affinity_dims,
            base_dims,
            factor: AFFINITY_FACTOR as u32,
            expected,
        });
    }

    if section.tile_dimension != base.tile_dimension || section.tile_border != base.tile_border {
        return Err(PrlLoadError::DeltaShTileGeometryMismatch {
            found_dimension: section.tile_dimension,
            found_border: section.tile_border,
            base_dimension: base.tile_dimension,
            base_border: base.tile_border,
        });
    }

    Ok(())
}

fn stale_section(section: &'static str, id: SectionId) -> PrlLoadError {
    PrlLoadError::StaleFormatMissingSection {
        section,
        id: id as u32,
    }
}

fn ambiguous_runtime_bsp_sections(sections: String) -> PrlLoadError {
    PrlLoadError::AmbiguousRuntimeBspSections { sections }
}

fn section_validation(section: &'static str, message: impl Into<String>) -> PrlLoadError {
    PrlLoadError::SectionValidation {
        section,
        message: message.into(),
    }
}

fn section_validation_from_error(
    section: &'static str,
    err: impl std::fmt::Display,
) -> PrlLoadError {
    section_validation(section, err.to_string())
}

fn validate_cells_against_geometry(
    cells: &[CellData],
    face_meta: &[FaceMeta],
) -> Result<(), PrlLoadError> {
    let face_count = face_meta.len();
    let mut claimed_by = vec![None; face_count];

    for (cell_idx, cell) in cells.iter().enumerate() {
        let end = cell
            .face_start
            .checked_add(cell.face_count)
            .ok_or_else(|| {
                section_validation(
                    "Cells",
                    format!(
                        "cell {cell_idx} face_start {} + face_count {} overflows u32",
                        cell.face_start, cell.face_count
                    ),
                )
            })?;
        if end as usize > face_count {
            return Err(section_validation(
                "Cells",
                format!(
                    "cell {cell_idx} face range [{}..{}) exceeds Geometry face count {face_count}",
                    cell.face_start, end
                ),
            ));
        }

        for face_idx in cell.face_start..end {
            let face_idx = face_idx as usize;
            if let Some(previous_cell_idx) = claimed_by[face_idx] {
                return Err(section_validation(
                    "Cells",
                    format!(
                        "face {face_idx} is claimed by both cell {previous_cell_idx} and cell {cell_idx}"
                    ),
                ));
            }
            claimed_by[face_idx] = Some(cell_idx);

            let face_owner = face_meta[face_idx].leaf_index as usize;
            if face_owner != cell_idx {
                return Err(section_validation(
                    "Cells",
                    format!(
                        "cell {cell_idx} face range includes face {face_idx}, but Geometry leaf_index is {}",
                        face_meta[face_idx].leaf_index
                    ),
                ));
            }
        }
    }

    for (face_idx, face) in face_meta.iter().enumerate() {
        let owner = face.leaf_index as usize;
        let owner_cell = &cells[owner];
        if claimed_by[face_idx] != Some(owner) {
            let owner_start = owner_cell.face_start;
            let owner_end = owner_start
                .checked_add(owner_cell.face_count)
                .ok_or_else(|| {
                    section_validation(
                        "Cells",
                        format!(
                            "cell {owner} face_start {} + face_count {} overflows u32",
                            owner_cell.face_start, owner_cell.face_count
                        ),
                    )
                })?;
            return Err(section_validation(
                "Cells",
                format!(
                    "face {face_idx} has Geometry leaf_index {}, but owning cell range [{}..{}) does not cover it",
                    face.leaf_index, owner_start, owner_end
                ),
            ));
        }
    }
    Ok(())
}

fn validate_face_meta_cells(
    face_meta: &[FaceMeta],
    cells: &[CellData],
) -> Result<(), PrlLoadError> {
    for (face_idx, face) in face_meta.iter().enumerate() {
        if face.leaf_index as usize >= cells.len() {
            return Err(section_validation(
                "Geometry",
                format!(
                    "face {face_idx} leaf_index {} out of range for {} cells",
                    face.leaf_index,
                    cells.len()
                ),
            ));
        }
    }
    Ok(())
}

fn validate_bvh_leaf_cells(bvh_leaves: &[BvhLeaf], cells: &[CellData]) -> Result<(), PrlLoadError> {
    let mut cell_has_indexed_leaf = vec![false; cells.len()];
    for (leaf_idx, leaf) in bvh_leaves.iter().enumerate() {
        let cell = cells.get(leaf.cell_id as usize).ok_or_else(|| {
            section_validation(
                "Bvh",
                format!(
                    "BVH leaf {leaf_idx} cell_id {} out of range for {} cells",
                    leaf.cell_id,
                    cells.len()
                ),
            )
        })?;
        if !cell.is_drawable {
            return Err(section_validation(
                "Bvh",
                format!(
                    "BVH leaf {leaf_idx} references non-drawable cell {}",
                    leaf.cell_id
                ),
            ));
        }
        if leaf.index_count > 0 {
            cell_has_indexed_leaf[leaf.cell_id as usize] = true;
        }
    }

    for (cell_idx, cell) in cells.iter().enumerate() {
        if cell.is_drawable && !cell_has_indexed_leaf[cell_idx] {
            return Err(section_validation(
                "Bvh",
                format!("drawable cell {cell_idx} has no BVH leaf with drawable indices"),
            ));
        }
    }
    Ok(())
}

fn validate_bvh_structure(bvh: &BvhTree, geometry_index_count: usize) -> Result<(), PrlLoadError> {
    for (node_idx, node) in bvh.nodes.iter().enumerate() {
        if node.flags & !BVH_NODE_FLAG_LEAF != 0 {
            return Err(section_validation(
                "Bvh",
                format!(
                    "BVH node {node_idx} has unsupported flags {:#010x}",
                    node.flags
                ),
            ));
        }
        if node.flags & BVH_NODE_FLAG_LEAF != 0 {
            let leaf_index = node.left_child_or_leaf_index as usize;
            if leaf_index >= bvh.leaves.len() {
                return Err(section_validation(
                    "Bvh",
                    format!(
                        "BVH leaf node {node_idx} references leaf {leaf_index} out of range for {} leaves",
                        bvh.leaves.len()
                    ),
                ));
            }
        }
    }

    for (leaf_idx, leaf) in bvh.leaves.iter().enumerate() {
        if leaf.index_offset % 3 != 0 {
            return Err(section_validation(
                "Bvh",
                format!(
                    "BVH leaf {leaf_idx} index_offset {} starts inside a triangle",
                    leaf.index_offset
                ),
            ));
        }
        if leaf.index_count % 3 != 0 {
            return Err(section_validation(
                "Bvh",
                format!(
                    "BVH leaf {leaf_idx} index_count {} does not cover whole triangles",
                    leaf.index_count
                ),
            ));
        }
        let end = leaf
            .index_offset
            .checked_add(leaf.index_count)
            .ok_or_else(|| {
                section_validation(
                    "Bvh",
                    format!(
                        "BVH leaf {leaf_idx} index_offset {} + index_count {} overflows u32",
                        leaf.index_offset, leaf.index_count
                    ),
                )
            })?;
        if end as usize > geometry_index_count {
            return Err(section_validation(
                "Bvh",
                format!(
                    "BVH leaf {leaf_idx} index range [{}..{}) exceeds Geometry index count {geometry_index_count}",
                    leaf.index_offset, end
                ),
            ));
        }
    }

    for (leaf_idx, pair) in bvh.leaves.windows(2).enumerate() {
        if pair[0].material_bucket_id > pair[1].material_bucket_id {
            return Err(section_validation(
                "Bvh",
                format!(
                    "BVH leaves must be sorted by material_bucket_id; leaf {leaf_idx} bucket {} precedes leaf {} bucket {}",
                    pair[0].material_bucket_id,
                    leaf_idx + 1,
                    pair[1].material_bucket_id
                ),
            ));
        }
    }

    Ok(())
}

fn validate_light_cells(lights: &[MapLight], cells: &[CellData]) -> Result<(), PrlLoadError> {
    for (light_idx, light) in lights.iter().enumerate() {
        if light.cell_index == ALPHA_LIGHT_LEAF_UNASSIGNED {
            continue;
        }
        let Some(cell) = cells.get(light.cell_index as usize) else {
            return Err(section_validation(
                "AlphaLights",
                format!(
                    "light {light_idx} cell_index {} out of range for {} cells",
                    light.cell_index,
                    cells.len()
                ),
            ));
        };
        if cell.is_solid {
            return Err(section_validation(
                "AlphaLights",
                format!(
                    "light {light_idx} cell_index {} references a solid cell",
                    light.cell_index
                ),
            ));
        }
    }
    Ok(())
}

fn all_fog_slots_mask(volume_count: usize) -> Result<u32, PrlLoadError> {
    if volume_count > MAX_FOG_VOLUMES {
        return Err(section_validation(
            "FogVolumes",
            format!("volume count {volume_count} exceeds MAX_FOG_VOLUMES {MAX_FOG_VOLUMES}"),
        ));
    }
    if volume_count == 0 {
        Ok(0)
    } else {
        Ok((1u32 << volume_count) - 1)
    }
}

fn validate_fog_cell_masks(
    masks: Option<Vec<u32>>,
    cell_count: usize,
    fog_volume_count: usize,
) -> Result<Option<Vec<u32>>, PrlLoadError> {
    let all_slots_mask = all_fog_slots_mask(fog_volume_count)?;

    let Some(masks) = masks else {
        if fog_volume_count == 0 {
            return Ok(None);
        }
        return Err(section_validation(
            "FogCellMasks",
            format!(
                "section is required when FogVolumes contains {fog_volume_count} canonical volume(s)"
            ),
        ));
    };

    if masks.len() != cell_count {
        return Err(section_validation(
            "FogCellMasks",
            format!(
                "mask count {} does not match Cells cell_count {cell_count}",
                masks.len()
            ),
        ));
    }

    for (cell_idx, mask) in masks.iter().enumerate() {
        let extra = *mask & !all_slots_mask;
        if extra != 0 {
            return Err(section_validation(
                "FogCellMasks",
                format!(
                    "cell {cell_idx} mask {mask:#010x} contains bits outside all_slots_mask {all_slots_mask:#010x}"
                ),
            ));
        }
    }

    Ok(Some(masks))
}

fn validate_cell_portal_refs(
    cells: &[CellData],
    portal_refs: &[u32],
    portal_count: Option<usize>,
) -> Result<(), PrlLoadError> {
    for (cell_idx, cell) in cells.iter().enumerate() {
        let start = cell.portal_ref_start as usize;
        let count = cell.portal_ref_count as usize;
        let end = start.checked_add(count).ok_or_else(|| {
            section_validation(
                "Cells",
                format!(
                    "cell {cell_idx} portal_ref_start {start} + portal_ref_count {count} overflows usize"
                ),
            )
        })?;
        let refs = portal_refs.get(start..end).ok_or_else(|| {
            section_validation(
                "Cells",
                format!(
                    "cell {cell_idx} portal ref range [{start}..{end}) exceeds portal_refs length {}",
                    portal_refs.len()
                ),
            )
        })?;
        for window in refs.windows(2) {
            if window[1] <= window[0] {
                return Err(section_validation(
                    "Cells",
                    format!(
                        "cell {cell_idx} portal_refs must be sorted ascending and duplicate-free, got {} then {}",
                        window[0], window[1]
                    ),
                ));
            }
        }
        if let Some(portal_count) = portal_count {
            for &portal_ref in refs {
                if portal_ref as usize >= portal_count {
                    return Err(section_validation(
                        "Cells",
                        format!(
                            "cell {cell_idx} portal_ref {portal_ref} out of range for {portal_count} portals"
                        ),
                    ));
                }
            }
        }
    }
    Ok(())
}

fn convert_usable_portals(section: &PortalsSection) -> Option<Vec<PortalData>> {
    if section.portals.is_empty() {
        log::warn!("[PRL] Portals section is empty — using no-portals fallback");
        return None;
    }

    let mut portal_data = Vec::with_capacity(section.portals.len());
    for (portal_idx, pr) in section.portals.iter().enumerate() {
        let start = pr.vertex_start as usize;
        let count = pr.vertex_count as usize;
        let Some(end) = start.checked_add(count) else {
            log::warn!(
                "[PRL] Portals section unusable: portal {portal_idx} vertex_start {start} + vertex_count {count} overflows"
            );
            return None;
        };
        if count < 3 {
            log::warn!(
                "[PRL] Portals section unusable: portal {portal_idx} has {count} vertices; at least 3 are required"
            );
            return None;
        }
        let Some(vertices) = section.vertices.get(start..end) else {
            log::warn!(
                "[PRL] Portals section unusable: portal {portal_idx} vertex range {start}..{end} exceeds vertex count {}",
                section.vertices.len()
            );
            return None;
        };
        if !vertices
            .iter()
            .flatten()
            .all(|component| component.is_finite())
        {
            log::warn!(
                "[PRL] Portals section unusable: portal {portal_idx} has non-finite polygon vertices"
            );
            return None;
        }

        let area_vector = vertices
            .iter()
            .zip(vertices.iter().cycle().skip(1))
            .take(vertices.len())
            .fold(Vec3::ZERO, |sum, (a, b)| {
                sum + Vec3::from(*a).cross(Vec3::from(*b))
            });
        if area_vector.length_squared() <= 1.0e-12 {
            log::warn!("[PRL] Portals section unusable: portal {portal_idx} polygon has zero area");
            return None;
        }

        portal_data.push(PortalData {
            polygon: vertices.iter().map(|v| Vec3::from(*v)).collect(),
            front_cell: pr.front_leaf as usize,
            back_cell: pr.back_leaf as usize,
        });
    }

    Some(portal_data)
}

fn validate_portal_adjacency(
    cells: &[CellData],
    portal_refs: &[u32],
    portals: &[PortalData],
) -> Result<(), PrlLoadError> {
    validate_cell_portal_refs(cells, portal_refs, Some(portals.len()))?;

    let mut front_seen = vec![0u8; portals.len()];
    let mut back_seen = vec![0u8; portals.len()];

    for (portal_idx, portal) in portals.iter().enumerate() {
        if portal.front_cell == portal.back_cell {
            return Err(section_validation(
                "Portals",
                format!(
                    "portal {portal_idx} has identical endpoints {}",
                    portal.front_cell
                ),
            ));
        }
        for (label, cell_idx) in [("front", portal.front_cell), ("back", portal.back_cell)] {
            let cell = cells.get(cell_idx).ok_or_else(|| {
                section_validation(
                    "Portals",
                    format!(
                        "portal {portal_idx} {label} endpoint cell {cell_idx} out of range for {} cells",
                        cells.len()
                    ),
                )
            })?;
            if cell.is_solid {
                return Err(section_validation(
                    "Portals",
                    format!("portal {portal_idx} {label} endpoint cell {cell_idx} is solid"),
                ));
            }
        }
    }

    for (cell_idx, cell) in cells.iter().enumerate() {
        let start = cell.portal_ref_start as usize;
        let end = start + cell.portal_ref_count as usize;
        for &portal_ref in &portal_refs[start..end] {
            let portal_idx = portal_ref as usize;
            let portal = &portals[portal_idx];
            if portal.front_cell == cell_idx {
                front_seen[portal_idx] += 1;
            } else if portal.back_cell == cell_idx {
                back_seen[portal_idx] += 1;
            } else {
                return Err(section_validation(
                    "Portals",
                    format!(
                        "cell {cell_idx} adjacency lists portal {portal_idx}, but the portal endpoints are {} and {}",
                        portal.front_cell, portal.back_cell
                    ),
                ));
            }
        }
    }

    for portal_idx in 0..portals.len() {
        if front_seen[portal_idx] != 1 || back_seen[portal_idx] != 1 {
            let portal = &portals[portal_idx];
            return Err(section_validation(
                "Portals",
                format!(
                    "portal {portal_idx} must appear exactly once in endpoint cells {} and {}; saw front {} and back {}",
                    portal.front_cell,
                    portal.back_cell,
                    front_seen[portal_idx],
                    back_seen[portal_idx]
                ),
            ));
        }
    }

    Ok(())
}

/// Cross-validate a decoded `CellDrawIndex` section against the runtime BVH
/// leaf array and the loaded Cells section. `from_bytes` already enforced structural
/// CSR invariants (version, length, monotonic offsets, non-empty spans, no
/// `leaf_start + leaf_count` overflow); this layer enforces every invariant that
/// requires the *other* sections to be present.
///
/// Pure (no I/O, no logging) so each reject path is unit-testable without a
/// `.prl`. Returns `Err(reason)` describing the first failing invariant; the
/// loader wraps it as a `CellDrawIndex` section-validation load error.
///
/// Rejected cases:
/// - unsupported `version`,
/// - `cell_count != Cells.cell_count`,
/// - `cell_span_offset[0] != 0`, non-monotonic offsets, or
///   `cell_span_offset[cell_count] != span_count`,
/// - any span outside `[0, total_leaves)` (checked-add),
/// - any span whose leaves don't all carry `BvhLeaf.cell_id == cell`,
/// - any span covering a non-drawable leaf, or any span on a non-drawable cell,
/// - any span crossing a material-bucket boundary,
/// - spans out of ascending `leaf_start` order within a cell,
/// - adjacent same-cell/same-bucket spans that form a non-maximal run,
/// - overlapping / duplicate leaf coverage,
/// - any drawable leaf missing from the index,
/// - any non-drawable cell with a non-empty CSR row.
///
/// `version` is taken from the section header; `from_bytes` already rejects a
/// non-matching version, but the explicit guard keeps the rule local and lets a
/// future structurally-valid version bump be rejected here too.
pub(crate) fn validate_cell_draw_index(
    section: &CellDrawIndexSection,
    bvh_leaves: &[BvhLeaf],
    cells: &[CellData],
    version: u32,
) -> Result<(), String> {
    if version != CELL_DRAW_INDEX_VERSION {
        return Err(format!(
            "unsupported version {version}, expected {CELL_DRAW_INDEX_VERSION}"
        ));
    }

    let cell_count = section.cell_count as usize;
    if cell_count != cells.len() {
        return Err(format!(
            "cell_count {cell_count} != Cells cell_count {}",
            cells.len()
        ));
    }

    // Re-check the CSR offset invariants here too: a structurally-valid section
    // for a *future* version could reach this layer, and the validity of every
    // span lookup below depends on these. (from_bytes guards the current shape.)
    let offsets = &section.cell_span_offset;
    if offsets.len() != cell_count + 1 {
        return Err(format!(
            "offset table length {} != cell_count + 1 ({})",
            offsets.len(),
            cell_count + 1
        ));
    }
    if offsets[0] != 0 {
        return Err(format!("offset[0] {} != 0", offsets[0]));
    }
    for w in offsets.windows(2) {
        if w[1] < w[0] {
            return Err(format!("non-monotonic offsets: {} after {}", w[1], w[0]));
        }
    }
    if offsets[cell_count] != section.span_count {
        return Err(format!(
            "offset[cell_count] {} != span_count {}",
            offsets[cell_count], section.span_count
        ));
    }
    if section.spans.len() != section.span_count as usize {
        return Err(format!(
            "span array length {} != span_count {}",
            section.spans.len(),
            section.span_count
        ));
    }

    let total_leaves = bvh_leaves.len();

    // A BVH leaf is drawable iff it has indices AND its cell is drawable. Both
    // halves join through `cell_id == cell index`.
    let cell_is_drawable =
        |cell: usize| -> bool { cells.get(cell).is_some_and(|cell| cell.is_drawable) };
    let leaf_is_drawable = |leaf: &BvhLeaf| -> bool {
        leaf.index_count > 0 && cell_is_drawable(leaf.cell_id as usize)
    };

    // Coverage map over every BVH leaf: which leaves a cell row claims.
    let mut covered = vec![false; total_leaves];

    for cell in 0..cell_count {
        let start = offsets[cell] as usize;
        let end = offsets[cell + 1] as usize;
        let cell_spans = &section.spans[start..end];

        let mut prev_end: Option<u32> = None; // exclusive end of previous span
        let mut prev_bucket: Option<u32> = None;

        for span in cell_spans {
            let leaf_start = span.leaf_start;
            let leaf_count = span.leaf_count;
            // Structural guard re-asserted for non-current versions.
            if leaf_count == 0 {
                return Err(format!("cell {cell} has an empty span"));
            }
            let span_end = match leaf_start.checked_add(leaf_count) {
                Some(e) => e,
                None => {
                    return Err(format!(
                        "cell {cell} span leaf_start {leaf_start} + leaf_count {leaf_count} \
                         overflows u32"
                    ));
                }
            };
            if span_end as usize > total_leaves {
                return Err(format!(
                    "cell {cell} span [{leaf_start}, {span_end}) exceeds total BVH leaves \
                     {total_leaves}"
                ));
            }

            // Ascending, non-overlapping `leaf_start` within the cell.
            if let Some(pe) = prev_end {
                if leaf_start < pe {
                    return Err(format!(
                        "cell {cell} spans out of order / overlapping: span starting \
                         {leaf_start} follows a span ending {pe}"
                    ));
                }
            }

            let span_bucket = bvh_leaves[leaf_start as usize].material_bucket_id;

            // Every leaf in the span: belongs to this cell, is drawable, shares
            // one material bucket, and is not already covered.
            for idx in leaf_start..span_end {
                let leaf = &bvh_leaves[idx as usize];
                if leaf.cell_id as usize != cell {
                    return Err(format!(
                        "cell {cell} span covers BVH leaf {idx} whose cell_id is {} \
                         (wrong cell)",
                        leaf.cell_id
                    ));
                }
                if leaf.material_bucket_id != span_bucket {
                    return Err(format!(
                        "cell {cell} span [{leaf_start}, {span_end}) crosses material bucket \
                         boundary at leaf {idx} (bucket {} != {span_bucket})",
                        leaf.material_bucket_id
                    ));
                }
                if !leaf_is_drawable(leaf) {
                    return Err(format!(
                        "cell {cell} span covers non-drawable BVH leaf {idx} \
                         (index_count {} on a {} cell)",
                        leaf.index_count,
                        if cell_is_drawable(cell) {
                            "drawable"
                        } else {
                            "non-drawable"
                        }
                    ));
                }
                if covered[idx as usize] {
                    return Err(format!(
                        "cell {cell} span re-covers already-claimed BVH leaf {idx} \
                         (overlap / duplicate)"
                    ));
                }
                covered[idx as usize] = true;
            }

            // Non-maximal run: an adjacent same-bucket span that abuts the
            // previous one (prev_end == leaf_start) could have been one span.
            if let (Some(pe), Some(pb)) = (prev_end, prev_bucket) {
                if pe == leaf_start && pb == span_bucket {
                    return Err(format!(
                        "cell {cell} has a non-maximal run: spans abutting at leaf \
                         {leaf_start} in bucket {span_bucket} should be one span"
                    ));
                }
            }

            prev_end = Some(span_end);
            prev_bucket = Some(span_bucket);
        }
    }

    // Cross-check both directions:
    //   - every drawable BVH leaf must be covered by exactly one span,
    //   - non-drawable cells must have an empty CSR row.
    let mut cell_has_drawable_leaf = vec![false; cell_count];
    for (idx, leaf) in bvh_leaves.iter().enumerate() {
        if leaf_is_drawable(leaf) {
            // `leaf_is_drawable` already required `cell_id` to name a drawable
            // cell, so the cast is in range here.
            cell_has_drawable_leaf[leaf.cell_id as usize] = true;
            if !covered[idx] {
                return Err(format!(
                    "drawable BVH leaf {idx} (cell {}) is missing from the draw index",
                    leaf.cell_id
                ));
            }
        }
    }

    for cell in 0..cell_count {
        let row_empty = offsets[cell] == offsets[cell + 1];
        if !cell_has_drawable_leaf[cell] && !row_empty {
            return Err(format!("non-drawable cell {cell} has a non-empty CSR row"));
        }
    }

    Ok(())
}

/// Read an optional section's raw bytes by id, or `None` if the section is
/// absent from the container.
///
/// Generic over `section_id` so any optional PRL section routes through the
/// same read point. Wraps `prl_format::read_section_data`; the `FormatError`
/// converts into `PrlLoadError` via the `#[from]` impl on the error enum.
pub(crate) fn read_optional_section_data<R: std::io::Read + std::io::Seek>(
    cursor: &mut R,
    meta: &prl_format::ContainerMeta,
    section_id: u32,
) -> Result<Option<Vec<u8>>, PrlLoadError> {
    Ok(prl_format::read_section_data(cursor, meta, section_id)?)
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
        .ok_or_else(|| stale_section("Geometry", SectionId::Geometry))?;
    let geom = GeometrySection::from_bytes(&geom_data)
        .map_err(|err| section_validation_from_error("Geometry", err))?;

    let texture_names_data =
        read_optional_section_data(&mut cursor, &meta, SectionId::TextureNames as u32)?;
    let texture_names_section = match texture_names_data {
        Some(data) => Some(TextureNamesSection::from_bytes(&data)?),
        None => None,
    };
    let texture_names: Vec<String> = texture_names_section.map(|s| s.names).unwrap_or_default();

    // Required. Absence means the file is corrupt or was produced by a writer
    // that omitted section 32; reject so the texture cache never silently
    // degrades every surface to a placeholder on a bad file.
    let texture_cache_keys_data =
        prl_format::read_section_data(&mut cursor, &meta, SectionId::TextureCacheKeys as u32)?
            .ok_or(PrlLoadError::NoTextureCacheKeys)?;
    let texture_cache_keys = TextureCacheKeysSection::from_bytes(&texture_cache_keys_data)?;

    let mut warned_prefixes = HashSet::new();
    let vertices: Vec<WorldVertex> = geom
        .vertices
        .iter()
        .map(|v| WorldVertex {
            position: v.position,
            base_uv: v.uv, // raw texel-space; normalized after texture dimensions are known
            normal_oct: v.normal_oct,
            tangent_packed: v.tangent_packed,
            lightmap_uv: v.lightmap_uv,
            lightmap_layer: v.lightmap_layer as u32,
        })
        .collect();

    let face_meta: Vec<FaceMeta> = geom
        .faces
        .iter()
        .map(|f| {
            let (tex_idx, tex_name) = if f.texture_index == NO_TEXTURE {
                (None, String::new())
            } else {
                let name = texture_names
                    .get(f.texture_index as usize)
                    .cloned()
                    .unwrap_or_default();
                (Some(f.texture_index), name)
            };
            let mat = derive_material_with_warning(&tex_name, &mut warned_prefixes);
            FaceMeta {
                leaf_index: f.leaf_index,
                texture_index: tex_idx,
                texture_dimensions: (64, 64),
                texture_name: tex_name,
                material: mat,
            }
        })
        .collect();

    let indices = geom.indices;

    log::info!(
        "[PRL] Geometry: {} vertices, {} indices, {} faces, {} textures referenced",
        vertices.len(),
        indices.len(),
        face_meta.len(),
        texture_names.len()
    );

    // Required. Pre-BVH maps must be rebuilt with `prl-build`.
    let bvh_data = prl_format::read_section_data(&mut cursor, &meta, SectionId::Bvh as u32)?
        .ok_or_else(|| stale_section("Bvh", SectionId::Bvh))?;
    let bvh_section = BvhSection::from_bytes(&bvh_data)
        .map_err(|err| section_validation_from_error("Bvh", err))?;
    let bvh = convert_bvh_section(bvh_section);
    validate_bvh_structure(&bvh, indices.len())?;
    log::info!(
        "[PRL] BVH: {} nodes, {} leaves, root={}",
        bvh.nodes.len(),
        bvh.leaves.len(),
        bvh.root_node_index,
    );
    debug_assert!(
        bvh.nodes.is_empty() || (bvh.root_node_index as usize) < bvh.nodes.len(),
        "BVH root_node_index {} out of range for {} nodes",
        bvh.root_node_index,
        bvh.nodes.len(),
    );
    // Flag-bit sanity: every node's flags must be either clean (internal) or
    // exactly the leaf bit — the compiler doesn't use the reserved bits yet.
    debug_assert!(
        bvh.nodes
            .iter()
            .all(|n| n.flags == 0 || n.flags == BVH_NODE_FLAG_LEAF),
        "BVH nodes carry unexpected flag bits",
    );

    let has_legacy_bsp_nodes = meta.find_section(SectionId::BspNodes as u32).is_some();
    let has_legacy_bsp_leaves = meta.find_section(SectionId::BspLeaves as u32).is_some();

    let portals_section =
        match prl_format::read_section_data(&mut cursor, &meta, SectionId::Portals as u32)? {
            Some(data) => match PortalsSection::from_bytes(&data) {
                Ok(section) => Some(section),
                Err(err) => {
                    log::warn!(
                        "[PRL] Portals section malformed ({err}); using no-portals fallback"
                    );
                    None
                }
            },
            None => None,
        };

    let cells_section =
        match prl_format::read_section_data(&mut cursor, &meta, SectionId::Cells as u32)? {
            Some(data) => CellsSection::from_bytes(&data)
                .map_err(|err| section_validation_from_error("Cells", err))?,
            None => return Err(stale_section("Cells", SectionId::Cells)),
        };
    let cell_count = cells_section.cells.len();
    let portal_ref_count = cells_section.portal_refs.len();
    let (cells, cell_portal_refs) = convert_cells_section(cells_section);
    validate_face_meta_cells(&face_meta, &cells)?;
    validate_cells_against_geometry(&cells, &face_meta)?;
    validate_bvh_leaf_cells(&bvh.leaves, &cells)?;
    log::info!(
        "[PRL] Cells: {} cells, {} portal refs loaded",
        cell_count,
        portal_ref_count,
    );

    let (cell_locator_root, cell_locator_nodes) =
        match prl_format::read_section_data(&mut cursor, &meta, SectionId::CellLocator as u32)? {
            Some(data) => {
                let section = CellLocatorSection::from_bytes(&data, cells.len() as u32)
                    .map_err(|err| section_validation_from_error("CellLocator", err))?;
                let node_count = section.nodes.len();
                let converted = convert_cell_locator_section(section);
                log::info!("[PRL] CellLocator: {node_count} node(s) loaded");
                converted
            }
            None => return Err(stale_section("CellLocator", SectionId::CellLocator)),
        };

    // Optional — older maps fall back to empty with a warning.
    let mut lights: Vec<MapLight> = match prl_format::read_section_data(
        &mut cursor,
        &meta,
        SectionId::AlphaLights as u32,
    )? {
        Some(data) => {
            let section = AlphaLightsSection::from_bytes(&data)?;
            let count = section.lights.len();
            let converted = convert_alpha_lights(section);
            log::info!("[PRL] AlphaLights: {count} lights loaded");
            converted
        }
        None => {
            log::warn!(
                "[PRL] AlphaLights section missing — map predates the lighting foundation milestone; recompile with `prl-build` for lights to appear"
            );
            Vec::new()
        }
    };

    // 1:1 with AlphaLights; count mismatch = format error. Absence = no tags.
    if let Some(data) =
        prl_format::read_section_data(&mut cursor, &meta, SectionId::LightTags as u32)?
    {
        let section = LightTagsSection::from_bytes(&data)?;
        if section.tags.len() != lights.len() {
            return Err(PrlLoadError::FormatError(prl_format::FormatError::Io(
                std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    format!(
                        "LightTags count ({}) does not match AlphaLights count ({})",
                        section.tags.len(),
                        lights.len()
                    ),
                ),
            )));
        }
        let mut tagged = 0usize;
        for (light, tag_str) in lights.iter_mut().zip(section.tags) {
            let tag_list: Vec<String> = tag_str.split_whitespace().map(|t| t.to_string()).collect();
            if !tag_list.is_empty() {
                tagged += 1;
                light.tags = tag_list;
            }
        }
        log::info!("[PRL] LightTags: {tagged} tagged lights");
    }

    // Optional — absent/short → missing lights are treated as infinite-bound by
    // downstream consumers. Extra records remain malformed: they cannot map to a
    // light and would hide writer bugs if silently ignored.
    let light_influences: Vec<LightInfluence> = match prl_format::read_section_data(
        &mut cursor,
        &meta,
        SectionId::LightInfluence as u32,
    )? {
        Some(data) => {
            let section = LightInfluenceSection::from_bytes(&data)?;
            if section.records.len() > lights.len() {
                return Err(PrlLoadError::FormatError(prl_format::FormatError::Io(
                    std::io::Error::new(
                        std::io::ErrorKind::InvalidData,
                        format!(
                            "LightInfluence record count ({}) exceeds AlphaLights count ({})",
                            section.records.len(),
                            lights.len()
                        ),
                    ),
                )));
            }
            if section.records.len() < lights.len() {
                log::warn!(
                    "[PRL] LightInfluence: {} records for {} lights; missing tail entries are uncullable",
                    section.records.len(),
                    lights.len()
                );
            }
            let converted: Vec<_> = section
                .records
                .into_iter()
                .map(|r| LightInfluence {
                    center: glam::Vec3::from(r.center),
                    radius: r.radius,
                })
                .collect();
            log::info!("[PRL] LightInfluence: {} records loaded", converted.len());
            converted
        }
        None => {
            log::warn!("[Loader] LightInfluence section missing, no spatial culling this map");
            Vec::new()
        }
    };

    let sh_volume: Option<OctahedralShVolumeSection> = match prl_format::read_section_data(
        &mut cursor,
        &meta,
        SectionId::OctahedralShVolume as u32,
    )? {
        Some(data) => {
            let section = OctahedralShVolumeSection::from_bytes(&data)?;
            log::info!(
                "[PRL] OctahedralShVolume: {}×{}×{} grid ({} probes, {}×{} atlas, tile {} + border {}, {} tile(s)/row, {} animated layers)",
                section.grid_dimensions[0],
                section.grid_dimensions[1],
                section.grid_dimensions[2],
                section.probes.len(),
                section.atlas_dimensions[0],
                section.atlas_dimensions[1],
                section.tile_dimension,
                section.tile_border,
                section.atlas_tiles_per_row,
                section.animation_descriptors.len(),
            );
            Some(section)
        }
        None => return Err(PrlLoadError::NoOctahedralShVolume),
    };

    // Populate `MapLight.animated_slot` from the SH-volume slot table.
    // Resolution happens once here (load time), not per
    // `setLightAnimation` call. Legacy PRLs lack the table — every slot stays
    // `None` and the bridge takes the legacy `is_dynamic`-gated path.
    if let Some(sh) = sh_volume.as_ref()
        && !sh.slot_for_map_light.is_empty()
    {
        use postretro_level_format::sh_volume::ANIMATED_SLOT_NONE;
        if sh.slot_for_map_light.len() != lights.len() {
            log::warn!(
                "[PRL] OctahedralShVolume slot_for_map_light count ({}) != AlphaLights count ({}); skipping animated-slot resolution",
                sh.slot_for_map_light.len(),
                lights.len(),
            );
        } else {
            let mut resolved = 0usize;
            for (light, &slot) in lights.iter_mut().zip(sh.slot_for_map_light.iter()) {
                if slot != ANIMATED_SLOT_NONE {
                    light.animated_slot = Some(slot);
                    resolved += 1;
                }
            }
            log::info!("[PRL] Resolved {resolved} map-light → animated-slot mapping(s)");
        }
    }

    // Optional — absent → 1×1 white placeholder; bumped-Lambert degrades to flat white.
    let lightmap: Option<LightmapSection> =
        match prl_format::read_section_data(&mut cursor, &meta, SectionId::Lightmap as u32)? {
            Some(data) => {
                let section = LightmapSection::from_bytes(&data)?;
                log::info!(
                    "[PRL] Lightmap: {}x{} atlas, {} layer(s), {} B irradiance, {} B direction",
                    section.irr_width,
                    section.irr_height,
                    section.layer_count,
                    section.irradiance.len(),
                    section.direction.len(),
                );
                Some(section)
            }
            None => {
                log::warn!(
                    "[PRL] Lightmap section missing — static direct lighting disabled for this map"
                );
                None
            }
        };

    // Optional — absent → no static-occluder SDF; runtime shadow pass disabled.
    // An empty-geometry section (zero grid dims) is also a valid "no SDF"
    // marker; the renderer collapses it to the same disabled state.
    let sdf_atlas: Option<SdfAtlasSection> = match prl_format::read_section_data(
        &mut cursor,
        &meta,
        SectionId::SdfAtlas as u32,
    )? {
        Some(data) => {
            let section = SdfAtlasSection::from_bytes(&data)?;
            log::info!(
                "[PRL] SdfAtlas: grid={}×{}×{}, voxel_size={:.4}m, brick={} voxels, {} surface bricks",
                section.grid_dims[0],
                section.grid_dims[1],
                section.grid_dims[2],
                section.voxel_size_m,
                section.brick_size_voxels,
                section.surface_brick_count,
            );
            Some(section)
        }
        None => {
            log::info!(
                "[PRL] SdfAtlas section missing — runtime SDF shadow pass disabled (legacy PRL or no SDF bake)"
            );
            None
        }
    };

    // Optional — absent → full spec-buffer scan fallback.
    let chunk_light_list: Option<ChunkLightListSection> = match prl_format::read_section_data(
        &mut cursor,
        &meta,
        SectionId::ChunkLightList as u32,
    )? {
        Some(data) => {
            let section = ChunkLightListSection::from_bytes(&data)?;
            log::info!(
                "[PRL] ChunkLightList: {}×{}×{} grid, {} indices",
                section.grid_dimensions[0],
                section.grid_dimensions[1],
                section.grid_dimensions[2],
                section.light_indices.len(),
            );
            Some(section)
        }
        None => {
            log::info!(
                "[PRL] ChunkLightList section missing — specular path uses full-buffer fallback"
            );
            None
        }
    };

    // Optional — cross-checked against weight-map chunk count at runtime.
    let animated_light_chunks: Option<AnimatedLightChunksSection> =
        match prl_format::read_section_data(
            &mut cursor,
            &meta,
            SectionId::AnimatedLightChunks as u32,
        )? {
            Some(data) => {
                let section = AnimatedLightChunksSection::from_bytes(&data)?;
                log::info!(
                    "[PRL] AnimatedLightChunks: {} chunks, {} flat indices",
                    section.chunks.len(),
                    section.light_indices.len(),
                );
                Some(section)
            }
            None => None,
        };

    // Optional — absent → 1×1 zero atlas on animated-contribution slot.
    let animated_light_weight_maps: Option<AnimatedLightWeightMapsSection> =
        match prl_format::read_section_data(
            &mut cursor,
            &meta,
            SectionId::AnimatedLightWeightMaps as u32,
        )? {
            Some(data) => {
                let section = AnimatedLightWeightMapsSection::from_bytes(&data)?;
                log::info!(
                    "[PRL] AnimatedLightWeightMaps: {} chunks, {} covered texels, {} weight entries",
                    section.chunk_rects.len(),
                    section.offset_counts.len(),
                    section.texel_lights.len(),
                );
                Some(section)
            }
            None => None,
        };

    // Optional — absent → SH compose pass falls back to base→total copy.
    let delta_sh_volumes: Option<DeltaShVolumesSection> = match prl_format::read_section_data(
        &mut cursor,
        &meta,
        SectionId::DeltaShVolumes as u32,
    )? {
        Some(data) => {
            let section = DeltaShVolumesSection::from_bytes(&data)?;

            // Validation (mirrors the section-version reject path): a mismatched
            // bake must fail the load with a clear error rather than feed the
            // compose pass garbage. `sh_volume` (id 20) was loaded above.
            validate_delta_sh(&section, sh_volume.as_ref())?;

            log::info!(
                "[PRL] DeltaShVolumes: {} animated light(s), affinity grid {}×{}×{} \
                 ({} CSR entr(y/ies), {} delta subblock halves)",
                section.animation_descriptor_indices.len(),
                section.affinity_dims[0],
                section.affinity_dims[1],
                section.affinity_dims[2],
                section.affinity_lights.len(),
                section.delta_subblocks.len(),
            );
            Some(section)
        }
        None => None,
    };

    // Optional — absent for legacy v7 maps (no `SH_VOLUME_VERSION` bump) and for
    // maps with no static lights. Dynamic objects fall back to indirect-only.
    let direct_sh_volume: Option<DirectShVolumeSection> = match prl_format::read_section_data(
        &mut cursor,
        &meta,
        SectionId::DirectShVolume as u32,
    )? {
        Some(data) => {
            let section = DirectShVolumeSection::from_bytes(&data)?;
            log::info!(
                "[PRL] DirectShVolume: {}×{}×{} grid ({} probes, {}×{} atlas, tile {} + border {}, {} tile(s)/row, format {}, {} atlas byte(s))",
                section.grid_dimensions[0],
                section.grid_dimensions[1],
                section.grid_dimensions[2],
                section.total_probes(),
                section.atlas_dimensions[0],
                section.atlas_dimensions[1],
                section.tile_dimension,
                section.tile_border,
                section.atlas_tiles_per_row,
                section.irradiance_format,
                section.atlas.len(),
            );
            Some(section)
        }
        None => None,
    };

    // Optional — absent when map has no `data_script` worldspawn KVP.
    let data_script: Option<DataScriptSection> =
        match prl_format::read_section_data(&mut cursor, &meta, SectionId::DataScript as u32)? {
            Some(data) => {
                let section = DataScriptSection::from_bytes(&data)?;
                log::info!(
                    "[PRL] DataScript: {} bytes from `{}`",
                    section.compiled_bytes.len(),
                    section.source_path,
                );
                Some(section)
            }
            None => None,
        };

    // Optional — absent when no non-light, non-worldspawn entities exist.
    let map_entities: Vec<MapEntityRecord> =
        match prl_format::read_section_data(&mut cursor, &meta, SectionId::MapEntity as u32)? {
            Some(data) => {
                let section = MapEntitySection::from_bytes(&data)?;
                log::info!("[PRL] MapEntity: {} entities", section.entries.len());
                section.entries
            }
            None => Vec::new(),
        };

    // Required — carries `initial_gravity` alongside fog volumes. Absence = pre-gravity PRL;
    // rejected so the engine never silently falls back to a hardcoded default.
    let (fog_volumes, fog_pixel_scale, initial_gravity): (Vec<FogVolumeRecord>, u32, f32) =
        match prl_format::read_section_data(&mut cursor, &meta, SectionId::FogVolumes as u32)? {
            Some(data) => {
                let section = FogVolumesSection::from_bytes(&data)
                    .map_err(|err| section_validation_from_error("FogVolumes", err))?;
                log::info!(
                    "[PRL] FogVolumes: {} volumes, pixel_scale={}, initial_gravity={}",
                    section.volumes.len(),
                    section.pixel_scale,
                    section.initial_gravity,
                );
                (
                    section.volumes,
                    section.pixel_scale,
                    section.initial_gravity,
                )
            }
            None => return Err(PrlLoadError::NoWorldspawnGravity),
        };

    // Required when FogVolumes contains canonical fog entities; optional only
    // for no-fog maps, where `compute_fog_cell_mask` can keep all zero slots.
    let fog_cell_masks: Option<Vec<u32>> =
        match prl_format::read_section_data(&mut cursor, &meta, SectionId::FogCellMasks as u32)? {
            Some(data) => {
                let section = FogCellMasksSection::from_bytes(&data)
                    .map_err(|err| section_validation_from_error("FogCellMasks", err))?;
                log::info!("[PRL] FogCellMasks: {} cells", section.masks.len());
                Some(section.masks)
            }
            None => None,
        };

    // Optional — absent → no runtime navigation (logged at info, mirroring the
    // SdfAtlas precedent for the absent-section case). A malformed body warns
    // and decodes to None (softer than SdfAtlas, which propagates with `?` and
    // fails the load): nothing depends on the navmesh yet, so warn-and-continue
    // is intentional rather than making a malformed navmesh unplayable.
    let navmesh: Option<NavMeshSection> =
        match prl_format::read_section_data(&mut cursor, &meta, SectionId::NavMesh as u32)? {
            Some(data) => match NavMeshSection::from_bytes(&data) {
                Ok(section) => {
                    log::info!(
                        "[PRL] NavMesh: {}×{} grid, cell_size={:.4}m, {} region(s), {} portal(s)",
                        section.dim_x,
                        section.dim_z,
                        section.cell_size,
                        section.regions.len(),
                        section.portals.len(),
                    );
                    Some(section)
                }
                Err(err) => {
                    log::warn!("[PRL] NavMesh section malformed, ignoring: {err}");
                    None
                }
            },
            None => {
                log::info!("[PRL] NavMesh section missing — no runtime navigation for this map");
                None
            }
        };

    // Required when the BVH has leaves; omitted only for empty-BVH maps. Hold
    // the raw bytes until Cells and BVH are both available for cross-validation.
    let cell_draw_index_data =
        read_optional_section_data(&mut cursor, &meta, SectionId::CellDrawIndex as u32)?;

    validate_light_cells(&lights, &cells)?;

    if has_legacy_bsp_nodes || has_legacy_bsp_leaves {
        let mut sections = Vec::new();
        if has_legacy_bsp_nodes {
            sections.push("BspNodes(id 12)");
        }
        if has_legacy_bsp_leaves {
            sections.push("BspLeaves(id 13)");
        }
        return Err(ambiguous_runtime_bsp_sections(sections.join(", ")));
    }

    let fog_cell_masks = validate_fog_cell_masks(fog_cell_masks, cells.len(), fog_volumes.len())?;

    if bvh.leaves.is_empty()
        && (!vertices.is_empty() || !indices.is_empty() || !face_meta.is_empty())
    {
        return Err(section_validation(
            "Bvh",
            format!(
                "zero-leaf BVH cannot draw non-empty Geometry ({} vertices, {} indices, {} faces)",
                vertices.len(),
                indices.len(),
                face_meta.len()
            ),
        ));
    }
    if bvh.leaves.is_empty() {
        for (cell_idx, cell) in cells.iter().enumerate() {
            if cell.is_drawable {
                return Err(section_validation(
                    "Bvh",
                    format!("zero-leaf BVH cannot draw drawable cell {cell_idx}"),
                ));
            }
        }
    }

    // Decode + cross-validate the CellDrawIndex (id 37) now that Cells and BVH
    // are both available. Non-empty BVHs require it; empty BVHs reject it.
    let cell_draw_index: Option<CellDrawIndex> = if bvh.leaves.is_empty() {
        if cell_draw_index_data.is_some() {
            return Err(section_validation(
                "CellDrawIndex",
                "section is present for a zero-leaf BVH; empty BVHs must omit CellDrawIndex",
            ));
        }
        None
    } else {
        let data = cell_draw_index_data.ok_or_else(|| {
            section_validation(
                "CellDrawIndex",
                format!(
                    "section is required when Bvh contains {} leaf/leaves",
                    bvh.leaves.len()
                ),
            )
        })?;
        {
            let header_version = data
                .get(0..4)
                .map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]));
            let section = CellDrawIndexSection::from_bytes(&data)
                .map_err(|err| section_validation("CellDrawIndex", err.to_string()))?;
            let version = header_version.unwrap_or(CELL_DRAW_INDEX_VERSION);
            validate_cell_draw_index(&section, &bvh.leaves, &cells, version)
                .map_err(|reason| section_validation("CellDrawIndex", reason))?;
            log::info!(
                "[PRL] CellDrawIndex: {} cells, {} spans (candidate-cull index loaded)",
                section.cell_count,
                section.span_count,
            );
            Some(section)
        }
    };

    let portal_data = portals_section.as_ref().and_then(convert_usable_portals);
    if portal_data.is_none() {
        validate_cell_portal_refs(&cells, &cell_portal_refs, None)?;
    }
    let (portals, has_portals) = if let Some(portal_data) = portal_data {
        validate_portal_adjacency(&cells, &cell_portal_refs, &portal_data)?;
        (portal_data, true)
    } else {
        (Vec::new(), false)
    };

    log::info!(
        "[PRL] Loaded: {} vertices, {} indices ({} triangles), {} faces, {} cells, bvh=[{} nodes, {} leaves], portals={}, textures={}",
        vertices.len(),
        indices.len(),
        indices.len() / 3,
        face_meta.len(),
        cells.len(),
        bvh.nodes.len(),
        bvh.leaves.len(),
        portals.len(),
        texture_names.len(),
    );

    Ok(LevelWorld {
        vertices,
        indices,
        face_meta,
        cells,
        cell_portal_refs,
        cell_locator_root,
        cell_locator_nodes,
        portals,
        has_portals,
        texture_names,
        texture_cache_keys,
        bvh,
        lights,
        light_influences,
        sh_volume,
        lightmap,
        // Current bakes load as Shadowed. Unshadowed remains for legacy PRL
        // wire compatibility; new lightmaps should carry baked visibility.
        lightmap_mode: LightmapMode::default(),
        sdf_atlas,
        chunk_light_list,
        animated_light_chunks,
        animated_light_weight_maps,
        delta_sh_volumes,
        direct_sh_volume,
        data_script,
        map_entities,
        fog_volumes,
        fog_pixel_scale,
        initial_gravity,
        fog_cell_masks,
        navmesh,
        cell_draw_index,
    })
}
