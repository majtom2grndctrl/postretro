// Small in-memory levels for the route tests.
// See: context/lib/testing_guide.md

use glam::Vec3;
use postretro_level_loader::{
    CellData, CellLocatorChild, CellLocatorNodeData, LevelWorld, PortalData,
};

/// Grid edge in metres.
const EDGE: f32 = 4.0;

fn cell(min: Vec3, max: Vec3, solid: bool, exterior: bool) -> CellData {
    CellData {
        bounds_min: min,
        bounds_max: max,
        face_start: 0,
        face_count: 0,
        portal_ref_start: 0,
        portal_ref_count: 0,
        is_solid: solid,
        is_exterior: exterior,
        is_drawable: !solid,
    }
}

fn plane(
    normal: Vec3,
    distance: f32,
    back: CellLocatorChild,
    front: CellLocatorChild,
) -> CellLocatorNodeData {
    CellLocatorNodeData {
        plane_normal: normal,
        plane_distance: distance,
        front,
        back,
    }
}

/// A world from cells, (front, back) portal pairs and a locator, with each
/// cell's portal refs derived from the portals.
fn assemble(
    mut cells: Vec<CellData>,
    pairs: &[(usize, usize)],
    root: CellLocatorChild,
    nodes: Vec<CellLocatorNodeData>,
) -> LevelWorld {
    let mut refs = Vec::new();
    for (index, data) in cells.iter_mut().enumerate() {
        data.portal_ref_start = refs.len() as u32;
        let before = refs.len();
        refs.extend(
            pairs
                .iter()
                .enumerate()
                .filter(|(_, (a, b))| *a == index || *b == index)
                .map(|(portal, _)| portal as u32),
        );
        data.portal_ref_count = (refs.len() - before) as u32;
    }
    let portals: Vec<PortalData> = pairs
        .iter()
        .map(|&(front, back)| {
            let a = (cells[front].bounds_min + cells[front].bounds_max) * 0.5;
            let b = (cells[back].bounds_min + cells[back].bounds_max) * 0.5;
            let mid = (a + b) * 0.5;
            PortalData {
                polygon: vec![mid, mid + Vec3::Y, mid + Vec3::Y + Vec3::X * 0.1],
                front_cell: front,
                back_cell: back,
            }
        })
        .collect();
    let has_portals = !portals.is_empty();
    LevelWorld::new_visibility_only(cells, refs, root, nodes, portals, has_portals)
        .expect("valid fixture world")
}

/// A 3×3 grid of 4 m cells on x/z (cell = row × 3 + col), its centre cell 4
/// solid. Bounds overhang the locator's regions by 1 m, so eye draws miss
/// and retry. Cell 8 has no portal (walks teleport to and from it). Cell 9
/// is exterior, portalled to cell 0. Cell 10 lies outside the grid and the
/// locator never returns it, so its eyes fall back to its AABB centre.
pub(crate) fn grid_world() -> LevelWorld {
    let mut cells = Vec::new();
    for row in 0..3 {
        for col in 0..3 {
            let min = Vec3::new(col as f32 * EDGE - 1.0, 0.0, row as f32 * EDGE - 1.0);
            let max = Vec3::new(
                (col + 1) as f32 * EDGE + 1.0,
                3.0,
                (row + 1) as f32 * EDGE + 1.0,
            );
            cells.push(cell(min, max, row == 1 && col == 1, false));
        }
    }
    cells.push(cell(
        Vec3::new(-8.0, 0.0, 0.0),
        Vec3::new(-4.0, 3.0, 4.0),
        false,
        true,
    ));
    cells.push(cell(
        Vec3::new(20.0, 0.0, 0.0),
        Vec3::new(24.0, 3.0, 4.0),
        false,
        false,
    ));
    let pairs = [
        (0, 1),
        (1, 2),
        (0, 3),
        (2, 5),
        (3, 6),
        (6, 7),
        (9, 0),
        (2, 10),
    ];

    // Columns split on x, then each column's rows on z.
    use CellLocatorChild::{Cell, Node};
    let column = |col: usize, next: usize| {
        [
            plane(Vec3::Z, EDGE, Cell(col), Node(next)),
            plane(Vec3::Z, 2.0 * EDGE, Cell(3 + col), Cell(6 + col)),
        ]
    };
    let mut nodes = vec![
        plane(Vec3::X, EDGE, Node(2), Node(1)),
        plane(Vec3::X, 2.0 * EDGE, Node(4), Node(6)),
    ];
    nodes.extend(column(0, 3));
    nodes.extend(column(1, 5));
    nodes.extend(column(2, 7));
    assemble(cells, &pairs, Node(0), nodes)
}

/// Two open cells with no portal between them.
pub(crate) fn portalless_world() -> LevelWorld {
    let cells = vec![
        cell(Vec3::ZERO, Vec3::splat(EDGE), false, false),
        cell(
            Vec3::new(EDGE, 0.0, 0.0),
            Vec3::new(2.0 * EDGE, EDGE, EDGE),
            false,
            false,
        ),
    ];
    let nodes = vec![plane(
        Vec3::X,
        EDGE,
        CellLocatorChild::Cell(0),
        CellLocatorChild::Cell(1),
    )];
    assemble(cells, &[], CellLocatorChild::Node(0), nodes)
}

/// One solid cell: no camera cell at all.
pub(crate) fn solid_world() -> LevelWorld {
    let cells = vec![cell(Vec3::ZERO, Vec3::splat(EDGE), true, false)];
    assemble(cells, &[], CellLocatorChild::Cell(0), Vec::new())
}
