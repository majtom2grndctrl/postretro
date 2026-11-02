//! Replay-local capability inputs; no registry or descriptor lookup in the tick.
use glam::Vec3;
use postretro_foundation::{
    Capability, LifeStatePosture, MovementState, PlayerMovementComponent, player_capability,
};

use super::substrate::{ResizeAnchor, anchored_standup_clearance_probe, resize_capsule};
use super::{MovementEvents, MovementInput, MovementStateEdge};
use crate::collision::moving::CombinedCollisionWorld;

pub(super) fn abilities(component: &PlayerMovementComponent) -> bool {
    player_capability(component, Capability::MovementAbilities).unwrap_or(true)
}

pub(super) fn forced_crouch(component: &PlayerMovementComponent) -> bool {
    component.life_posture == LifeStatePosture::Crouch
}

/// Suppress refused ability inputs for this tick and clear the jump buffer.
/// The persistent crouch-toggle preference belongs to the input owner.
pub(super) fn input(
    component: &mut PlayerMovementComponent,
    input: &MovementInput,
) -> MovementInput {
    let mut input = input.clone();
    if !abilities(component) {
        input.jump_pressed = false;
        input.dash_pressed = false;
        input.crouch_intent = matches!(
            component.movement_state,
            MovementState::Crouching { .. } | MovementState::Sliding { .. }
        );
        component.jump_buffer_timer_ms = 0.0;
    }
    input
}

/// Forced posture wins over a banked dash/slide and vault. Swim remains the
/// medium: it holds a crouched capsule instead of turning posture into a dive
/// input or making a submerged pawn stop swimming.
pub(super) fn posture(
    component: &mut PlayerMovementComponent,
    collision: &CombinedCollisionWorld<'_>,
    position: &mut Vec3,
    dt: f32,
    events: &mut MovementEvents,
) -> bool {
    let forced = forced_crouch(component);
    let from = component.movement_state.kind();
    if forced {
        match component.movement_state {
            MovementState::Dash { .. } => component.movement_state = MovementState::Normal,
            MovementState::Sliding { eye_current, .. } => {
                component.movement_state = MovementState::Crouching { eye_current };
            }
            MovementState::Vault { .. } => {
                component.break_off_vault();
                // The helper expects between-tick total velocity. This tick
                // already peeled both protected layers; discard the path base
                // here and let the normal recomposition add those layers once.
                component.velocity = Vec3::ZERO;
            }
            _ => {}
        }
    }
    if forced
        && matches!(component.movement_state, MovementState::Normal)
        && let Some(crouch) = component.crouch.as_ref()
    {
        let (height, eye) = (crouch.half_height, crouch.eye_height);
        let anchor = if component.is_grounded() {
            ResizeAnchor::Feet
        } else {
            ResizeAnchor::Head
        };
        if anchored_standup_clearance_probe(
            component,
            collision,
            *position,
            component.capsule.half_height,
            height,
            anchor,
        ) {
            let eye_current = component.capsule.eye_height;
            position.y += resize_capsule(component, height, eye, anchor);
            component.capsule.eye_height = eye_current;
            component.movement_state = MovementState::Crouching { eye_current };
        }
    }
    if component.movement_state.kind() != from {
        events.state_edges.push(MovementStateEdge {
            from,
            to: component.movement_state.kind(),
        });
    }
    if !matches!(component.movement_state, MovementState::Swim { .. }) {
        return component.movement_state.kind() != from;
    }
    let Some(crouch) = component.crouch.as_ref() else {
        return false;
    };
    let (height, eye, rate) = if forced {
        (
            crouch.half_height,
            crouch.eye_height,
            crouch.transition_rate,
        )
    } else {
        (
            component.standing_half_height,
            component.standing_eye_height,
            crouch.transition_rate,
        )
    };
    let anchor = if component.is_grounded() {
        ResizeAnchor::Feet
    } else {
        ResizeAnchor::Head
    };
    let changed = (component.capsule.half_height - height).abs() > f32::EPSILON;
    if changed {
        if !anchored_standup_clearance_probe(
            component,
            collision,
            *position,
            component.capsule.half_height,
            height,
            anchor,
        ) {
            return false;
        }
        let current_eye = component.capsule.eye_height;
        position.y += resize_capsule(component, height, eye, anchor);
        component.capsule.eye_height = current_eye;
    }
    component.capsule.eye_height +=
        (eye - component.capsule.eye_height) * (1.0 - (-rate * dt).exp());
    changed
}
