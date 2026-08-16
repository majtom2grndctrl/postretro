//! `updateEnemyState` reaction primitive: mutate consequential, authored enemy
//! state on Brain-bearing entities. Tag lookup belongs to the caller: named
//! reactions arrive with Transform-resolved targets, while trigger commands
//! resolve the live Brain-tag set at fire time.

use serde::{Deserialize, Serialize};

use postretro_entities::components::brain::BrainComponent;
use postretro_entities::{EntityId, EntityRegistry};
use postretro_scripting_core::reaction_registry::{ReactionError, ReactionPrimitiveRegistry};

/// Typed, additive partial for consequential enemy-state updates.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct UpdateEnemyStateArgs {
    pub(crate) aggro: Option<bool>,
}

/// Apply one partial to one Brain. Callers must select Brain-bearing targets;
/// the defensive missing-component check keeps the shared helper safe if a
/// target disappears between selection and application.
pub(crate) fn apply_update_enemy_state_to_brain(
    registry: &mut EntityRegistry,
    entity: EntityId,
    args: &UpdateEnemyStateArgs,
) {
    let Ok(mut brain) = registry.get_component::<BrainComponent>(entity).cloned() else {
        return;
    };

    let Some(aggro) = args.aggro else {
        return;
    };
    if brain.aggro_armed != aggro {
        brain.aggro_armed = aggro;
        let _ = registry.set_component(entity, brain);
    }
}

/// Register the app-drain arm. Its targets were already resolved through the
/// ordinary Transform query, so retain only Brain-bearing entities here.
pub(crate) fn register_enemy_state_reaction_primitives(registry: &mut ReactionPrimitiveRegistry) {
    registry.register("updateEnemyState", |registry, targets, args| {
        let args: UpdateEnemyStateArgs = serde_json::from_value(args.clone()).map_err(|error| {
            ReactionError::InvalidArgument {
                reason: format!("updateEnemyState: failed to deserialize args: {error}"),
            }
        })?;

        let brain_targets: Vec<_> = targets
            .iter()
            .copied()
            .filter(|&entity| registry.get_component::<BrainComponent>(entity).is_ok())
            .collect();
        if brain_targets.is_empty() {
            log::debug!("[Scripting] updateEnemyState: empty Brain target set, no-op");
            return Ok(());
        }
        for entity in brain_targets {
            apply_update_enemy_state_to_brain(registry, entity, &args);
        }
        Ok(())
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use postretro_entities::Transform;

    #[test]
    fn rejects_unknown_argument_keys() {
        let error = serde_json::from_value::<UpdateEnemyStateArgs>(serde_json::json!({
            "aggression": true
        }))
        .unwrap_err();
        assert!(error.to_string().contains("unknown field"));
    }

    #[test]
    fn registrar_exposes_enemy_state_primitive() {
        let mut reactions = ReactionPrimitiveRegistry::new();
        register_enemy_state_reaction_primitives(&mut reactions);
        assert!(reactions.contains("updateEnemyState"));
    }

    #[test]
    fn app_drain_filters_non_brain_targets_and_is_idempotent() {
        let mut registry = EntityRegistry::new();
        let non_brain = registry.spawn(Transform::default());
        let brain = registry.spawn(Transform::default());
        registry
            .set_component(
                brain,
                BrainComponent::from_graph(&postretro_foundation::BehaviorGraphDescriptor {
                    initial: "idle".to_string(),
                    states: std::collections::BTreeMap::from([(
                        "idle".to_string(),
                        postretro_foundation::BehaviorStateDescriptor {
                            animation: "idle".to_string(),
                            motion: postretro_foundation::MotionVerb::Hold,
                            action: None,
                            transitions: Vec::new(),
                            on_enter: None,
                        },
                    )]),
                    interrupts: Vec::new(),
                    candidate_filter: None,
                    patrol: None,
                    attack: None,
                    engagement_radius: None,
                    move_speed: 1.0,
                }),
            )
            .unwrap();
        let mut reactions = ReactionPrimitiveRegistry::new();
        register_enemy_state_reaction_primitives(&mut reactions);
        let args = serde_json::json!({ "aggro": false });
        assert!(
            reactions
                .dispatch(
                    "updateEnemyState",
                    &mut registry,
                    &[non_brain, brain],
                    &args,
                )
                .unwrap()
        );
        assert!(
            reactions
                .dispatch("updateEnemyState", &mut registry, &[brain], &args)
                .unwrap()
        );

        assert!(
            !registry
                .get_component::<BrainComponent>(brain)
                .unwrap()
                .aggro_armed
        );
        assert!(registry.get_component::<BrainComponent>(non_brain).is_err());
    }
}
