use crate::weapon;
use postretro_entities::components::agent::AgentComponent;
use postretro_entities::components::health::{
    DamageContext, DamageProducer, HealthComponent, apply_damage_with_context,
};
#[cfg(test)]
use postretro_entities::components::inventory::Inventory;
use postretro_entities::components::player_movement::PlayerMovementComponent;
use postretro_entities::components::weapon::UNKNOWN_WEAPON_CREDIT_SOURCE;
#[cfg(test)]
use postretro_entities::components::weapon::WeaponComponent;
use postretro_entities::{EntityId, EntityRegistry};

#[cfg(test)]
pub(crate) fn apply_weapon_impact_damage(
    registry: &mut EntityRegistry,
    attacker: Option<EntityId>,
    impact: &weapon::WeaponImpact,
) {
    let (Some(_), weapon::ActivationOutcome::Hit(payload)) = (impact.target, impact.outcome) else {
        return;
    };
    let Some(weapon_id) = attacker
        .and_then(|pawn| registry.get_component::<Inventory>(pawn).ok())
        .and_then(Inventory::active_wieldable)
    else {
        log::warn!("[Weapon] hitscan impact had no active wieldable; dropping damage");
        return;
    };
    let Ok(component) = registry.get_component::<WeaponComponent>(weapon_id) else {
        log::warn!("[Weapon] active wieldable {weapon_id} has no WeaponComponent; dropping damage");
        return;
    };

    let effective = component.effective();
    apply_weapon_impact_damage_with_source(
        registry,
        weapon_id,
        attacker,
        impact,
        effective.credit_source.to_string(),
        payload.amount,
    );
}

pub(crate) fn apply_authorized_weapon_impact_damage(
    registry: &mut EntityRegistry,
    weapon_id: EntityId,
    attacker: Option<EntityId>,
    impact: &weapon::WeaponImpact,
    credit_source: String,
    damage_amount: f32,
) {
    apply_weapon_impact_damage_with_source(
        registry,
        weapon_id,
        attacker,
        impact,
        credit_source,
        damage_amount,
    );
}

fn apply_weapon_impact_damage_with_source(
    registry: &mut EntityRegistry,
    weapon_id: EntityId,
    attacker: Option<EntityId>,
    impact: &weapon::WeaponImpact,
    credit_source: String,
    damage_amount: f32,
) {
    let (Some(target), weapon::ActivationOutcome::Hit(payload)) = (impact.target, impact.outcome)
    else {
        return;
    };
    // Capture liveness before lethal damage. Push-only effects never enter the
    // health ledger, and hit-zone damage multipliers never amplify movement.
    if crate::scripting_systems::health::is_damage_target_eligible(registry, target) {
        apply_hit_knockback(registry, target, payload.impulse);
    }
    let source_id = if credit_source.is_empty() {
        log::warn!(
            "[Weapon] active wieldable {weapon_id} resolved an empty credit source; using {UNKNOWN_WEAPON_CREDIT_SOURCE}"
        );
        UNKNOWN_WEAPON_CREDIT_SOURCE.to_string()
    } else {
        credit_source
    };
    let multiplier = impact
        .zone
        .as_deref()
        .and_then(|tag| {
            registry
                .get_component::<HealthComponent>(target)
                .ok()
                .and_then(|health| health.zone_multipliers.get(tag).copied())
        })
        .unwrap_or(1.0);
    let scaled = weapon::DamagePayload {
        amount: damage_amount * multiplier,
        impulse: glam::Vec3::ZERO,
    };
    if !scaled.amount.is_finite() {
        log::warn!(
            "[Weapon] scaled damage amount {} is non-finite; dropping damage",
            scaled.amount
        );
        return;
    }
    apply_damage_with_context(
        registry,
        target,
        &scaled,
        DamageContext {
            source_id,
            attacker,
            weapon: Some(weapon_id),
            zone: impact.zone.clone(),
            producer: DamageProducer::InTick,
        },
    );
}

fn apply_hit_knockback(registry: &mut EntityRegistry, target: EntityId, impulse: glam::Vec3) {
    if !impulse.is_finite() || impulse == glam::Vec3::ZERO {
        return;
    }
    if let Ok(mut movement) = registry
        .get_component::<PlayerMovementComponent>(target)
        .cloned()
    {
        if movement.add_knockback(impulse) {
            let _ = registry.set_component(target, movement);
        }
    } else if let Ok(mut agent) = registry.get_component::<AgentComponent>(target).cloned()
        && agent.add_knockback(impulse)
    {
        let _ = registry.set_component(target, agent);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use glam::Vec3;
    use postretro_entities::Transform;

    fn target(registry: &mut EntityRegistry) -> EntityId {
        let id = registry.spawn(Transform::default());
        registry
            .set_component(id, crate::sim::tests::trigger_movement())
            .unwrap();
        registry
            .set_component(
                id,
                HealthComponent {
                    max: 100.0,
                    current: 100.0,
                    hitbox: None,
                    death_handled: false,
                    pending_kill_credit: None,
                    zone_multipliers: Default::default(),
                    contributor_ledger: Default::default(),
                },
            )
            .unwrap();
        id
    }

    #[test]
    fn direct_knockback_is_independent_of_damage_and_target_resistance() {
        let mut registry = EntityRegistry::new();
        let weapon = registry.spawn(Transform::default());
        let target = target(&mut registry);
        let impact = weapon::WeaponImpact {
            point: Vec3::ZERO,
            normal: Vec3::Y,
            target: Some(target),
            zone: None,
            outcome: weapon::ActivationOutcome::Hit(weapon::DamagePayload {
                amount: 0.0,
                impulse: Vec3::new(8.0, 4.0, 0.0),
            }),
        };
        apply_authorized_weapon_impact_damage(
            &mut registry,
            weapon,
            None,
            &impact,
            "test.push".into(),
            0.0,
        );
        let health = registry.get_component::<HealthComponent>(target).unwrap();
        assert!((health.current - 100.0).abs() < 1.0e-5);
        assert!(health.contributor_ledger.entries().is_empty());
        let mut movement = registry
            .get_component::<PlayerMovementComponent>(target)
            .unwrap()
            .clone();
        assert!((movement.velocity - Vec3::new(8.0, 4.0, 0.0)).length() < 1.0e-5);
        movement.knockback.scale = 0.0;
        registry.set_component(target, movement).unwrap();
        apply_authorized_weapon_impact_damage(
            &mut registry,
            weapon,
            None,
            &impact,
            "test.push".into(),
            10.0,
        );
        assert!(
            (registry
                .get_component::<HealthComponent>(target)
                .unwrap()
                .current
                - 90.0)
                .abs()
                < 1.0e-5
        );
        assert!(
            (registry
                .get_component::<PlayerMovementComponent>(target)
                .unwrap()
                .velocity
                - Vec3::new(8.0, 4.0, 0.0))
            .length()
                < 1.0e-5
        );
    }

    #[test]
    fn lethal_hit_pushes_once_without_hit_zone_amplifying_knockback() {
        let mut registry = EntityRegistry::new();
        let weapon = registry.spawn(Transform::default());
        let target = target(&mut registry);
        let mut health = registry
            .get_component::<HealthComponent>(target)
            .unwrap()
            .clone();
        health.zone_multipliers.insert("head".into(), 2.0);
        registry.set_component(target, health).unwrap();
        let impact = weapon::WeaponImpact {
            point: Vec3::ZERO,
            normal: Vec3::Y,
            target: Some(target),
            zone: Some("head".into()),
            outcome: weapon::ActivationOutcome::Hit(weapon::DamagePayload {
                amount: 50.0,
                impulse: Vec3::X * 8.0,
            }),
        };
        for _ in 0..2 {
            apply_authorized_weapon_impact_damage(
                &mut registry,
                weapon,
                None,
                &impact,
                "test.head".into(),
                50.0,
            );
        }
        assert!(
            registry
                .get_component::<HealthComponent>(target)
                .unwrap()
                .current
                .abs()
                < 1.0e-5
        );
        assert!(
            (registry
                .get_component::<PlayerMovementComponent>(target)
                .unwrap()
                .velocity
                .x
                - 8.0)
                .abs()
                < 1.0e-5
        );
    }
}
