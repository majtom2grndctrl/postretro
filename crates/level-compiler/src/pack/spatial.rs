//! Runtime spatial-section encoding from finalized compiler data.
//! See: context/lib/build_pipeline.md §PRL Compilation

use super::*;

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
        anyhow::bail!("cannot encode Cells: source BspLeavesSection is empty");
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

/// Encode the point-to-cell locator from the final BSP tree.
pub fn encode_cell_locator(tree: &BspTree) -> anyhow::Result<CellLocatorSection> {
    if tree.leaves.is_empty() {
        anyhow::bail!("cannot encode CellLocator: source BspLeavesSection is empty");
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
