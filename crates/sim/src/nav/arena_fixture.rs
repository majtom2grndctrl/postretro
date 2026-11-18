// Test fixture: a multi-arena level with a realistic region count. Four 64×64 m
// arenas in a row along +X, joined by 40 m corridors 4 m wide, on 2×2 m nav
// tiles: 4,216 regions and about 8.3k portals (env-volumes has 4,326). Walls
// are 0.8 m: bodies collide with them and sight passes over them.
// See: context/plans/in-progress/perf-ai-tick-cost (multi-arena row, P3, P5)

use glam::Vec3;
use postretro_level_format::navmesh::{NAVMESH_VERSION, NavMeshSection, NavPortal, NavRegion};

pub const ARENA: f32 = 64.0;
pub const GAP: f32 = 40.0;
pub const ARENAS: usize = 4;
const TILE: u32 = 2;
const WALL_HEIGHT: f32 = 0.8;
/// Corridor span on Z, centred on the arenas.
const CORRIDOR_Z: std::ops::Range<f32> = 30.0..34.0;

/// World X of arena `index`'s west edge.
pub fn arena_x0(index: usize) -> f32 {
    index as f32 * (ARENA + GAP)
}

/// The nav section and collision triangles of the multi-arena level.
pub struct ArenaMap {
    pub section: NavMeshSection,
    pub points: Vec<Vec3>,
    pub triangles: Vec<[u32; 3]>,
}

/// A nav section of 2×2 m tiles over `tiles_x` × `tiles_z`, one region per
/// tile where `walkable` holds, with a portal between each pair of walkable
/// neighbours. Region indices ascend row by row.
pub fn tile_grid_section(
    tiles_x: u32,
    tiles_z: u32,
    walkable: impl Fn(u32, u32) -> bool,
) -> NavMeshSection {
    let mut index = vec![u32::MAX; (tiles_x * tiles_z) as usize];
    let mut regions = Vec::new();
    for tz in 0..tiles_z {
        for tx in 0..tiles_x {
            if walkable(tx, tz) {
                index[(tz * tiles_x + tx) as usize] = regions.len() as u32;
                regions.push(NavRegion {
                    x0: tx * TILE,
                    z0: tz * TILE,
                    x1: (tx + 1) * TILE,
                    z1: (tz + 1) * TILE,
                    floor_y_min: 0.0,
                    floor_y_max: 0.25,
                });
            }
        }
    }
    let mut portals = Vec::new();
    for tz in 0..tiles_z {
        for tx in 0..tiles_x {
            let a = index[(tz * tiles_x + tx) as usize];
            if a == u32::MAX {
                continue;
            }
            if tx + 1 < tiles_x {
                let b = index[(tz * tiles_x + tx + 1) as usize];
                if b != u32::MAX {
                    let x = ((tx + 1) * TILE) as f32;
                    portals.push(NavPortal {
                        region_a: a,
                        region_b: b,
                        left: [x, 0.0, ((tz + 1) * TILE) as f32],
                        right: [x, 0.0, (tz * TILE) as f32],
                    });
                }
            }
            if tz + 1 < tiles_z {
                let b = index[((tz + 1) * tiles_x + tx) as usize];
                if b != u32::MAX {
                    let z = ((tz + 1) * TILE) as f32;
                    portals.push(NavPortal {
                        region_a: a,
                        region_b: b,
                        left: [(tx * TILE) as f32, 0.0, z],
                        right: [((tx + 1) * TILE) as f32, 0.0, z],
                    });
                }
            }
        }
    }
    portals.sort_by_key(|portal| (portal.region_a, portal.region_b));
    NavMeshSection {
        version: NAVMESH_VERSION,
        origin: [0.0, 0.0, 0.0],
        cell_size: 1.0,
        dim_x: tiles_x * TILE,
        dim_z: tiles_z * TILE,
        agent_radius: 0.35,
        agent_height: 1.8,
        step_height: 0.4,
        max_slope_deg: 45.0,
        regions,
        portals,
        gates: Vec::new(),
        links: Vec::new(),
    }
}

pub fn multi_arena_map() -> ArenaMap {
    let total_x = (ARENAS as f32 * ARENA + (ARENAS - 1) as f32 * GAP) as u32;
    let total_z = ARENA as u32;
    let walkable = |tx: u32, tz: u32| -> bool {
        let (x, z) = ((tx * TILE) as f32, (tz * TILE) as f32);
        (0..ARENAS).any(|i| {
            let x0 = arena_x0(i);
            (x >= x0 && x < x0 + ARENA)
                || (i + 1 < ARENAS
                    && x >= x0 + ARENA
                    && x < x0 + ARENA + GAP
                    && CORRIDOR_Z.contains(&z))
        })
    };
    let section = tile_grid_section(total_x / TILE, total_z / TILE, walkable);

    // One floor quad plus low wall quads.
    let mut points = vec![
        Vec3::new(-1.0, 0.0, -1.0),
        Vec3::new(total_x as f32 + 1.0, 0.0, -1.0),
        Vec3::new(total_x as f32 + 1.0, 0.0, total_z as f32 + 1.0),
        Vec3::new(-1.0, 0.0, total_z as f32 + 1.0),
    ];
    let mut triangles: Vec<[u32; 3]> = vec![[0, 1, 2], [0, 2, 3]];
    let mut wall = |ax: f32, az: f32, bx: f32, bz: f32| {
        let base = points.len() as u32;
        points.extend([
            Vec3::new(ax, 0.0, az),
            Vec3::new(bx, 0.0, bz),
            Vec3::new(bx, WALL_HEIGHT, bz),
            Vec3::new(ax, WALL_HEIGHT, az),
        ]);
        triangles.push([base, base + 1, base + 2]);
        triangles.push([base, base + 2, base + 3]);
    };
    let (z_lo, z_hi) = (CORRIDOR_Z.start, CORRIDOR_Z.end);
    for i in 0..ARENAS {
        let x0 = arena_x0(i);
        let x1 = x0 + ARENA;
        wall(x0, 0.0, x1, 0.0);
        wall(x0, ARENA, x1, ARENA);
        if i == 0 {
            wall(x0, 0.0, x0, ARENA);
        } else {
            wall(x0, 0.0, x0, z_lo);
            wall(x0, z_hi, x0, ARENA);
        }
        if i + 1 == ARENAS {
            wall(x1, 0.0, x1, ARENA);
        } else {
            wall(x1, 0.0, x1, z_lo);
            wall(x1, z_hi, x1, ARENA);
            wall(x1, z_lo, x1 + GAP, z_lo);
            wall(x1, z_hi, x1 + GAP, z_hi);
        }
    }
    ArenaMap {
        section,
        points,
        triangles,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn multi_arena_map_has_the_realistic_region_count() {
        let map = multi_arena_map();
        assert_eq!(map.section.regions.len(), 4_216);
        assert!(map.section.portals.len() > 8_000);
    }
}
