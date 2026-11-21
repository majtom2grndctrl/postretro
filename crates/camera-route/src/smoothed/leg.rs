// Proven legs: a leg's segments, checked down the cell locator to stay in its two cells.
// See: context/lib/rendering_pipeline.md §12

use glam::{DVec3, Vec3};
use postretro_level_loader::{CellLocatorChild, LevelWorld};

use crate::path::SplitMix64;
use crate::walk::EYE_TRIES;

/// How far past a portal's centroid a leg ends, metres, largest first: the
/// next cell may be thinner than the first.
const PORTAL_PUSHES: [f32; 3] = [0.25, 0.05, 0.01];
/// Seeded detour points drawn in each of a leg's two cells per round, and the
/// rounds before a leg becomes a cut.
const DETOUR_DRAWS: usize = 12;
const DETOUR_ROUNDS: usize = 4;
/// A locator plane within this distance of a segment counts as crossed, so an
/// `f32` sample beside the segment still locates in a cell the proof visited.
const LOCATE_MARGIN: f64 = 1e-3;

/// A leg's polyline after its start, ending at a point in `to`, every
/// segment proven to stay in `from` or `to`: straight through the nearest
/// portal when that line holds, else the shortest detour through portal,
/// centre and seeded points.
pub(super) fn leg(
    world: &LevelWorld,
    start: Vec3,
    from: usize,
    to: usize,
    rng: &mut SplitMix64,
) -> Option<Vec<Vec3>> {
    let allowed = [from, to];
    let data = &world.cells[from];
    let refs =
        &world.cell_portal_refs[data.portal_ref_start as usize..][..data.portal_ref_count as usize];
    let portals: Vec<&[Vec3]> = refs
        .iter()
        .map(|&index| &world.portals[index as usize])
        .filter(|p| {
            [p.front_cell, p.back_cell] == allowed || [p.back_cell, p.front_cell] == allowed
        })
        .map(|p| p.polygon.as_slice())
        .collect();
    let mut targets: Vec<Vec3> = portals
        .iter()
        .filter_map(|polygon| portal_point(world, polygon, to))
        .collect();
    targets.sort_by(|a, b| start.distance(*a).total_cmp(&start.distance(*b)));
    if let Some(&target) = targets.first()
        && segment_stays_in(world, start, target, allowed)
    {
        return Some(vec![target]);
    }

    let mut nodes = vec![start];
    nodes.extend(targets);
    nodes.extend(
        portals
            .iter()
            .filter_map(|polygon| portal_point(world, polygon, from)),
    );
    nodes.extend(allowed.iter().filter_map(|&cell| cell_centre(world, cell)));
    // Each round adds seeded points in both cells and searches again, so a
    // crowded cell gets a denser graph only when a sparse one falls short.
    for _ in 0..DETOUR_ROUNDS {
        for cell in allowed {
            nodes.extend((0..DETOUR_DRAWS).filter_map(|_| drawn_point(world, cell, rng)));
        }
        let is_target: Vec<bool> = nodes
            .iter()
            .map(|&at| world.locate_cell(at) == to)
            .collect();
        if let Some(path) = shortest_path(world, &nodes, &is_target, allowed) {
            return Some(path);
        }
    }
    None
}

/// Dijkstra from `nodes[0]` to the nearest target over proven segments;
/// ties go to the lower index, so the detour is deterministic.
fn shortest_path(
    world: &LevelWorld,
    nodes: &[Vec3],
    is_target: &[bool],
    allowed: [usize; 2],
) -> Option<Vec<Vec3>> {
    let n = nodes.len();
    let mut dist = vec![f32::INFINITY; n];
    let mut prev = vec![usize::MAX; n];
    let mut done = vec![false; n];
    dist[0] = 0.0;
    loop {
        let u = (0..n)
            .filter(|&i| !done[i] && dist[i].is_finite())
            .min_by(|&a, &b| dist[a].total_cmp(&dist[b]))?;
        if is_target[u] {
            let mut path = vec![nodes[u]];
            let mut at = prev[u];
            while at != 0 {
                path.push(nodes[at]);
                at = prev[at];
            }
            path.reverse();
            return Some(path);
        }
        done[u] = true;
        for v in 0..n {
            let through = dist[u] + nodes[u].distance(nodes[v]);
            if !done[v] && through < dist[v] && segment_stays_in(world, nodes[u], nodes[v], allowed)
            {
                dist[v] = through;
                prev[v] = u;
            }
        }
    }
}

/// A point just off `polygon`'s centroid that locates in `cell`: pushed along
/// the polygon's normal either way, or toward the cell's centre.
fn portal_point(world: &LevelWorld, polygon: &[Vec3], cell: usize) -> Option<Vec3> {
    if polygon.is_empty() {
        return None;
    }
    let centroid = polygon.iter().copied().sum::<Vec3>() / polygon.len() as f32;
    let normal = polygon
        .iter()
        .zip(polygon.iter().cycle().skip(1))
        .map(|(a, b)| (*a - *b).cross(*a + *b))
        .sum::<Vec3>()
        .normalize_or_zero();
    let data = &world.cells[cell];
    let inward = ((data.bounds_min + data.bounds_max) * 0.5 - centroid).normalize_or_zero();
    PORTAL_PUSHES.into_iter().find_map(|push| {
        [normal, -normal, inward]
            .into_iter()
            .filter(|direction| *direction != Vec3::ZERO)
            .map(|direction| centroid + direction * push)
            .find(|&at| world.locate_cell(at) == cell)
    })
}

fn cell_centre(world: &LevelWorld, cell: usize) -> Option<Vec3> {
    let data = &world.cells[cell];
    let centre = (data.bounds_min + data.bounds_max) * 0.5;
    (world.locate_cell(centre) == cell).then_some(centre)
}

/// A seeded point inside `cell`'s AABB (10–90% per axis) that locates there.
fn drawn_point(world: &LevelWorld, cell: usize, rng: &mut SplitMix64) -> Option<Vec3> {
    let data = &world.cells[cell];
    let extent = data.bounds_max - data.bounds_min;
    (0..EYE_TRIES).find_map(|_| {
        let fraction = Vec3::new(rng.unit(), rng.unit(), rng.unit()) * 0.8 + Vec3::splat(0.1);
        let at = data.bounds_min + extent * fraction;
        (world.locate_cell(at) == cell).then_some(at)
    })
}

/// Where a piece starts in `cell`: its centre, else a seeded point.
pub(super) fn in_cell_point(world: &LevelWorld, cell: usize, rng: &mut SplitMix64) -> Option<Vec3> {
    cell_centre(world, cell).or_else(|| drawn_point(world, cell, rng))
}

/// Whether every point within `LOCATE_MARGIN` of segment `a`–`b` locates in
/// `allowed`: the segment is clipped down the locator tree, entering both
/// children wherever it comes within the margin of a node's plane.
fn segment_stays_in(world: &LevelWorld, a: Vec3, b: Vec3, allowed: [usize; 2]) -> bool {
    fn clip(
        world: &LevelWorld,
        child: CellLocatorChild,
        a: DVec3,
        b: DVec3,
        allowed: [usize; 2],
    ) -> bool {
        let node = match child {
            CellLocatorChild::Cell(cell) => return allowed.contains(&cell),
            CellLocatorChild::Node(node) => &world.cell_locator_nodes[node],
        };
        let normal = node.plane_normal.as_dvec3();
        let distance = f64::from(node.plane_distance);
        let (da, db) = (normal.dot(a) - distance, normal.dot(b) - distance);
        [(1.0, node.front), (-1.0, node.back)]
            .into_iter()
            .all(|(side, child)| {
                // The part of the segment with side × distance ≥ −margin.
                let (fa, fb) = (side * da + LOCATE_MARGIN, side * db + LOCATE_MARGIN);
                let (t0, t1) = match (fa >= 0.0, fb >= 0.0) {
                    (false, false) => return true,
                    (true, true) => (0.0, 1.0),
                    (true, false) => (0.0, fa / (fa - fb)),
                    (false, true) => (fa / (fa - fb), 1.0),
                };
                clip(world, child, a.lerp(b, t0), a.lerp(b, t1), allowed)
            })
    }
    clip(
        world,
        world.cell_locator_root,
        a.as_dvec3(),
        b.as_dvec3(),
        allowed,
    )
}
