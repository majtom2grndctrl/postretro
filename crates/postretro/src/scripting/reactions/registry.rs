// Registrar functions for tag-targeted reaction primitives.
// See: context/lib/scripting.md §10 (Reaction Primitives)

use crate::scripting::ctx::ScriptCtx;
use crate::scripting::sequence::{SequenceError, SequencedPrimitiveRegistry};

use super::ReactionError;
pub(crate) use postretro_scripting_core::reaction_registry::ReactionPrimitiveRegistry;

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
    registry.register("setAnimationState", |reg, targets, args| {
        let parsed: super::set_animation_state::SetAnimationStateArgs =
            serde_json::from_value(args.clone()).map_err(|e| ReactionError::InvalidArgument {
                reason: format!("setAnimationState: failed to deserialize args: {e}"),
            })?;
        super::set_animation_state::dispatch(reg, targets, &parsed)
    });
    registry.register("applyDamage", |reg, targets, args| {
        let parsed: super::apply_damage::ApplyDamageArgs = serde_json::from_value(args.clone())
            .map_err(|e| ReactionError::InvalidArgument {
                reason: format!("applyDamage: failed to deserialize args: {e}"),
            })?;
        super::apply_damage::dispatch(reg, targets, &parsed)
    });
}

pub(crate) fn register_fog_reaction_primitives(registry: &mut ReactionPrimitiveRegistry) {
    registry.register("setFogDensity", |reg, targets, args| {
        let parsed: super::set_fog_density::SetFogDensityArgs =
            serde_json::from_value(args.clone()).map_err(|e| ReactionError::InvalidArgument {
                reason: format!("setFogDensity: failed to deserialize args: {e}"),
            })?;
        super::set_fog_density::dispatch(reg, targets, &parsed)
    });
    registry.register("setFogGlow", |reg, targets, args| {
        let parsed: super::set_fog_glow::SetFogGlowArgs = serde_json::from_value(args.clone())
            .map_err(|e| ReactionError::InvalidArgument {
                reason: format!("setFogGlow: failed to deserialize args: {e}"),
            })?;
        super::set_fog_glow::dispatch(reg, targets, &parsed)
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
    registry.register("setFogAnimation", |reg, targets, args| {
        let parsed: super::set_fog_animation::SetFogAnimationArgs =
            serde_json::from_value(args.clone()).map_err(|e| ReactionError::InvalidArgument {
                reason: format!("setFogAnimation: failed to deserialize args: {e}"),
            })?;
        super::set_fog_animation::dispatch(reg, targets, &parsed)
    });
}

// Bridges the reaction error type into the sequenced-dispatch error surface
// so fog failures are reported uniformly with other sequenced-primitive failures.
fn reaction_to_sequence_error(err: ReactionError) -> SequenceError {
    let reason = match err {
        ReactionError::InvalidArgument { reason } => reason,
    };
    SequenceError::InvalidArgument { reason }
}

/// Register fog primitives as sequenced (per-step) handlers so `fogPulse` /
/// `fogFade` step arrays can be dispatched through `SequencedPrimitiveRegistry`
/// at run time.
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

    let ctx_glow = ctx.clone();
    registry.register("setFogGlow", move |id, args| {
        let parsed: super::set_fog_glow::SetFogGlowArgs = serde_json::from_value(args.clone())
            .map_err(|e| SequenceError::InvalidArgument {
                reason: format!("setFogGlow: failed to deserialize args: {e}"),
            })?;
        let mut reg = ctx_glow.registry.borrow_mut();
        super::set_fog_glow::dispatch(&mut reg, &[id], &parsed).map_err(reaction_to_sequence_error)
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

    let ctx_params = ctx.clone();
    registry.register("setFogParams", move |id, args| {
        let parsed: super::set_fog_params::SetFogParamsArgs = serde_json::from_value(args.clone())
            .map_err(|e| SequenceError::InvalidArgument {
                reason: format!("setFogParams: failed to deserialize args: {e}"),
            })?;
        let mut reg = ctx_params.registry.borrow_mut();
        super::set_fog_params::dispatch(&mut reg, &[id], &parsed)
            .map_err(reaction_to_sequence_error)
    });

    let ctx_animation = ctx;
    registry.register("setFogAnimation", move |id, args| {
        let parsed: super::set_fog_animation::SetFogAnimationArgs =
            serde_json::from_value(args.clone()).map_err(|e| SequenceError::InvalidArgument {
                reason: format!("setFogAnimation: failed to deserialize args: {e}"),
            })?;
        let mut reg = ctx_animation.registry.borrow_mut();
        super::set_fog_animation::dispatch(&mut reg, &[id], &parsed)
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
        assert!(r.contains("setAnimationState"));
        assert!(r.contains("applyDamage"));
        assert!(!r.contains("setLightAnimation"));
    }

    #[test]
    fn registers_all_fog_primitives_under_expected_names() {
        let mut r = ReactionPrimitiveRegistry::new();
        register_fog_reaction_primitives(&mut r);
        assert!(r.contains("setFogDensity"));
        assert!(r.contains("setFogGlow"));
        assert!(r.contains("setFogEdgeSoftness"));
        assert!(r.contains("setFogFalloff"));
        assert!(r.contains("setFogParams"));
        assert!(r.contains("setFogAnimation"));
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
        assert!(r.contains("setFogGlow"));
        assert!(r.contains("setFogEdgeSoftness"));
        assert!(r.contains("setFogFalloff"));
        assert!(r.contains("setFogParams"));
        assert!(r.contains("setFogAnimation"));
    }

    #[test]
    fn sequenced_fog_primitive_round_trip_through_dispatcher() {
        // End-to-end: a `Sequence` reaction whose steps name fog primitives
        // must (a) survive `validate_sequence_primitives` (the registry
        // contains the names) and (b) mutate the targeted fog component when
        // fired through `fire_named_event_with_sequences`. This is exactly
        // the path `fogPulse` / `fogFade` step arrays travel at level load.
        use crate::scripting::ctx::ScriptCtx;
        use crate::scripting::data_descriptors::{NamedReaction, ReactionDescriptor, SequenceStep};
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
                    glow: 0.6,
                    edge_softness: 0.25,
                    falloff: 2.0,
                    tint: [1.0, 1.0, 1.0],
                    saturation: 1.0,
                    min_brightness: 0.0,
                    light_range: 1.0,
                    animation: None,
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
                        "glow": 0.4,
                        "edgeSoftness": 0.5,
                    }),
                },
            ]),
        }];

        // Validation must accept all six fog primitive names; nothing dropped.
        let validated = validate_sequence_primitives(raw_reactions, &seq_reg);
        assert_eq!(validated.len(), 1, "fog steps survived validation");

        let mut data = DataRegistry::new();
        data.populate_level(validated, Vec::new(), &[]);

        let reaction_reg = ReactionPrimitiveRegistry::new();
        let system_reg =
            crate::scripting::reactions::system_commands::SystemReactionRegistry::new();
        fire_named_event_with_sequences(
            "levelLoad",
            &data,
            &seq_reg,
            &reaction_reg,
            &system_reg,
            &script_ctx,
        );

        let after = script_ctx
            .registry
            .borrow()
            .get_component::<FogVolumeComponent>(id)
            .unwrap()
            .clone();
        assert_eq!(after.density, 0.9, "setFogDensity step applied");
        assert_eq!(after.glow, 0.4, "setFogParams.glow applied");
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
                glow: 0.6,
                edge_softness: 0.25,
                falloff: 2.0,
                tint: [1.0, 1.0, 1.0],
                saturation: 1.0,
                min_brightness: 0.0,
                light_range: 1.0,
                animation: None,
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
        assert_eq!(after.glow, 0.6);
        assert_eq!(after.edge_softness, 0.5);
        assert_eq!(after.falloff, 3.0);
    }
}
