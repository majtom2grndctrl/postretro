use crate::weapon;
use postretro_entities::components::health::{
    DamageContext, DamageProducer, HealthComponent, apply_damage_with_context,
};
#[cfg(test)]
use postretro_entities::components::inventory::Inventory;
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
    let (Some(target), weapon::ActivationOutcome::Hit(_)) = (impact.target, impact.outcome) else {
        return;
    };
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
