// Per-frame systems that bridge the scripting surface to other engine
// subsystems. Each system is plain data-and-logic — no wgpu types, no input
// handles — the owning subsystem (renderer, audio, …) consumes the system's
// outputs through a narrow API.
//
// See: context/lib/scripting.md

pub(crate) mod ai;
#[path = "../frame_systems/attachments.rs"]
pub(crate) mod attachments;
#[path = "../frame_systems/emitter_bridge.rs"]
pub(crate) mod emitter_bridge;
pub(crate) mod flash_decay;
#[path = "../frame_systems/fog_volume_bridge.rs"]
pub(crate) mod fog_volume_bridge;
pub(crate) mod health;
pub(crate) mod hit_zones;
#[path = "../frame_systems/input_mode.rs"]
pub(crate) mod input_mode;
#[path = "../frame_systems/light_bridge.rs"]
pub(crate) mod light_bridge;
pub(crate) mod mesh_anim;
#[path = "../frame_systems/mesh_render.rs"]
pub(crate) mod mesh_render;
#[path = "../frame_systems/particle_render.rs"]
pub(crate) mod particle_render;
pub(crate) mod particle_sim;
#[path = "../frame_systems/presentation_cells.rs"]
pub(crate) mod presentation_cells;
pub(crate) mod reaction_scheduler;
#[cfg(test)]
mod reaction_scheduler_ordering_tests;
pub(crate) mod shake_decay;
pub(crate) mod slot_accumulators;
pub(crate) mod system_reactions;
pub(crate) mod trigger_volume_bridge;
pub(crate) mod ui_proxy;
pub(crate) mod vignette_decay;

/// Linear-interpolated curve evaluation over `[0, 1]`. Shared by the emitter
/// bridge (spin animation) and the particle sim (size/opacity curves). Empty
/// curve defaults to `1.0` — unreachable from script, reserved for Rust-side
/// defaulting.
pub(crate) fn eval_curve(curve: &[f32], t: f32) -> f32 {
    if curve.is_empty() {
        return 1.0;
    }
    if curve.len() == 1 {
        return curve[0];
    }
    let s = t * (curve.len() - 1) as f32;
    let i = s.floor() as usize;
    let frac = s - i as f32;
    let a = curve[i];
    let b = curve[(i + 1).min(curve.len() - 1)];
    a * (1.0 - frac) + b * frac
}
