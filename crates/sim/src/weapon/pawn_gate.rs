// Fire gating by the shooter's own movement state: a vault with `allowFire`
// off refuses starts on every fire path, judged at the start's own tick.
// See: context/lib/networking.md §Combat authority · context/lib/movement.md §4

use postretro_entities::components::inventory::{Inventory, WIELDABLE_SLOT_CAPACITY};
use postretro_entities::components::weapon::WeaponComponent;
use postretro_entities::components::wieldable_state::WieldableState;
use postretro_entities::{EntityId, EntityRegistry};
use postretro_foundation::{
    ActivationInput, ActivationLane, ActivationRelease, MovementState, PlayerMovementComponent,
    VaultParams,
};

use super::FireButtonState;
use super::execution::action_descriptor;

/// Whether `movement` refuses activation starts this tick: it starts the tick
/// vaulting with `allowFire` off and `elapsed_ms < duration_ms`. The gate opens
/// on the first tick that starts with `elapsed_ms >= duration_ms`, which is the
/// exit tick or, when the path takes one extra tick to reach its corner, the
/// tick before it. `elapsed_ms` is the physics vault clock, whole ticks
/// (`postretro_physics::movement::vault_clock`), and the path's tick count is
/// defined by this same comparison (`vault_path_ticks`), so the gate opens on
/// exactly the tick the path ends at every tick rate. Read at tick start on
/// every path.
pub fn vault_blocks_fire(movement: &PlayerMovementComponent) -> bool {
    let MovementState::Vault { elapsed_ms, .. } = movement.movement_state else {
        return false;
    };
    movement
        .vault
        .as_ref()
        .is_some_and(|vault| vault_clock_blocks_fire(vault, elapsed_ms))
}

/// [`vault_blocks_fire`]'s test on a vault clock reading.
fn vault_clock_blocks_fire(vault: &VaultParams, elapsed_ms: f32) -> bool {
    !vault.allow_fire && elapsed_ms < vault.duration_ms
}

/// [`vault_blocks_fire`] for `pawn`; a pawn without movement never blocks.
pub fn pawn_fire_blocked(registry: &EntityRegistry, pawn: EntityId) -> bool {
    registry
        .get_component::<PlayerMovementComponent>(pawn)
        .is_ok_and(vault_blocks_fire)
}

/// Whether a closed vault gate spares `weapon`'s execution: it is still
/// charging an action whose charge opts out of vault interruption
/// (`charge.interruptedBy.vault: false`). Every other execution is cancelled
/// as the gate closes, as at death. Read on the client and host-local paths;
/// the host-remote lane reads the same action through [`VaultSparedCharges`].
pub fn vault_spares_activation(weapon: &WeaponComponent) -> bool {
    let WieldableState::Charging(cursor) = weapon.state else {
        return false;
    };
    action_descriptor(weapon, cursor.token.lane)
        .is_some_and(|action| action.charge_survives_vault())
}

/// Hold the release of a charge a closed gate spares until the gate opens: a
/// charged shot is fire, so it cannot fire mid-vault. While `blocked`, a
/// release of the charging execution is removed from `input` and remembered;
/// pressing its trigger again forgets it, so the charge keeps charging. On the
/// first open tick a remembered release is emitted, stamped `tick`, if the
/// trigger is still up. Controllers call this after the gate's cancel and
/// before advancing the weapon, so the host-remote lane only ever receives a
/// release stamped on an open tick.
pub fn hold_release_over_gate(
    weapon: &mut WeaponComponent,
    input: &mut ActivationInput,
    primary: FireButtonState,
    secondary: FireButtonState,
    blocked: bool,
    tick: u32,
) {
    let charging = match weapon.state {
        WieldableState::Charging(cursor) => Some(cursor.token),
        _ => None,
    };
    // A held release belongs to the charge it was made on.
    if weapon.release_held_over_gate != charging {
        weapon.release_held_over_gate = None;
    }
    let Some(token) = charging else {
        return;
    };
    let trigger_held = match token.lane {
        ActivationLane::Primary => primary.active,
        ActivationLane::Secondary => secondary.active,
    };
    if blocked {
        if input.release.is_some_and(|release| release.token == token) {
            input.release = None;
            weapon.release_held_over_gate = Some(token);
        } else if trigger_held {
            weapon.release_held_over_gate = None;
        }
    } else if weapon.release_held_over_gate.take().is_some()
        && !trigger_held
        && input.release.is_none()
    {
        input.release = Some(ActivationRelease {
            token,
            release_tick: tick,
        });
    }
}

/// Which firing slots' actions charge through a vault: one bit per slot and
/// lane whose action's charge opts out of vault interruption. The host-remote
/// lane holds the shooter's last-recorded set to judge, as the client did,
/// whether a closed gate spares an execution still charging.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct VaultSparedCharges(u32);

impl VaultSparedCharges {
    /// The set for `pawn`'s inventory as it stands.
    pub fn of_pawn(registry: &EntityRegistry, pawn: EntityId) -> Self {
        let mut spared = Self::default();
        let Ok(inventory) = registry.get_component::<Inventory>(pawn) else {
            return spared;
        };
        for (slot, weapon) in inventory.wieldables.iter().enumerate() {
            let Some(weapon) = weapon else {
                continue;
            };
            let Ok(component) = registry.get_component::<WeaponComponent>(*weapon) else {
                continue;
            };
            for lane in [ActivationLane::Primary, ActivationLane::Secondary] {
                if action_descriptor(component, lane)
                    .is_some_and(|action| action.charge_survives_vault())
                {
                    spared.0 |= Self::bit(slot, lane);
                }
            }
        }
        spared
    }

    /// Mark `firing_slot`'s `lane` action as charging through a vault.
    pub fn with(mut self, firing_slot: u8, lane: ActivationLane) -> Self {
        if usize::from(firing_slot) < WIELDABLE_SLOT_CAPACITY {
            self.0 |= Self::bit(usize::from(firing_slot), lane);
        }
        self
    }

    /// Whether the action `firing_slot` holds on `lane` charges through a vault.
    pub fn spares(self, firing_slot: u8, lane: ActivationLane) -> bool {
        usize::from(firing_slot) < WIELDABLE_SLOT_CAPACITY
            && self.0 & Self::bit(usize::from(firing_slot), lane) != 0
    }

    fn bit(slot: usize, lane: ActivationLane) -> u32 {
        1 << (slot * 2 + lane as usize)
    }
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
    use postretro_physics::movement::{vault_clock, vault_path_ticks};

    fn button(pressed: bool, active: bool) -> FireButtonState {
        FireButtonState { pressed, active }
    }

    // Regression: at 400 ms and 60 Hz the gate stayed shut a tick past the path's end.
    #[test]
    fn the_vault_gate_opens_on_the_tick_the_path_ends_at_every_tick_rate() {
        let cases: [(f32, &[f32]); 5] = [
            (30.0, &[1000.0]),
            (60.0, &[400.0, 500.0, 320.0, 420.0]),
            (120.0, &[200.0, 250.0, 1000.0]),
            (144.0, &[250.0]),
            (240.0, &[100.0, 400.0, 500.0, 600.0]),
        ];
        for (hz, durations) in cases {
            let step_ms = (1.0 / hz) * 1000.0;
            for &duration_ms in durations {
                let vault = VaultParams {
                    max_height: 1.6,
                    swim_max_height: 0.9,
                    duration_ms,
                    from: postretro_foundation::VaultEntry::default(),
                    allow_fire: false,
                };
                let path_ticks = vault_path_ticks(duration_ms, step_ms);
                for ticks in 0..path_ticks {
                    assert!(
                        vault_clock_blocks_fire(&vault, vault_clock(ticks, step_ms)),
                        "{duration_ms} ms at {hz} Hz: shut after {ticks} of {path_ticks} path ticks"
                    );
                }
                assert!(
                    !vault_clock_blocks_fire(&vault, vault_clock(path_ticks, step_ms)),
                    "{duration_ms} ms at {hz} Hz: open once the {path_ticks} path ticks have run"
                );
                let allowed = VaultParams {
                    allow_fire: true,
                    ..vault
                };
                assert!(!vault_clock_blocks_fire(&allowed, 0.0));
            }
        }
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
