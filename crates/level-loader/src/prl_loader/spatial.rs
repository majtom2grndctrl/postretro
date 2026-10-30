// Spatial section group: BVH, portals, cells, visibility, residency set, locator.
// See: context/lib/build_pipeline.md §PRL Compilation

use glam::Vec3;
use postretro_level_format::SectionId;
use postretro_level_format::bvh::{BVH_NODE_FLAG_LEAF, BvhSection};
use postretro_level_format::cell_locator::CellLocatorSection;
use postretro_level_format::cell_residency_set::CellResidencySetSection;
use postretro_level_format::cell_visibility::CellVisibilitySection;
use postretro_level_format::cells::CellsSection;
use postretro_level_format::portals::PortalsSection;
use postretro_render_data::geometry::{BvhLeaf, BvhNode, BvhTree};

use super::geometry::GeometryDecode;
use super::{section_validation, section_validation_from_error, stale_section};
use crate::prl_container::PrlContainer;
use crate::{
    CellData, CellLocatorChild, CellLocatorNodeData, CellVisibility, CoupledCellPair, FaceMeta,
    PortalData, PrlLoadError,
};

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

/// Rejects invalid baked leaf ranges in every build; release indirect-call
/// validation is disabled, so draw safety depends on this and the install check.
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

pub(crate) fn convert_usable_portals(section: &PortalsSection) -> Option<Vec<PortalData>> {
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

/// Spatial data that ships in `LevelWorld` as decoded.
pub(super) struct SpatialSections {
    pub(super) bvh: BvhTree,
    pub(super) cells: Vec<CellData>,
    pub(super) cell_portal_refs: Vec<u32>,
    pub(super) cell_visibility: Option<CellVisibility>,
    pub(super) cell_residency_set: Option<CellResidencySetSection>,
    pub(super) cell_locator_root: CellLocatorChild,
    pub(super) cell_locator_nodes: Vec<CellLocatorNodeData>,
}

/// Wire-form sections and table facts kept only for parse-end validation.
pub(super) struct SpatialValidationInputs {
    pub(super) bvh_section: BvhSection,
    pub(super) portals_section: Option<PortalsSection>,
    pub(super) cells_section: CellsSection,
    pub(super) cell_locator_section: CellLocatorSection,
    pub(super) has_legacy_bsp_nodes: bool,
    pub(super) has_legacy_bsp_leaves: bool,
}

pub(super) struct SpatialDecode {
    pub(super) sections: SpatialSections,
    pub(super) validation_inputs: SpatialValidationInputs,
    /// Usable portal graph; adjacency against the cells is validated at parse end.
    pub(super) portal_data: Option<Vec<PortalData>>,
}

pub(super) fn decode_spatial(
    container: &PrlContainer,
    geometry: &GeometryDecode,
) -> Result<SpatialDecode, PrlLoadError> {
    let meta = container.metadata();
    let read_section = |section: SectionId| container.read_section(section as u32);
    let GeometryDecode {
        indices, face_meta, ..
    } = geometry;

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
    // Converted here, before id 22 is read: lightmap streaming needs a usable
    // portal graph. Adjacency against the cells is validated below.
    let portal_data = portals_section.as_ref().and_then(convert_usable_portals);

    let cells_section = match read_section(SectionId::Cells)? {
        Some(data) => CellsSection::from_bytes(&data)
            .map_err(|err| section_validation_from_error("Cells", err))?,
        None => return Err(stale_section("Cells", SectionId::Cells)),
    };
    let cell_count = cells_section.cells.len();
    let portal_ref_count = cells_section.portal_refs.len();
    let (cells, cell_portal_refs) = convert_cells_section(cells_section.clone());
    validate_face_meta_cells(face_meta, &cells)?;
    validate_cells_against_geometry(&cells, face_meta)?;
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

    // Optional — absent when the level has no usable portals, which runs
    // lightmaps all-resident. A present section must match the cell count.
    let cell_residency_set = match read_section(SectionId::CellResidencySet)? {
        Some(data) => {
            let section = CellResidencySetSection::from_bytes(&data, cells.len())
                .map_err(|err| section_validation_from_error("CellResidencySet", err))?;
            log::info!(
                "[PRL] CellResidencySet: {} camera cells, {} entries, max lead {}",
                section.camera_cell_count(),
                section.entries.len(),
                section.max_lead,
            );
            Some(section)
        }
        None => None,
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

    Ok(SpatialDecode {
        sections: SpatialSections {
            bvh,
            cells,
            cell_portal_refs,
            cell_visibility,
            cell_residency_set,
            cell_locator_root,
            cell_locator_nodes,
        },
        validation_inputs: SpatialValidationInputs {
            bvh_section,
            portals_section,
            cells_section,
            cell_locator_section,
            has_legacy_bsp_nodes,
            has_legacy_bsp_leaves,
        },
        portal_data,
    })
}
