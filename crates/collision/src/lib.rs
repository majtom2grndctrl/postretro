// Pure collision geometry over level data: convex leaves, environment volumes, and the environment resolve.
// See: context/lib/entity_model.md §7 (Environment Volumes)

#![deny(unsafe_code)]

mod bvh;
mod crossings;
mod eye;
mod leaf;
mod line;
mod polytope;
mod query_timer;
mod resolve;
#[cfg(test)]
mod test_fixtures;
mod volumes;

#[cfg(test)]
mod crossings_tests;
#[cfg(test)]
mod fluid_resolve_tests;
#[cfg(test)]
mod query_equivalence_tests;

pub use crossings::{
    CROSSING_SEAM_EPSILON, CROSSING_START_EPSILON, FluidCrossing, visit_fluid_crossings,
};
pub use eye::{
    EYE_DEPTH_REACH, EyeFluid, fluid_at_eye, fluid_volume_plane_depth, fluid_volume_signed_depth,
};
pub use leaf::{ContentsMask, ConvexLeaf, LeafPlane};
pub use query_timer::EnvironmentQueryTimer;
pub use resolve::{
    BodyProbe, EnvironmentView, ResolvedEnvironment, resolve_environment, resolve_gravity,
    resolve_particle_acceleration,
};
pub use volumes::{EnvironmentVolume, EnvironmentVolumes};
