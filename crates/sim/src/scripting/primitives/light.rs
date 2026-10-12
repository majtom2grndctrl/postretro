// Compatibility wrapper for the relocated light primitive family.
// The implementation now lives with the lighting subsystem.

pub(crate) use postretro_lighting::script_primitives::*;
pub use postretro_lighting::script_primitives::{
    register_light_reaction_primitives, register_sequenced_light_primitives,
};
