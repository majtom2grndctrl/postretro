// Fire gating by the shooter's own movement state: a vault with `allowFire`
// off refuses starts on every fire path, judged at the start's own tick.
// See: context/lib/networking.md §Combat authority · context/lib/movement.md §4

use postretro_entities::components::weapon::WeaponComponent;
use postretro_entities::{EntityId, EntityRegistry};
use postretro_foundation::{MovementState, PlayerMovementComponent};

use super::FireButtonState;

/// Whether `movement` refuses activation starts this tick: it starts the tick
/// vaulting with `allowFire` off and `elapsed_ms < duration_ms`. The gate opens
/// on the first tick that starts with `elapsed_ms >= duration_ms`, which is the
/// exit tick or, when the path takes one extra tick to reach its corner, the
/// tick before it. Read at tick start on every path.
pub fn vault_blocks_fire(movement: &PlayerMovementComponent) -> bool {
    let MovementState::Vault { elapsed_ms, .. } = movement.movement_state else {
        return false;
    };
    movement
        .vault
        .as_ref()
        .is_some_and(|vault| !vault.allow_fire && elapsed_ms < vault.duration_ms)
}

/// [`vault_blocks_fire`] for `pawn`; a pawn without movement never blocks.
pub fn pawn_fire_blocked(registry: &EntityRegistry, pawn: EntityId) -> bool {
    registry
        .get_component::<PlayerMovementComponent>(pawn)
        .is_ok_and(vault_blocks_fire)
}

/// Hold a fresh press across a blocked tick instead of letting the weapon
/// consume it: while `blocked`, a press is remembered and hidden; on the first
/// open tick it is replayed if the button is still held. Releasing during the
/// block forgets it. Controllers call this before advancing the weapon.
pub fn hold_presses_over_gate(
    weapon: &mut WeaponComponent,
    primary: &mut FireButtonState,
    secondary: &mut FireButtonState,
    blocked: bool,
) {
    hold(&mut weapon.primary_press_held_over_gate, primary, blocked);
    hold(
        &mut weapon.secondary_press_held_over_gate,
        secondary,
        blocked,
    );
}

fn hold(held: &mut bool, button: &mut FireButtonState, blocked: bool) {
    if blocked {
        *held = (*held || button.pressed) && button.active;
        button.pressed = false;
    } else if std::mem::take(held) && button.active {
        button.pressed = true;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn button(pressed: bool, active: bool) -> FireButtonState {
        FireButtonState { pressed, active }
    }

    // A press during the block replays once the gate opens with the button
    // still held; a release during the block forgets it; an open tick passes
    // input through untouched.
    #[test]
    fn a_press_held_through_the_block_replays_once_the_gate_opens() {
        let mut held = false;
        let mut pressed = button(true, true);
        hold(&mut held, &mut pressed, true);
        assert!(
            !pressed.pressed && held,
            "hidden and remembered while blocked"
        );
        let mut still = button(false, true);
        hold(&mut held, &mut still, true);
        assert!(held, "a held button keeps the press");
        let mut open = button(false, true);
        hold(&mut held, &mut open, false);
        assert!(
            open.pressed && !held,
            "replayed once, on the first open tick"
        );
        let mut next = button(false, true);
        hold(&mut held, &mut next, false);
        assert!(!next.pressed, "never replayed twice");

        let mut held = false;
        hold(&mut held, &mut button(true, true), true);
        hold(&mut held, &mut button(false, false), true);
        let mut open = button(false, true);
        hold(&mut held, &mut open, false);
        assert!(
            !open.pressed,
            "a release during the block forgets the press"
        );

        let mut held = false;
        let mut free = button(true, true);
        hold(&mut held, &mut free, false);
        assert!(
            free.pressed && !held,
            "an open tick passes the press through"
        );
    }
}
