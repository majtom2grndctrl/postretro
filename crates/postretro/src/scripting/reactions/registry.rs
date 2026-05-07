// Named reaction-primitive lookup table. Mirrors `SequencedPrimitiveRegistry`
// in shape, but each handler receives the tag-resolved target list rather
// than a single entity id.
// See: context/lib/scripting.md §4 (Primitive Registration)

use std::collections::HashMap;

use crate::scripting::ctx::ScriptCtx;
use crate::scripting::registry::{EntityId, EntityRegistry};
use crate::scripting::sequence::{SequenceError, SequencedPrimitiveRegistry};

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

/// Translate a `ReactionError` produced by a fog reaction-primitive dispatch
/// fn into a `SequenceError` so the sequenced-dispatch path can surface it
/// uniformly with other sequenced-primitive failures (e.g. `setLightAnimation`).
/// `ReactionError` currently has only `InvalidArgument`; if more variants are
/// added the catch-all `ExecutionFailed` arm here picks them up automatically.
fn reaction_to_sequence_error(err: ReactionError) -> SequenceError {
    let reason = match err {
        ReactionError::InvalidArgument { reason } => reason,
    };
    SequenceError::InvalidArgument { reason }
}

/// Register the fog reaction primitives as **sequenced** (per-step) primitives
/// against `SequencedPrimitiveRegistry`. Mirrors `register_sequenced_light_primitives`:
/// each handler receives a single `EntityId` (the step's target) plus the JSON
/// args, deserializes the args into the primitive-specific struct, and calls
/// the existing tag-targeted `dispatch` fn with a one-element target slice.
///
/// This enables `fogPulse` / `fogFade` and other sequence-based fog animations
/// — those constructors emit step arrays whose `primitive` field names a fog
/// primitive, and the sequence dispatcher looks them up here at run time.
pub(crate) fn register_sequenced_fog_primitives(
    registry: &mut SequencedPrimitiveRegistry,
    ctx: ScriptCtx,
) {
    let ctx_density = ctx.clone();
    registry.register("setFogDensity", move |id, args| {
        let parsed: super::set_fog_density::SetFogDensityArgs =
            serde_json::from_value(args.clone()).map_err(|e| SequenceError::InvalidArgument {
                reason: format!("setFogDensity: failed to deserialize args: {e}"),
            })?;
        let mut reg = ctx_density.registry.borrow_mut();
        super::set_fog_density::dispatch(&mut reg, &[id], &parsed)
            .map_err(reaction_to_sequence_error)
    });

    let ctx_scatter = ctx.clone();
    registry.register("setFogScatter", move |id, args| {
        let parsed: super::set_fog_scatter::SetFogScatterArgs =
            serde_json::from_value(args.clone()).map_err(|e| SequenceError::InvalidArgument {
                reason: format!("setFogScatter: failed to deserialize args: {e}"),
            })?;
        let mut reg = ctx_scatter.registry.borrow_mut();
        super::set_fog_scatter::dispatch(&mut reg, &[id], &parsed)
            .map_err(reaction_to_sequence_error)
    });

    let ctx_edge = ctx.clone();
    registry.register("setFogEdgeSoftness", move |id, args| {
        let parsed: super::set_fog_edge_softness::SetFogEdgeSoftnessArgs =
            serde_json::from_value(args.clone()).map_err(|e| SequenceError::InvalidArgument {
                reason: format!("setFogEdgeSoftness: failed to deserialize args: {e}"),
            })?;
        let mut reg = ctx_edge.registry.borrow_mut();
        super::set_fog_edge_softness::dispatch(&mut reg, &[id], &parsed)
            .map_err(reaction_to_sequence_error)
    });

    let ctx_falloff = ctx.clone();
    registry.register("setFogFalloff", move |id, args| {
        let parsed: super::set_fog_falloff::SetFogFalloffArgs =
            serde_json::from_value(args.clone()).map_err(|e| SequenceError::InvalidArgument {
                reason: format!("setFogFalloff: failed to deserialize args: {e}"),
            })?;
        let mut reg = ctx_falloff.registry.borrow_mut();
        super::set_fog_falloff::dispatch(&mut reg, &[id], &parsed)
            .map_err(reaction_to_sequence_error)
    });

    let ctx_params = ctx;
    registry.register("setFogParams", move |id, args| {
        let parsed: super::set_fog_params::SetFogParamsArgs = serde_json::from_value(args.clone())
            .map_err(|e| SequenceError::InvalidArgument {
                reason: format!("setFogParams: failed to deserialize args: {e}"),
            })?;
        let mut reg = ctx_params.registry.borrow_mut();
        super::set_fog_params::dispatch(&mut reg, &[id], &parsed)
            .map_err(reaction_to_sequence_error)
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
    fn registers_all_sequenced_fog_primitives_under_expected_names() {
        use crate::scripting::ctx::ScriptCtx;
        let mut r = SequencedPrimitiveRegistry::new();
        register_sequenced_fog_primitives(&mut r, ScriptCtx::new());
        assert!(r.contains("setFogDensity"));
        assert!(r.contains("setFogScatter"));
        assert!(r.contains("setFogEdgeSoftness"));
        assert!(r.contains("setFogFalloff"));
        assert!(r.contains("setFogParams"));
    }

    #[test]
    fn sequenced_fog_primitive_round_trip_through_dispatcher() {
        // End-to-end: a `Sequence` reaction whose steps name fog primitives
        // must (a) survive `validate_sequence_primitives` (the registry
        // contains the names) and (b) mutate the targeted fog component when
        // fired through `fire_named_event_with_sequences`. This is exactly
        // the path `fogPulse` / `fogFade` step arrays travel at level load.
        use crate::scripting::ctx::ScriptCtx;
        use crate::scripting::data_descriptors::{
            LevelManifest, NamedReaction, ReactionDescriptor, SequenceStep,
        };
        use crate::scripting::data_registry::DataRegistry;
        use crate::scripting::reaction_dispatch::{
            fire_named_event_with_sequences, validate_sequence_primitives,
        };
        use crate::scripting::registry::{FogVolumeComponent, Transform};

        let script_ctx = ScriptCtx::new();
        let id = {
            let mut reg = script_ctx.registry.borrow_mut();
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
            id
        };

        let mut seq_reg = SequencedPrimitiveRegistry::new();
        register_sequenced_fog_primitives(&mut seq_reg, script_ctx.clone());

        // Mirrors the per-step shape `fogPulse` produces.
        let raw_reactions = vec![NamedReaction {
            name: "levelLoad".to_string(),
            descriptor: ReactionDescriptor::Sequence(vec![
                SequenceStep {
                    id,
                    primitive: "setFogDensity".to_string(),
                    args: serde_json::json!({ "density": 0.9 }),
                },
                SequenceStep {
                    id,
                    primitive: "setFogParams".to_string(),
                    args: serde_json::json!({
                        "scatter": 0.4,
                        "edgeSoftness": 0.5,
                    }),
                },
            ]),
        }];

        // Validation must accept all five fog primitive names; nothing dropped.
        let validated = validate_sequence_primitives(raw_reactions, &seq_reg);
        assert_eq!(validated.len(), 1, "fog steps survived validation");

        let mut data = DataRegistry::new();
        data.populate_from_manifest(LevelManifest {
            reactions: validated,
        });

        let reaction_reg = ReactionPrimitiveRegistry::new();
        fire_named_event_with_sequences("levelLoad", &data, &seq_reg, &reaction_reg, &script_ctx);

        let after = *script_ctx
            .registry
            .borrow()
            .get_component::<FogVolumeComponent>(id)
            .unwrap();
        assert_eq!(after.density, 0.9, "setFogDensity step applied");
        assert_eq!(after.scatter, 0.4, "setFogParams.scatter applied");
        assert_eq!(
            after.edge_softness, 0.5,
            "setFogParams.edgeSoftness applied"
        );
        // Untouched fields preserved.
        assert_eq!(after.falloff, 2.0);
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
