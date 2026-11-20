// Emissive reaction primitives: tag-targeted and sequenced writes of an
// emissive volume's scripted brightness, colour and brightness animation.
// See: context/lib/scripting.md §10.2a

pub(crate) mod set_emissive_animation;
pub(crate) mod set_emissive_brightness;
pub(crate) mod set_emissive_color;

use serde::de::DeserializeOwned;

use postretro_entities::{
    ComponentKind, ComponentValue, EmissiveVolumeComponent, EntityId, EntityRegistry, ScriptCtx,
};
use postretro_scripting_core::reaction_registry::{ReactionError, ReactionPrimitiveRegistry};
use postretro_scripting_core::sequence::{SequenceError, SequencedPrimitiveRegistry};

/// Every primitive that writes an emissive volume. None carries the authored
/// `min` or `max`: the map owns the range.
pub const EMISSIVE_PRIMITIVES: [&str; 3] = [
    "setEmissiveBrightness",
    "setEmissiveColor",
    "setEmissiveAnimation",
];

fn parse<T: DeserializeOwned>(primitive: &str, args: &serde_json::Value) -> Result<T, String> {
    serde_json::from_value(args.clone())
        .map_err(|e| format!("{primitive}: failed to deserialize args: {e}"))
}

fn dispatch_parsed(
    primitive: &str,
    registry: &mut EntityRegistry,
    targets: &[EntityId],
    args: &serde_json::Value,
) -> Result<(), String> {
    match primitive {
        "setEmissiveBrightness" => {
            set_emissive_brightness::dispatch(registry, targets, &parse(primitive, args)?);
        }
        "setEmissiveColor" => {
            set_emissive_color::dispatch(registry, targets, &parse(primitive, args)?);
        }
        "setEmissiveAnimation" => {
            set_emissive_animation::dispatch(registry, targets, &parse(primitive, args)?);
        }
        other => unreachable!("`{other}` is not an emissive primitive"),
    }
    Ok(())
}

pub fn register_emissive_reaction_primitives(registry: &mut ReactionPrimitiveRegistry) {
    for primitive in EMISSIVE_PRIMITIVES {
        registry.register(primitive, move |reg, targets, args| {
            dispatch_parsed(primitive, reg, targets, args)
                .map_err(|reason| ReactionError::InvalidArgument { reason })
        });
    }
}

/// Register the emissive primitives as sequenced (per-step) handlers, so the
/// steps `EmissiveVolumeHandle`'s `pulse` / `fade` / `flicker` return run
/// through `SequencedPrimitiveRegistry`.
pub fn register_sequenced_emissive_primitives(
    registry: &mut SequencedPrimitiveRegistry,
    ctx: ScriptCtx,
) {
    for primitive in EMISSIVE_PRIMITIVES {
        let ctx = ctx.clone();
        registry.register(primitive, move |id, args| {
            let mut reg = ctx.registry.borrow_mut();
            dispatch_parsed(primitive, &mut reg, &[id], args)
                .map_err(|reason| SequenceError::InvalidArgument { reason })
        });
    }
}

/// Apply `write` to every target's emissive volume. A target with no emissive
/// volume (a tag matching another kind) warns and is skipped while the others
/// are still written. Each primitive returns before this
/// on an empty target set (a tag matching nothing), a debug no-op. Writes in
/// place: nothing is cloned.
fn write_targets(
    registry: &mut EntityRegistry,
    targets: &[EntityId],
    primitive: &str,
    mut write: impl FnMut(&mut EmissiveVolumeComponent),
) {
    for &id in targets {
        match registry.get_component_value_mut(id, ComponentKind::EmissiveVolume) {
            Ok(ComponentValue::EmissiveVolume(volume)) => write(volume),
            _ => log::warn!(
                "[Scripting] {primitive}: entity {id} has no EmissiveVolumeComponent; skipping"
            ),
        }
    }
}

#[cfg(test)]
mod tests;
