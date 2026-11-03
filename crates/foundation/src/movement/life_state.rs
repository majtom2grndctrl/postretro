//! Authored player capability profiles and their replay-safe component stamps.
//! See: context/lib/scripting.md §12 and context/lib/movement.md.

use serde::{Deserialize, Serialize};

use super::PlayerMovementComponent;
use crate::NumberOrIr;

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

/// Assist reach when `assistable.radius` is omitted, in metres.
pub const ASSIST_DEFAULT_RADIUS_M: f32 = 1.5;
/// Assist hold when `assistable.holdMs` is omitted, in milliseconds.
pub const ASSIST_DEFAULT_HOLD_MS: f32 = 3000.0;
/// Largest literal radius a profile may author; evaluated radii clamp to it.
pub const ASSIST_MAX_RADIUS_M: f32 = 16.0;
/// Largest literal hold a profile may author; evaluated holds clamp to it.
pub const ASSIST_MAX_HOLD_MS: f32 = 60000.0;

fn default_assist_radius() -> NumberOrIr {
    NumberOrIr::Literal(ASSIST_DEFAULT_RADIUS_M)
}

fn default_assist_hold_ms() -> NumberOrIr {
    NumberOrIr::Literal(ASSIST_DEFAULT_HOLD_MS)
}

/// A teammate may assist a pawn in this state. Each field is a literal or a
/// Number expression over the helper and the target, evaluated on the host
/// per pair; an omitted field is filled with its default at parse.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Assistable {
    #[serde(default = "default_assist_radius")]
    pub radius: NumberOrIr,
    #[serde(default = "default_assist_hold_ms")]
    pub hold_ms: NumberOrIr,
}

impl Default for Assistable {
    fn default() -> Self {
        Self {
            radius: default_assist_radius(),
            hold_ms: default_assist_hold_ms(),
        }
    }
}

/// Whether a literal `radius` is authorable: finite, above 0, at most the cap.
pub fn assist_radius_literal_valid(radius: f32) -> bool {
    radius.is_finite() && radius > 0.0 && radius <= ASSIST_MAX_RADIUS_M
}

/// Whether a literal `holdMs` is authorable: finite, from 0, at most the cap.
pub fn assist_hold_ms_literal_valid(hold_ms: f32) -> bool {
    hold_ms.is_finite() && (0.0..=ASSIST_MAX_HOLD_MS).contains(&hold_ms)
}

pub const ASSIST_RADIUS_LITERAL_RULE: &str = "must be a finite number in (0, 16]";
pub const ASSIST_HOLD_MS_LITERAL_RULE: &str = "must be a finite number in [0, 60000]";

impl Assistable {
    /// The first literal field outside its authorable range, as its camelCase
    /// key and rule. Expression fields are not checked here.
    pub fn literal_fault(&self) -> Option<(&'static str, &'static str)> {
        if let NumberOrIr::Literal(radius) = self.radius
            && !assist_radius_literal_valid(radius)
        {
            return Some(("radius", ASSIST_RADIUS_LITERAL_RULE));
        }
        if let NumberOrIr::Literal(hold_ms) = self.hold_ms
            && !assist_hold_ms_literal_valid(hold_ms)
        {
            return Some(("holdMs", ASSIST_HOLD_MS_LITERAL_RULE));
        }
        None
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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub assistable: Option<Assistable>,
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
            assistable: None,
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
    pawn.assistable = profile.is_some_and(|p| p.assistable.is_some());
    if let Some(profile) = profile {
        pawn.vault_barred = !profile.movement.abilities;
        // An edgeless early end: no `vault_ended`, which only an abort, the
        // exit and a swim pre-emption emit. Forced crouch with abilities on
        // breaks the vault on the next movement tick instead, also edgeless.
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
    pawn.life_state_entry = pawn.life_state_entry.wrapping_add(1);
    restamp_life_state(pawn, Some(profile));
}

pub fn clear_life_state(pawn: &mut PlayerMovementComponent, revision: u8) {
    pawn.life_state = UNDECLARED_LIFE_STATE;
    pawn.life_state_table_revision = revision;
    pawn.life_state_elapsed_ticks = 0;
    pawn.life_state_entry = pawn.life_state_entry.wrapping_add(1);
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
    to.assistable = from.assistable;
    to.vault_barred = from.vault_barred;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::IrNode;

    #[test]
    fn assistable_omitted_fields_take_their_defaults_and_unknown_keys_fail() {
        let empty: Assistable = serde_json::from_str("{}").unwrap();
        assert_eq!(empty, Assistable::default());
        assert_eq!(empty.radius, NumberOrIr::Literal(ASSIST_DEFAULT_RADIUS_M));
        assert_eq!(empty.hold_ms, NumberOrIr::Literal(ASSIST_DEFAULT_HOLD_MS));
        let profile: LifeStateProfile =
            serde_json::from_str(r#"{"assistable":{"holdMs":{"op":"const","value":500}}}"#)
                .unwrap();
        let assistable = profile.assistable.unwrap();
        assert_eq!(assistable.radius, NumberOrIr::Literal(1.5));
        assert!(matches!(
            assistable.hold_ms,
            NumberOrIr::Ir(IrNode::Const { .. })
        ));
        assert!(serde_json::from_str::<Assistable>(r#"{"range":2}"#).is_err());
    }

    #[test]
    fn assistable_literal_fault_names_the_first_out_of_range_literal() {
        let at = |radius: f32, hold_ms: f32| Assistable {
            radius: NumberOrIr::Literal(radius),
            hold_ms: NumberOrIr::Literal(hold_ms),
        };
        for valid in [at(16.0, 0.0), at(0.01, 60000.0), at(1.5, -0.0)] {
            assert_eq!(valid.literal_fault(), None, "{valid:?}");
        }
        for (radius, hold_ms, field) in [
            (0.0, 3000.0, "radius"),
            (16.01, 3000.0, "radius"),
            (f32::NAN, 3000.0, "radius"),
            (f32::INFINITY, 3000.0, "radius"),
            (1.5, -1.0, "holdMs"),
            (1.5, 60000.5, "holdMs"),
            (1.5, f32::NAN, "holdMs"),
        ] {
            assert_eq!(
                at(radius, hold_ms).literal_fault().map(|f| f.0),
                Some(field)
            );
        }
        let expression = Assistable {
            radius: NumberOrIr::Ir(IrNode::Input {
                name: "x".into(),
                owner: None,
            }),
            hold_ms: NumberOrIr::Literal(3000.0),
        };
        assert_eq!(expression.literal_fault(), None);
    }
}
