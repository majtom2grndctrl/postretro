//! Authored player capability profiles and their replay-safe component stamps.
//! See: context/lib/scripting.md §12 and context/lib/movement.md.

use serde::{Deserialize, Serialize};

use super::PlayerMovementComponent;

pub const UNDECLARED_LIFE_STATE: u8 = u8::MAX;

pub const fn undeclared_life_state() -> u8 {
    UNDECLARED_LIFE_STATE
}

pub const fn unit_speed_scale() -> f32 {
    1.0
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum LifeStatePosture {
    #[default]
    Stand,
    Crouch,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase", deny_unknown_fields)]
pub struct LifeStateMovementProfile {
    pub speed_scale: f32,
    pub abilities: bool,
    pub posture: LifeStatePosture,
}

impl Default for LifeStateMovementProfile {
    fn default() -> Self {
        Self {
            speed_scale: 1.0,
            abilities: true,
            posture: LifeStatePosture::Stand,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase", deny_unknown_fields)]
pub struct LifeStateProfile {
    pub movement: LifeStateMovementProfile,
    pub look: bool,
    pub fire: bool,
    pub switch_weapon: bool,
    pub reload: bool,
    #[serde(rename = "use")]
    pub use_enabled: bool,
    pub touch: bool,
    pub occupancy: bool,
    pub damageable: bool,
}

impl Default for LifeStateProfile {
    fn default() -> Self {
        Self {
            movement: Default::default(),
            look: true,
            fire: true,
            switch_weapon: true,
            reload: true,
            use_enabled: true,
            touch: true,
            occupancy: true,
            damageable: true,
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub enum Capability {
    MovementAbilities,
    Look,
    Fire,
    SwitchWeapon,
    Reload,
    Use,
    Touch,
    Occupancy,
    Damageable,
}

/// None preserves each undeclared pawn consumer's existing liveness rule.
pub fn player_capability(pawn: &PlayerMovementComponent, capability: Capability) -> Option<bool> {
    if pawn.life_state == UNDECLARED_LIFE_STATE {
        return None;
    }
    Some(match capability {
        Capability::MovementAbilities => pawn.life_movement_abilities,
        Capability::Look => pawn.life_look,
        Capability::Fire => pawn.life_fire,
        Capability::SwitchWeapon => pawn.life_switch_weapon,
        Capability::Reload => pawn.life_reload,
        Capability::Use => pawn.life_use,
        Capability::Touch => pawn.life_touch,
        Capability::Occupancy => pawn.life_occupancy,
        Capability::Damageable => pawn.life_damageable,
    })
}

/// Refresh derived capabilities after every state write, including wire merge.
pub fn restamp_life_state(pawn: &mut PlayerMovementComponent, profile: Option<&LifeStateProfile>) {
    pawn.life_movement_abilities = profile.is_some_and(|p| p.movement.abilities);
    pawn.life_look = profile.is_some_and(|p| p.look);
    pawn.life_fire = profile.is_some_and(|p| p.fire);
    pawn.life_switch_weapon = profile.is_some_and(|p| p.switch_weapon);
    pawn.life_reload = profile.is_some_and(|p| p.reload);
    pawn.life_use = profile.is_some_and(|p| p.use_enabled);
    pawn.life_touch = profile.is_some_and(|p| p.touch);
    pawn.life_occupancy = profile.is_some_and(|p| p.occupancy);
    pawn.life_damageable = profile.is_some_and(|p| p.damageable);
    pawn.life_speed_scale = profile.map_or(1.0, |p| p.movement.speed_scale);
    pawn.life_posture = profile.map_or(LifeStatePosture::Stand, |p| p.movement.posture);
    if let Some(profile) = profile {
        pawn.vault_barred = !profile.movement.abilities;
        if pawn.vault_barred {
            pawn.break_off_vault();
        }
    }
    // Clearing an undeclared pawn must retain its HP-derived replay vault bar.
}

pub fn enter_life_state(
    pawn: &mut PlayerMovementComponent,
    id: u8,
    profile: &LifeStateProfile,
    revision: u8,
) {
    pawn.life_state = id;
    pawn.life_state_table_revision = revision;
    pawn.life_state_elapsed_ticks = 0;
    restamp_life_state(pawn, Some(profile));
}

pub fn clear_life_state(pawn: &mut PlayerMovementComponent, revision: u8) {
    pawn.life_state = UNDECLARED_LIFE_STATE;
    pawn.life_state_table_revision = revision;
    pawn.life_state_elapsed_ticks = 0;
    restamp_life_state(pawn, None);
}

/// Copy derived stamps when a table is temporarily unavailable during rebuild.
pub fn carry_life_state_stamps(from: &PlayerMovementComponent, to: &mut PlayerMovementComponent) {
    to.life_movement_abilities = from.life_movement_abilities;
    to.life_look = from.life_look;
    to.life_fire = from.life_fire;
    to.life_switch_weapon = from.life_switch_weapon;
    to.life_reload = from.life_reload;
    to.life_use = from.life_use;
    to.life_touch = from.life_touch;
    to.life_occupancy = from.life_occupancy;
    to.life_damageable = from.life_damageable;
    to.life_speed_scale = from.life_speed_scale;
    to.life_posture = from.life_posture;
    to.vault_barred = from.vault_barred;
}
