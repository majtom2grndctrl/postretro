// Compatibility entry points for reaction primitive registration.
// Handler implementations now live with the subsystems they mutate.

pub(crate) use postretro_scripting_core::reaction_registry::ReactionPrimitiveRegistry;

pub(crate) fn register_emitter_reaction_primitives(registry: &mut ReactionPrimitiveRegistry) {
    crate::fx::emitter_reactions::register_emitter_reaction_primitives(registry);
    crate::scripting::reactions::animation::register_mesh_reaction_primitives(registry);
    crate::health::reactions::register_health_reaction_primitives(registry);
}

pub(crate) fn register_enemy_state_reaction_primitives(registry: &mut ReactionPrimitiveRegistry) {
    crate::scripting::reactions::enemy_state::register_enemy_state_reaction_primitives(registry);
}

pub(crate) fn register_grant_reactions(registry: &mut ReactionPrimitiveRegistry) {
    crate::grant::register_grant_reactions(registry);
}

pub(crate) use crate::fx::fog_reactions::{
    register_fog_reaction_primitives, register_sequenced_fog_primitives,
};

pub(crate) use crate::kinematic_mover::register_sequenced_mover_primitives;
pub(crate) use crate::trigger_system::register_sequenced_trigger_primitives;

pub(crate) fn register_spawner_reaction_primitives(
    registry: &mut ReactionPrimitiveRegistry,
    context: crate::spawner::SpawnContext,
) {
    crate::spawner::register_spawner_reaction_primitives(registry, context);
}

pub(crate) fn register_mover_reaction_primitives(
    registry: &mut ReactionPrimitiveRegistry,
    diagnostics: crate::kinematic_mover::MoverCommandDiagnostics,
) {
    crate::kinematic_mover::register_mover_reaction_primitives(registry, diagnostics);
}

pub(crate) fn register_trigger_reaction_primitives(
    registry: &mut ReactionPrimitiveRegistry,
    diagnostics: crate::kinematic_mover::MoverCommandDiagnostics,
) {
    crate::trigger_system::register_trigger_reaction_primitives(registry, diagnostics);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn legacy_emitter_registrar_keeps_non_fog_reaction_surface() {
        let mut r = ReactionPrimitiveRegistry::new();
        register_emitter_reaction_primitives(&mut r);
        assert!(r.contains("setEmitterRate"));
        assert!(r.contains("setSpinRate"));
        assert!(r.contains("setAnimationState"));
        assert!(r.contains("applyDamage"));
        assert!(!r.contains("setLightAnimation"));
    }

    #[test]
    fn mover_registrar_exposes_closed_command_vocabulary() {
        let mut r = ReactionPrimitiveRegistry::new();
        register_mover_reaction_primitives(&mut r, Default::default());
        for name in [
            "moverStart",
            "moverStop",
            "moverReverse",
            "moverGoToPathNode",
            "moverSetSpinRate",
        ] {
            assert!(r.contains(name), "missing {name}");
        }
    }

    #[test]
    fn trigger_registrar_exposes_arm_and_disarm() {
        let mut r = ReactionPrimitiveRegistry::new();
        register_trigger_reaction_primitives(&mut r, Default::default());
        assert!(r.contains("armTrigger"));
        assert!(r.contains("disarmTrigger"));
    }

    #[test]
    fn enemy_state_registrar_exposes_update_enemy_state() {
        let mut r = ReactionPrimitiveRegistry::new();
        register_enemy_state_reaction_primitives(&mut r);
        assert!(r.contains("updateEnemyState"));
    }

    #[test]
    fn grant_registrar_exposes_additive_resource_primitives() {
        let mut r = ReactionPrimitiveRegistry::new();
        register_grant_reactions(&mut r);
        assert!(r.contains("grantHealth"));
        assert!(r.contains("grantAmmo"));
    }
}
