// Per-frame systems that bridge the scripting surface to other engine
// subsystems. Each system is plain data-and-logic — no wgpu types, no input
// handles — the owning subsystem (renderer, audio, …) consumes the system's
// outputs through a narrow API.
//
// See: context/lib/scripting.md

pub(crate) mod emitter_bridge;
pub(crate) mod light_bridge;
pub(crate) mod particle_render;
pub(crate) mod particle_sim;

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
