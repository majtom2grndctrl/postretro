//! Reaction primitives for additive engine-owned resources.
//! See: context/lib/scripting.md §10.8.

use serde::{Deserialize, Serialize};

use postretro_entities::components::grant::{GrantOutcome, grant_ammo, grant_health};
use postretro_entities::{EntityId, EntityRegistry};
use postretro_scripting_core::reaction_registry::{ReactionError, ReactionPrimitiveRegistry};

/// Register additive resource reaction primitives.
///
/// Registration lives beside the resource wiring rather than under `health`:
/// ammo reserves are a separate engine-owned component and both primitives
/// share the same post-spawn grant chokepoint.
pub(crate) fn register_grant_reactions(registry: &mut ReactionPrimitiveRegistry) {
    registry.register("grantHealth", |registry, targets, args| {
        let parsed: GrantHealthArgs = serde_json::from_value(args.clone()).map_err(|error| {
            ReactionError::InvalidArgument {
                reason: format!("grantHealth: failed to deserialize args: {error}"),
            }
        })?;
        dispatch_health(registry, targets, &parsed)
    });
    registry.register("grantAmmo", |registry, targets, args| {
        let parsed: GrantAmmoArgs = serde_json::from_value(args.clone()).map_err(|error| {
            ReactionError::InvalidArgument {
                reason: format!("grantAmmo: failed to deserialize args: {error}"),
            }
        })?;
        dispatch_ammo(registry, targets, &parsed)
    });
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct GrantHealthArgs {
    pub(crate) amount: f32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct GrantAmmoArgs {
    #[serde(rename = "type")]
    pub(crate) ammo_type: String,
    pub(crate) amount: f32,
}

/// Apply a health grant to every resolved reaction target.
///
/// The grant chokepoint owns all invalid-amount and missing-component warnings.
/// This handler only preserves fan-out flow after a skipped target.
pub(crate) fn dispatch_health(
    registry: &mut EntityRegistry,
    targets: &[EntityId],
    args: &GrantHealthArgs,
) -> Result<(), ReactionError> {
    if targets.is_empty() {
        log::debug!("[Scripting] grantHealth: empty target set, no-op");
        return Ok(());
    }

    for &target in targets {
        match grant_health(registry, target, args.amount) {
            GrantOutcome::Applied
            | GrantOutcome::SkippedNoComponent
            | GrantOutcome::SkippedInvalidAmount => {}
        }
    }
    Ok(())
}

/// Apply an ammo grant to every resolved reaction target.
///
/// Pool-key validation happens while descriptors load; this path deliberately
/// forwards the validated key to the shared resource chokepoint unchanged.
pub(crate) fn dispatch_ammo(
    registry: &mut EntityRegistry,
    targets: &[EntityId],
    args: &GrantAmmoArgs,
) -> Result<(), ReactionError> {
    if targets.is_empty() {
        log::debug!("[Scripting] grantAmmo: empty target set, no-op");
        return Ok(());
    }

    for &target in targets {
        match grant_ammo(registry, target, &args.ammo_type, args.amount) {
            GrantOutcome::Applied
            | GrantOutcome::SkippedNoComponent
            | GrantOutcome::SkippedInvalidAmount => {}
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use postretro_entities::Transform;
    use postretro_entities::components::ammo_reserve::AmmoReserve;
    use postretro_entities::components::health::HealthComponent;
    use postretro_foundation::HealthDescriptor;

    use super::*;

    fn health(registry: &mut EntityRegistry, current: f32) -> EntityId {
        let id = registry.spawn(Transform::default());
        let mut component = HealthComponent::from_descriptor(&HealthDescriptor {
            max: 100.0,
            hitbox: None,
            zone_multipliers: Default::default(),
        });
        component.current = current;
        registry.set_component(id, component).unwrap();
        id
    }

    #[test]
    fn registrar_exposes_both_grant_primitives() {
        let mut registry = ReactionPrimitiveRegistry::new();
        register_grant_reactions(&mut registry);
        assert!(registry.contains("grantHealth"));
        assert!(registry.contains("grantAmmo"));
    }

    #[test]
    fn health_grants_fan_out_and_compose_in_effect_order() {
        let mut registry = EntityRegistry::new();
        let target = health(&mut registry, 10.0);
        let args = GrantHealthArgs { amount: 15.0 };

        dispatch_health(&mut registry, &[target], &args).unwrap();
        dispatch_health(&mut registry, &[target], &args).unwrap();

        assert_eq!(
            registry
                .get_component::<HealthComponent>(target)
                .unwrap()
                .current,
            40.0
        );
    }

    #[test]
    fn skipped_target_does_not_abort_a_sibling_grant_and_logs_only_chokepoint() {
        let mut registry = EntityRegistry::new();
        let bare = registry.spawn(Transform::default());
        let target = health(&mut registry, 10.0);
        let captured = crate::scripting::reactions::log_capture::capture(|| {
            dispatch_health(
                &mut registry,
                &[bare, target],
                &GrantHealthArgs { amount: 20.0 },
            )
            .unwrap();
        });

        assert_eq!(
            registry
                .get_component::<HealthComponent>(target)
                .unwrap()
                .current,
            30.0
        );
        let warnings: Vec<_> = captured
            .iter()
            .filter(|(level, _)| *level == log::Level::Warn)
            .map(|(_, message)| message.clone())
            .collect();
        assert_eq!(
            warnings,
            vec![format!(
                "[Grant] grantHealth: entity {bare} has no HealthComponent; skipping"
            )],
            "the reaction handler must not duplicate chokepoint warnings"
        );
    }

    #[test]
    fn ammo_grant_fans_out_and_uses_the_shared_chokepoint() {
        let mut registry = EntityRegistry::new();
        let bare = registry.spawn(Transform::default());
        let recipient = registry.spawn(Transform::default());
        registry
            .set_component(recipient, AmmoReserve::new())
            .unwrap();

        let captured = crate::scripting::reactions::log_capture::capture(|| {
            dispatch_ammo(
                &mut registry,
                &[bare, recipient],
                &GrantAmmoArgs {
                    ammo_type: "bullets.light".to_string(),
                    amount: 8.9,
                },
            )
            .unwrap();
        });

        assert_eq!(
            registry
                .get_component::<AmmoReserve>(recipient)
                .unwrap()
                .available("bullets.light"),
            8
        );
        let warnings: Vec<_> = captured
            .iter()
            .filter(|(level, _)| *level == log::Level::Warn)
            .map(|(_, message)| message.clone())
            .collect();
        assert_eq!(
            warnings,
            vec![format!(
                "[Grant] grantAmmo: entity {bare} has no AmmoReserve; skipping"
            )]
        );
    }

    #[test]
    fn invalid_amount_warns_once_per_target_from_the_chokepoint() {
        let mut registry = EntityRegistry::new();
        let target = health(&mut registry, 10.0);
        let captured = crate::scripting::reactions::log_capture::capture(|| {
            dispatch_health(&mut registry, &[target], &GrantHealthArgs { amount: -1.0 }).unwrap();
        });
        assert_eq!(
            registry
                .get_component::<HealthComponent>(target)
                .unwrap()
                .current,
            10.0
        );
        let warnings: Vec<_> = captured
            .iter()
            .filter(|(level, _)| *level == log::Level::Warn)
            .map(|(_, message)| message.clone())
            .collect();
        assert_eq!(
            warnings,
            vec!["[Grant] grantHealth: amount -1 is negative or non-finite; no-op".to_string()]
        );
    }
}
