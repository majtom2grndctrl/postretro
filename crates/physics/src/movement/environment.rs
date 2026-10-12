// Movement's one environment resolve per tick, at the tick-start position.
// See: context/lib/movement.md §4 (Environment resolution)

use glam::Vec3;

use crate::movement::substrate::ResizeAnchor;
use postretro_collision::{BodyProbe, EnvironmentView, ResolvedEnvironment, resolve_environment};
use postretro_foundation::PlayerMovementComponent;

/// Resolve the pawn's environment at the tick-start `position` with its live
/// capsule. Called once per tick before the state intent, so a body the
/// substrate carries out of a volume still integrates that volume's gravity on
/// the crossing tick and the next tick resolves at its new position. Host
/// movement, prediction and reconcile replay all reach this through `tick`.
pub(super) fn resolve_at_tick_start(
    component: &PlayerMovementComponent,
    environment: EnvironmentView<'_>,
    position: Vec3,
) -> ResolvedEnvironment {
    resolve_environment(
        environment,
        &BodyProbe {
            position,
            half_height: component.capsule.half_height,
            radius: component.capsule.radius,
            eye_height: component.capsule.eye_height,
        },
    )
}

/// Resolve the environment the pawn would have this tick after standing up from
/// a crouched capsule about `anchor`: the standing capsule, centred where
/// `dispatch::stand_up_resize` with that anchor would put it. Swim entry tests
/// immersion against the capsule the pawn will swim in, so a crouch in water
/// shallower than the standing capsule's `enter_depth` never toggles between
/// crouching and swimming.
pub(super) fn resolve_standing(
    component: &PlayerMovementComponent,
    environment: EnvironmentView<'_>,
    position: Vec3,
    anchor: ResizeAnchor,
) -> ResolvedEnvironment {
    let standing_half_height = component.standing_half_height;
    let growth = standing_half_height - component.capsule.half_height;
    let delta = match anchor {
        ResizeAnchor::Feet => growth,
        ResizeAnchor::Head => -growth,
    };
    resolve_environment(
        environment,
        &BodyProbe {
            position: Vec3::new(position.x, position.y + delta, position.z),
            half_height: standing_half_height,
            radius: component.capsule.radius,
            eye_height: component.standing_eye_height,
        },
    )
}
