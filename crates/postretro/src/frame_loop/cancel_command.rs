//! The cancel-only input command a connected client sends on a zero-tick
//! frame.
//! See: context/lib/networking.md §Host input command queue · context/lib/input.md §2
//!
//! The command carries a real client tick, so the host resolves it as a real
//! command. Every edge and level stays neutral except `assist`: it carries the
//! current level, because the host reads a real command with `assist_held`
//! false as a release (step row 1), and only a real release may end a hold.

use postretro_foundation::ActivationToken;

use crate::camera::Camera;
use crate::input::{Action, ActionSnapshot};
use crate::{build_sim_command, sim};

/// Build the zero-tick cancel-only command from this frame's gameplay
/// snapshot (`None` while UI captures gameplay, which releases `assist` as
/// the tick path does).
pub(super) fn cancel_only_command(
    gameplay_snapshot: Option<&ActionSnapshot>,
    camera: &Camera,
    token: ActivationToken,
) -> sim::SimCommand {
    let assist_held =
        gameplay_snapshot.is_some_and(|snapshot| snapshot.button(Action::Assist).is_active());
    let mut command = build_sim_command(
        &ActionSnapshot::neutral(),
        camera,
        false,
        false,
        false,
        false,
        false,
        false,
        false,
        assist_held,
    );
    command.activation.cancel = Some(token);
    command
}

#[cfg(test)]
mod tests {
    use glam::Vec3;

    use super::*;
    use crate::input::ButtonState;

    fn token() -> ActivationToken {
        ActivationToken {
            start_tick: 7,
            lane: postretro_foundation::ActivationLane::Primary,
        }
    }

    /// A zero-tick cancel while `assist` is held must not read as a release on
    /// the host, which would reset progress mid-hold; its other inputs stay
    /// neutral.
    #[test]
    fn assist_cancel_only_command_carries_the_held_assist_level() {
        let camera = Camera::new(Vec3::ZERO, 0.0, 0.0);
        let token = token();
        for state in [ButtonState::Pressed, ButtonState::Held] {
            let snapshot = ActionSnapshot::with_button_state(Action::Assist, state);
            let command = cancel_only_command(Some(&snapshot), &camera, token);
            assert!(command.assist_held, "{state:?}");
            assert_eq!(command.activation.cancel, Some(token));
            assert!(!command.fire_button.active);
            assert!(!command.movement.use_pressed);
        }
        for snapshot in [
            Some(ActionSnapshot::with_button_state(
                Action::Assist,
                ButtonState::Released,
            )),
            Some(ActionSnapshot::neutral()),
            None,
        ] {
            let command = cancel_only_command(snapshot.as_ref(), &camera, token);
            assert!(!command.assist_held);
            assert_eq!(command.activation.cancel, Some(token));
        }
    }
}
