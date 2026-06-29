// Compatibility barrel for reaction primitive paths.
//
// Handler implementations live in their subsystem modules; these re-exports
// keep `crate::scripting::reactions::*` paths working while call sites migrate.
#![allow(unused_imports)]

pub(crate) use crate::fx::emitter_reactions::{set_emitter_rate, set_spin_rate};
pub(crate) use crate::fx::fog_reactions::{
    set_fog_animation, set_fog_density, set_fog_edge_softness, set_fog_falloff, set_fog_glow,
    set_fog_params,
};
pub(crate) use crate::health::reactions as apply_damage;
pub(crate) use crate::model::animation_reactions as set_animation_state;

pub(crate) mod registry;
pub(crate) mod system_commands;

#[cfg(test)]
pub(crate) mod log_capture;

pub(crate) use postretro_scripting_core::reaction_registry::ReactionError;
