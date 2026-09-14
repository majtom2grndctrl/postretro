// Compatibility wrapper for the relocated light primitive family.
// The implementation now lives with the lighting subsystem.

pub use postretro_lighting::script_primitives::register_sequenced_light_primitives;
pub(crate) use postretro_lighting::script_primitives::*;
