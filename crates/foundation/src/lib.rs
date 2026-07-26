//! VM-free foundation data and evaluation substrate for Postretro.
//! See: context/lib/scripting.md §12.

#![deny(unsafe_code)]

pub mod brain;
pub mod data_descriptors;
pub mod foundation_pods;
pub mod ir;
pub mod movement;
pub mod pose;
pub mod value_types;

pub use brain::{
    BRAIN_ACQUISITION_DUE_INPUT, BRAIN_ATTACK_COOLDOWN_MS_INPUT, BRAIN_HAS_TARGET_INPUT,
    BRAIN_HEALTH_INPUT, BRAIN_INPUT_PREFIX, BRAIN_INPUTS, BRAIN_MAX_HEALTH_INPUT,
    BRAIN_NO_TARGET_DISTANCE, BRAIN_TARGET_DISTANCE_INPUT, BRAIN_TIME_IN_STATE_MS_INPUT,
    BrainInputRef, BrainValidationScope, bind_brain_guard, resolve_brain_input,
};
pub use data_descriptors::*;
pub use foundation_pods::{DamagePayload, ModMapEntry, NavAgentParams};
pub use ir::*;
pub use movement::{
    DashPrograms, GroundRef, MovementScope, MovementState, PlayerMovementComponent,
};
pub use pose::{PoseInputs, WALKABLE_SURFACE_MIN_UP_DOT};
pub use value_types::{EulerDegrees, Vec3Lit};
