//! The residency walks' seeded camera paths: cells, teleports, dwell, heading and eye points.
//! See: context/lib/rendering_pipeline.md §4

use glam::Vec3;
use postretro_level_loader::LevelWorld;

use super::FRAME_SECONDS;
use super::paths::{SplitMix64, WALK_SEED, WalkKind, camera_adjacency, walk_path};

/// A brisk boomer-shooter run speed, metres per second.
pub(super) const RUN_SPEED: f32 = 8.0;
pub(super) const MAX_DWELL_FRAMES: u32 = 60;
const EYE_TRIES: usize = 32;

/// One walk, resolved to cell ids, with its teleports marked and each step's
/// dwell in frames.
pub(super) struct Walk {
    pub(super) kind: WalkKind,
    pub(super) cells: Vec<u32>,
    /// Step `i` arrived by teleport: not portal-adjacent to step `i - 1`.
    pub(super) teleport: Vec<bool>,
    pub(super) dwell: Vec<u32>,
    pub(super) travel_yaw: Vec<f32>,
}

impl Walk {
    pub(super) fn build(world: &LevelWorld, kind: WalkKind, steps: usize) -> Self {
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
        let (path, _) = walk_path(kind, &adjacency, steps, WALK_SEED);
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
            cells,
            teleport,
            dwell,
            travel_yaw,
        }
    }

    pub(super) fn teleports(&self) -> usize {
        self.teleport.iter().filter(|&&t| t).count()
    }

    pub(super) fn frames(&self) -> u64 {
        self.dwell.iter().map(|&d| u64::from(d)).sum()
    }
}

/// A seeded eye point inside `cell` (10–90% of its AABB per axis, off the
/// bake's sampling lattice), accepted only where the locator agrees; the
/// AABB centre after `EYE_TRIES` misses.
pub(super) fn eye_point(world: &LevelWorld, cell: u32, rng: &mut SplitMix64) -> Vec3 {
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
