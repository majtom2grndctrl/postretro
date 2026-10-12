// Pure collision geometry over level data: convex leaves, environment volumes, and the environment resolve.
// See: context/plans/in-progress/E24--water-and-environment-volumes/index.md

#![deny(unsafe_code)]

mod leaf;
mod polytope;
mod resolve;
#[cfg(test)]
mod test_fixtures;
mod volumes;

pub use leaf::{ContentsMask, ConvexLeaf, LeafPlane};
pub use resolve::{BodyProbe, EnvironmentView, ResolvedEnvironment, resolve_environment};
pub use volumes::{EnvironmentVolume, EnvironmentVolumes};
