// Named reaction-primitive lookup table. Mirrors `SequencedPrimitiveRegistry`
// in shape, but each handler receives the tag-resolved target list rather
// than a single entity id.
// See: context/lib/scripting.md §2 (Data context lifecycle)

use std::collections::HashMap;

use crate::scripting::registry::{EntityId, EntityRegistry};

use super::ReactionError;

pub(crate) type ReactionPrimitiveFn = Box<
    dyn Fn(&mut EntityRegistry, &[EntityId], &serde_json::Value) -> Result<(), ReactionError>,
>;

#[derive(Default)]
pub(crate) struct ReactionPrimitiveRegistry {
    handlers: HashMap<String, ReactionPrimitiveFn>,
}

impl ReactionPrimitiveRegistry {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    pub(crate) fn register<F>(&mut self, name: impl Into<String>, handler: F)
    where
        F: Fn(&mut EntityRegistry, &[EntityId], &serde_json::Value) -> Result<(), ReactionError>
            + 'static,
    {
        let name = name.into();
        if self.handlers.contains_key(&name) {
            debug_assert!(false, "duplicate reaction primitive registration: {name}");
            log::warn!(
                "[Scripting] ReactionPrimitiveRegistry: overwriting existing handler for '{name}'"
            );
        }
        self.handlers.insert(name, Box::new(handler));
    }

    pub(crate) fn contains(&self, name: &str) -> bool {
        self.handlers.contains_key(name)
    }

    pub(crate) fn get(&self, name: &str) -> Option<&ReactionPrimitiveFn> {
        self.handlers.get(name)
    }
}

impl std::fmt::Debug for ReactionPrimitiveRegistry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ReactionPrimitiveRegistry")
            .field("handlers", &self.handlers.keys().collect::<Vec<_>>())
            .finish()
    }
}

/// Register the emitter-side reaction primitives (`setEmitterRate`,
/// `setSpinRate`) into the supplied registry.
pub(crate) fn register_emitter_reaction_primitives(registry: &mut ReactionPrimitiveRegistry) {
    registry.register("setEmitterRate", |reg, targets, args| {
        let parsed: super::set_emitter_rate::SetEmitterRateArgs = serde_json::from_value(
            args.clone(),
        )
        .map_err(|e| ReactionError::InvalidArgument {
            reason: format!("setEmitterRate: failed to deserialize args: {e}"),
        })?;
        super::set_emitter_rate::dispatch(reg, targets, &parsed)
    });
    registry.register("setSpinRate", |reg, targets, args| {
        let parsed = super::set_spin_rate::SetSpinRateArgs::from_json(args)?;
        super::set_spin_rate::dispatch(reg, targets, &parsed)
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registers_both_emitter_primitives_under_expected_names() {
        let mut r = ReactionPrimitiveRegistry::new();
        register_emitter_reaction_primitives(&mut r);
        assert!(r.contains("setEmitterRate"));
        assert!(r.contains("setSpinRate"));
        assert!(!r.contains("setLightAnimation"));
    }
}
