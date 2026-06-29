// `applyDamage` reaction primitive: route a fixed damage amount through the
// `apply_damage` health chokepoint for every entity matching the reaction's
// tag. The player-side damage stand-in and the only non-weapon damage producer
// in M10; death is resolved by the next death-sweep pass, never here.
// See: context/lib/scripting.md §10 (Reaction Primitives)

use serde::{Deserialize, Serialize};

use crate::scripting::components::health::{HealthComponent, apply_damage};
use crate::scripting::registry::{EntityId, EntityRegistry};
use postretro_foundation::DamagePayload;

use super::ReactionError;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct ApplyDamageArgs {
    pub(crate) amount: f32,
}

/// Apply `args.amount` of damage to every target via the `apply_damage`
/// chokepoint.
///
/// Per-dispatch / per-target behavior:
/// - Empty target set → no-op, debug log.
/// - `amount` negative or non-finite → `log::warn!` and no-op for the whole
///   dispatch (healing is out of scope; the chokepoint only ever reduces HP).
/// - Target missing a `Health` component → `log::warn!`, skip (tag matched a
///   non-damageable entity — most likely a tag typo). Other targets still take
///   damage.
///
/// The handler never despawns: a target whose HP reaches zero is resolved by
/// the next death-sweep pass.
pub(crate) fn dispatch(
    registry: &mut EntityRegistry,
    targets: &[EntityId],
    args: &ApplyDamageArgs,
) -> Result<(), ReactionError> {
    if targets.is_empty() {
        log::debug!("[Scripting] applyDamage: empty target set, no-op");
        return Ok(());
    }

    if !args.amount.is_finite() || args.amount < 0.0 {
        log::warn!(
            "[Scripting] applyDamage: amount {} is negative or non-finite; no-op (healing is out of scope)",
            args.amount
        );
        return Ok(());
    }

    let payload = DamagePayload {
        amount: args.amount,
    };

    for &id in targets {
        if registry.get_component::<HealthComponent>(id).is_err() {
            log::warn!("[Scripting] applyDamage: entity {id} has no HealthComponent; skipping");
            continue;
        }
        apply_damage(registry, id, &payload);
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scripting::data_descriptors::HealthDescriptor;
    use crate::scripting::registry::Transform;

    fn spawn_health(reg: &mut EntityRegistry, max: f32) -> EntityId {
        let id = reg.spawn(Transform::default());
        let health = HealthComponent::from_descriptor(&HealthDescriptor {
            max,
            hitbox: None,
            zone_multipliers: std::collections::HashMap::new(),
        });
        reg.set_component(id, health).unwrap();
        id
    }

    fn hp(reg: &EntityRegistry, id: EntityId) -> f32 {
        reg.get_component::<HealthComponent>(id).unwrap().current
    }

    #[test]
    fn positive_amount_reduces_hp_on_each_target() {
        let mut reg = EntityRegistry::new();
        let a = spawn_health(&mut reg, 100.0);
        let b = spawn_health(&mut reg, 50.0);
        dispatch(&mut reg, &[a, b], &ApplyDamageArgs { amount: 25.0 }).unwrap();
        assert_eq!(hp(&reg, a), 75.0);
        assert_eq!(hp(&reg, b), 25.0);
    }

    #[test]
    fn target_without_health_is_skipped_others_still_damaged() {
        let mut reg = EntityRegistry::new();
        // Bare entity carries only a Transform — no Health component.
        let bare = reg.spawn(Transform::default());
        let healthy = spawn_health(&mut reg, 100.0);

        let captured = crate::scripting::reactions::log_capture::capture(|| {
            dispatch(
                &mut reg,
                &[bare, healthy],
                &ApplyDamageArgs { amount: 30.0 },
            )
            .unwrap();
        });

        assert_eq!(
            hp(&reg, healthy),
            70.0,
            "the health target still took damage"
        );
        assert!(
            reg.get_component::<HealthComponent>(bare).is_err(),
            "bare entity gained no Health component"
        );
        assert!(
            captured
                .iter()
                .any(|(lvl, msg)| *lvl == log::Level::Warn && msg.contains("no HealthComponent")),
            "expected a warn-level log naming the missing component, got: {captured:?}"
        );
    }

    #[test]
    fn negative_amount_is_a_noop() {
        let mut reg = EntityRegistry::new();
        let id = spawn_health(&mut reg, 100.0);
        let captured = crate::scripting::reactions::log_capture::capture(|| {
            dispatch(&mut reg, &[id], &ApplyDamageArgs { amount: -10.0 }).unwrap();
        });
        assert_eq!(hp(&reg, id), 100.0, "no healing, no change");
        assert!(
            captured
                .iter()
                .any(|(lvl, msg)| *lvl == log::Level::Warn && msg.contains("no-op")),
            "expected a warn-level no-op log, got: {captured:?}"
        );
    }

    #[test]
    fn nan_amount_is_a_noop() {
        let mut reg = EntityRegistry::new();
        let id = spawn_health(&mut reg, 100.0);
        dispatch(&mut reg, &[id], &ApplyDamageArgs { amount: f32::NAN }).unwrap();
        assert_eq!(hp(&reg, id), 100.0);
    }

    #[test]
    fn infinite_amount_is_a_noop() {
        let mut reg = EntityRegistry::new();
        let id = spawn_health(&mut reg, 100.0);
        dispatch(
            &mut reg,
            &[id],
            &ApplyDamageArgs {
                amount: f32::INFINITY,
            },
        )
        .unwrap();
        assert_eq!(hp(&reg, id), 100.0);
    }

    #[test]
    fn empty_target_set_is_a_noop() {
        let mut reg = EntityRegistry::new();
        let id = spawn_health(&mut reg, 100.0);
        dispatch(&mut reg, &[], &ApplyDamageArgs { amount: 99.0 }).unwrap();
        assert_eq!(hp(&reg, id), 100.0);
    }

    #[test]
    fn lethal_amount_floors_at_zero_without_despawn() {
        let mut reg = EntityRegistry::new();
        let id = spawn_health(&mut reg, 40.0);
        dispatch(&mut reg, &[id], &ApplyDamageArgs { amount: 999.0 }).unwrap();
        // Floored at zero by the chokepoint; the entity still exists — death is
        // the next death-sweep pass's job, not the handler's.
        assert_eq!(hp(&reg, id), 0.0);
        assert!(reg.get_component::<HealthComponent>(id).is_ok());
    }
}
