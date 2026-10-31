// Parse-end cross-section checks and the runtime rewrites that follow them.
// See: context/lib/build_pipeline.md §PRL Compilation

use postretro_level_format::alpha_lights::ALPHA_LIGHT_LEAF_UNASSIGNED;
use postretro_level_format::animated_billboard_direct_scatter_delta_volumes::AnimatedBillboardDirectScatterDeltaVolumesSection;
use postretro_level_format::animated_direct_sh_delta_volumes::AnimatedDirectShDeltaVolumesSection;
use postretro_level_format::billboard_direct_scatter_volume::BillboardDirectScatterVolumeSection;
use postretro_level_format::delta_sh_volumes::DeltaShVolumesSection;
use postretro_level_format::direct_sh_delta_volumes::DirectShDeltaVolumesSection;
use postretro_level_format::direct_sh_volume::DirectShVolumeSection;
use postretro_level_format::fog_volumes::MAX_FOG_VOLUMES;

use super::geometry::GeometryDecode;
use super::lighting_sh::ShCompanions;
use super::spatial::{SpatialSections, SpatialValidationInputs};
use super::{ambiguous_runtime_bsp_sections, section_validation};
use crate::{CellData, MapLight, PortalData, PrlLoadError};

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

/// Checks that need sections from more than one group. Returns the fog cell
/// masks in their validated form; they are final only after this check.
pub(super) fn validate_cross_sections(
    geometry: &GeometryDecode,
    spatial: &SpatialSections,
    validation_inputs: &SpatialValidationInputs,
    lights: &[MapLight],
    fog_volume_count: usize,
    fog_cell_masks: Option<Vec<u32>>,
) -> Result<Option<Vec<u32>>, PrlLoadError> {
    let GeometryDecode {
        vertices,
        indices,
        face_meta,
        ..
    } = geometry;
    let SpatialSections { bvh, cells, .. } = spatial;
    let has_legacy_bsp_nodes = validation_inputs.has_legacy_bsp_nodes;
    let has_legacy_bsp_leaves = validation_inputs.has_legacy_bsp_leaves;

    validate_light_cells(lights, cells)?;

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

    let fog_cell_masks = validate_fog_cell_masks(fog_cell_masks, cells.len(), fog_volume_count)?;

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
    Ok(fog_cell_masks)
}

/// Runtime SH companion fields, derived after cluster-directory validation.
pub(super) struct RuntimeShCompanions {
    pub(super) delta_sh_volumes: Option<DeltaShVolumesSection>,
    pub(super) direct_sh_volume: Option<DirectShVolumeSection>,
    pub(super) direct_sh_delta_volumes: Option<DirectShDeltaVolumesSection>,
    pub(super) animated_direct_sh_delta_volumes: Option<AnimatedDirectShDeltaVolumesSection>,
    pub(super) billboard_direct_scatter_volume: Option<BillboardDirectScatterVolumeSection>,
    pub(super) animated_billboard_direct_scatter_delta_volumes:
        Option<AnimatedBillboardDirectScatterDeltaVolumesSection>,
    pub(super) entity_shadow_lights: Vec<u32>,
}

pub(super) fn finalize_sh_companions(sh: ShCompanions) -> RuntimeShCompanions {
    let ShCompanions {
        delta_sh_volumes,
        parsed_animated_direct_sh_delta_volumes,
        parsed_billboard_direct_scatter_volume,
        parsed_animated_billboard_direct_scatter_delta_volumes,
        scatter_pair_is_usable,
        direct_sh_volume,
        parsed_entity_shadow_lights: _,
        entity_shadow_lights,
        direct_sh_delta_volumes,
    } = sh;

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

    RuntimeShCompanions {
        delta_sh_volumes,
        direct_sh_volume,
        direct_sh_delta_volumes,
        animated_direct_sh_delta_volumes,
        billboard_direct_scatter_volume,
        animated_billboard_direct_scatter_delta_volumes,
        entity_shadow_lights,
    }
}

/// Validates portal adjacency against the cells. Returns the runtime
/// portals and `has_portals`. Mover sealed-portal ids the usable array cannot
/// address were already dropped at entity decode.
pub(super) fn validate_portals(
    cells: &[CellData],
    cell_portal_refs: &[u32],
    portal_data: Option<Vec<PortalData>>,
) -> Result<(Vec<PortalData>, bool), PrlLoadError> {
    if portal_data.is_none() {
        validate_cell_portal_refs(cells, cell_portal_refs, None)?;
    }
    let (portals, has_portals) = if let Some(portal_data) = portal_data {
        validate_portal_adjacency(cells, cell_portal_refs, &portal_data)?;
        (portal_data, true)
    } else {
        (Vec::new(), false)
    };
    Ok((portals, has_portals))
}
