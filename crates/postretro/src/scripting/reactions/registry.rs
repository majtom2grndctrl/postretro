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
}
