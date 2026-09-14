// Emitter reaction primitives: script-authored named reactions that mutate
// BillboardEmitterComponent state consumed by the emitter bridge.
// See: context/lib/scripting.md §10.1

pub(crate) mod set_emitter_rate;
pub(crate) mod set_spin_rate;

use postretro_scripting_core::reaction_registry::{ReactionError, ReactionPrimitiveRegistry};

pub(crate) fn register_emitter_reaction_primitives(registry: &mut ReactionPrimitiveRegistry) {
    registry.register("setEmitterRate", |reg, targets, args| {
        let parsed: set_emitter_rate::SetEmitterRateArgs = serde_json::from_value(args.clone())
            .map_err(|e| ReactionError::InvalidArgument {
                reason: format!("setEmitterRate: failed to deserialize args: {e}"),
            })?;
        set_emitter_rate::dispatch(reg, targets, &parsed)
    });
    registry.register("setSpinRate", |reg, targets, args| {
        let parsed = set_spin_rate::SetSpinRateArgs::from_json(args)?;
        set_spin_rate::dispatch(reg, targets, &parsed)
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registers_emitter_primitives_under_expected_names() {
        let mut r = ReactionPrimitiveRegistry::new();
        register_emitter_reaction_primitives(&mut r);
        assert!(r.contains("setEmitterRate"));
        assert!(r.contains("setSpinRate"));
        assert!(!r.contains("setAnimationState"));
        assert!(!r.contains("applyDamage"));
    }
}
