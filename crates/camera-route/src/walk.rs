// The walk's steps resolved to cells, teleports, dwell, heading and eye points.
// See: context/lib/rendering_pipeline.md §12

use glam::Vec3;
use postretro_level_loader::LevelWorld;

use crate::FRAME_SECONDS;
use crate::path::{SplitMix64, WALK_SEED, WalkKind, camera_adjacency, walk_path};

/// A brisk boomer-shooter run speed, metres per second.
pub const RUN_SPEED: f32 = 8.0;
pub const MAX_DWELL_FRAMES: u32 = 60;
/// Eye draws per point before falling back to the cell's AABB centre.
pub const EYE_TRIES: usize = 32;

/// The level's camera cells (neither solid nor exterior, ascending id) and
/// their portal adjacency, by index into the camera cells.
pub fn camera_graph(world: &LevelWorld) -> (Vec<u32>, Vec<Vec<u32>>) {
    let camera_cells: Vec<u32> = (0..world.cells.len() as u32)
        .filter(|&cell| {
            let data = &world.cells[cell as usize];
            !data.is_solid && !data.is_exterior
        })
        .collect();
    let adjacency = camera_adjacency(
        world.cells.len(),
        world.portals.iter().map(|p| (p.front_cell, p.back_cell)),
        &camera_cells,
    );
    (camera_cells, adjacency)
}

/// One walk, resolved to cell ids, with its teleports marked and each step's
/// dwell in frames.
pub struct Walk {
    pub kind: WalkKind,
    /// The path's seed; the eye stream is seeded from it (`EYE_STREAM_SALT`).
    pub seed: u64,
    pub cells: Vec<u32>,
    /// Step `i` arrived by teleport: not portal-adjacent to step `i - 1`.
    pub teleport: Vec<bool>,
    pub dwell: Vec<u32>,
    pub travel_yaw: Vec<f32>,
}

impl Walk {
    /// The walk at the harness's seed, `WALK_SEED`.
    pub fn build(world: &LevelWorld, kind: WalkKind, steps: usize) -> Self {
        Self::build_seeded(world, kind, steps, WALK_SEED)
    }

    pub fn build_seeded(world: &LevelWorld, kind: WalkKind, steps: usize, seed: u64) -> Self {
        let (camera_cells, adjacency) = camera_graph(world);
        let (path, _) = walk_path(kind, &adjacency, steps, seed);
        let teleport = (0..path.len())
            .map(|i| {
                i > 0
                    && path[i] != path[i - 1]
                    && adjacency[path[i - 1] as usize]
                        .binary_search(&path[i])
                        .is_err()
            })
            .collect();
        let cells: Vec<u32> = path.iter().map(|&i| camera_cells[i as usize]).collect();
        let centre = |cell: u32| {
            let data = &world.cells[cell as usize];
            (data.bounds_min + data.bounds_max) * 0.5
        };
        let mut dwell = Vec::with_capacity(cells.len());
        let mut travel_yaw = Vec::with_capacity(cells.len());
        let mut yaw = 0.0f32;
        for i in 0..cells.len() {
            let Some(&next) = cells.get(i + 1) else {
                dwell.push(1);
                travel_yaw.push(yaw);
                continue;
            };
            let delta = centre(next) - centre(cells[i]);
            let frames = (delta.length() / (RUN_SPEED * FRAME_SECONDS as f32)).ceil() as u32;
            dwell.push(frames.clamp(1, MAX_DWELL_FRAMES));
            if delta.x != 0.0 || delta.z != 0.0 {
                yaw = (-delta.x).atan2(-delta.z);
            }
            travel_yaw.push(yaw);
        }
        Self {
            kind,
            seed,
            cells,
            teleport,
            dwell,
            travel_yaw,
        }
    }

    pub fn teleports(&self) -> usize {
        self.teleport.iter().filter(|&&t| t).count()
    }

    pub fn frames(&self) -> u64 {
        self.dwell.iter().map(|&d| u64::from(d)).sum()
    }
}

/// A seeded eye point inside `cell` (10–90% of its AABB per axis, off the
/// bake's sampling lattice), accepted only where the locator agrees; the
/// AABB centre after `EYE_TRIES` misses. Each try draws x, y, z in that
/// order: the draw order is the route's contract.
pub fn eye_point(world: &LevelWorld, cell: u32, rng: &mut SplitMix64) -> Vec3 {
    let data = &world.cells[cell as usize];
    let extent = data.bounds_max - data.bounds_min;
    for _ in 0..EYE_TRIES {
        let fraction = Vec3::new(rng.unit(), rng.unit(), rng.unit()) * 0.8 + Vec3::splat(0.1);
        let point = data.bounds_min + extent * fraction;
        if world.locate_cell(point) == cell as usize {
            return point;
        }
    }
    (data.bounds_min + data.bounds_max) * 0.5
}
