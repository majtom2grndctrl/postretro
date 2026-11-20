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

/// Tile edge and height for `tile_world`, metres.
const TILE: f32 = 4.0;
const TILE_HEIGHT: f32 = 3.0;

/// A world of 4 m tiles on x/z: `layout[row][col]` names each tile's open
/// cell, `None` the shared solid cell, numbered after the open ones. A cell
/// may span tiles, so it can be non-convex. Each tile face between two open
/// cells is a portal whose polygon is that face. The locator splits columns
/// on x, then each column's rows on z.
pub(crate) fn tile_world(layout: &[&[Option<usize>]]) -> LevelWorld {
    use CellLocatorChild::{Cell, Node};

    let rows = layout.len();
    let cols = layout[0].len();
    let solid = layout
        .iter()
        .flat_map(|row| row.iter().flatten())
        .max()
        .map_or(0, |&max| max + 1);
    let tile = |row: usize, col: usize| layout[row][col].unwrap_or(solid);
    let tile_min = |row: usize, col: usize| Vec3::new(col as f32 * TILE, 0.0, row as f32 * TILE);

    let mut cells: Vec<CellData> = (0..=solid)
        .map(|id| cell(Vec3::MAX, Vec3::MIN, id == solid, false))
        .collect();
    for row in 0..rows {
        for col in 0..cols {
            let data = &mut cells[tile(row, col)];
            let min = tile_min(row, col);
            data.bounds_min = data.bounds_min.min(min);
            data.bounds_max = data
                .bounds_max
                .max(min + Vec3::new(TILE, TILE_HEIGHT, TILE));
        }
    }

    let mut pairs = Vec::new();
    let mut polygons = Vec::new();
    let up = Vec3::Y * TILE_HEIGHT;
    for row in 0..rows {
        for col in 0..cols {
            let here = tile(row, col);
            // The face toward the next column (+x), then the next row (+z).
            for (next, normal, span) in [
                (
                    (col + 1 < cols).then(|| tile(row, col + 1)),
                    Vec3::X,
                    Vec3::Z,
                ),
                (
                    (row + 1 < rows).then(|| tile(row + 1, col)),
                    Vec3::Z,
                    Vec3::X,
                ),
            ] {
                let Some(next) = next.filter(|&next| next != here && next != solid) else {
                    continue;
                };
                if here == solid {
                    continue;
                }
                let base = tile_min(row, col) + normal * TILE;
                let side = span * TILE;
                pairs.push((here, next));
                polygons.push(vec![base, base + side, base + side + up, base + up]);
            }
        }
    }

    fn split(
        nodes: &mut Vec<CellLocatorNodeData>,
        (lo, hi): (usize, usize),
        normal: Vec3,
        leaf: &dyn Fn(usize) -> CellLocatorChild,
    ) -> CellLocatorChild {
        if hi - lo == 1 {
            return leaf(lo);
        }
        let mid = (lo + hi) / 2;
        let back = split(nodes, (lo, mid), normal, leaf);
        let front = split(nodes, (mid, hi), normal, leaf);
        nodes.push(plane(normal, mid as f32 * TILE, back, front));
        Node(nodes.len() - 1)
    }
    let mut nodes = Vec::new();
    let columns: Vec<CellLocatorChild> = (0..cols)
        .map(|col| split(&mut nodes, (0, rows), Vec3::Z, &|row| Cell(tile(row, col))))
        .collect();
    let root = split(&mut nodes, (0, cols), Vec3::X, &|col| columns[col]);

    let mut world = assemble(cells, &pairs, root, nodes);
    for (portal, polygon) in world.portals.iter_mut().zip(polygons) {
        portal.polygon = polygon;
    }
    world
}

/// The smoothed-route layout, rows of +z from the top: a straight corridor
/// 0–1–2 into 3, an L turn north into 4, then cell 5, three tiles in an L
/// whose straight line from 4's portal to 6's crosses the solid tile in its
/// crook; and the pair 7–8, joined to nothing else, so walks teleport to and
/// from it.
pub(crate) fn smoothed_world() -> LevelWorld {
    const N: Option<usize> = None;
    tile_world(&[
        &[
            Some(0),
            Some(1),
            Some(2),
            Some(3),
            N,
            N,
            N,
            Some(7),
            Some(8),
        ],
        &[N, N, N, Some(4), N, N, N, N, N],
        &[N, N, N, Some(5), N, N, N, N, N],
        &[N, N, N, Some(5), Some(5), Some(6), N, N, N],
    ])
}
