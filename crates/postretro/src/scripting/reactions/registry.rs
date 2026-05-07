// Named reaction-primitive lookup table. Mirrors `SequencedPrimitiveRegistry`
// in shape, but each handler receives the tag-resolved target list rather
// than a single entity id.
// See: context/lib/scripting.md §4 (Primitive Registration)

use std::collections::HashMap;

use crate::scripting::registry::{EntityId, EntityRegistry};

use super::ReactionError;

pub(crate) type ReactionPrimitiveFn =
    Box<dyn Fn(&mut EntityRegistry, &[EntityId], &serde_json::Value) -> Result<(), ReactionError>>;

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

    /// Resolve `name` and run its handler against `targets` and `args`.
    ///
    /// Returns `Ok(false)` when no handler is registered under `name` —
    /// callers log this defensively. Per-target failures inside the handler
    /// are logged as warnings by the handler itself; this method only
    /// surfaces invariant violations such as `InvalidArgument`.
    pub(crate) fn dispatch(
        &self,
        name: &str,
        registry: &mut EntityRegistry,
        targets: &[EntityId],
        args: &serde_json::Value,
    ) -> Result<bool, ReactionError> {
        let Some(handler) = self.handlers.get(name) else {
            return Ok(false);
        };
        handler(registry, targets, args).map(|_| true)
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
        let parsed: super::set_emitter_rate::SetEmitterRateArgs =
            serde_json::from_value(args.clone()).map_err(|e| ReactionError::InvalidArgument {
                reason: format!("setEmitterRate: failed to deserialize args: {e}"),
            })?;
        super::set_emitter_rate::dispatch(reg, targets, &parsed)
    });
    registry.register("setSpinRate", |reg, targets, args| {
        let parsed = super::set_spin_rate::SetSpinRateArgs::from_json(args)?;
        super::set_spin_rate::dispatch(reg, targets, &parsed)
    });
}

/// Register the fog-side reaction primitives (`setFogDensity`,
/// `setFogScatter`, `setFogEdgeSoftness`, `setFogFalloff`, `setFogParams`)
/// into the supplied registry.
pub(crate) fn register_fog_reaction_primitives(registry: &mut ReactionPrimitiveRegistry) {
    registry.register("setFogDensity", |reg, targets, args| {
        let parsed: super::set_fog_density::SetFogDensityArgs =
            serde_json::from_value(args.clone()).map_err(|e| ReactionError::InvalidArgument {
                reason: format!("setFogDensity: failed to deserialize args: {e}"),
            })?;
        super::set_fog_density::dispatch(reg, targets, &parsed)
    });
    registry.register("setFogScatter", |reg, targets, args| {
        let parsed: super::set_fog_scatter::SetFogScatterArgs =
            serde_json::from_value(args.clone()).map_err(|e| ReactionError::InvalidArgument {
                reason: format!("setFogScatter: failed to deserialize args: {e}"),
            })?;
        super::set_fog_scatter::dispatch(reg, targets, &parsed)
    });
    registry.register("setFogEdgeSoftness", |reg, targets, args| {
        let parsed: super::set_fog_edge_softness::SetFogEdgeSoftnessArgs =
            serde_json::from_value(args.clone()).map_err(|e| ReactionError::InvalidArgument {
                reason: format!("setFogEdgeSoftness: failed to deserialize args: {e}"),
            })?;
        super::set_fog_edge_softness::dispatch(reg, targets, &parsed)
    });
    registry.register("setFogFalloff", |reg, targets, args| {
        let parsed: super::set_fog_falloff::SetFogFalloffArgs =
            serde_json::from_value(args.clone()).map_err(|e| ReactionError::InvalidArgument {
                reason: format!("setFogFalloff: failed to deserialize args: {e}"),
            })?;
        super::set_fog_falloff::dispatch(reg, targets, &parsed)
    });
    registry.register("setFogParams", |reg, targets, args| {
        let parsed: super::set_fog_params::SetFogParamsArgs = serde_json::from_value(args.clone())
            .map_err(|e| ReactionError::InvalidArgument {
                reason: format!("setFogParams: failed to deserialize args: {e}"),
            })?;
        super::set_fog_params::dispatch(reg, targets, &parsed)
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

    #[test]
    fn registers_all_fog_primitives_under_expected_names() {
        let mut r = ReactionPrimitiveRegistry::new();
        register_fog_reaction_primitives(&mut r);
        assert!(r.contains("setFogDensity"));
        assert!(r.contains("setFogScatter"));
        assert!(r.contains("setFogEdgeSoftness"));
        assert!(r.contains("setFogFalloff"));
        assert!(r.contains("setFogParams"));
        // Defensive: we did not accidentally register a live-mutation
        // primitive surface for fog.
        assert!(!r.contains("setComponent"));
    }

    #[test]
    fn fog_primitive_dispatch_round_trip() {
        use crate::scripting::registry::{EntityRegistry, FogVolumeComponent, Transform};

        let mut reg = EntityRegistry::new();
        let id = reg.spawn(Transform::default());
        reg.set_component(
            id,
            FogVolumeComponent {
                density: 0.5,
                scatter: 0.6,
                edge_softness: 0.25,
                falloff: 2.0,
            },
        )
        .unwrap();

        let mut r = ReactionPrimitiveRegistry::new();
        register_fog_reaction_primitives(&mut r);

        // Mixed update with camelCase JSON, exercising the registered
        // dispatcher rather than the dispatch fn directly.
        let args = serde_json::json!({
            "density": 1.25,
            "edgeSoftness": 0.5,
            "falloff": 3.0,
        });
        let dispatched = r.dispatch("setFogParams", &mut reg, &[id], &args).unwrap();
        assert!(dispatched);

        let after = reg.get_component::<FogVolumeComponent>(id).unwrap();
        assert_eq!(after.density, 1.25);
        assert_eq!(after.scatter, 0.6);
        assert_eq!(after.edge_softness, 0.5);
        assert_eq!(after.falloff, 3.0);
    }
}
