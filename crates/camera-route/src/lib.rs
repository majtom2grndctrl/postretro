// Seeded camera routes over a level's cells and portals, and the pose file that carries them.
// See: context/lib/rendering_pipeline.md §12

// One generator for the residency walk harness, bake-gap materialisation and
// `postretro-tool generate-camera-route`, so a route an SDK user generates is
// the route the walk measures. The engine's non-test code reads pose files
// and the bake identity; it never generates a route.

mod bake;
mod exact;
mod path;
mod pose_file;
mod walk;

#[cfg(test)]
mod exact_tests;
#[cfg(test)]
mod pose_file_tests;
#[cfg(test)]
mod test_fixtures;

pub use bake::{bake_identity, bake_token};
pub use exact::{
    EYE_STREAM_SALT, ExactRoute, RouteError, RoutePose, exact_route, exact_route_seeded,
    pose_fingerprint,
};
pub use path::{
    STALL_TELEPORT_STEPS, SplitMix64, WALK_SEED, WalkKind, camera_adjacency, walk_path,
};
pub use pose_file::{
    GeneratedKind, POSE_FILE_MAGIC, ParseReason, PoseFile, PoseFileError, Projection, RouteHeader,
    RouteOrigin, format_pose_file, parse_pose_file,
};
pub use walk::{EYE_TRIES, MAX_DWELL_FRAMES, RUN_SPEED, Walk, camera_graph, eye_point};

/// The walk's frame period: 60 Hz, the cadence every pose file shares.
pub const FRAME_SECONDS: f64 = 1.0 / 60.0;
/// Poses per second of route time, written as the pose file's `cadence_hz`.
pub const CADENCE_HZ: u32 = 60;
/// The walk's default step count.
pub const DEFAULT_STEPS: usize = 2_000;
/// The walk's default horizontal FOV: the engine's `HFOV`, in degrees.
pub const DEFAULT_HFOV_DEGREES: f32 = 100.0;
/// The walk's default aspect, 16:9, as the walk computes it in `f32`; its
/// bits enter every pose fingerprint.
pub const DEFAULT_ASPECT: f32 = 16.0 / 9.0;
