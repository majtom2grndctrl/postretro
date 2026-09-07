//! State-transition camera impulse springs.

use postretro_foundation::{ImpulseChannels, ImpulseParams, MovementStateKind};

use super::{TimedMovementEdge, ViewFeelState};

/// Critically-damped, three-channel spring for one state key. Critical damping
/// is intentional: `tension` means a straightforward settle-speed knob, with
/// no hidden rebound for an author or player to infer.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct ImpulseSpring {
    pub(super) position: ImpulseChannels,
    pub(super) velocity: ImpulseChannels,
}

impl ImpulseSpring {
    pub(super) const ZERO: Self = Self {
        position: ImpulseChannels {
            fov: 0.0,
            pitch: 0.0,
            roll: 0.0,
        },
        velocity: ImpulseChannels {
            fov: 0.0,
            pitch: 0.0,
            roll: 0.0,
        },
    };
}

/// Advance the springs that existed before this render frame, then consume the
/// ordered tick edges with each tick's explicit age. This keeps a fresh edge at
/// its authored peak on its first presented frame instead of aging it by both
/// the render delta and its backlog age. The final ceiling applies only to
/// presented output, keeping the underlying linear integrators frame-rate
/// independent.
pub(super) fn evaluate(
    params: &ImpulseParams,
    edges: &[TimedMovementEdge],
    state: &mut ViewFeelState,
    frame_dt: f32,
) -> ImpulseChannels {
    for (index, spring) in state.impulse_springs.iter_mut().enumerate() {
        let tension = state_tension(params, index);
        advance_critical(
            &mut spring.position,
            &mut spring.velocity,
            tension,
            frame_dt,
        );
    }

    for timed in edges {
        apply_edge(params, state, timed.edge.from, true, timed.age);
        apply_edge(params, state, timed.edge.to, false, timed.age);
    }

    let mut summed = zero_channels();
    for spring in &state.impulse_springs {
        add_assign(&mut summed, spring.position);
    }
    clamp_channels(summed, params.max)
}

fn apply_edge(
    params: &ImpulseParams,
    state: &mut ViewFeelState,
    state_key: MovementStateKind,
    is_exit: bool,
    age: f32,
) {
    let Some(kick) = state_params(params, state_key)
        .and_then(|entry| if is_exit { entry.exit } else { entry.enter })
    else {
        return;
    };

    let index = state_index(state_key);
    let tension = state_tension(params, index);
    let mut aged_position = kick;
    let mut aged_velocity = zero_channels();
    advance_critical(
        &mut aged_position,
        &mut aged_velocity,
        tension,
        age.max(0.0),
    );
    add_assign(&mut state.impulse_springs[index].position, aged_position);
    add_assign(&mut state.impulse_springs[index].velocity, aged_velocity);
}

fn state_tension(params: &ImpulseParams, index: usize) -> f32 {
    state_params_by_index(params, index)
        .and_then(|entry| entry.tension)
        .unwrap_or(params.tension)
}

fn state_params(
    params: &ImpulseParams,
    state: MovementStateKind,
) -> Option<&postretro_foundation::ImpulseStateParams> {
    state_params_by_index(params, state_index(state))
}

fn state_params_by_index(
    params: &ImpulseParams,
    index: usize,
) -> Option<&postretro_foundation::ImpulseStateParams> {
    match index {
        0 => params.states.normal.as_ref(),
        1 => params.states.dash.as_ref(),
        2 => params.states.crouch.as_ref(),
        3 => params.states.slide.as_ref(),
        _ => unreachable!("movement state index is closed"),
    }
}

fn state_index(state: MovementStateKind) -> usize {
    match state {
        MovementStateKind::Normal => 0,
        MovementStateKind::Dash => 1,
        MovementStateKind::Crouch => 2,
        MovementStateKind::Slide => 3,
    }
}

/// Exact critical-damping update for all channels toward a zero target.
fn advance_critical(
    position: &mut ImpulseChannels,
    velocity: &mut ImpulseChannels,
    tension: f32,
    dt: f32,
) {
    if dt <= 0.0 || tension <= 0.0 {
        return;
    }
    advance_channel(&mut position.fov, &mut velocity.fov, tension, dt);
    advance_channel(&mut position.pitch, &mut velocity.pitch, tension, dt);
    advance_channel(&mut position.roll, &mut velocity.roll, tension, dt);
}

fn advance_channel(position: &mut f32, velocity: &mut f32, tension: f32, dt: f32) {
    if !position.is_finite() || !velocity.is_finite() || !tension.is_finite() || !dt.is_finite() {
        *position = 0.0;
        *velocity = 0.0;
        return;
    }

    // Use f64 intermediates so a long render pause cannot turn the exact
    // solution's exponentially-decayed terms into `0 * infinity` in f32.
    let x0 = f64::from(*position);
    let v0 = f64::from(*velocity);
    let tension = f64::from(tension);
    let dt = f64::from(dt);
    let exp = (-tension * dt).exp();
    let common = v0 + tension * x0;
    let next_position = exp * (x0 + common * dt);
    let next_velocity = exp * (v0 - tension * common * dt);
    if next_position.is_finite()
        && next_velocity.is_finite()
        && next_position.abs() <= f64::from(f32::MAX)
        && next_velocity.abs() <= f64::from(f32::MAX)
    {
        *position = next_position as f32;
        *velocity = next_velocity as f32;
    } else {
        *position = 0.0;
        *velocity = 0.0;
    }
}

fn zero_channels() -> ImpulseChannels {
    ImpulseChannels {
        fov: 0.0,
        pitch: 0.0,
        roll: 0.0,
    }
}

fn add_assign(sum: &mut ImpulseChannels, add: ImpulseChannels) {
    sum.fov = finite_sum(sum.fov, add.fov);
    sum.pitch = finite_sum(sum.pitch, add.pitch);
    sum.roll = finite_sum(sum.roll, add.roll);
}

fn finite_sum(left: f32, right: f32) -> f32 {
    let sum = f64::from(left) + f64::from(right);
    if sum.is_finite() {
        sum.clamp(f64::from(f32::MIN), f64::from(f32::MAX)) as f32
    } else {
        0.0
    }
}

fn clamp_channels(value: ImpulseChannels, max: ImpulseChannels) -> ImpulseChannels {
    ImpulseChannels {
        fov: value.fov.clamp(-max.fov, max.fov),
        pitch: value.pitch.clamp(-max.pitch, max.pitch),
        roll: value.roll.clamp(-max.roll, max.roll),
    }
}
