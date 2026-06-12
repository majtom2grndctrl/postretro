// Health component: descriptor-authored hit points plus the damage chokepoint.
// Spawn and hot reload set `max` and the optional hitbox; `current` is live HP
// that damage mutates and a later death sweep resolves.
//
// See: context/lib/entity_model.md §2 (Health component), §7 (hitbox AABB)

use glam::Vec3;
use serde::{Deserialize, Serialize};

use crate::scripting::data_descriptors::HealthDescriptor;
use crate::scripting::registry::{EntityId, EntityRegistry};
use crate::weapon::DamagePayload;

/// One world-aligned AABB hitbox, fixed per archetype. An entity is
/// hitscan-targetable iff it carries one. `offset` shifts the box center from
/// the entity's `Transform.position`; entity rotation is ignored.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub(crate) struct Hitbox {
    pub(crate) half_extents: Vec3,
    pub(crate) offset: Vec3,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub(crate) struct HealthComponent {
    pub(crate) max: f32,
    pub(crate) current: f32,
    pub(crate) hitbox: Option<Hitbox>,
    /// One-shot latch: set when a persisting zero-HP player's death is reported
    /// so the `playerDied` event fires exactly once. The death sweep (a later
    /// task) is this field's only writer; nothing here mutates it.
    #[serde(default)]
    pub(crate) death_handled: bool,
}

impl HealthComponent {
    /// Materialize from a descriptor at spawn. `current` initializes to `max`.
    pub(crate) fn from_descriptor(desc: &HealthDescriptor) -> Self {
        Self {
            max: desc.max,
            current: desc.max,
            hitbox: desc.hitbox.as_ref().map(|h| Hitbox {
                half_extents: Vec3::from_array(h.half_extents),
                offset: Vec3::from_array(h.offset.unwrap_or([0.0, 0.0, 0.0])),
            }),
            death_handled: false,
        }
    }

    /// Hot-reload refresh: `max` and `hitbox` reseed from the new descriptor;
    /// `current` clamps to the new max so an authored max reduction cannot leave
    /// HP above the cap. `death_handled` is live state and is preserved.
    pub(crate) fn refresh_from_descriptor(&mut self, desc: &HealthDescriptor) {
        self.max = desc.max;
        self.current = self.current.min(desc.max);
        self.hitbox = desc.hitbox.as_ref().map(|h| Hitbox {
            half_extents: Vec3::from_array(h.half_extents),
            offset: Vec3::from_array(h.offset.unwrap_or([0.0, 0.0, 0.0])),
        });
    }
}

/// The damage chokepoint every producer routes through. Subtracts the payload's
/// `amount` from the entity's current HP, flooring at zero. No-ops (returns
/// without error) when the entity carries no `Health` component or no longer
/// exists — damage to a non-health entity is simply ignored, never an error.
///
/// Damage arrives only as a [`DamagePayload`] (never a bare scalar); spatial
/// info rides beside the payload, never inside it.
pub(crate) fn apply_damage(registry: &mut EntityRegistry, id: EntityId, payload: &DamagePayload) {
    let Ok(health) = registry.get_component::<HealthComponent>(id) else {
        return;
    };
    let mut updated = *health;
    updated.current = (updated.current - payload.amount).max(0.0);
    // `set_component` only fails on a stale id, which `get_component` already
    // ruled out above.
    let _ = registry.set_component(id, updated);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scripting::data_descriptors::HitboxDescriptor;
    use crate::scripting::registry::Transform;

    fn descriptor(max: f32) -> HealthDescriptor {
        HealthDescriptor { max, hitbox: None }
    }

    #[test]
    fn from_descriptor_initializes_current_to_max() {
        let component = HealthComponent::from_descriptor(&descriptor(80.0));
        assert_eq!(component.current, 80.0);
        assert_eq!(component.max, 80.0);
        assert!(component.hitbox.is_none());
        assert!(!component.death_handled);
    }

    #[test]
    fn from_descriptor_carries_hitbox_with_default_offset() {
        let desc = HealthDescriptor {
            max: 50.0,
            hitbox: Some(HitboxDescriptor {
                half_extents: [0.5, 1.0, 0.5],
                offset: None,
            }),
        };
        let component = HealthComponent::from_descriptor(&desc);
        let hitbox = component.hitbox.expect("hitbox materialized");
        assert_eq!(hitbox.half_extents, Vec3::new(0.5, 1.0, 0.5));
        assert_eq!(hitbox.offset, Vec3::ZERO, "absent offset defaults to zero");
    }

    #[test]
    fn refresh_clamps_current_to_new_lower_max() {
        let mut component = HealthComponent::from_descriptor(&descriptor(100.0));
        component.current = 90.0;
        component.refresh_from_descriptor(&descriptor(40.0));
        assert_eq!(component.max, 40.0);
        assert_eq!(component.current, 40.0, "current clamps to the new max");
    }

    #[test]
    fn refresh_preserves_current_below_new_max() {
        let mut component = HealthComponent::from_descriptor(&descriptor(100.0));
        component.current = 30.0;
        component.refresh_from_descriptor(&descriptor(200.0));
        assert_eq!(component.max, 200.0);
        assert_eq!(
            component.current, 30.0,
            "current under the cap is untouched"
        );
    }

    #[test]
    fn apply_damage_subtracts_amount() {
        let mut reg = EntityRegistry::new();
        let id = reg.spawn(Transform::default());
        reg.set_component(id, HealthComponent::from_descriptor(&descriptor(100.0)))
            .unwrap();

        apply_damage(&mut reg, id, &DamagePayload { amount: 25.0 });

        assert_eq!(
            reg.get_component::<HealthComponent>(id).unwrap().current,
            75.0
        );
    }

    #[test]
    fn apply_damage_floors_current_at_zero() {
        let mut reg = EntityRegistry::new();
        let id = reg.spawn(Transform::default());
        reg.set_component(id, HealthComponent::from_descriptor(&descriptor(10.0)))
            .unwrap();

        apply_damage(&mut reg, id, &DamagePayload { amount: 999.0 });

        assert_eq!(
            reg.get_component::<HealthComponent>(id).unwrap().current,
            0.0,
            "HP never goes negative"
        );
    }

    #[test]
    fn apply_damage_is_noop_without_health_component() {
        let mut reg = EntityRegistry::new();
        let id = reg.spawn(Transform::default());

        // No Health component attached: must not error or panic.
        apply_damage(&mut reg, id, &DamagePayload { amount: 25.0 });

        assert!(reg.get_component::<HealthComponent>(id).is_err());
    }
}
