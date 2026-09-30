// Synthetic Cells/Portals/CellLocator sections shaped like compiler output,
// shared by the residency bake's tests and the dry run's parity test.
// See: context/lib/testing_guide.md §4 (Test fixtures)

use postretro_level_format::cell_locator::{
    CellLocatorChild, CellLocatorNodeRecord, CellLocatorSection,
};
use postretro_level_format::cells::{
    CELL_FLAG_DRAWABLE, CELL_FLAG_SOLID, CellRecord, CellsSection,
};
use postretro_level_format::portals::{PortalRecord, PortalsSection};

/// Open cells of the L-shaped corridor; cell `L_SOLID` fills its inner corner.
pub(crate) const L_OPEN_CELLS: u32 = 11;
pub(crate) const L_SOLID: u32 = 11;

/// An L-shaped corridor, 4 m tall and 10 m per cell. The X leg is cells 0–4
/// (x 0–50, z 0–4); the Z leg is cells 5–10 (x 40–50, z 4–64), opening off
/// cell 4 through the corner portal at z = 4. Solid cell 11 fills x 0–40,
/// z 4–64, so the corner hides the far end of each leg from the other: a
/// camera sees only the first cells past the corner, while the hub metric
/// reaches three cells within 32 m.
pub(crate) fn l_corridor_sections() -> (CellsSection, PortalsSection, CellLocatorSection) {
    let mut bounds: Vec<([f32; 3], [f32; 3])> = (0..5)
        .map(|k| {
            let x = 10.0 * k as f32;
            ([x, 0.0, 0.0], [x + 10.0, 4.0, 4.0])
        })
        .collect();
    bounds.extend((0..6).map(|j| {
        let z = 4.0 + 10.0 * j as f32;
        ([40.0, 0.0, z], [50.0, 4.0, z + 10.0])
    }));
    bounds.push(([0.0, 0.0, 4.0], [40.0, 4.0, 64.0]));

    // (front, back, polygon) per portal, in portal-id order.
    let x_face = |x: f32| vec![[x, 0.0, 0.0], [x, 4.0, 0.0], [x, 4.0, 4.0], [x, 0.0, 4.0]];
    let z_face = |z: f32| {
        vec![
            [40.0, 0.0, z],
            [50.0, 0.0, z],
            [50.0, 4.0, z],
            [40.0, 4.0, z],
        ]
    };
    let mut portal_list: Vec<(u32, u32, Vec<[f32; 3]>)> = (0..4)
        .map(|k| (k, k + 1, x_face(10.0 * (k + 1) as f32)))
        .collect();
    portal_list.push((4, 5, z_face(4.0)));
    portal_list.extend((0..5).map(|j| (5 + j, 6 + j, z_face(14.0 + 10.0 * j as f32))));

    let mut refs_by_cell: Vec<Vec<u32>> = vec![Vec::new(); bounds.len()];
    for (portal, (front, back, _)) in portal_list.iter().enumerate() {
        refs_by_cell[*front as usize].push(portal as u32);
        refs_by_cell[*back as usize].push(portal as u32);
    }
    let mut portal_refs = Vec::new();
    let cells = bounds
        .iter()
        .zip(&refs_by_cell)
        .enumerate()
        .map(|(cell, ((min, max), refs))| {
            let solid = cell as u32 == L_SOLID;
            let record = CellRecord {
                bounds_min: *min,
                bounds_max: *max,
                flags: if solid {
                    CELL_FLAG_SOLID
                } else {
                    CELL_FLAG_DRAWABLE
                },
                face_start: cell as u32,
                face_count: u32::from(!solid),
                portal_ref_start: portal_refs.len() as u32,
                portal_ref_count: refs.len() as u32,
            };
            portal_refs.extend_from_slice(refs);
            record
        })
        .collect();

    let mut vertices = Vec::new();
    let portals = portal_list
        .into_iter()
        .map(|(front, back, polygon)| {
            let record = PortalRecord {
                vertex_start: vertices.len() as u32,
                vertex_count: polygon.len() as u32,
                front_leaf: front,
                back_leaf: back,
            };
            vertices.extend(polygon);
            record
        })
        .collect();

    // Split z = 4 first: below is the X leg, above is the solid corner (x < 40)
    // or the Z leg. Each leg is then a chain of splits along its axis.
    let node = |normal: [f32; 3], distance: f32, front, back| CellLocatorNodeRecord {
        plane_normal: normal,
        plane_distance: distance,
        front,
        back,
    };
    let (cell, next) = (CellLocatorChild::Cell, CellLocatorChild::Node);
    let x = [1.0, 0.0, 0.0];
    let z = [0.0, 0.0, 1.0];
    let nodes = vec![
        node(z, 4.0, next(1), next(2)),
        node(x, 40.0, next(6), cell(L_SOLID)),
        node(x, 10.0, next(3), cell(0)),
        node(x, 20.0, next(4), cell(1)),
        node(x, 30.0, next(5), cell(2)),
        node(x, 40.0, cell(4), cell(3)),
        node(z, 14.0, next(7), cell(5)),
        node(z, 24.0, next(8), cell(6)),
        node(z, 34.0, next(9), cell(7)),
        node(z, 44.0, next(10), cell(8)),
        node(z, 54.0, cell(10), cell(9)),
    ];
    (
        CellsSection { cells, portal_refs },
        PortalsSection { vertices, portals },
        CellLocatorSection {
            root: next(0),
            nodes,
        },
    )
}
