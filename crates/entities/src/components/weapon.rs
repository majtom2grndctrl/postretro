// Weapon component: descriptor-authored stats plus live wieldable state.
// Spawn and hot reload refresh descriptor stats; firing preserves and mutates
// per-instance cooldown here.
//
// See: context/lib/entity_model.md §4 (descriptor-owned tuning params)

use serde::{Deserialize, Serialize};

use crate::data_descriptors::{FireMode, ResolutionMode, WeaponDescriptor};

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EffectiveStats {
    pub damage: f32,
    pub range: f32,
    pub cooldown_ms: f32,
    pub fire_mode: FireMode,
    pub resolution: ResolutionMode,
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
}

impl WeaponComponent {
    pub fn from_descriptor(desc: &WeaponDescriptor) -> Self {
        Self {
            damage: desc.damage,
            range: desc.range,
            cooldown_ms: desc.cooldown_ms,
            fire_mode: desc.fire_mode,
            resolution: desc.resolution,
            cooldown_remaining_ms: 0.0,
            shoot_press_consumed: false,
        }
    }

    pub fn effective(&self) -> EffectiveStats {
        EffectiveStats {
            damage: self.damage,
            range: self.range,
            cooldown_ms: self.cooldown_ms,
            fire_mode: self.fire_mode,
            resolution: self.resolution,
        }
    }

    pub fn refresh_from_descriptor(&mut self, desc: &WeaponDescriptor) {
        self.damage = desc.damage;
        self.range = desc.range;
        self.cooldown_ms = desc.cooldown_ms;
        self.fire_mode = desc.fire_mode;
        self.resolution = desc.resolution;
        // `cooldown_remaining_ms` and `shoot_press_consumed` are live instance
        // state. Hot reload changes authored tuning, not the current trigger
        // edge or whether this instance is mid-cooldown.
    }
}

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
        }
    }

    #[test]
    fn refresh_from_descriptor_updates_stats_and_preserves_live_state() {
        let mut component = WeaponComponent::from_descriptor(&descriptor(10.0, 20.0, 100.0));
        component.cooldown_remaining_ms = 42.0;
        component.shoot_press_consumed = true;

        component.refresh_from_descriptor(&descriptor(25.0, 80.0, 250.0));

        assert_eq!(component.damage, 25.0);
        assert_eq!(component.range, 80.0);
        assert_eq!(component.cooldown_ms, 250.0);
        assert_eq!(component.cooldown_remaining_ms, 42.0);
        assert!(component.shoot_press_consumed);
    }
}
