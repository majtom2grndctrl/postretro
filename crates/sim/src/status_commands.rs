// `applyStatus` / `clearStatus` reaction verbs and their trigger-bound form:
// host-only, applied as `script.applyStatus`, skipping a subject without
// health with a warning.
// See: context/lib/scripting.md §2 (Status declarations), §12

use std::cell::Cell;
use std::rc::Rc;

use serde::Deserialize;

use postretro_entities::components::health::HealthComponent;
use postretro_entities::components::statuses::StatusApplier;
use postretro_entities::{EntityId, EntityRegistry};
use postretro_foundation::AuthoredHandle;
use postretro_foundation::defense::DefenseVocabulary;
use postretro_foundation::status::APPLY_STATUS_SOURCE_ID;
use postretro_scripting_core::reaction_registry::{ReactionError, ReactionPrimitiveRegistry};

pub const APPLY_STATUS: &str = "applyStatus";
pub const CLEAR_STATUS: &str = "clearStatus";

/// Both verbs' args: a status handle.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StatusArgs {
    pub status: AuthoredHandle,
}

/// The status name `args` reference, or why it is a bad reference.
pub fn status_name(primitive: &str, args: &serde_json::Value) -> Result<String, String> {
    let parsed: StatusArgs = serde_json::from_value(args.clone())
        .map_err(|error| format!("{primitive} args: {error}"))?;
    let handle = parsed.status.handle(&format!("{primitive}.status"))?;
    Ok(handle.expect_status()?.to_string())
}

/// Install-time check: the handle names a registered status. A bad reference
/// drops the reaction.
pub fn validate_status_args(
    primitive: &str,
    args: &serde_json::Value,
    vocabulary: &DefenseVocabulary,
) -> Result<(), String> {
    let name = status_name(primitive, args)?;
    vocabulary
        .status_index(&name)
        .map(|_| ())
        .ok_or_else(|| format!("{primitive} `status`: `{name}` is not a registered status"))
}

/// Apply status `name` to every target with health, as `script.applyStatus`.
/// A target without health warns and is skipped; siblings still apply.
pub(crate) fn apply_status_to(registry: &mut EntityRegistry, targets: &[EntityId], name: &str) {
    let Some(status) = registered(registry, name) else {
        return;
    };
    let source = registry.intern_status_source(APPLY_STATUS_SOURCE_ID);
    for &target in targets {
        if !has_health(registry, target, APPLY_STATUS) {
            continue;
        }
        registry.apply_status(target, status, StatusApplier::Source(source), source);
    }
}

/// Remove every applier's instance of status `name` from every target with
/// health. A target without health warns and is skipped.
pub(crate) fn clear_status_on(registry: &mut EntityRegistry, targets: &[EntityId], name: &str) {
    let Some(status) = registered(registry, name) else {
        return;
    };
    for &target in targets {
        if has_health(registry, target, CLEAR_STATUS) {
            registry.clear_status(target, status);
        }
    }
}

fn registered(registry: &EntityRegistry, name: &str) -> Option<u16> {
    let index = registry
        .defense_vocabulary()
        .status_index(name)
        .and_then(|index| u16::try_from(index).ok());
    if index.is_none() {
        log::warn!("[Status] `{name}` is no longer a registered status; skipping");
    }
    index
}

fn has_health(registry: &EntityRegistry, target: EntityId, verb: &str) -> bool {
    let health = registry.get_component::<HealthComponent>(target).is_ok();
    if !health {
        log::warn!("[Status] {verb}: entity {target} has no HealthComponent; skipping");
    }
    health
}

/// Register the two verbs. `authoritative` is the host / single-player role
/// gate (`ScriptCtx::owner_slot_writes_enabled`): a connected client applies
/// and clears nothing, as it simulates no status.
pub fn register_status_reactions(
    registry: &mut ReactionPrimitiveRegistry,
    authoritative: Rc<Cell<bool>>,
) {
    let gate = Rc::clone(&authoritative);
    registry.register(APPLY_STATUS, move |registry, targets, args| {
        let name = status_name(APPLY_STATUS, args)
            .map_err(|reason| ReactionError::InvalidArgument { reason })?;
        if gate.get() {
            apply_status_to(registry, targets, &name);
        }
        Ok(())
    });
    registry.register(CLEAR_STATUS, move |registry, targets, args| {
        let name = status_name(CLEAR_STATUS, args)
            .map_err(|reason| ReactionError::InvalidArgument { reason })?;
        if authoritative.get() {
            clear_status_on(registry, targets, &name);
        }
        Ok(())
    });
}
