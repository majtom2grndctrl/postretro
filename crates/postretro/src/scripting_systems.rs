//! Binary-side frame bridges plus the fixed-tick scripting handlers in sim.

pub(crate) use postretro_sim::scripting_systems::{
    ai, flash_decay, hit_zones, mesh_anim, particle_sim, reaction_scheduler, shake_decay,
    slot_accumulators, system_reactions, trigger_volume_bridge, ui_proxy, vignette_decay,
};

pub(crate) fn eval_curve(curve: &[f32], t: f32) -> f32 {
    postretro_sim::scripting_systems::eval_curve(curve, t)
}

#[path = "scripting/frame_systems/attachments.rs"]
pub(crate) mod attachments;
#[path = "scripting/frame_systems/emitter_bridge.rs"]
pub(crate) mod emitter_bridge;
#[path = "scripting/frame_systems/fog_volume_bridge.rs"]
pub(crate) mod fog_volume_bridge;
#[path = "scripting/frame_systems/input_mode.rs"]
pub(crate) mod input_mode;
#[path = "scripting/frame_systems/light_bridge.rs"]
pub(crate) mod light_bridge;
#[path = "scripting/frame_systems/mesh_render.rs"]
pub(crate) mod mesh_render;
#[path = "scripting/frame_systems/particle_render.rs"]
pub(crate) mod particle_render;
#[path = "scripting/frame_systems/presentation_cells.rs"]
pub(crate) mod presentation_cells;
