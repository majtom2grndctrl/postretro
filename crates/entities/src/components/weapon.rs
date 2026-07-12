// Weapon component: descriptor-authored stats plus live wieldable state.
// Spawn and hot reload refresh descriptor stats; firing preserves and mutates
// per-instance cooldown here.
//
// See: context/lib/entity_model.md §4 (descriptor-owned tuning params)

use serde::{Deserialize, Serialize};
#[cfg(debug_assertions)]
use std::sync::Once;

use crate::data_descriptors::{FireMode, ResolutionMode, WeaponDescriptor};

pub const UNKNOWN_WEAPON_CREDIT_SOURCE: &str = "weapon.unknown";

#[cfg(debug_assertions)]
static WARNED_UNKNOWN_CREDIT_SOURCE: Once = Once::new();

#[derive(Debug, Clone, PartialEq)]
pub struct EffectiveStats {
    pub damage: f32,
    pub range: f32,
    pub cooldown_ms: f32,
    pub fire_mode: FireMode,
    pub resolution: ResolutionMode,
    pub credit_source: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WeaponComponent {
    pub damage: f32,
    pub range: f32,
    pub cooldown_ms: f32,
    pub fire_mode: FireMode,
    pub resolution: ResolutionMode,
    pub cooldown_remaining_ms: f32,
    #[serde(default)]
    pub shoot_press_consumed: bool,
    #[serde(default = "default_credit_source")]
    pub credit_source: String,
}

impl WeaponComponent {
    pub fn from_descriptor(desc: &WeaponDescriptor) -> Self {
        Self::from_descriptor_with_canonical(desc, None)
    }

    pub fn from_descriptor_with_canonical(
        desc: &WeaponDescriptor,
        canonical_name: Option<&str>,
    ) -> Self {
        Self {
            damage: desc.damage,
            range: desc.range,
            cooldown_ms: desc.cooldown_ms,
            fire_mode: desc.fire_mode,
            resolution: desc.resolution,
            cooldown_remaining_ms: 0.0,
            shoot_press_consumed: false,
            credit_source: resolve_credit_source(desc, canonical_name),
        }
    }

    pub fn effective(&self) -> EffectiveStats {
        EffectiveStats {
            damage: self.damage,
            range: self.range,
            cooldown_ms: self.cooldown_ms,
            fire_mode: self.fire_mode,
            resolution: self.resolution,
            credit_source: self.credit_source.clone(),
        }
    }

    pub fn refresh_from_descriptor(&mut self, desc: &WeaponDescriptor) {
        self.damage = desc.damage;
        self.range = desc.range;
        self.cooldown_ms = desc.cooldown_ms;
        self.fire_mode = desc.fire_mode;
        self.resolution = desc.resolution;
        if let Some(credit_source) = desc.credit_source.as_ref() {
            self.credit_source = credit_source.clone();
        }
        // `cooldown_remaining_ms` and `shoot_press_consumed` are live instance
        // state. Hot reload changes authored tuning, not the current trigger
        // edge or whether this instance is mid-cooldown. An absent
        // `creditSource` also keeps the already-resolved spawn-time default so
        // canonical defaults do not regress to `weapon.unknown` on reload.
    }
}

fn resolve_credit_source(desc: &WeaponDescriptor, canonical_name: Option<&str>) -> String {
    if let Some(credit_source) = desc.credit_source.as_ref() {
        return credit_source.clone();
    }
    if let Some(canonical_name) = canonical_name {
        return canonical_name.to_string();
    }
    warn_unknown_credit_source_once();
    UNKNOWN_WEAPON_CREDIT_SOURCE.to_string()
}

fn default_credit_source() -> String {
    UNKNOWN_WEAPON_CREDIT_SOURCE.to_string()
}

#[cfg(debug_assertions)]
fn warn_unknown_credit_source_once() {
    WARNED_UNKNOWN_CREDIT_SOURCE.call_once(|| {
        log::warn!(
            "weapon descriptor materialized without authored creditSource or canonical name; using {UNKNOWN_WEAPON_CREDIT_SOURCE}"
        );
    });
}

#[cfg(not(debug_assertions))]
fn warn_unknown_credit_source_once() {}

#[cfg(test)]
mod tests {
    use super::*;

    fn descriptor(damage: f32, range: f32, cooldown_ms: f32) -> WeaponDescriptor {
        WeaponDescriptor {
            damage,
            range,
            cooldown_ms,
            fire_mode: FireMode::Semi,
            resolution: ResolutionMode::Hitscan,
            credit_source: None,
            resource: None,
        }
    }

    #[test]
    fn refresh_from_descriptor_updates_stats_and_preserves_live_state() {
        let mut component = WeaponComponent::from_descriptor_with_canonical(
            &descriptor(10.0, 20.0, 100.0),
            Some("reference_pistol"),
        );
        component.cooldown_remaining_ms = 42.0;
        component.shoot_press_consumed = true;

        component.refresh_from_descriptor(&descriptor(25.0, 80.0, 250.0));

        assert_eq!(component.damage, 25.0);
        assert_eq!(component.range, 80.0);
        assert_eq!(component.cooldown_ms, 250.0);
        assert_eq!(component.cooldown_remaining_ms, 42.0);
        assert!(component.shoot_press_consumed);
        assert_eq!(component.credit_source, "reference_pistol");
    }

    #[test]
    fn from_descriptor_prefers_authored_credit_source_over_canonical_name() {
        let mut descriptor = descriptor(10.0, 20.0, 100.0);
        descriptor.credit_source = Some("plasma.primary".to_string());

        let component =
            WeaponComponent::from_descriptor_with_canonical(&descriptor, Some("plasma_rifle"));

        assert_eq!(component.credit_source, "plasma.primary");
        assert_eq!(component.effective().credit_source, "plasma.primary");
    }

    #[test]
    fn from_descriptor_uses_canonical_name_when_credit_source_is_absent() {
        let component = WeaponComponent::from_descriptor_with_canonical(
            &descriptor(10.0, 20.0, 100.0),
            Some("reference_pistol"),
        );

        assert_eq!(component.credit_source, "reference_pistol");
        assert_eq!(component.effective().credit_source, "reference_pistol");
    }

    #[test]
    fn from_descriptor_uses_unknown_fallback_without_authored_or_canonical_source() {
        let component = WeaponComponent::from_descriptor(&descriptor(10.0, 20.0, 100.0));

        assert_eq!(component.credit_source, UNKNOWN_WEAPON_CREDIT_SOURCE);
    }

    #[test]
    fn refresh_from_descriptor_updates_authored_credit_source_when_present() {
        let mut component = WeaponComponent::from_descriptor_with_canonical(
            &descriptor(10.0, 20.0, 100.0),
            Some("reference_pistol"),
        );
        let mut reloaded = descriptor(25.0, 80.0, 250.0);
        reloaded.credit_source = Some("pistol.alt".to_string());

        component.refresh_from_descriptor(&reloaded);

        assert_eq!(component.credit_source, "pistol.alt");
    }
}
