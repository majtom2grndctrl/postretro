// Exact routes: the walk's own frames, one seeded eye per dwell frame.
// See: context/lib/rendering_pipeline.md §12

use glam::Vec3;
use postretro_level_loader::LevelWorld;
use thiserror::Error;

use crate::path::{SplitMix64, WALK_SEED, WalkKind};
use crate::walk::{Walk, camera_graph, eye_point};

/// XORed into the path seed to seed the eye stream, so eyes and path never
/// share draws.
pub const EYE_STREAM_SALT: u64 = 0xE7E5;

/// One frame's camera in world space.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RoutePose {
    /// The eye point, world metres.
    pub eye: Vec3,
    /// Engine radians; 0 faces -Z.
    pub yaw: f32,
    /// Engine radians, positive looking up.
    pub pitch: f32,
    /// The walk step and walk cell this frame belongs to, on walk-made poses.
    pub walk: Option<(u32, u32)>,
    /// Reached by a teleport: interpolation never crosses it.
    pub cut: bool,
}

/// A walk's frames as the harness draws them.
#[derive(Debug, Clone, PartialEq)]
pub struct ExactRoute {
    /// The settled-entry eye, drawn first from the eye stream.
    pub spawn: Vec3,
    pub poses: Vec<RoutePose>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum RouteError {
    #[error("a route needs at least one step")]
    ZeroSteps,
    #[error("no camera cells: every cell is solid or exterior")]
    NoCameraCells,
    #[error("no portal between camera cells")]
    NoPortalBetweenCameraCells,
    #[error("a smoothed route's speed must be finite and positive")]
    NonPositiveSpeed,
    #[error("a smoothed route's turn cap must be finite and positive")]
    NonPositiveTurnCap,
}

/// The walk of `kind` at the harness's seed, as exact frames at `pitch`
/// (radians): the frames the residency walk measures.
pub fn exact_route(
    world: &LevelWorld,
    kind: WalkKind,
    steps: usize,
    pitch: f32,
) -> Result<ExactRoute, RouteError> {
    exact_route_seeded(world, kind, steps, pitch, WALK_SEED)
}

/// [`exact_route`] with the path seeded from `seed` and the eyes from
/// `seed ^ EYE_STREAM_SALT`.
pub fn exact_route_seeded(
    world: &LevelWorld,
    kind: WalkKind,
    steps: usize,
    pitch: f32,
    seed: u64,
) -> Result<ExactRoute, RouteError> {
    walkable(world, steps)?;
    ExactRoute::of_walk(world, &Walk::build_seeded(world, kind, steps, seed), pitch)
}

/// Refuses zero steps and a level whose camera cells share no portal: a
/// route over it would be all teleports.
pub(crate) fn walkable(world: &LevelWorld, steps: usize) -> Result<(), RouteError> {
    if steps == 0 {
        return Err(RouteError::ZeroSteps);
    }
    let (camera_cells, adjacency) = camera_graph(world);
    if camera_cells.is_empty() {
        return Err(RouteError::NoCameraCells);
    }
    if adjacency.iter().all(Vec::is_empty) {
        return Err(RouteError::NoPortalBetweenCameraCells);
    }
    Ok(())
}

impl ExactRoute {
    /// `walk`'s frames: one stream seeded `walk.seed ^ EYE_STREAM_SALT`
    /// draws the spawn eye in the first cell, then one eye per dwell frame.
    /// Frame `f` of step `s` faces `travel_yaw[s] + f × 90°`. The harness
    /// run, bake-gap materialisation and the tool all take their frames from
    /// here, so no second copy of the stream exists.
    pub fn of_walk(world: &LevelWorld, walk: &Walk, pitch: f32) -> Result<Self, RouteError> {
        let Some(&first) = walk.cells.first() else {
            return Err(RouteError::NoCameraCells);
        };
        let mut rng = SplitMix64(walk.seed ^ EYE_STREAM_SALT);
        let spawn = eye_point(world, first, &mut rng);
        let mut poses = Vec::with_capacity(walk.frames() as usize);
        for (step, &cell) in walk.cells.iter().enumerate() {
            for frame in 0..walk.dwell[step] {
                poses.push(RoutePose {
                    eye: eye_point(world, cell, &mut rng),
                    yaw: walk.travel_yaw[step] + frame as f32 * std::f32::consts::FRAC_PI_2,
                    pitch,
                    walk: Some((step as u32, cell)),
                    cut: false,
                });
            }
        }
        Ok(Self { spawn, poses })
    }
}

/// Fingerprint of a walk frame's pose: step, walk cell, eye, yaw, pitch and
/// projection `(hfov_degrees, aspect)`, bit-exact.
pub fn pose_fingerprint(
    step: usize,
    cell: u32,
    eye: Vec3,
    yaw: f32,
    pitch: f32,
    projection: (f32, f32),
) -> u64 {
    let mut bytes = Vec::with_capacity(40);
    bytes.extend((step as u64).to_le_bytes());
    bytes.extend(cell.to_le_bytes());
    for value in [eye.x, eye.y, eye.z, yaw, pitch, projection.0, projection.1] {
        bytes.extend(value.to_bits().to_le_bytes());
    }
    let hash = blake3::hash(&bytes);
    u64::from_le_bytes(hash.as_bytes()[..8].try_into().expect("8 bytes"))
}
