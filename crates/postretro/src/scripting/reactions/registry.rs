// Compatibility entry points for reaction primitive registration.
// Handler implementations now live with the subsystems they mutate.

pub(crate) use postretro_scripting_core::reaction_registry::ReactionPrimitiveRegistry;

pub(crate) fn register_emitter_reaction_primitives(registry: &mut ReactionPrimitiveRegistry) {
    crate::fx::emitter_reactions::register_emitter_reaction_primitives(registry);
    crate::scripting::reactions::animation::register_mesh_reaction_primitives(registry);
    crate::health::reactions::register_health_reaction_primitives(registry);
}

pub(crate) use crate::fx::fog_reactions::{
    register_fog_reaction_primitives, register_sequenced_fog_primitives,
};

pub(crate) use crate::kinematic_mover::register_sequenced_mover_primitives;

pub(crate) fn register_mover_reaction_primitives(registry: &mut ReactionPrimitiveRegistry) {
    crate::kinematic_mover::register_mover_reaction_primitives(registry);
}

pub(crate) fn register_trigger_reaction_primitives(registry: &mut ReactionPrimitiveRegistry) {
    crate::trigger_system::register_trigger_reaction_primitives(registry);
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
        register_mover_reaction_primitives(&mut r);
        for name in [
            "moverStart",
            "moverStop",
            "moverReverse",
            "moverGoToPathNode",
        ] {
            assert!(r.contains(name), "missing {name}");
        }
    }

    #[test]
    fn trigger_registrar_exposes_arm_and_disarm() {
        let mut r = ReactionPrimitiveRegistry::new();
        register_trigger_reaction_primitives(&mut r);
        assert!(r.contains("armTrigger"));
        assert!(r.contains("disarmTrigger"));
    }
}
