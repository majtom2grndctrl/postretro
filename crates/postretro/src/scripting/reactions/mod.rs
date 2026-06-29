// Tag-targeted reaction primitives invoked by `Primitive` reactions at
// dispatch time. Distinct from the per-step `SequencedPrimitiveRegistry`.
// See: context/lib/scripting.md §4 (Primitive Registration)

pub(crate) mod apply_damage;
pub(crate) mod registry;
pub(crate) mod set_animation_state;
pub(crate) mod set_emitter_rate;
pub(crate) mod set_fog_animation;
pub(crate) mod set_fog_density;
pub(crate) mod set_fog_edge_softness;
pub(crate) mod set_fog_falloff;
pub(crate) mod set_fog_glow;
pub(crate) mod set_fog_params;
pub(crate) mod set_spin_rate;
pub(crate) mod system_commands;

#[cfg(test)]
pub(crate) mod log_capture;

pub(crate) use postretro_scripting_core::reaction_registry::ReactionError;
