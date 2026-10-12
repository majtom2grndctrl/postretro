// Movement's one environment resolve per tick, at the tick-start position.
// See: context/plans/in-progress/E24--water-and-environment-volumes/index.md

use glam::Vec3;

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
