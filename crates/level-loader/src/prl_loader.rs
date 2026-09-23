// PRL section decoding and cross-validation for runtime level data.
// See: context/lib/build_pipeline.md §PRL Compilation

use std::collections::{BTreeSet, HashMap, HashSet};

use glam::Vec3;
use postretro_level_format::alpha_lights::ALPHA_LIGHT_LEAF_UNASSIGNED;
use postretro_level_format::alpha_lights::{
    AlphaFalloffModel, AlphaLightType, AlphaLightsSection, AlphaShadowType,
};
use postretro_level_format::animated_billboard_direct_scatter_delta_volumes::AnimatedBillboardDirectScatterDeltaVolumesSection;
use postretro_level_format::animated_direct_sh_delta_volumes::AnimatedDirectShDeltaVolumesSection;
use postretro_level_format::animated_light_chunks::AnimatedLightChunksSection;
use postretro_level_format::animated_light_weight_maps::AnimatedLightWeightMapsSection;
use postretro_level_format::billboard_direct_scatter_volume::BillboardDirectScatterVolumeSection;
use postretro_level_format::bvh::{BVH_NODE_FLAG_LEAF, BvhSection};
use postretro_level_format::cell_draw_index::{CELL_DRAW_INDEX_VERSION, CellDrawIndexSection};
use postretro_level_format::cell_locator::CellLocatorSection;
use postretro_level_format::cell_visibility::CellVisibilitySection;
use postretro_level_format::cells::CellsSection;
use postretro_level_format::chunk_light_list::ChunkLightListSection;
use postretro_level_format::cluster_directory::{
    CLUSTER_DIRECTORY_CONTAINER_VERSION, ClusterDirectoryError, ClusterDirectorySection,
    ClusterDirectoryShInventory, ClusterDirectoryValidationInputs,
};
use postretro_level_format::data_script::DataScriptSection;
use postretro_level_format::delta_sh_volumes::{AFFINITY_FACTOR, DeltaShVolumesSection};
use postretro_level_format::direct_sh_delta_volumes::DirectShDeltaVolumesSection;
use postretro_level_format::direct_sh_volume::DirectShVolumeSection;
use postretro_level_format::entity_shadow_lights::EntityShadowLightsSection;
use postretro_level_format::fog_cell_masks::FogCellMasksSection;
use postretro_level_format::fog_volumes::{FogVolumeRecord, FogVolumesSection, MAX_FOG_VOLUMES};
use postretro_level_format::geometry::{GeometrySection, NO_TEXTURE};
use postretro_level_format::kinematic_geometry::{
    KINEMATIC_WAYPOINT_MIN_SEGMENT_LENGTH, KinematicGeometrySection,
};
use postretro_level_format::light_influence::LightInfluenceSection;
use postretro_level_format::light_tags::LightTagsSection;
use postretro_level_format::lightmap::LightmapSection;
use postretro_level_format::map_entity::{MapEntityRecord, MapEntitySection};
use postretro_level_format::navmesh::NavMeshSection;
use postretro_level_format::portals::PortalsSection;
use postretro_level_format::sdf_atlas::SdfAtlasSection;
use postretro_level_format::sh_volume::OctahedralShVolumeSection;
use postretro_level_format::shadowmask_atlas::ShadowmaskAtlasSection;
use postretro_level_format::texture_cache_keys::TextureCacheKeysSection;
use postretro_level_format::texture_names::TextureNamesSection;
use postretro_level_format::trigger_volumes::TriggerVolumesSection;
use postretro_level_format::{self as prl_format, SectionId};

use postretro_render_data::geometry::{BvhLeaf, BvhNode, BvhTree, WorldVertex};
use postretro_render_data::influence::LightInfluence;
use postretro_render_data::material;

use super::{
    CellData, CellDrawIndex, CellLocatorChild, CellLocatorNodeData, CellVisibility,
    CoupledCellPair, FaceMeta, FalloffModel, LevelWorld, LightType, LightmapMode, MapLight,
    PortalData, PrlLoadError, ShadowType,
};
use crate::prl::{KinematicGeometry, LoadedKinematicWaypoint};
use crate::prl_container::PrlContainer;
use crate::prl_lighting::LoadedLighting;
#[cfg(test)]
use crate::prl_lighting::read_bounded_delta_section_data;
pub(crate) use crate::prl_lighting::{
    BoundedDeltaSectionData, BoundedScatterSectionData, delta_grid_matches_base,
    read_bounded_delta_section_data_with_limit, read_bounded_scatter_section_data_with_limit,
    read_soft_optional_scatter_section_data,
    validate_animated_billboard_direct_scatter_against_metadata,
    validate_animated_billboard_direct_scatter_delta_volumes, validate_animated_direct_sh_delta,
    validate_billboard_direct_scatter_against_metadata, validate_billboard_direct_scatter_volume,
    validate_delta_sh, validate_direct_sh_delta, validate_direct_sh_layout,
    validate_entity_shadow_light_selection, validate_storage_ceiling_for_delta,
};
#[cfg(test)]
pub(crate) use crate::prl_lighting::{expected_affinity_dims, valid_probe_mask_for_affinity_cell};
#[cfg(test)]
use crate::prl_streaming::load_prl;
#[cfg(test)]
pub(crate) use crate::prl_streaming::{
    load_prl_with_delta_binding_limit, load_prl_with_scatter_pack_limit,
    load_prl_with_streaming_mode_for_test,
};
#[cfg(test)]
use crate::sh_stream::ShStreamingMode;
use crate::sh_stream::{ShStorage, ShStreamManifest};

/// Conservative desktop floor for one sparse SH delta section bound as a
/// storage buffer. This is intentionally a loader policy rather than a wire
/// limit: the compiler's separate aggregate cap protects bake output.
pub(crate) const MAX_DELTA_SECTION_BINDING_BYTES: u64 = 128 * 1024 * 1024;

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

pub(crate) fn convert_cell_visibility_section(section: CellVisibilitySection) -> CellVisibility {
    CellVisibility::new(
        section.component_ids,
        section
            .coupled_pairs
            .into_iter()
            .map(|pair| CoupledCellPair {
                cell_a: pair.cell_a as usize,
                cell_b: pair.cell_b as usize,
                distance: pair.distance,
                aperture: pair.aperture,
            })
            .collect(),
    )
}

pub(crate) fn convert_kinematic_geometry_section(
    section: KinematicGeometrySection,
) -> Result<KinematicGeometry, PrlLoadError> {
    let geometry = KinematicGeometry {
        movers: section.movers.into_iter().map(Into::into).collect(),
        waypoints: section.waypoints.into_iter().map(Into::into).collect(),
    };
    validate_kinematic_geometry(&geometry)?;
    Ok(geometry)
}

/// Drop presentation-only mover associations that cannot address this map's
/// usable portal array. The loader keeps one warning for the whole section.
fn drop_out_of_range_sealed_portal_ids(
    geometry: &mut KinematicGeometry,
    portal_count: usize,
) -> usize {
    let mut dropped_count = 0usize;
    for mover in &mut geometry.movers {
        let previous_len = mover.sealed_portal_ids.len();
        mover
            .sealed_portal_ids
            .retain(|&portal_id| (portal_id as usize) < portal_count);
        dropped_count += previous_len - mover.sealed_portal_ids.len();
    }
    dropped_count
}

/// Drop presentation-only light associations that cannot resolve to exactly
/// one dynamic AlphaLights record or compose to a runtime-finite spawn pose.
/// Invalid content leaves the light unbound.
fn drop_invalid_carried_light_links(
    geometry: &mut KinematicGeometry,
    lights: &[MapLight],
) -> usize {
    let mut reference_counts = HashMap::<u32, usize>::new();
    for member in geometry
        .movers
        .iter()
        .flat_map(|mover| mover.carried_lights.iter())
    {
        *reference_counts
            .entry(member.alpha_light_index)
            .or_default() += 1;
    }

    let duplicate_indices: HashSet<u32> = reference_counts
        .iter()
        .filter_map(|(&alpha_light_index, &count)| (count > 1).then_some(alpha_light_index))
        .collect();
    for &alpha_light_index in &duplicate_indices {
        let count = reference_counts[&alpha_light_index];
        log::warn!(
            "[PRL] KinematicGeometry: dropped {count} duplicate carried-light references to AlphaLight {alpha_light_index}; a light may have only one carrier"
        );
    }

    let mut dropped_count = 0usize;
    for mover in &mut geometry.movers {
        let mover_id = mover.mover_id;
        let mover_name = mover.name.clone();
        mover.carried_lights.retain(|member| {
            if duplicate_indices.contains(&member.alpha_light_index) {
                dropped_count += 1;
                return false;
            }
            let Some(light) = lights.get(member.alpha_light_index as usize) else {
                log::warn!(
                    "[PRL] KinematicGeometry: dropped mover {mover_id} (`{mover_name}`) carried-light reference to AlphaLight {} outside the {} loaded lights",
                    member.alpha_light_index,
                    lights.len(),
                );
                dropped_count += 1;
                return false;
            };
            if !light.is_dynamic {
                log::warn!(
                    "[PRL] KinematicGeometry: dropped mover {mover_id} (`{mover_name}`) carried-light reference to non-dynamic AlphaLight {}",
                    member.alpha_light_index,
                );
                dropped_count += 1;
                return false;
            }
            let composed_position = [
                f64::from(mover.origin.x) + f64::from(member.local_offset.x),
                f64::from(mover.origin.y) + f64::from(member.local_offset.y),
                f64::from(mover.origin.z) + f64::from(member.local_offset.z),
            ];
            if composed_position
                .iter()
                .any(|component| {
                    !component.is_finite() || component.abs() > f64::from(f32::MAX)
                })
            {
                log::warn!(
                    "[PRL] KinematicGeometry: dropped mover {mover_id} (`{mover_name}`) carried-light reference to AlphaLight {} because its authored position is outside the runtime f32 range",
                    member.alpha_light_index,
                );
                dropped_count += 1;
                return false;
            }
            true
        });
    }
    dropped_count
}

fn validate_kinematic_geometry(geometry: &KinematicGeometry) -> Result<(), PrlLoadError> {
    let mut waypoints: HashMap<&str, usize> = HashMap::new();
    for (index, waypoint) in geometry.waypoints.iter().enumerate() {
        if waypoint.name.is_empty() {
            return Err(section_validation(
                "KinematicGeometry",
                format!("waypoint {index} has an empty name"),
            ));
        }
        if !waypoint.origin.is_finite() {
            return Err(section_validation(
                "KinematicGeometry",
                format!(
                    "waypoint `{}` has non-finite origin {:?}",
                    waypoint.name, waypoint.origin
                ),
            ));
        }
        if waypoints.insert(waypoint.name.as_str(), index).is_some() {
            return Err(section_validation(
                "KinematicGeometry",
                format!("duplicate waypoint name `{}`", waypoint.name),
            ));
        }
    }

    let mut mover_ids = HashSet::new();
    for mover in &geometry.movers {
        if !mover_ids.insert(mover.mover_id) {
            return Err(section_validation(
                "KinematicGeometry",
                format!("duplicate mover_id {}", mover.mover_id),
            ));
        }
        if mover.vertices.is_empty() || mover.indices.is_empty() {
            return Err(section_validation(
                "KinematicGeometry",
                format!(
                    "mover {} (`{}`) geometry must contain vertices and indices",
                    mover.mover_id, mover.name
                ),
            ));
        }
        if !mover.origin.is_finite() {
            return Err(section_validation(
                "KinematicGeometry",
                format!(
                    "mover {} (`{}`) has non-finite origin {:?}",
                    mover.mover_id, mover.name, mover.origin
                ),
            ));
        }
        let resolved = resolve_kinematic_waypoint_chain(
            mover.mover_id,
            &mover.name,
            &mover.path,
            &geometry.waypoints,
            &waypoints,
        )?;
        if mover.spin_speed_deg_s != 0.0 && mover.spin_speed_deg_s.to_radians() == 0.0 {
            return Err(section_validation(
                "KinematicGeometry",
                format!(
                    "mover {} (`{}`) has nonzero spin_speed_deg_s that becomes zero after conversion to radians/sec",
                    mover.mover_id, mover.name
                ),
            ));
        }
        if mover.spin_accel_deg_s2 > 0.0 && mover.spin_accel_deg_s2.to_radians() == 0.0 {
            return Err(section_validation(
                "KinematicGeometry",
                format!(
                    "mover {} (`{}`) has positive spin_accel_deg_s2 that becomes zero after conversion to radians/sec²",
                    mover.mover_id, mover.name
                ),
            ));
        }
        if mover.spin_speed_deg_s != 0.0 && mover.spin_axis.normalize_or_zero() == Vec3::ZERO {
            return Err(section_validation(
                "KinematicGeometry",
                format!(
                    "mover {} (`{}`) has nonzero spin_speed_deg_s but spin_axis normalizes to zero",
                    mover.mover_id, mover.name
                ),
            ));
        }
        let allow_single_waypoint = mover.spin_speed_deg_s != 0.0;
        if resolved.len() < 2 && !allow_single_waypoint {
            return Err(section_validation(
                "KinematicGeometry",
                format!(
                    "mover {} (`{}`) path `{}` resolves to {} waypoint(s); at least 2 required",
                    mover.mover_id,
                    mover.name,
                    mover.path,
                    resolved.len()
                ),
            ));
        }
        if (mover.origin - resolved[0]).length() > KINEMATIC_WAYPOINT_MIN_SEGMENT_LENGTH {
            return Err(section_validation(
                "KinematicGeometry",
                format!(
                    "mover {} (`{}`) origin {:?} must match first waypoint `{}` at {:?}",
                    mover.mover_id, mover.name, mover.origin, mover.path, resolved[0]
                ),
            ));
        }
    }

    Ok(())
}

fn resolve_kinematic_waypoint_chain(
    mover_id: u32,
    mover_name: &str,
    path: &str,
    waypoints: &[LoadedKinematicWaypoint],
    waypoint_indices: &HashMap<&str, usize>,
) -> Result<Vec<Vec3>, PrlLoadError> {
    if path.is_empty() {
        return Err(section_validation(
            "KinematicGeometry",
            format!("mover {mover_id} (`{mover_name}`) has an empty path"),
        ));
    }

    let mut positions: Vec<Vec3> = Vec::new();
    let mut seen = HashSet::new();
    let mut current = path;
    loop {
        if !seen.insert(current.to_string()) {
            return Err(section_validation(
                "KinematicGeometry",
                format!("mover {mover_id} (`{mover_name}`) waypoint chain cycles at `{current}`"),
            ));
        }
        let Some(&index) = waypoint_indices.get(current) else {
            return Err(section_validation(
                "KinematicGeometry",
                format!(
                    "mover {mover_id} (`{mover_name}`) references unknown waypoint `{current}`"
                ),
            ));
        };
        let waypoint = &waypoints[index];
        if let Some(previous) = positions.last() {
            if (*previous - waypoint.origin).length() <= KINEMATIC_WAYPOINT_MIN_SEGMENT_LENGTH {
                return Err(section_validation(
                    "KinematicGeometry",
                    format!(
                        "mover {mover_id} (`{mover_name}`) path has zero-length segment ending at waypoint `{}`",
                        waypoint.name
                    ),
                ));
            }
        }
        positions.push(waypoint.origin);
        if waypoint.next.is_empty() {
            break;
        }
        current = waypoint.next.as_str();
    }
    Ok(positions)
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

fn stale_section(section: &'static str, id: SectionId) -> PrlLoadError {
    PrlLoadError::StaleFormatMissingSection {
        section,
        id: id as u32,
    }
}

fn ambiguous_runtime_bsp_sections(sections: String) -> PrlLoadError {
    PrlLoadError::AmbiguousRuntimeBspSections { sections }
}

pub(crate) fn section_validation(
    section: &'static str,
    message: impl Into<String>,
) -> PrlLoadError {
    PrlLoadError::SectionValidation {
        section,
        message: message.into(),
    }
}

pub(crate) fn section_validation_from_error(
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

fn cluster_adjacency_from_portals(
    directory: &ClusterDirectorySection,
    portals: &PortalsSection,
) -> Result<Vec<Vec<u32>>, PrlLoadError> {
    let runtime_cell_count = usize::try_from(directory.runtime_cell_count).map_err(|_| {
        section_validation(
            "ClusterDirectory",
            "runtime cell count exceeds this platform's address space",
        )
    })?;
    let mut cell_to_cluster = vec![None; runtime_cell_count];
    for (cluster_id, cluster) in directory.clusters.iter().enumerate() {
        let member_start = usize::try_from(cluster.member_start)
            .map_err(|_| section_validation("ClusterDirectory", "member start exceeds usize"))?;
        let member_end = member_start
            .checked_add(usize::try_from(cluster.member_count).map_err(|_| {
                section_validation("ClusterDirectory", "member count exceeds usize")
            })?)
            .ok_or_else(|| section_validation("ClusterDirectory", "member range overflows"))?;
        let members = directory
            .members
            .get(member_start..member_end)
            .ok_or_else(|| {
                section_validation(
                    "ClusterDirectory",
                    "validated directory member range is unavailable",
                )
            })?;
        for &cell_id in members {
            let Some(slot) = cell_to_cluster.get_mut(cell_id as usize) else {
                return Err(section_validation(
                    "ClusterDirectory",
                    "validated directory member exceeds runtime cell count",
                ));
            };
            *slot =
                Some(u32::try_from(cluster_id).map_err(|_| {
                    section_validation("ClusterDirectory", "cluster id exceeds u32")
                })?);
        }
    }
    let mut adjacency = vec![BTreeSet::new(); directory.clusters.len()];
    for portal in &portals.portals {
        let front = cell_to_cluster
            .get(portal.front_leaf as usize)
            .and_then(|cluster| *cluster)
            .ok_or_else(|| {
                section_validation(
                    "ClusterDirectory",
                    "validated portal front cell has no cluster membership",
                )
            })?;
        let back = cell_to_cluster
            .get(portal.back_leaf as usize)
            .and_then(|cluster| *cluster)
            .ok_or_else(|| {
                section_validation(
                    "ClusterDirectory",
                    "validated portal back cell has no cluster membership",
                )
            })?;
        if front != back {
            adjacency[front as usize].insert(back);
            adjacency[back as usize].insert(front);
        }
    }
    Ok(adjacency
        .into_iter()
        .map(|neighbors| neighbors.into_iter().collect())
        .collect())
}

fn validate_streamed_direct_delta_selection(
    metadata: &crate::sh_stream::ShStreamSparseMetadata,
    selected_light_count: usize,
) -> Result<(), PrlLoadError> {
    let mut seen = vec![false; selected_light_count];
    for &selection in &metadata.affinity_lights {
        let selection = usize::try_from(selection).map_err(|_| {
            section_validation("DirectShDeltaVolumes", "selection index exceeds usize")
        })?;
        let Some(seen) = seen.get_mut(selection) else {
            return Err(section_validation(
                "DirectShDeltaVolumes",
                format!(
                    "affinity light selection {selection} out of range for {selected_light_count} selected light(s)"
                ),
            ));
        };
        *seen = true;
    }
    if let Some(missing) = seen.iter().position(|&has_delta| !has_delta) {
        return Err(section_validation(
            "DirectShDeltaVolumes",
            format!("missing usable delta entry for selected light index {missing}"),
        ));
    }
    Ok(())
}

fn base_projection(base: &crate::sh_stream::ShStreamBaseMetadata) -> OctahedralShVolumeSection {
    OctahedralShVolumeSection {
        grid_origin: base.grid_origin,
        cell_size: base.cell_size,
        grid_dimensions: base.grid_dimensions,
        probe_stride: base.probe_stride,
        tile_dimension: base.tile_dimension,
        tile_border: base.tile_border,
        atlas_dimensions: base.atlas_dimensions,
        layer_count: base.layer_count,
        tiles_per_layer: base.tiles_per_layer,
        atlas_tiles_per_row: base.atlas_tiles_per_row,
        probes: base.probes.clone(),
        irradiance_format: base.irradiance_format,
        // Directory semantic validation consumes only metadata; never create
        // a stand-in for the streamed compact atlas.
        compact_atlas: Vec::new(),
        animation_descriptors: base.animation_descriptors.clone(),
        slot_for_map_light: base.slot_for_map_light.clone(),
    }
}

fn direct_projection(direct: &crate::sh_stream::ShStreamDirectMetadata) -> DirectShVolumeSection {
    DirectShVolumeSection {
        grid_origin: direct.grid_origin,
        cell_size: direct.cell_size,
        grid_dimensions: direct.grid_dimensions,
        tile_dimension: direct.tile_dimension,
        tile_border: direct.tile_border,
        atlas_dimensions: direct.atlas_dimensions,
        layer_count: direct.layer_count,
        tiles_per_layer: direct.tiles_per_layer,
        atlas_tiles_per_row: direct.atlas_tiles_per_row,
        irradiance_format: direct.irradiance_format,
        atlas: Vec::new(),
    }
}

fn delta_projection(metadata: &crate::sh_stream::ShStreamSparseMetadata) -> DeltaShVolumesSection {
    DeltaShVolumesSection {
        affinity_factor: AFFINITY_FACTOR,
        affinity_dims: metadata.affinity_dims,
        tile_dimension: metadata.tile_dimension,
        tile_border: metadata.tile_border,
        animation_descriptor_indices: metadata.animation_descriptor_indices.clone(),
        valid_probe_masks: metadata.valid_probe_masks.clone(),
        cell_levels: metadata.cell_levels.clone(),
        affinity_offsets: metadata.affinity_offsets.clone(),
        affinity_lights: metadata.affinity_lights.clone(),
        delta_subblocks: Vec::new(),
    }
}

fn direct_delta_projection(
    metadata: &crate::sh_stream::ShStreamSparseMetadata,
) -> DirectShDeltaVolumesSection {
    DirectShDeltaVolumesSection {
        affinity_factor: AFFINITY_FACTOR,
        affinity_dims: metadata.affinity_dims,
        tile_dimension: metadata.tile_dimension,
        tile_border: metadata.tile_border,
        valid_probe_masks: metadata.valid_probe_masks.clone(),
        cell_levels: metadata.cell_levels.clone(),
        affinity_offsets: metadata.affinity_offsets.clone(),
        affinity_lights: metadata.affinity_lights.clone(),
        delta_subblocks: Vec::new(),
    }
}

fn animated_direct_delta_projection(
    metadata: &crate::sh_stream::ShStreamSparseMetadata,
) -> AnimatedDirectShDeltaVolumesSection {
    AnimatedDirectShDeltaVolumesSection {
        affinity_factor: AFFINITY_FACTOR,
        affinity_dims: metadata.affinity_dims,
        tile_dimension: metadata.tile_dimension,
        tile_border: metadata.tile_border,
        animation_descriptor_indices: metadata.animation_descriptor_indices.clone(),
        valid_probe_masks: metadata.valid_probe_masks.clone(),
        cell_levels: metadata.cell_levels.clone(),
        affinity_offsets: metadata.affinity_offsets.clone(),
        affinity_lights: metadata.affinity_lights.clone(),
        delta_subblocks: Vec::new(),
    }
}

struct StreamingDirectoryProjections {
    base: OctahedralShVolumeSection,
    direct: Option<DirectShVolumeSection>,
    delta: Option<DeltaShVolumesSection>,
    direct_delta: Option<DirectShDeltaVolumesSection>,
    animated_direct_delta: Option<AnimatedDirectShDeltaVolumesSection>,
}

fn directory_projections(manifest: &ShStreamManifest) -> StreamingDirectoryProjections {
    let sources = manifest.sources();
    StreamingDirectoryProjections {
        base: base_projection(manifest.base()),
        direct: sources.direct.as_ref().map(direct_projection),
        delta: sources.indirect_delta.as_ref().map(delta_projection),
        direct_delta: sources.direct_delta.as_ref().map(direct_delta_projection),
        animated_direct_delta: sources
            .animated_direct_delta
            .as_ref()
            .map(animated_direct_delta_projection),
    }
}

pub(crate) fn load_prl_from_container(
    container: PrlContainer,
    max_delta_section_binding_bytes: u64,
    max_scatter_section_bytes: u64,
    stream_manifest: Option<std::sync::Arc<ShStreamManifest>>,
) -> Result<LevelWorld, PrlLoadError> {
    let meta = container.metadata();
    let read_section = |section: SectionId| container.read_section(section as u32);
    let streaming = stream_manifest.as_deref();

    // Section 49 is optional, but when present its structural contract is
    // checked before any optional lighting fallback can affect diagnostics.
    let directory_entries: Vec<_> = meta
        .sections
        .iter()
        .filter(|entry| entry.section_id == SectionId::ClusterDirectory as u32)
        .collect();
    if directory_entries.len() > 1 {
        return Err(ClusterDirectoryError::InvalidData(format!(
            "duplicate section 49 entries ({})",
            directory_entries.len()
        ))
        .into());
    }
    let parsed_cluster_directory = match directory_entries.first() {
        Some(entry) => {
            if entry.version != CLUSTER_DIRECTORY_CONTAINER_VERSION {
                return Err(ClusterDirectoryError::VersionMismatch {
                    version: u32::from(entry.version),
                    expected: u32::from(CLUSTER_DIRECTORY_CONTAINER_VERSION),
                }
                .into());
            }
            let data = read_section(SectionId::ClusterDirectory)?.ok_or_else(|| {
                ClusterDirectoryError::InvalidData(
                    "section 49 table entry could not be read".into(),
                )
            })?;
            Some(ClusterDirectorySection::from_bytes(&data)?)
        }
        None => None,
    };

    let geom_data = read_section(SectionId::Geometry)?
        .ok_or_else(|| stale_section("Geometry", SectionId::Geometry))?;
    let geom = GeometrySection::from_bytes(&geom_data)
        .map_err(|err| section_validation_from_error("Geometry", err))?;

    let texture_names_data = read_section(SectionId::TextureNames)?;
    let texture_names_section = match texture_names_data {
        Some(data) => Some(TextureNamesSection::from_bytes(&data)?),
        None => None,
    };
    let texture_names: Vec<String> = texture_names_section.map(|s| s.names).unwrap_or_default();

    // Required. Absence means the file is corrupt or was produced by a writer
    // that omitted section 32; reject so the texture cache never silently
    // degrades every surface to a placeholder on a bad file.
    let texture_cache_keys_data =
        read_section(SectionId::TextureCacheKeys)?.ok_or(PrlLoadError::NoTextureCacheKeys)?;
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
    let bvh_data =
        read_section(SectionId::Bvh)?.ok_or_else(|| stale_section("Bvh", SectionId::Bvh))?;
    let bvh_section = BvhSection::from_bytes(&bvh_data)
        .map_err(|err| section_validation_from_error("Bvh", err))?;
    let bvh = convert_bvh_section(bvh_section.clone());
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

    let has_legacy_bsp_nodes = container.has_section(SectionId::BspNodes as u32);
    let has_legacy_bsp_leaves = container.has_section(SectionId::BspLeaves as u32);

    let portals_section = match read_section(SectionId::Portals)? {
        Some(data) => match PortalsSection::from_bytes(&data) {
            Ok(section) => Some(section),
            Err(err) => {
                log::warn!("[PRL] Portals section malformed ({err}); using no-portals fallback");
                None
            }
        },
        None => None,
    };

    let cells_section = match read_section(SectionId::Cells)? {
        Some(data) => CellsSection::from_bytes(&data)
            .map_err(|err| section_validation_from_error("Cells", err))?,
        None => return Err(stale_section("Cells", SectionId::Cells)),
    };
    let cell_count = cells_section.cells.len();
    let portal_ref_count = cells_section.portal_refs.len();
    let (cells, cell_portal_refs) = convert_cells_section(cells_section.clone());
    validate_face_meta_cells(&face_meta, &cells)?;
    validate_cells_against_geometry(&cells, &face_meta)?;
    validate_bvh_leaf_cells(&bvh.leaves, &cells)?;
    log::info!(
        "[PRL] Cells: {} cells, {} portal refs loaded",
        cell_count,
        portal_ref_count,
    );

    // Optional — its absence is the conservative compatibility path for maps
    // compiled before CellVisibility was introduced. A present malformed
    // section remains a format error rather than silently weakening a new map.
    let expected_cell_count = u32::try_from(cells.len()).map_err(|_| {
        section_validation(
            "CellVisibility",
            "Cells count exceeds the CellVisibility u32 cell-id limit",
        )
    })?;
    if let Some(entry) = meta.find_section(SectionId::CellVisibility as u32) {
        let max_size = CellVisibilitySection::max_encoded_len(expected_cell_count)
            .map_err(|err| section_validation_from_error("CellVisibility", err))?;
        if entry.size > max_size {
            return Err(section_validation(
                "CellVisibility",
                format!(
                    "section size {} exceeds {expected_cell_count} cells' bounded maximum {max_size}",
                    entry.size
                ),
            ));
        }
    }
    let cell_visibility = match read_section(SectionId::CellVisibility)? {
        Some(data) => {
            let section = CellVisibilitySection::from_bytes(&data, expected_cell_count)
                .map_err(|err| section_validation_from_error("CellVisibility", err))?;
            log::info!(
                "[PRL] CellVisibility: {} cells, {} coupled pair(s)",
                section.cell_count,
                section.coupled_pairs.len(),
            );
            Some(convert_cell_visibility_section(section))
        }
        None => {
            log::info!(
                "[PRL] CellVisibility section missing — using conservative all-perceivable fallback"
            );
            None
        }
    };

    let (cell_locator_section, cell_locator_root, cell_locator_nodes) =
        match read_section(SectionId::CellLocator)? {
            Some(data) => {
                let section = CellLocatorSection::from_bytes(&data, cells.len() as u32)
                    .map_err(|err| section_validation_from_error("CellLocator", err))?;
                let node_count = section.nodes.len();
                let converted = convert_cell_locator_section(section.clone());
                log::info!("[PRL] CellLocator: {node_count} node(s) loaded");
                (section, converted.0, converted.1)
            }
            None => return Err(stale_section("CellLocator", SectionId::CellLocator)),
        };

    // Optional — older maps fall back to empty with a warning.
    let mut lights: Vec<MapLight> = match read_section(SectionId::AlphaLights)? {
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
    if let Some(data) = read_section(SectionId::LightTags)? {
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
    let light_influences: Vec<LightInfluence> = match read_section(SectionId::LightInfluence)? {
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

    let sh_volume: Option<OctahedralShVolumeSection> = match streaming {
        // The manifest retains this metadata; legacy body fields are reserved
        // for a whole-resident legacy load.
        Some(_) => None,
        None => match read_section(SectionId::OctahedralShVolume)? {
            Some(data) => {
                let section = OctahedralShVolumeSection::from_bytes(&data)?;
                log::info!(
                    "[PRL] OctahedralShVolume: {}×{}×{} grid ({} probes, {}×{} atlas, {} atlas layer(s), tile {} + border {}, {} tile(s)/row, {} tile(s)/layer, {} animated descriptor(s))",
                    section.grid_dimensions[0],
                    section.grid_dimensions[1],
                    section.grid_dimensions[2],
                    section.probes.len(),
                    section.atlas_dimensions[0],
                    section.atlas_dimensions[1],
                    section.layer_count,
                    section.tile_dimension,
                    section.tile_border,
                    section.atlas_tiles_per_row,
                    section.tiles_per_layer,
                    section.animation_descriptors.len(),
                );
                Some(section)
            }
            None => return Err(PrlLoadError::NoOctahedralShVolume),
        },
    };

    // Populate `MapLight.animated_slot` from the SH-volume slot table.
    // Resolution happens once here (load time), not per
    // `setLightAnimation` call. Legacy PRLs lack the table — every slot stays
    // `None` and the bridge takes the legacy `is_dynamic`-gated path.
    let sh_slot_for_map_light = streaming.map_or_else(
        || {
            sh_volume
                .as_ref()
                .map_or(&[][..], |section| section.slot_for_map_light.as_slice())
        },
        |manifest| manifest.base().slot_for_map_light.as_slice(),
    );
    if !sh_slot_for_map_light.is_empty() {
        use postretro_level_format::sh_volume::ANIMATED_SLOT_NONE;
        if sh_slot_for_map_light.len() != lights.len() {
            log::warn!(
                "[PRL] OctahedralShVolume slot_for_map_light count ({}) != AlphaLights count ({}); skipping animated-slot resolution",
                sh_slot_for_map_light.len(),
                lights.len(),
            );
        } else {
            let mut resolved = 0usize;
            for (light, &slot) in lights.iter_mut().zip(sh_slot_for_map_light) {
                if slot != ANIMATED_SLOT_NONE {
                    light.animated_slot = Some(slot);
                    resolved += 1;
                }
            }
            log::info!("[PRL] Resolved {resolved} map-light → animated-slot mapping(s)");
        }
    }

    // Optional — absent → 1×1 white placeholder; bumped-Lambert degrades to flat white.
    let lightmap: Option<LightmapSection> = match read_section(SectionId::Lightmap)? {
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

    let mut shadowmask_atlas: Option<ShadowmaskAtlasSection> = match read_section(
        SectionId::ShadowmaskAtlas,
    )? {
        Some(data) => match ShadowmaskAtlasSection::from_bytes(&data) {
            Ok(section) => match lightmap.as_ref() {
                Some(lm) => {
                    if section.width == lm.irr_width
                        && section.height == lm.irr_height
                        && section.layer_count == lm.layer_count
                    {
                        log::info!(
                            "[PRL] ShadowmaskAtlas: {}x{} atlas, {} layer(s), {} selected channel entr(y/ies), {} payload byte(s)",
                            section.width,
                            section.height,
                            section.layer_count,
                            section.channels.len(),
                            section.data.len(),
                        );
                        Some(section)
                    } else {
                        log::warn!(
                            "[PRL] ShadowmaskAtlas dimensions do not match Lightmap irradiance atlas; ignoring section"
                        );
                        None
                    }
                }
                None => {
                    log::warn!(
                        "[PRL] ShadowmaskAtlas present without Lightmap; ignoring SectionId 42"
                    );
                    None
                }
            },
            Err(err) => {
                log::warn!("[PRL] ShadowmaskAtlas malformed; ignoring section: {err}");
                None
            }
        },
        None => None,
    };

    // Optional — absent → no static-occluder SDF; runtime shadow pass disabled.
    // An empty-geometry section (zero grid dims) is also a valid "no SDF"
    // marker; the renderer collapses it to the same disabled state.
    let sdf_atlas: Option<SdfAtlasSection> = match read_section(SectionId::SdfAtlas)? {
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
    let chunk_light_list: Option<ChunkLightListSection> =
        match read_section(SectionId::ChunkLightList)? {
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
        match read_section(SectionId::AnimatedLightChunks)? {
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
    let animated_light_weight_maps: Option<AnimatedLightWeightMapsSection> = match read_section(
        SectionId::AnimatedLightWeightMaps,
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
    let delta_sh_volumes: Option<DeltaShVolumesSection> = if streaming.is_some() {
        // Id 27's body is cluster-streamed. Manifest construction has retained
        // and cross-validated the CSR projection but intentionally no tiles.
        None
    } else {
        match read_bounded_delta_section_data_with_limit(
            container.data(),
            meta,
            SectionId::DeltaShVolumes,
            "DeltaShVolumes",
            max_delta_section_binding_bytes,
        )? {
            BoundedDeltaSectionData::Data(data) => {
                let section = DeltaShVolumesSection::from_bytes(data)?;

                // Validation (mirrors the section-version reject path): a mismatched
                // bake must fail the load with a clear error rather than feed the
                // compose pass garbage. `sh_volume` (id 34) was loaded above.
                validate_delta_sh(&section, sh_volume.as_ref())?;
                validate_storage_ceiling_for_delta(
                    "DeltaShVolumes",
                    sh_volume.as_ref().expect("id-34 is required before id-27"),
                    &section.cell_levels,
                    &section.affinity_offsets,
                )?;

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
            BoundedDeltaSectionData::Absent | BoundedDeltaSectionData::OverBindingFloor => None,
        }
    };

    // Optional — malformed animated-direct deltas disable only this additive
    // term. An id-45/id-34 valid-probe descriptor disagreement is different:
    // it would make compact payload offsets address the wrong tiles, so reject
    // the complete load before any renderer buffers are built.
    let parsed_animated_direct_sh_delta_volumes: Option<AnimatedDirectShDeltaVolumesSection> =
        if streaming.is_some() {
            // Id 45 is cluster-streamed. Id 48 validates against the retained
            // metadata projection below.
            None
        } else {
            match read_bounded_delta_section_data_with_limit(
                container.data(),
                meta,
                SectionId::AnimatedDirectShDeltaVolumes,
                "AnimatedDirectShDeltaVolumes",
                max_delta_section_binding_bytes,
            )? {
                BoundedDeltaSectionData::Data(data) => {
                    match AnimatedDirectShDeltaVolumesSection::from_bytes(data) {
                        Ok(section) => {
                            let base = sh_volume.as_ref().expect("id-34 is required before id-45");
                            if delta_grid_matches_base(
                                base,
                                section.affinity_factor,
                                section.affinity_dims,
                            ) {
                                validate_storage_ceiling_for_delta(
                                    "AnimatedDirectShDeltaVolumes",
                                    base,
                                    &section.cell_levels,
                                    &section.affinity_offsets,
                                )?;
                            }
                            match validate_animated_direct_sh_delta(&section, sh_volume.as_ref()) {
                                Ok(()) => {
                                    log::info!(
                                        "[PRL] AnimatedDirectShDeltaVolumes: {} animated light(s), affinity grid {}×{}×{} ({} CSR entr(y/ies), {} delta subblock halves)",
                                        section.animation_descriptor_indices.len(),
                                        section.affinity_dims[0],
                                        section.affinity_dims[1],
                                        section.affinity_dims[2],
                                        section.affinity_lights.len(),
                                        section.delta_subblocks.len(),
                                    );
                                    Some(section)
                                }
                                Err(
                                    err @ PrlLoadError::AnimatedDirectShDeltaValidityMismatch {
                                        ..
                                    },
                                ) => {
                                    return Err(err);
                                }
                                Err(err) => {
                                    log::warn!(
                                        "[PRL] AnimatedDirectShDeltaVolumes unusable; disabling animated direct SH: {err}"
                                    );
                                    None
                                }
                            }
                        }
                        Err(err) => {
                            log::warn!(
                                "[PRL] AnimatedDirectShDeltaVolumes malformed; disabling animated direct SH: {err}"
                            );
                            None
                        }
                    }
                }
                BoundedDeltaSectionData::Absent | BoundedDeltaSectionData::OverBindingFloor => None,
            }
        };

    // Optional normal-free direct scatter for billboard lighting. This is an
    // optimization with a legacy direct-light fallback, so all parse and
    // cross-validation failures deliberately clear it instead of rejecting a
    // map that otherwise loads.
    let parsed_billboard_direct_scatter_volume: Option<BillboardDirectScatterVolumeSection> =
        if let Some(manifest) = streaming {
            match read_section(SectionId::BillboardDirectScatterVolume)? {
                Some(data) => match BillboardDirectScatterVolumeSection::from_bytes(&data) {
                    Ok(section) => match validate_billboard_direct_scatter_against_metadata(
                        &section,
                        manifest.base(),
                    ) {
                        Ok(()) => Some(section),
                        Err(error) => {
                            log::warn!(
                                "[PRL] BillboardDirectScatterVolume unusable; disabling billboard direct scatter: {error}"
                            );
                            None
                        }
                    },
                    Err(error) => {
                        log::warn!(
                            "[PRL] BillboardDirectScatterVolume malformed; disabling billboard direct scatter: {error}"
                        );
                        None
                    }
                },
                None => None,
            }
        } else {
            match read_soft_optional_scatter_section_data(
                container.data(),
                meta,
                SectionId::BillboardDirectScatterVolume,
                "BillboardDirectScatterVolume",
            ) {
                Some(data) => match BillboardDirectScatterVolumeSection::from_bytes(data) {
                    Ok(section) => {
                        match validate_billboard_direct_scatter_volume(&section, sh_volume.as_ref())
                        {
                            Ok(()) => {
                                log::info!(
                                    "[PRL] BillboardDirectScatterVolume: {}×{}×{} grid ({} RGBA16F scatter halves)",
                                    section.grid_dimensions[0],
                                    section.grid_dimensions[1],
                                    section.grid_dimensions[2],
                                    section.scatter_rgba.len(),
                                );
                                Some(section)
                            }
                            Err(error) => {
                                log::warn!(
                                    "[PRL] BillboardDirectScatterVolume unusable; disabling billboard direct scatter: {error}"
                                );
                                None
                            }
                        }
                    }
                    Err(error) => {
                        log::warn!(
                            "[PRL] BillboardDirectScatterVolume malformed; disabling billboard direct scatter: {error}"
                        );
                        None
                    }
                },
                None => None,
            }
        };

    let id45_present = container.has_section(SectionId::AnimatedDirectShDeltaVolumes as u32);
    let id48_present =
        container.has_section(SectionId::AnimatedBillboardDirectScatterDeltaVolumes as u32);
    let parsed_animated_billboard_direct_scatter_delta_volumes: Option<
        AnimatedBillboardDirectScatterDeltaVolumesSection,
    > = if let Some(manifest) = streaming {
        match manifest.sources().animated_direct_delta.as_ref() {
            None => None,
            Some(animated_direct) => {
                if let Some(entry) =
                    meta.find_section(SectionId::AnimatedBillboardDirectScatterDeltaVolumes as u32)
                    && entry.size > max_scatter_section_bytes
                {
                    log::warn!(
                        "[PRL] AnimatedBillboardDirectScatterDeltaVolumes raw payload is {} B, above the {} B pack cap; disabling billboard direct scatter",
                        entry.size,
                        max_scatter_section_bytes,
                    );
                    None
                } else {
                    match read_section(SectionId::AnimatedBillboardDirectScatterDeltaVolumes)? {
                        Some(data) => match AnimatedBillboardDirectScatterDeltaVolumesSection::from_bytes(&data) {
                            Ok(section) => match validate_animated_billboard_direct_scatter_against_metadata(
                                &section,
                                animated_direct,
                            ) {
                                Ok(()) => Some(section),
                                Err(error) => {
                                    log::warn!(
                                        "[PRL] AnimatedBillboardDirectScatterDeltaVolumes unusable; disabling billboard direct scatter: {error}"
                                    );
                                    None
                                }
                            },
                            Err(error) => {
                                log::warn!(
                                    "[PRL] AnimatedBillboardDirectScatterDeltaVolumes malformed; disabling billboard direct scatter: {error}"
                                );
                                None
                            }
                        },
                        None => None,
                    }
                }
            }
        }
    } else {
        match parsed_animated_direct_sh_delta_volumes.as_ref() {
            Some(animated_direct) => match read_bounded_scatter_section_data_with_limit(
                container.data(),
                meta,
                max_scatter_section_bytes,
            ) {
                BoundedScatterSectionData::Data(data) => {
                    match AnimatedBillboardDirectScatterDeltaVolumesSection::from_bytes(data) {
                        Ok(section) => {
                            match validate_animated_billboard_direct_scatter_delta_volumes(
                                &section,
                                animated_direct,
                            ) {
                                Ok(()) => Some(section),
                                Err(error) => {
                                    log::warn!(
                                        "[PRL] AnimatedBillboardDirectScatterDeltaVolumes unusable; disabling billboard direct scatter: {error}"
                                    );
                                    None
                                }
                            }
                        }
                        Err(error) => {
                            log::warn!(
                                "[PRL] AnimatedBillboardDirectScatterDeltaVolumes malformed; disabling billboard direct scatter: {error}"
                            );
                            None
                        }
                    }
                }
                BoundedScatterSectionData::Absent | BoundedScatterSectionData::OverPackCap => None,
            },
            None => None,
        }
    };

    // The id-45 layout is authoritative whenever present. A static-only map
    // needs only id 47; an animated map needs both the usable id-45 layout and
    // its id-48 dense companion. Id 48 without id 45 is likewise an invalid
    // pair. Keep both fields absent so downstream code cannot accidentally mix
    // scatter with the legacy animated-direct path.
    let scatter_pair_is_usable = match (id45_present, id48_present) {
        (false, false) => true,
        (true, true) => parsed_animated_billboard_direct_scatter_delta_volumes.is_some(),
        (true, false) => false,
        (false, true) => false,
    };
    if parsed_billboard_direct_scatter_volume.is_some() && !scatter_pair_is_usable {
        let reason = match (id45_present, id48_present) {
            (true, false) => {
                "AnimatedDirectShDeltaVolumes (id 45) is present without AnimatedBillboardDirectScatterDeltaVolumes (id 48)"
            }
            (false, true) => {
                "AnimatedBillboardDirectScatterDeltaVolumes (id 48) is present without AnimatedDirectShDeltaVolumes (id 45)"
            }
            (true, true) => "the id-45/id-48 animated scatter pair is unusable",
            (false, false) => "the static-only scatter pair is unexpectedly unusable",
        };
        log::warn!("[PRL] {reason}; disabling billboard direct scatter");
    }

    // Optional — absent when the map has no static direct SH/static lights.
    // Dynamic objects fall back to indirect-only.
    let direct_sh_volume: Option<DirectShVolumeSection> = match streaming {
        Some(_) => None,
        None => match read_section(SectionId::DirectShVolume)? {
            Some(data) => {
                // id-35 is a stored-set sibling of required id-34. A present stale,
                // malformed, geometry-mismatched, or length-mismatched payload has
                // no safe downgrade: it would address a different stored slot map.
                let section = DirectShVolumeSection::from_bytes(&data)
                    .map_err(|error| section_validation_from_error("DirectShVolume", error))?;
                let base = sh_volume.as_ref().expect("id-34 is required before id-35");
                validate_direct_sh_layout(&section, base)?;
                log::info!(
                    "[PRL] DirectShVolume: {}×{}×{} grid ({} probes, {}×{} stored atlas, {} atlas layer(s), tile {} + border {}, {} tile(s)/row, {} tile(s)/layer, format {}, {} atlas byte(s))",
                    section.grid_dimensions[0],
                    section.grid_dimensions[1],
                    section.grid_dimensions[2],
                    section.total_probes(),
                    section.atlas_dimensions[0],
                    section.atlas_dimensions[1],
                    section.layer_count,
                    section.tile_dimension,
                    section.tile_border,
                    section.atlas_tiles_per_row,
                    section.tiles_per_layer,
                    section.irradiance_format,
                    section.atlas.len(),
                );
                Some(section)
            }
            None => None,
        },
    };

    let has_direct_sh = streaming.map_or(direct_sh_volume.is_some(), |manifest| {
        manifest.sources().direct.is_some()
    });
    let parsed_entity_shadow_lights: Option<EntityShadowLightsSection> = match read_section(
        SectionId::EntityShadowLights,
    )? {
        // A corrupt/tampered EntityShadowLights section degrades to empty (warn +
        // clear), mirroring the sibling DirectShDeltaVolumes path below — presence
        // without a usable selection means "no promotion", not a bricked load.
        // Older PRLs (absent section) already load fine via the `None` arm. The
        // outer `?` still propagates a structurally broken section table.
        Some(data) => match EntityShadowLightsSection::from_bytes(&data) {
            Ok(section) => {
                if !has_direct_sh {
                    if !section.light_indices.is_empty() {
                        log::warn!(
                            "[PRL] EntityShadowLights present without DirectShVolume; ignoring {} selected light(s)",
                            section.light_indices.len()
                        );
                    }
                    None
                } else if let Err(err) =
                    validate_entity_shadow_light_selection(&section.light_indices, &lights)
                {
                    log::warn!(
                        "[PRL] EntityShadowLights invalid selection; clearing {} selected static light(s): {err}",
                        section.light_indices.len()
                    );
                    None
                } else {
                    log::info!(
                        "[PRL] EntityShadowLights: {} selected static light(s)",
                        section.light_indices.len()
                    );
                    Some(section)
                }
            }
            Err(err) => {
                log::warn!(
                    "[PRL] EntityShadowLights malformed; treating as empty (no promotion): {err}"
                );
                None
            }
        },
        None => None,
    };
    let mut entity_shadow_lights = parsed_entity_shadow_lights
        .as_ref()
        .map_or_else(Vec::new, |section| section.light_indices.clone());

    let direct_sh_delta_volumes: Option<DirectShDeltaVolumesSection> = if let Some(manifest) =
        streaming
    {
        match manifest.sources().direct_delta.as_ref() {
            Some(metadata) if !entity_shadow_lights.is_empty() => {
                if let Err(error) =
                    validate_streamed_direct_delta_selection(metadata, entity_shadow_lights.len())
                {
                    log::warn!(
                        "[PRL] streamed DirectShDeltaVolumes metadata unusable for EntityShadowLights; clearing {} selected static light(s): {error}",
                        entity_shadow_lights.len(),
                    );
                    entity_shadow_lights.clear();
                }
                None
            }
            Some(_) => None,
            None => {
                if !entity_shadow_lights.is_empty() {
                    log::warn!(
                        "[PRL] EntityShadowLights present without streamed DirectShDeltaVolumes; clearing {} selected static light(s)",
                        entity_shadow_lights.len(),
                    );
                    entity_shadow_lights.clear();
                }
                None
            }
        }
    } else {
        match read_bounded_delta_section_data_with_limit(
            container.data(),
            meta,
            SectionId::DirectShDeltaVolumes,
            "DirectShDeltaVolumes",
            max_delta_section_binding_bytes,
        )? {
            BoundedDeltaSectionData::Data(data) => {
                match DirectShDeltaVolumesSection::from_bytes(data) {
                    Ok(section) => {
                        let base = sh_volume.as_ref().expect("id-34 is required before id-41");
                        if delta_grid_matches_base(
                            base,
                            section.affinity_factor,
                            section.affinity_dims,
                        ) {
                            validate_storage_ceiling_for_delta(
                                "DirectShDeltaVolumes",
                                base,
                                &section.cell_levels,
                                &section.affinity_offsets,
                            )?;
                        }
                        if entity_shadow_lights.is_empty() {
                            log::warn!(
                                "[PRL] DirectShDeltaVolumes present without selected EntityShadowLights; ignoring section"
                            );
                            None
                        } else {
                            if let (Some(direct), Some(base)) =
                                (direct_sh_volume.as_ref(), sh_volume.as_ref())
                            {
                                match validate_direct_sh_delta(
                                    &section,
                                    direct,
                                    base,
                                    entity_shadow_lights.len(),
                                ) {
                                    Ok(()) => {
                                        log::info!(
                                            "[PRL] DirectShDeltaVolumes: {} selected-light CSR entr(y/ies), affinity grid {}×{}×{} ({} delta subblock halves)",
                                            section.affinity_lights.len(),
                                            section.affinity_dims[0],
                                            section.affinity_dims[1],
                                            section.affinity_dims[2],
                                            section.delta_subblocks.len(),
                                        );
                                        Some(section)
                                    }
                                    Err(err) => {
                                        log::warn!(
                                            "[PRL] DirectShDeltaVolumes unusable for EntityShadowLights; clearing {} selected static light(s): {err}",
                                            entity_shadow_lights.len()
                                        );
                                        entity_shadow_lights.clear();
                                        None
                                    }
                                }
                            } else {
                                log::warn!(
                                    "[PRL] DirectShDeltaVolumes present without DirectShVolume or OctahedralShVolume; clearing {} selected static light(s)",
                                    entity_shadow_lights.len()
                                );
                                entity_shadow_lights.clear();
                                None
                            }
                        }
                    }
                    Err(err) => {
                        log::warn!(
                            "[PRL] DirectShDeltaVolumes malformed; clearing {} selected static light(s): {err}",
                            entity_shadow_lights.len()
                        );
                        entity_shadow_lights.clear();
                        None
                    }
                }
            }
            BoundedDeltaSectionData::OverBindingFloor => {
                if !entity_shadow_lights.is_empty() {
                    log::warn!(
                        "[PRL] DirectShDeltaVolumes exceeded the storage-binding floor; clearing {} selected static light(s)",
                        entity_shadow_lights.len()
                    );
                    entity_shadow_lights.clear();
                }
                None
            }
            BoundedDeltaSectionData::Absent => {
                if !entity_shadow_lights.is_empty() {
                    log::warn!(
                        "[PRL] EntityShadowLights present without DirectShDeltaVolumes; clearing {} selected static light(s)",
                        entity_shadow_lights.len()
                    );
                    entity_shadow_lights.clear();
                }
                None
            }
        }
    };

    if let Some(section) = shadowmask_atlas.as_ref() {
        if entity_shadow_lights.is_empty() {
            log::warn!(
                "[PRL] ShadowmaskAtlas present without usable EntityShadowLights; ignoring section"
            );
            shadowmask_atlas = None;
        } else if section.channels.len() != entity_shadow_lights.len() {
            log::warn!(
                "[PRL] ShadowmaskAtlas channel table has {} entr(y/ies), but EntityShadowLights has {}; ignoring section",
                section.channels.len(),
                entity_shadow_lights.len(),
            );
            shadowmask_atlas = None;
        }
    }

    // Optional — absent when map has no `data_script` worldspawn KVP.
    let data_script: Option<DataScriptSection> = match read_section(SectionId::DataScript)? {
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
    let map_entities: Vec<MapEntityRecord> = match read_section(SectionId::MapEntity)? {
        Some(data) => {
            let section = MapEntitySection::from_bytes(&data)?;
            log::info!("[PRL] MapEntity: {} entities", section.entries.len());
            section.entries
        }
        None => Vec::new(),
    };

    // Optional — absent or empty means the level has no kinematic movers.
    let mut kinematic_geometry: KinematicGeometry =
        match read_section(SectionId::KinematicGeometry)? {
            Some(data) => {
                let section = KinematicGeometrySection::from_bytes(&data)
                    .map_err(|err| section_validation_from_error("KinematicGeometry", err))?;
                convert_kinematic_geometry_section(section)?
            }
            None => KinematicGeometry::default(),
        };
    drop_invalid_carried_light_links(&mut kinematic_geometry, &lights);
    let kinematic_vertex_count: usize = kinematic_geometry
        .movers
        .iter()
        .map(|mover| mover.vertices.len())
        .sum();
    let kinematic_index_count: usize = kinematic_geometry
        .movers
        .iter()
        .map(|mover| mover.indices.len())
        .sum();
    log::info!(
        "[PRL] KinematicGeometry: {} movers, {} waypoints, {} vertices, {} indices",
        kinematic_geometry.movers.len(),
        kinematic_geometry.waypoints.len(),
        kinematic_vertex_count,
        kinematic_index_count,
    );
    // Optional — absent or empty means no trigger volumes.
    let trigger_volumes = match read_section(SectionId::TriggerVolumes)? {
        Some(data) => {
            TriggerVolumesSection::from_bytes(&data)
                .map_err(|err| section_validation_from_error("TriggerVolumes", err))?
                .triggers
        }
        None => Vec::new(),
    };
    let touch_trigger_count = trigger_volumes
        .iter()
        .filter(|trigger| trigger.activation == 0)
        .count();
    let use_trigger_count = trigger_volumes
        .iter()
        .filter(|trigger| trigger.activation == 1)
        .count();
    let start_command_count = trigger_volumes
        .iter()
        .filter(|trigger| trigger.command == 0)
        .count();
    let stop_command_count = trigger_volumes
        .iter()
        .filter(|trigger| trigger.command == 1)
        .count();
    let reverse_command_count = trigger_volumes
        .iter()
        .filter(|trigger| trigger.command == 2)
        .count();
    let go_to_path_node_command_count = trigger_volumes
        .iter()
        .filter(|trigger| trigger.command == 3)
        .count();
    log::info!(
        "[PRL] TriggerVolumes: {} trigger(s): {touch_trigger_count} touch, {use_trigger_count} use; commands: {start_command_count} start, {stop_command_count} stop, {reverse_command_count} reverse, {go_to_path_node_command_count} go_to_path_node",
        trigger_volumes.len(),
    );

    // Required — carries `initial_gravity` alongside fog volumes. Absence = pre-gravity PRL;
    // rejected so the engine never silently falls back to a hardcoded default.
    let (fog_volumes, fog_pixel_scale, initial_gravity): (Vec<FogVolumeRecord>, u32, f32) =
        match read_section(SectionId::FogVolumes)? {
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
    let fog_cell_masks: Option<Vec<u32>> = match read_section(SectionId::FogCellMasks)? {
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
    let navmesh: Option<NavMeshSection> = match read_section(SectionId::NavMesh)? {
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
    let cell_draw_index_data = read_section(SectionId::CellDrawIndex)?;

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

    let on_wire = |section: SectionId| container.has_section(section as u32);
    let streaming_directory_projections = streaming.map(directory_projections);
    let mut unavailable_directory_companions = Vec::new();
    for (section, available) in [
        (
            SectionId::DeltaShVolumes,
            streaming_directory_projections
                .as_ref()
                .map_or(delta_sh_volumes.is_some(), |projection| {
                    projection.delta.is_some()
                }),
        ),
        (
            SectionId::DirectShVolume,
            streaming_directory_projections
                .as_ref()
                .map_or(direct_sh_volume.is_some(), |projection| {
                    projection.direct.is_some()
                }),
        ),
        (
            SectionId::DirectShDeltaVolumes,
            streaming_directory_projections
                .as_ref()
                .map_or(direct_sh_delta_volumes.is_some(), |projection| {
                    projection.direct_delta.is_some()
                }),
        ),
        (
            SectionId::AnimatedDirectShDeltaVolumes,
            streaming_directory_projections.as_ref().map_or(
                parsed_animated_direct_sh_delta_volumes.is_some(),
                |projection| projection.animated_direct_delta.is_some(),
            ),
        ),
        (
            SectionId::BillboardDirectScatterVolume,
            parsed_billboard_direct_scatter_volume.is_some() && scatter_pair_is_usable,
        ),
        (
            SectionId::AnimatedBillboardDirectScatterDeltaVolumes,
            parsed_animated_billboard_direct_scatter_delta_volumes.is_some()
                && scatter_pair_is_usable,
        ),
    ] {
        if on_wire(section) && !available {
            unavailable_directory_companions.push(section as u32);
        }
    }

    let cluster_directory = if let Some(manifest) = streaming {
        if !unavailable_directory_companions.is_empty() {
            // A streaming session has to retain a complete, semantically
            // validated id-49 topology for the residency planner. Legacy maps
            // may soft-disable a malformed optional scatter companion, but
            // silently returning a manifest without its adjacency graph would
            // be unsafe; require the caller to use legacy/off mode instead.
            return Err(section_validation(
                "SH streaming",
                format!(
                    "cannot start streaming because ClusterDirectory companion section(s) {:?} are unavailable; retry with POSTRETRO_SH_STREAMING=off",
                    unavailable_directory_companions,
                ),
            ));
        }
        let projections = streaming_directory_projections
            .as_ref()
            .expect("streaming manifest always has directory projections");
        let empty_portals = PortalsSection {
            vertices: Vec::new(),
            portals: Vec::new(),
        };
        let directory = manifest.cluster_directory();
        directory.validate_semantics(ClusterDirectoryValidationInputs {
            cells: &cells_section,
            portals: portals_section.as_ref().unwrap_or(&empty_portals),
            bvh: &bvh_section,
            cell_locator: &cell_locator_section,
            sh: ClusterDirectoryShInventory {
                octahedral: Some(&projections.base),
                direct: projections.direct.as_ref(),
                delta: projections.delta.as_ref(),
                shadow_selection: parsed_entity_shadow_lights.as_ref(),
                direct_delta: projections.direct_delta.as_ref(),
                animated_direct_delta: projections.animated_direct_delta.as_ref(),
                billboard: parsed_billboard_direct_scatter_volume.as_ref(),
                animated_billboard_delta: parsed_animated_billboard_direct_scatter_delta_volumes
                    .as_ref(),
            },
        })?;
        manifest.install_cluster_adjacency(cluster_adjacency_from_portals(
            directory,
            portals_section.as_ref().unwrap_or(&empty_portals),
        )?)?;
        Some(directory.clone())
    } else {
        match parsed_cluster_directory {
            None => None,
            Some(_directory) if !unavailable_directory_companions.is_empty() => {
                log::warn!(
                    "[PRL] ClusterDirectoryUnavailableCompanion: section(s) {:?}; validated structural metadata remains inert and unavailable",
                    unavailable_directory_companions,
                );
                None
            }
            Some(directory) => {
                let empty_portals = PortalsSection {
                    vertices: Vec::new(),
                    portals: Vec::new(),
                };
                directory.validate_semantics(ClusterDirectoryValidationInputs {
                    cells: &cells_section,
                    portals: portals_section.as_ref().unwrap_or(&empty_portals),
                    bvh: &bvh_section,
                    cell_locator: &cell_locator_section,
                    sh: ClusterDirectoryShInventory {
                        octahedral: sh_volume.as_ref(),
                        direct: direct_sh_volume.as_ref(),
                        delta: delta_sh_volumes.as_ref(),
                        shadow_selection: parsed_entity_shadow_lights.as_ref(),
                        direct_delta: direct_sh_delta_volumes.as_ref(),
                        animated_direct_delta: parsed_animated_direct_sh_delta_volumes.as_ref(),
                        billboard: parsed_billboard_direct_scatter_volume.as_ref(),
                        animated_billboard_delta:
                            parsed_animated_billboard_direct_scatter_delta_volumes.as_ref(),
                    },
                })?;
                Some(directory)
            }
        }
    };

    // These legacy runtime fields are derived only after directory validation,
    // so valid-empty id 45/id 48 evidence remains available to the validator.
    let animated_direct_sh_delta_volumes = parsed_animated_direct_sh_delta_volumes
        .filter(|section| !section.affinity_lights.is_empty());
    let billboard_direct_scatter_volume = scatter_pair_is_usable
        .then_some(parsed_billboard_direct_scatter_volume)
        .flatten();
    let animated_billboard_direct_scatter_delta_volumes =
        if billboard_direct_scatter_volume.is_some() {
            parsed_animated_billboard_direct_scatter_delta_volumes
        } else {
            None
        };

    let portal_data = portals_section.as_ref().and_then(convert_usable_portals);
    if let Some(portal_data) = portal_data.as_ref() {
        let dropped_count =
            drop_out_of_range_sealed_portal_ids(&mut kinematic_geometry, portal_data.len());
        if dropped_count > 0 {
            log::warn!(
                "[PRL] KinematicGeometry: dropped {dropped_count} sealed portal id(s) outside the {} loaded portals",
                portal_data.len(),
            );
        }
    }
    if portal_data.is_none() {
        validate_cell_portal_refs(&cells, &cell_portal_refs, None)?;
    }
    let (portals, has_portals) = if let Some(portal_data) = portal_data {
        validate_portal_adjacency(&cells, &cell_portal_refs, &portal_data)?;
        (portal_data, true)
    } else {
        (Vec::new(), false)
    };

    let lighting = LoadedLighting {
        lights,
        light_influences,
        sh_volume,
        lightmap,
        lightmap_mode: LightmapMode::default(),
        sdf_atlas,
        chunk_light_list,
        animated_light_chunks,
        animated_light_weight_maps,
        delta_sh_volumes,
        direct_sh_volume,
        direct_sh_delta_volumes,
        animated_direct_sh_delta_volumes,
        billboard_direct_scatter_volume,
        animated_billboard_direct_scatter_delta_volumes,
        entity_shadow_lights,
        shadowmask_atlas,
        cluster_directory,
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
        cell_visibility,
        texture_names,
        texture_cache_keys,
        bvh,
        lights: lighting.lights,
        light_influences: lighting.light_influences,
        sh_volume: lighting.sh_volume,
        sh_storage: match stream_manifest {
            Some(manifest) => ShStorage::Streaming(manifest),
            None => ShStorage::Legacy,
        },
        lightmap: lighting.lightmap,
        // Current bakes load as Shadowed. Unshadowed remains for legacy PRL
        // wire compatibility; new lightmaps should carry baked visibility.
        lightmap_mode: lighting.lightmap_mode,
        sdf_atlas: lighting.sdf_atlas,
        chunk_light_list: lighting.chunk_light_list,
        animated_light_chunks: lighting.animated_light_chunks,
        animated_light_weight_maps: lighting.animated_light_weight_maps,
        delta_sh_volumes: lighting.delta_sh_volumes,
        direct_sh_volume: lighting.direct_sh_volume,
        direct_sh_delta_volumes: lighting.direct_sh_delta_volumes,
        animated_direct_sh_delta_volumes: lighting.animated_direct_sh_delta_volumes,
        billboard_direct_scatter_volume: lighting.billboard_direct_scatter_volume,
        animated_billboard_direct_scatter_delta_volumes: lighting
            .animated_billboard_direct_scatter_delta_volumes,
        entity_shadow_lights: lighting.entity_shadow_lights,
        shadowmask_atlas: lighting.shadowmask_atlas,
        data_script,
        map_entities,
        kinematic_geometry,
        trigger_volumes,
        fog_volumes,
        fog_pixel_scale,
        initial_gravity,
        fog_cell_masks,
        navmesh,
        cell_draw_index,
        cluster_directory: lighting.cluster_directory,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use log::Level;
    use postretro_level_format::alpha_lights::AlphaLightRecord;
    use postretro_level_format::cell_locator::CellLocatorChild as FormatCellLocatorChild;
    use postretro_level_format::cell_visibility::CoupledPairRecord;
    use postretro_level_format::cells::CellRecord;
    use postretro_level_format::cluster_directory::{
        ClusterRecord, ClusterResourceDomain, ClusterResourceRecord,
    };
    use postretro_level_format::geometry::{FaceMeta as PrlFaceMeta, Vertex as PrlVertex};
    use postretro_level_format::kinematic_geometry::{
        KINEMATIC_GEOMETRY_VERSION, KINEMATIC_GEOMETRY_VERSION_V4, KINEMATIC_GEOMETRY_VERSION_V5,
        KinematicMoverRecord, KinematicWaypointRecord, MemberLight,
    };
    use postretro_level_format::lightmap::IRRADIANCE_FORMAT_RGBA16F;
    use postretro_test_log_capture::LogCapture;
    use std::io::Cursor;

    #[test]
    fn cell_visibility_section_round_trips_through_runtime_lowering() {
        let encoded = CellVisibilitySection {
            cell_count: 3,
            component_ids: vec![0, 0, 1],
            coupled_pairs: vec![CoupledPairRecord {
                cell_a: 0,
                cell_b: 1,
                distance: 128,
                aperture: 64,
            }],
        }
        .to_bytes()
        .unwrap();
        let parsed = CellVisibilitySection::from_bytes(&encoded, 3).unwrap();
        let visibility = convert_cell_visibility_section(parsed);

        assert_eq!(visibility.component_ids(), &[0, 0, 1]);
        assert_eq!(
            visibility.coupled_pairs().copied().collect::<Vec<_>>(),
            vec![CoupledCellPair {
                cell_a: 0,
                cell_b: 1,
                distance: 128,
                aperture: 64,
            }]
        );

        let component_only = convert_cell_visibility_section(CellVisibilitySection {
            cell_count: 3,
            component_ids: vec![0, 0, 1],
            coupled_pairs: vec![],
        });
        assert_eq!(component_only.component_ids().len(), 3);
        assert_eq!(component_only.coupled_pairs().count(), 0);
    }

    fn write_prl_load_fixture(
        additional_sections: impl IntoIterator<Item = prl_format::SectionBlob>,
        name: &str,
    ) -> std::path::PathBuf {
        let mut sections = vec![
            prl_format::SectionBlob {
                section_id: SectionId::Geometry as u32,
                version: 1,
                data: GeometrySection {
                    vertices: Vec::new(),
                    indices: Vec::new(),
                    faces: Vec::new(),
                }
                .to_bytes(),
            },
            prl_format::SectionBlob {
                section_id: SectionId::Bvh as u32,
                version: 1,
                data: BvhSection {
                    nodes: Vec::new(),
                    leaves: Vec::new(),
                    root_node_index: 0,
                }
                .to_bytes(),
            },
            prl_format::SectionBlob {
                section_id: SectionId::Cells as u32,
                version: 1,
                data: CellsSection {
                    cells: vec![
                        CellRecord {
                            bounds_min: [0.0, 0.0, 0.0],
                            bounds_max: [1.0, 1.0, 1.0],
                            flags: 0,
                            face_start: 0,
                            face_count: 0,
                            portal_ref_start: 0,
                            portal_ref_count: 0,
                        },
                        CellRecord {
                            bounds_min: [2.0, 0.0, 0.0],
                            bounds_max: [3.0, 1.0, 1.0],
                            flags: 0,
                            face_start: 0,
                            face_count: 0,
                            portal_ref_start: 0,
                            portal_ref_count: 0,
                        },
                    ],
                    portal_refs: Vec::new(),
                }
                .to_bytes(),
            },
            prl_format::SectionBlob {
                section_id: SectionId::CellLocator as u32,
                version: 1,
                data: CellLocatorSection {
                    root: FormatCellLocatorChild::Node(0),
                    nodes: vec![
                        postretro_level_format::cell_locator::CellLocatorNodeRecord {
                            plane_normal: [1.0, 0.0, 0.0],
                            plane_distance: 1.5,
                            front: FormatCellLocatorChild::Cell(0),
                            back: FormatCellLocatorChild::Cell(1),
                        },
                    ],
                }
                .to_bytes(),
            },
            prl_format::SectionBlob {
                section_id: SectionId::OctahedralShVolume as u32,
                version: 1,
                data: OctahedralShVolumeSection::placeholder().to_bytes(),
            },
            prl_format::SectionBlob {
                section_id: SectionId::TextureCacheKeys as u32,
                version: 1,
                data: TextureCacheKeysSection::default().to_bytes(),
            },
            prl_format::SectionBlob {
                section_id: SectionId::FogVolumes as u32,
                version: 1,
                data: FogVolumesSection::default().to_bytes(),
            },
        ];
        sections.extend(additional_sections);

        let path = std::env::temp_dir().join(name);
        let mut file = std::fs::File::create(&path).unwrap();
        prl_format::write_prl(&mut file, &sections).unwrap();
        path
    }

    #[test]
    fn id50_without_id49_is_rejected_before_off_mode_can_select_legacy_loading() {
        let path = write_prl_load_fixture(
            [prl_format::SectionBlob {
                section_id: SectionId::ClusterShPayloads as u32,
                version:
                    postretro_level_format::cluster_sh_payloads::CLUSTER_SH_PAYLOADS_CONTAINER_VERSION,
                // The id-50/id-49 pair is mandatory before the injected Off
                // mode can select retained-handle legacy loading. The
                // end-to-end streaming fixture separately covers malformed
                // id-50 bytes with a valid id-49 directory.
                data: vec![0],
            }],
            "postretro_test_invalid_id50_before_off.prl",
        );
        let error =
            load_prl_with_streaming_mode_for_test(path.to_str().unwrap(), ShStreamingMode::Off)
                .unwrap_err();
        assert!(matches!(error, PrlLoadError::ClusterShPayloads(_)));
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn legacy_off_mode_soft_disables_an_out_of_bounds_optional_scatter_entry() {
        let path = write_prl_load_fixture(
            [prl_format::SectionBlob {
                section_id: SectionId::BillboardDirectScatterVolume as u32,
                version: 1,
                data: vec![0],
            }],
            "postretro_test_legacy_optional_scatter_out_of_bounds.prl",
        );
        let mut bytes = std::fs::read(&path).unwrap();
        let section_count = u16::from_le_bytes([bytes[6], bytes[7]]) as usize;
        let table_entry = (0..section_count)
            .map(|index| 8 + index * 22)
            .find(|&offset| {
                u32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap())
                    == SectionId::BillboardDirectScatterVolume as u32
            })
            .expect("fixture contains id 47");
        let out_of_bounds = u64::try_from(bytes.len()).unwrap() + 1;
        bytes[table_entry + 4..table_entry + 12].copy_from_slice(&out_of_bounds.to_le_bytes());
        std::fs::write(&path, bytes).unwrap();

        let loaded =
            load_prl_with_streaming_mode_for_test(path.to_str().unwrap(), ShStreamingMode::Off)
                .expect("legacy/off loading soft-disables malformed optional scatter");
        assert!(loaded.billboard_direct_scatter_volume().is_none());
        let _ = std::fs::remove_file(path);
    }

    fn write_cell_visibility_load_fixture(
        section: Option<prl_format::SectionBlob>,
        name: &str,
    ) -> std::path::PathBuf {
        write_prl_load_fixture(section, name)
    }

    fn zero_grid_cluster_directory(resource_ids: &[SectionId]) -> ClusterDirectorySection {
        ClusterDirectorySection {
            runtime_cell_count: 2,
            primitive_limit: 64,
            cell_limit: 64,
            clusters: vec![
                ClusterRecord {
                    bounds_min: [0.0, 0.0, 0.0],
                    bounds_max: [1.0, 1.0, 1.0],
                    member_start: 0,
                    member_count: 1,
                    range_start: 0,
                    range_count: 0,
                    primitive_count: 0,
                    flags: 0,
                },
                ClusterRecord {
                    bounds_min: [2.0, 0.0, 0.0],
                    bounds_max: [3.0, 1.0, 1.0],
                    member_start: 1,
                    member_count: 1,
                    range_start: 0,
                    range_count: 0,
                    primitive_count: 0,
                    flags: 0,
                },
            ],
            resources: resource_ids
                .iter()
                .map(|&section| ClusterResourceRecord {
                    section_id: section as u32,
                    domain: match section {
                        SectionId::OctahedralShVolume
                        | SectionId::DirectShVolume
                        | SectionId::BillboardDirectScatterVolume => {
                            ClusterResourceDomain::DenseProbe
                        }
                        _ => ClusterResourceDomain::AffinityCell,
                    },
                    dimensions: [0, 0, 0],
                })
                .collect(),
            members: vec![0, 1],
            ranges: Vec::new(),
        }
    }

    fn cluster_directory_blob(section: &ClusterDirectorySection) -> prl_format::SectionBlob {
        prl_format::SectionBlob {
            section_id: SectionId::ClusterDirectory as u32,
            version: CLUSTER_DIRECTORY_CONTAINER_VERSION,
            data: section.try_to_bytes().unwrap(),
        }
    }

    #[test]
    fn cluster_directory_missing_is_silent_and_valid_zero_grid_directory_loads_inert() {
        let missing_path =
            write_prl_load_fixture([], "postretro_test_cluster_directory_missing.prl");
        let missing = load_prl(missing_path.to_str().unwrap()).unwrap();
        assert!(missing.cluster_directory.is_none());
        std::fs::remove_file(missing_path).unwrap();

        let directory = zero_grid_cluster_directory(&[SectionId::OctahedralShVolume]);
        let path = write_prl_load_fixture(
            [cluster_directory_blob(&directory)],
            "postretro_test_cluster_directory_zero_grid.prl",
        );
        let loaded = load_prl(path.to_str().unwrap()).unwrap();
        assert_eq!(loaded.cluster_directory.as_ref(), Some(&directory));
        assert!(loaded.sh_volume.is_some());
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn cluster_directory_rejects_duplicate_versions_corruption_and_absent_targets() {
        let directory = zero_grid_cluster_directory(&[SectionId::OctahedralShVolume]);
        let duplicate_path = write_prl_load_fixture(
            [
                cluster_directory_blob(&directory),
                cluster_directory_blob(&directory),
            ],
            "postretro_test_cluster_directory_duplicate.prl",
        );
        assert!(matches!(
            load_prl(duplicate_path.to_str().unwrap()),
            Err(PrlLoadError::ClusterDirectory(
                ClusterDirectoryError::InvalidData(_)
            ))
        ));
        std::fs::remove_file(duplicate_path).unwrap();

        let mut wrong_container = cluster_directory_blob(&directory);
        wrong_container.version += 1;
        let version_path = write_prl_load_fixture(
            [wrong_container],
            "postretro_test_cluster_directory_container_version.prl",
        );
        assert!(matches!(
            load_prl(version_path.to_str().unwrap()),
            Err(PrlLoadError::ClusterDirectory(
                ClusterDirectoryError::VersionMismatch { .. }
            ))
        ));
        std::fs::remove_file(version_path).unwrap();

        let corrupt_path = write_prl_load_fixture(
            [prl_format::SectionBlob {
                section_id: SectionId::ClusterDirectory as u32,
                version: CLUSTER_DIRECTORY_CONTAINER_VERSION,
                data: vec![0; 7],
            }],
            "postretro_test_cluster_directory_corrupt.prl",
        );
        assert!(matches!(
            load_prl(corrupt_path.to_str().unwrap()),
            Err(PrlLoadError::ClusterDirectory(
                ClusterDirectoryError::InvalidData(_)
            ))
        ));
        std::fs::remove_file(corrupt_path).unwrap();

        let absent_target = zero_grid_cluster_directory(&[
            SectionId::OctahedralShVolume,
            SectionId::DirectShVolume,
        ]);
        let absent_path = write_prl_load_fixture(
            [cluster_directory_blob(&absent_target)],
            "postretro_test_cluster_directory_absent_target.prl",
        );
        assert!(matches!(
            load_prl(absent_path.to_str().unwrap()),
            Err(PrlLoadError::ClusterDirectory(
                ClusterDirectoryError::MissingResource(_)
            ))
        ));
        std::fs::remove_file(absent_path).unwrap();
    }

    #[test]
    fn cluster_directory_becomes_unavailable_when_present_delta_exceeds_policy_floor() {
        let delta_data = empty_delta_sh_section_bytes();
        let binding_floor = u64::try_from(delta_data.len() - 1).unwrap();
        let directory = zero_grid_cluster_directory(&[
            SectionId::DeltaShVolumes,
            SectionId::OctahedralShVolume,
        ]);
        let path = write_prl_load_fixture(
            [
                prl_format::SectionBlob {
                    section_id: SectionId::DeltaShVolumes as u32,
                    version: 1,
                    data: delta_data,
                },
                cluster_directory_blob(&directory),
            ],
            "postretro_test_cluster_directory_policy_floor.prl",
        );
        let capture = LogCapture::start();
        let loaded = load_prl_with_delta_binding_limit(path.to_str().unwrap(), binding_floor)
            .expect("legacy over-floor degradation must remain a successful load");
        assert!(loaded.delta_sh_volumes.is_none());
        assert!(loaded.cluster_directory.is_none());
        capture.assert_logged_once(Level::Warn, "ClusterDirectoryUnavailableCompanion");
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn cluster_directory_validates_present_empty_id27_with_production_codec() {
        let directory = zero_grid_cluster_directory(&[
            SectionId::DeltaShVolumes,
            SectionId::OctahedralShVolume,
        ]);
        let path = write_prl_load_fixture(
            [
                prl_format::SectionBlob {
                    section_id: SectionId::DeltaShVolumes as u32,
                    version: 1,
                    data: empty_delta_sh_section_bytes(),
                },
                cluster_directory_blob(&directory),
            ],
            "postretro_test_cluster_directory_empty_id27.prl",
        );
        let loaded = load_prl(path.to_str().unwrap()).unwrap();
        assert!(loaded.delta_sh_volumes.is_some());
        assert_eq!(loaded.cluster_directory, Some(directory));
        std::fs::remove_file(path).unwrap();
    }

    fn mark_section_out_of_bounds_above_binding_floor(
        path: &std::path::Path,
        section_id: SectionId,
    ) {
        const PRL_HEADER_SIZE: usize = 8;
        const PRL_SECTION_ENTRY_SIZE: usize = 22;
        const SECTION_SIZE_OFFSET: usize = 12;

        let mut file_data = std::fs::read(path).unwrap();
        let mut cursor = std::io::Cursor::new(&file_data);
        let meta = prl_format::read_container(&mut cursor).unwrap();
        let section_index = meta
            .sections
            .iter()
            .position(|entry| entry.section_id == section_id as u32)
            .expect("fixture must contain the section it makes oversized");
        let size_start =
            PRL_HEADER_SIZE + section_index * PRL_SECTION_ENTRY_SIZE + SECTION_SIZE_OFFSET;
        file_data[size_start..size_start + std::mem::size_of::<u64>()]
            .copy_from_slice(&(MAX_DELTA_SECTION_BINDING_BYTES + 1).to_le_bytes());
        std::fs::write(path, file_data).unwrap();
    }

    fn static_alpha_lights_blob() -> prl_format::SectionBlob {
        prl_format::SectionBlob {
            section_id: SectionId::AlphaLights as u32,
            version: 1,
            data: AlphaLightsSection {
                lights: vec![AlphaLightRecord {
                    origin: [0.0, 0.0, 0.0],
                    light_type: AlphaLightType::Point,
                    intensity: 1.0,
                    color: [1.0, 1.0, 1.0],
                    falloff_model: AlphaFalloffModel::Linear,
                    falloff_range: 8.0,
                    cone_angle_inner: 0.0,
                    cone_angle_outer: 0.0,
                    cone_direction: [0.0, 0.0, 0.0],
                    is_dynamic: false,
                    casts_entity_shadows: false,
                    leaf_index: ALPHA_LIGHT_LEAF_UNASSIGNED,
                    shadow_type: AlphaShadowType::StaticLightMap,
                }],
            }
            .to_bytes(),
        }
    }

    fn empty_delta_sh_section_bytes() -> Vec<u8> {
        let base = OctahedralShVolumeSection::placeholder();
        let affinity_dims = base
            .grid_dimensions
            .map(|dimension| dimension.div_ceil(AFFINITY_FACTOR as u32));
        let cell_count = affinity_dims.iter().product::<u32>() as usize;
        DeltaShVolumesSection {
            affinity_factor: AFFINITY_FACTOR,
            affinity_dims,
            tile_dimension: base.tile_dimension,
            tile_border: base.tile_border,
            animation_descriptor_indices: Vec::new(),
            valid_probe_masks: (0..cell_count)
                .map(|cell| valid_probe_mask_for_affinity_cell(&base, affinity_dims, cell))
                .collect(),
            cell_levels: vec![0; cell_count],
            affinity_offsets: vec![0; cell_count + 1],
            affinity_lights: Vec::new(),
            delta_subblocks: Vec::new(),
        }
        .to_bytes()
    }

    fn empty_animated_direct_sh_delta_section_bytes() -> Vec<u8> {
        let base = OctahedralShVolumeSection::placeholder();
        let affinity_dims = base
            .grid_dimensions
            .map(|dimension| dimension.div_ceil(AFFINITY_FACTOR as u32));
        let cell_count = affinity_dims.iter().product::<u32>() as usize;
        AnimatedDirectShDeltaVolumesSection {
            affinity_factor: AFFINITY_FACTOR,
            affinity_dims,
            tile_dimension: base.tile_dimension,
            tile_border: base.tile_border,
            animation_descriptor_indices: Vec::new(),
            valid_probe_masks: (0..cell_count)
                .map(|cell| valid_probe_mask_for_affinity_cell(&base, affinity_dims, cell))
                .collect(),
            cell_levels: vec![0; cell_count],
            affinity_offsets: vec![0; cell_count + 1],
            affinity_lights: Vec::new(),
            delta_subblocks: Vec::new(),
        }
        .to_bytes()
    }

    fn empty_direct_sh_delta_section_bytes() -> Vec<u8> {
        let base = OctahedralShVolumeSection::placeholder();
        let affinity_dims = base
            .grid_dimensions
            .map(|dimension| dimension.div_ceil(AFFINITY_FACTOR as u32));
        let cell_count = affinity_dims.iter().product::<u32>() as usize;
        DirectShDeltaVolumesSection {
            affinity_factor: AFFINITY_FACTOR,
            affinity_dims,
            tile_dimension: base.tile_dimension,
            tile_border: base.tile_border,
            valid_probe_masks: (0..cell_count)
                .map(|cell| valid_probe_mask_for_affinity_cell(&base, affinity_dims, cell))
                .collect(),
            cell_levels: vec![0; cell_count],
            affinity_offsets: vec![0; cell_count + 1],
            affinity_lights: Vec::new(),
            delta_subblocks: Vec::new(),
        }
        .to_bytes()
    }

    // Regression: oversized malformed metadata bypassed container-bounds validation.
    #[test]
    fn binding_floor_validates_each_delta_section_container_before_degrading() {
        for (section_id, name) in [
            (SectionId::DeltaShVolumes, "DeltaShVolumes"),
            (SectionId::DirectShDeltaVolumes, "DirectShDeltaVolumes"),
            (
                SectionId::AnimatedDirectShDeltaVolumes,
                "AnimatedDirectShDeltaVolumes",
            ),
        ] {
            let meta = prl_format::ContainerMeta {
                header: prl_format::Header {
                    version: prl_format::CURRENT_VERSION,
                    section_count: 1,
                },
                sections: vec![prl_format::SectionEntry {
                    section_id: section_id as u32,
                    offset: u64::MAX,
                    size: MAX_DELTA_SECTION_BINDING_BYTES + 1,
                    version: 1,
                }],
            };

            let result = read_bounded_delta_section_data(&[], &meta, section_id, name);
            assert!(
                matches!(
                    result,
                    Err(PrlLoadError::FormatError(
                        prl_format::FormatError::SectionOffsetOverflow { .. }
                    ))
                ),
                "{name} must reject invalid container bounds before applying the binding floor"
            );
        }
    }

    // Regression: valid over-floor payloads must retain the established degradation path.
    #[test]
    fn binding_floor_degrades_each_valid_oversized_delta_section_after_borrowing() {
        for (section_id, name) in [
            (SectionId::DeltaShVolumes, "DeltaShVolumes"),
            (SectionId::DirectShDeltaVolumes, "DirectShDeltaVolumes"),
            (
                SectionId::AnimatedDirectShDeltaVolumes,
                "AnimatedDirectShDeltaVolumes",
            ),
        ] {
            let mut file_data = Vec::new();
            prl_format::write_prl(
                &mut file_data,
                &[prl_format::SectionBlob {
                    section_id: section_id as u32,
                    version: 1,
                    data: vec![0_u8; 2],
                }],
            )
            .expect("fixture container should serialize");
            let mut cursor = Cursor::new(&file_data);
            let meta = prl_format::read_container(&mut cursor)
                .expect("fixture container metadata should parse");

            let outcome =
                read_bounded_delta_section_data_with_limit(&file_data, &meta, section_id, name, 1)
                    .expect("valid container bounds must reach the binding-floor policy");
            assert!(matches!(outcome, BoundedDeltaSectionData::OverBindingFloor));
        }
    }

    #[test]
    fn load_prl_degrades_valid_over_floor_id27_and_id45_independently() {
        for (section_id, name, data) in [
            (
                SectionId::DeltaShVolumes,
                "DeltaShVolumes",
                empty_delta_sh_section_bytes(),
            ),
            (
                SectionId::AnimatedDirectShDeltaVolumes,
                "AnimatedDirectShDeltaVolumes",
                empty_animated_direct_sh_delta_section_bytes(),
            ),
        ] {
            let binding_floor = u64::try_from(data.len() - 1)
                .expect("fixture must fit the test-only binding-floor type");
            let path = write_prl_load_fixture(
                [prl_format::SectionBlob {
                    section_id: section_id as u32,
                    version: 1,
                    data,
                }],
                &format!("postretro_test_{name}_binding_floor_degrade.prl"),
            );

            let world = load_prl_with_delta_binding_limit(path.to_str().unwrap(), binding_floor)
                .expect("a valid over-floor optional delta section must degrade, not fail loading");
            match section_id {
                SectionId::DeltaShVolumes => assert!(world.delta_sh_volumes.is_none()),
                SectionId::AnimatedDirectShDeltaVolumes => {
                    assert!(world.animated_direct_sh_delta_volumes.is_none())
                }
                _ => unreachable!("table contains only independently degradable delta sections"),
            }
            std::fs::remove_file(path).ok();
        }
    }

    #[test]
    fn load_prl_over_floor_id41_clears_paired_entity_shadow_selection() {
        let direct_delta = empty_direct_sh_delta_section_bytes();
        let binding_floor = u64::try_from(direct_delta.len() - 1)
            .expect("fixture must fit the test-only binding-floor type");
        let path = write_prl_load_fixture(
            [
                static_alpha_lights_blob(),
                prl_format::SectionBlob {
                    section_id: SectionId::DirectShVolume as u32,
                    version: 1,
                    data: DirectShVolumeSection::placeholder().to_bytes(),
                },
                prl_format::SectionBlob {
                    section_id: SectionId::EntityShadowLights as u32,
                    version: 1,
                    data: EntityShadowLightsSection {
                        light_indices: vec![0],
                    }
                    .to_bytes(),
                },
                prl_format::SectionBlob {
                    section_id: SectionId::DirectShDeltaVolumes as u32,
                    version: 1,
                    data: direct_delta,
                },
            ],
            "postretro_test_direct_sh_binding_floor_pair_clear.prl",
        );

        let world = load_prl_with_delta_binding_limit(path.to_str().unwrap(), binding_floor)
            .expect("an over-floor id-41 section must degrade through the load path");
        assert!(world.direct_sh_delta_volumes.is_none());
        assert!(world.entity_shadow_lights.is_empty());
        std::fs::remove_file(path).ok();
    }

    #[test]
    fn out_of_bounds_id27_above_binding_floor_still_fails_load() {
        let oversized_path = write_cell_visibility_load_fixture(
            Some(prl_format::SectionBlob {
                section_id: SectionId::DeltaShVolumes as u32,
                version: 1,
                data: vec![0],
            }),
            "postretro_test_delta_sh_over_binding_floor.prl",
        );
        mark_section_out_of_bounds_above_binding_floor(&oversized_path, SectionId::DeltaShVolumes);

        let err = load_prl(oversized_path.to_str().unwrap()).unwrap_err();
        assert!(matches!(
            err,
            PrlLoadError::FormatError(prl_format::FormatError::SectionOutOfBounds { .. })
        ));
        std::fs::remove_file(&oversized_path).ok();
    }

    #[test]
    fn malformed_id27_with_valid_container_bounds_still_fails_load() {
        let malformed_path = write_cell_visibility_load_fixture(
            Some(prl_format::SectionBlob {
                section_id: SectionId::DeltaShVolumes as u32,
                version: 1,
                data: vec![0],
            }),
            "postretro_test_delta_sh_malformed.prl",
        );
        assert!(
            load_prl(malformed_path.to_str().unwrap()).is_err(),
            "id-27 malformed bytes retain the established hard-fail path"
        );
        std::fs::remove_file(malformed_path).ok();
    }

    #[test]
    fn out_of_bounds_id45_above_binding_floor_still_fails_load() {
        let path = write_cell_visibility_load_fixture(
            Some(prl_format::SectionBlob {
                section_id: SectionId::AnimatedDirectShDeltaVolumes as u32,
                version: 1,
                data: vec![0],
            }),
            "postretro_test_animated_direct_sh_over_binding_floor.prl",
        );
        mark_section_out_of_bounds_above_binding_floor(
            &path,
            SectionId::AnimatedDirectShDeltaVolumes,
        );

        let err = load_prl(path.to_str().unwrap()).unwrap_err();
        assert!(matches!(
            err,
            PrlLoadError::FormatError(prl_format::FormatError::SectionOutOfBounds { .. })
        ));
        std::fs::remove_file(path).ok();
    }

    #[test]
    fn out_of_bounds_id41_above_binding_floor_still_fails_load() {
        let path = write_prl_load_fixture(
            vec![
                static_alpha_lights_blob(),
                prl_format::SectionBlob {
                    section_id: SectionId::DirectShVolume as u32,
                    version: 1,
                    data: DirectShVolumeSection::placeholder().to_bytes(),
                },
                prl_format::SectionBlob {
                    section_id: SectionId::EntityShadowLights as u32,
                    version: 1,
                    data: EntityShadowLightsSection {
                        light_indices: vec![0],
                    }
                    .to_bytes(),
                },
                prl_format::SectionBlob {
                    section_id: SectionId::DirectShDeltaVolumes as u32,
                    version: 1,
                    data: vec![0],
                },
            ],
            "postretro_test_direct_sh_over_binding_floor.prl",
        );
        mark_section_out_of_bounds_above_binding_floor(&path, SectionId::DirectShDeltaVolumes);

        let err = load_prl(path.to_str().unwrap()).unwrap_err();
        assert!(matches!(
            err,
            PrlLoadError::FormatError(prl_format::FormatError::SectionOutOfBounds { .. })
        ));
        std::fs::remove_file(path).ok();
    }

    #[test]
    fn load_prl_lowers_cell_visibility_section_into_coupling() {
        let path = write_cell_visibility_load_fixture(
            Some(prl_format::SectionBlob {
                section_id: SectionId::CellVisibility as u32,
                version: 1,
                data: CellVisibilitySection {
                    cell_count: 2,
                    component_ids: vec![0, 0],
                    coupled_pairs: vec![CoupledPairRecord {
                        cell_a: 0,
                        cell_b: 1,
                        distance: 128,
                        aperture: 64,
                    }],
                }
                .to_bytes()
                .unwrap(),
            }),
            "postretro_test_cell_visibility_loaded.prl",
        );

        let world = load_prl(path.to_str().unwrap()).expect("valid CellVisibility must load");

        assert_eq!(
            world.coupling(0, 1),
            crate::prl::CouplingTuple {
                perceivable: true,
                distance: Some(128),
                aperture: Some(64),
            }
        );
        std::fs::remove_file(path).ok();
    }

    #[test]
    fn load_prl_missing_cell_visibility_uses_conservative_coupling_fallback() {
        let path =
            write_cell_visibility_load_fixture(None, "postretro_test_cell_visibility_absent.prl");

        let world = load_prl(path.to_str().unwrap()).expect("missing optional section must load");

        assert_eq!(
            world.coupling(0, 1),
            crate::prl::CouplingTuple {
                perceivable: true,
                distance: None,
                aperture: None,
            }
        );
        std::fs::remove_file(path).ok();
    }

    #[test]
    fn load_prl_rejects_malformed_cell_visibility_section() {
        let path = write_cell_visibility_load_fixture(
            Some(prl_format::SectionBlob {
                section_id: SectionId::CellVisibility as u32,
                version: 1,
                data: vec![0],
            }),
            "postretro_test_cell_visibility_malformed.prl",
        );

        let err = load_prl(path.to_str().unwrap()).unwrap_err();

        assert!(
            matches!(
                err,
                PrlLoadError::SectionValidation {
                    section: "CellVisibility",
                    ..
                }
            ),
            "got {err:?}"
        );
        std::fs::remove_file(path).ok();
    }

    #[test]
    fn load_prl_rejects_cell_visibility_section_larger_than_fanout_bound() {
        let max_size = CellVisibilitySection::max_encoded_len(2).unwrap() as usize;
        let path = write_cell_visibility_load_fixture(
            Some(prl_format::SectionBlob {
                section_id: SectionId::CellVisibility as u32,
                version: 1,
                data: vec![0; max_size + 1],
            }),
            "postretro_test_cell_visibility_oversized.prl",
        );

        let err = load_prl(path.to_str().unwrap()).unwrap_err();
        assert!(
            matches!(
                err,
                PrlLoadError::SectionValidation {
                    section: "CellVisibility",
                    ..
                }
            ),
            "got {err:?}"
        );
        std::fs::remove_file(path).ok();
    }

    fn matching_direct_and_base_sh() -> (DirectShVolumeSection, OctahedralShVolumeSection) {
        let mut direct = DirectShVolumeSection::placeholder();
        let mut base = OctahedralShVolumeSection::placeholder();

        direct.grid_origin = [1.0, 2.0, 3.0];
        base.grid_origin = direct.grid_origin;
        direct.cell_size = [0.5, 0.75, 1.25];
        base.cell_size = direct.cell_size;
        direct.grid_dimensions = [2, 3, 4];
        base.grid_dimensions = direct.grid_dimensions;
        direct.tile_dimension = 6;
        base.tile_dimension = direct.tile_dimension;
        direct.tile_border = 1;
        base.tile_border = direct.tile_border;
        direct.atlas_dimensions = [24, 18];
        base.atlas_dimensions = direct.atlas_dimensions;
        direct.atlas_tiles_per_row = 4;
        base.atlas_tiles_per_row = direct.atlas_tiles_per_row;
        direct.layer_count = 1;
        base.layer_count = direct.layer_count;
        direct.tiles_per_layer = 24;
        base.tiles_per_layer = direct.tiles_per_layer;

        (direct, base)
    }

    fn assert_direct_sh_layout_message(err: PrlLoadError, expected: &str) {
        match err {
            PrlLoadError::SectionValidation { section, message } => {
                assert_eq!(section, "DirectShVolume");
                assert!(
                    message.contains(expected),
                    "expected validation message to contain `{expected}`, got `{message}`"
                );
            }
            other => panic!("unexpected validation error: {other:?}"),
        }
    }

    fn static_light() -> MapLight {
        MapLight {
            origin: [0.0, 0.0, 0.0],
            light_type: LightType::Point,
            intensity: 1.0,
            color: [1.0, 1.0, 1.0],
            falloff_model: FalloffModel::Linear,
            falloff_range: 8.0,
            cone_angle_inner: 0.0,
            cone_angle_outer: 0.0,
            cone_direction: [0.0, 0.0, 0.0],
            is_dynamic: false,
            casts_entity_shadows: false,
            animated_slot: None,
            tags: Vec::new(),
            cell_index: ALPHA_LIGHT_LEAF_UNASSIGNED,
            shadow_type: ShadowType::StaticLightMap,
        }
    }

    fn dynamic_light() -> MapLight {
        MapLight {
            is_dynamic: true,
            ..static_light()
        }
    }

    fn sample_kinematic_vertex(position: [f32; 3]) -> PrlVertex {
        PrlVertex::new(
            position,
            [0.25, 0.5],
            [0.0, 1.0, 0.0],
            [1.0, 0.0, 0.0],
            true,
            [0.0, 0.0],
            0,
        )
    }

    fn sample_kinematic_section() -> KinematicGeometrySection {
        KinematicGeometrySection {
            version: KINEMATIC_GEOMETRY_VERSION,
            movers: vec![KinematicMoverRecord {
                mover_id: 7,
                name: "lift".to_string(),
                tags: vec!["platform".to_string()],
                origin: [1.0, 2.0, 3.0],
                path: "a".to_string(),
                speed: 2.0,
                wait_ms: 125.0,
                move_mode: 1,
                start_on_spawn: true,
                vertices: vec![
                    sample_kinematic_vertex([0.0, 0.0, 0.0]),
                    sample_kinematic_vertex([1.0, 0.0, 0.0]),
                    sample_kinematic_vertex([0.0, 1.0, 0.0]),
                ],
                indices: vec![0, 1, 2],
                face_meta: vec![PrlFaceMeta {
                    leaf_index: 0,
                    texture_index: 0,
                }],
                spin_axis: [0.0; 3],
                spin_speed_deg_s: 0.0,
                spin_accel_deg_s2: 0.0,
                carry_yaw: false,
                block_policy: "displace".to_string(),
                crush_damage: 0.0,
                crush_interval_ms: 0.0,
                auto_close_ms: None,
                open_event: None,
                close_event: None,
                blocked_event: None,
                crush_event: None,
                sealed_portal_ids: Vec::new(),
                carried_lights: Vec::new(),
            }],
            waypoints: vec![
                KinematicWaypointRecord {
                    name: "a".to_string(),
                    next: "b".to_string(),
                    origin: [1.0, 2.0, 3.0],
                },
                KinematicWaypointRecord {
                    name: "b".to_string(),
                    next: String::new(),
                    origin: [3.0, 2.0, 3.0],
                },
            ],
        }
    }

    fn assert_kinematic_validation_message(err: PrlLoadError, expected: &str) {
        match err {
            PrlLoadError::SectionValidation { section, message } => {
                assert_eq!(section, "KinematicGeometry");
                assert!(
                    message.contains(expected),
                    "expected validation message to contain `{expected}`, got `{message}`"
                );
            }
            other => panic!("unexpected validation error: {other:?}"),
        }
    }

    #[test]
    fn kinematic_geometry_rejects_duplicate_mover_ids() {
        let mut section = sample_kinematic_section();
        section.movers.push(section.movers[0].clone());

        let err = convert_kinematic_geometry_section(section).unwrap_err();

        assert_kinematic_validation_message(err, "duplicate mover_id");
    }

    #[test]
    fn kinematic_geometry_v4_loads_with_no_sealed_portals() {
        let mut section = sample_kinematic_section();
        section.version = KINEMATIC_GEOMETRY_VERSION_V4;

        let geometry = convert_kinematic_geometry_section(
            KinematicGeometrySection::from_bytes(&section.to_bytes())
                .expect("v4 section bytes must remain readable"),
        )
        .expect("v4 section must remain runtime-loadable");

        assert!(geometry.movers[0].sealed_portal_ids.is_empty());
        assert!(geometry.movers[0].carried_lights.is_empty());
    }

    #[test]
    fn kinematic_geometry_v5_loads_sealed_portals_with_no_member_lights() {
        let mut section = sample_kinematic_section();
        section.version = KINEMATIC_GEOMETRY_VERSION_V5;
        section.movers[0].sealed_portal_ids = vec![1];

        let geometry = convert_kinematic_geometry_section(
            KinematicGeometrySection::from_bytes(&section.to_bytes())
                .expect("v5 section bytes must remain readable"),
        )
        .expect("v5 section must remain runtime-loadable");

        assert_eq!(geometry.movers[0].sealed_portal_ids, vec![1]);
        assert!(geometry.movers[0].carried_lights.is_empty());
    }

    #[test]
    fn kinematic_geometry_drops_sealed_portals_outside_loaded_portal_array() {
        let mut geometry = convert_kinematic_geometry_section(sample_kinematic_section())
            .expect("sample section must lower");
        geometry.movers[0].sealed_portal_ids = vec![0, 2, u32::MAX];

        let dropped = drop_out_of_range_sealed_portal_ids(&mut geometry, 2);

        assert_eq!(dropped, 2);
        assert_eq!(geometry.movers[0].sealed_portal_ids, vec![0]);
    }

    #[test]
    fn kinematic_geometry_keeps_unique_dynamic_carried_light_link() {
        let mut section = sample_kinematic_section();
        section.movers[0].carried_lights = vec![MemberLight {
            alpha_light_index: 0,
            local_offset: [1.0, 2.0, 3.0],
        }];
        let mut geometry = convert_kinematic_geometry_section(section).unwrap();

        let dropped = drop_invalid_carried_light_links(&mut geometry, &[dynamic_light()]);

        assert_eq!(dropped, 0);
        assert_eq!(geometry.movers[0].carried_lights.len(), 1);
        assert_eq!(
            geometry.movers[0].carried_lights[0].local_offset,
            Vec3::new(1.0, 2.0, 3.0)
        );
    }

    // Regression: finite V6 mover and offset components could overflow when
    // composed and then propagate infinity to GPU-facing light buffers.
    #[test]
    fn kinematic_geometry_drops_carried_light_with_unrepresentable_authored_position() {
        let mut section = sample_kinematic_section();
        section.movers[0].origin = [3.0e38, 0.0, 0.0];
        section.movers[0].carried_lights = vec![MemberLight {
            alpha_light_index: 0,
            local_offset: [3.0e38, 0.0, 0.0],
        }];
        section.waypoints[0].origin = [3.0e38, 0.0, 0.0];
        section.waypoints[1].origin = [3.0e38, 1.0, 0.0];
        let decoded = KinematicGeometrySection::from_bytes(&section.to_bytes())
            .expect("finite wire components should decode");
        let mut geometry = convert_kinematic_geometry_section(decoded).unwrap();
        let capture = LogCapture::start();

        let dropped = drop_invalid_carried_light_links(&mut geometry, &[dynamic_light()]);

        assert_eq!(dropped, 1);
        assert!(geometry.movers[0].carried_lights.is_empty());
        capture.assert_logged_once(
            Level::Warn,
            "because its authored position is outside the runtime f32 range",
        );
    }

    #[test]
    fn kinematic_geometry_drops_carried_light_index_outside_alpha_lights() {
        let mut section = sample_kinematic_section();
        section.movers[0].carried_lights = vec![MemberLight {
            alpha_light_index: 1,
            local_offset: [1.0, 2.0, 3.0],
        }];
        let mut geometry = convert_kinematic_geometry_section(section).unwrap();

        let dropped = drop_invalid_carried_light_links(&mut geometry, &[dynamic_light()]);

        assert_eq!(dropped, 1);
        assert!(geometry.movers[0].carried_lights.is_empty());
    }

    #[test]
    fn kinematic_geometry_drops_carried_link_to_baked_alpha_light() {
        let mut section = sample_kinematic_section();
        section.movers[0].carried_lights = vec![MemberLight {
            alpha_light_index: 0,
            local_offset: [1.0, 2.0, 3.0],
        }];
        let mut geometry = convert_kinematic_geometry_section(section).unwrap();

        let dropped = drop_invalid_carried_light_links(&mut geometry, &[static_light()]);

        assert_eq!(dropped, 1);
        assert!(geometry.movers[0].carried_lights.is_empty());
    }

    #[test]
    fn kinematic_geometry_drops_every_duplicate_carried_light_reference() {
        let mut section = sample_kinematic_section();
        section.movers[0].carried_lights = vec![MemberLight {
            alpha_light_index: 0,
            local_offset: [1.0, 2.0, 3.0],
        }];
        let mut later_mover = section.movers[0].clone();
        later_mover.mover_id = 8;
        later_mover.name = "door".to_string();
        later_mover.carried_lights = vec![MemberLight {
            alpha_light_index: 0,
            local_offset: [4.0, 5.0, 6.0],
        }];
        section.movers.push(later_mover);
        let mut geometry = convert_kinematic_geometry_section(section).unwrap();

        let dropped = drop_invalid_carried_light_links(&mut geometry, &[dynamic_light()]);

        assert_eq!(dropped, 2);
        assert!(
            geometry
                .movers
                .iter()
                .all(|mover| mover.carried_lights.is_empty())
        );
    }

    #[test]
    fn kinematic_geometry_rejects_empty_mover_geometry() {
        let mut section = sample_kinematic_section();
        section.movers[0].vertices.clear();
        section.movers[0].indices.clear();

        let err = convert_kinematic_geometry_section(section).unwrap_err();

        assert_kinematic_validation_message(err, "geometry must contain vertices and indices");
    }

    #[test]
    fn kinematic_geometry_rejects_zero_length_waypoint_segments() {
        let mut section = sample_kinematic_section();
        section.waypoints[1].origin = section.waypoints[0].origin;

        let err = convert_kinematic_geometry_section(section).unwrap_err();

        assert_kinematic_validation_message(err, "zero-length segment");
    }

    #[test]
    fn kinematic_geometry_rejects_near_zero_waypoint_segments() {
        let mut section = sample_kinematic_section();
        section.waypoints[1].origin = [
            section.waypoints[0].origin[0] + KINEMATIC_WAYPOINT_MIN_SEGMENT_LENGTH * 0.5,
            section.waypoints[0].origin[1],
            section.waypoints[0].origin[2],
        ];

        let err = convert_kinematic_geometry_section(section).unwrap_err();

        assert_kinematic_validation_message(err, "zero-length segment");
    }

    #[test]
    fn kinematic_geometry_accepts_single_waypoint_pure_rotator() {
        let mut section = sample_kinematic_section();
        section.movers[0].spin_axis = [0.0, 1.0, 0.0];
        section.movers[0].spin_speed_deg_s = 90.0;
        section.waypoints.truncate(1);
        section.waypoints[0].next.clear();

        let geometry = convert_kinematic_geometry_section(section)
            .expect("a rotating mover can use a single waypoint");

        assert_eq!(geometry.movers.len(), 1);
        assert_eq!(geometry.waypoints.len(), 1);
    }

    #[test]
    fn kinematic_geometry_rejects_single_waypoint_nonzero_spin_with_zero_axis() {
        for spin_axis in [[0.0; 3], [f32::MIN_POSITIVE; 3]] {
            let mut section = sample_kinematic_section();
            section.movers[0].spin_axis = spin_axis;
            section.movers[0].spin_speed_deg_s = 90.0;
            section.waypoints.truncate(1);
            section.waypoints[0].next.clear();

            let err = convert_kinematic_geometry_section(section).unwrap_err();

            assert_kinematic_validation_message(err, "spin_axis normalizes to zero");
        }
    }

    // Regression: non-zero PRL degrees could authorize a pure rotator that is static in radians.
    #[test]
    fn kinematic_geometry_rejects_nonzero_spin_speed_that_underflows_in_radians() {
        let mut section = sample_kinematic_section();
        section.movers[0].spin_axis = [0.0, 1.0, 0.0];
        section.movers[0].spin_speed_deg_s = f32::from_bits(1);
        section.waypoints.truncate(1);
        section.waypoints[0].next.clear();

        let err = convert_kinematic_geometry_section(section).unwrap_err();

        assert_kinematic_validation_message(err, "conversion to radians/sec");
    }

    // Regression: positive PRL acceleration could become zero radians and turn a ramp into a snap.
    #[test]
    fn kinematic_geometry_rejects_positive_spin_accel_that_underflows_in_radians() {
        let mut section = sample_kinematic_section();
        section.movers[0].spin_accel_deg_s2 = f32::from_bits(1);

        let err = convert_kinematic_geometry_section(section).unwrap_err();

        assert_kinematic_validation_message(err, "conversion to radians/sec²");
    }

    #[test]
    fn kinematic_geometry_rejects_single_waypoint_without_spin() {
        let mut section = sample_kinematic_section();
        section.waypoints.truncate(1);
        section.waypoints[0].next.clear();

        let err = convert_kinematic_geometry_section(section).unwrap_err();

        assert_kinematic_validation_message(err, "at least 2 required");
    }

    #[test]
    fn kinematic_geometry_rejects_mover_origin_that_differs_from_first_waypoint() {
        let mut section = sample_kinematic_section();
        section.movers[0].origin[0] += KINEMATIC_WAYPOINT_MIN_SEGMENT_LENGTH * 2.0;

        let err = convert_kinematic_geometry_section(section).unwrap_err();

        assert_kinematic_validation_message(err, "must match first waypoint");
    }

    #[test]
    fn direct_sh_layout_rejects_grid_origin_mismatch() {
        let (mut direct, base) = matching_direct_and_base_sh();
        direct.grid_origin[0] += 1.0;

        let err = validate_direct_sh_layout(&direct, &base).unwrap_err();

        assert_direct_sh_layout_message(err, "grid_origin");
    }

    #[test]
    fn direct_sh_layout_rejects_cell_size_mismatch() {
        let (mut direct, base) = matching_direct_and_base_sh();
        direct.cell_size[2] += 0.25;

        let err = validate_direct_sh_layout(&direct, &base).unwrap_err();

        assert_direct_sh_layout_message(err, "cell_size");
    }

    #[test]
    fn direct_sh_layout_requires_matching_irradiance_format() {
        let (mut direct, base) = matching_direct_and_base_sh();

        validate_direct_sh_layout(&direct, &base)
            .expect("matching id-35 and id-34 stored geometry and format must be accepted");

        direct.irradiance_format = IRRADIANCE_FORMAT_RGBA16F;
        let err = validate_direct_sh_layout(&direct, &base).unwrap_err();

        assert_direct_sh_layout_message(err, "irradiance_format");
    }

    #[test]
    fn entity_shadow_selection_rejects_animated_static_light_for_degrade_path() {
        let mut light = static_light();
        light.animated_slot = Some(0);

        let err = validate_entity_shadow_light_selection(&[0], &[light]).unwrap_err();

        match err {
            PrlLoadError::SectionValidation { section, message } => {
                assert_eq!(section, "EntityShadowLights");
                assert!(
                    message.contains("animated static light"),
                    "unexpected validation message: {message}"
                );
            }
            other => panic!("unexpected validation error: {other:?}"),
        }
    }
}
