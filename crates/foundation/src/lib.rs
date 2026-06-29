//! VM-free foundation data and evaluation substrate for Postretro.
//! See: context/lib/scripting.md §12.

#![deny(unsafe_code)]

pub mod data_descriptors;
pub mod foundation_pods;
pub mod ir;
pub mod movement;
pub mod value_types;

pub use data_descriptors::*;
pub use foundation_pods::{DamagePayload, ModMapEntry, NavAgentParams};
pub use ir::*;
pub use movement::{DashPrograms, MovementScope, MovementState, PlayerMovementComponent};
pub use value_types::{EulerDegrees, Vec3Lit};
