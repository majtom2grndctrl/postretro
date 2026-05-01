// Built-in classname dispatch: map FGD `classname` → handler that spawns an
// engine-native entity with components configured from FGD KVPs.
// See: context/lib/build_pipeline.md §Built-in Classname Routing

use std::collections::{HashMap, HashSet};

use crate::scripting::registry::EntityRegistry;

pub(crate) mod billboard_emitter;
pub(crate) mod data_archetype;

// Used by `main.rs` for the level-load sweep. The `gen-script-types` bin
// includes the scripting tree via `#[path]` but never references this
// re-export, so the warning fires there only.
#[allow(unused_imports)]
pub(crate) use data_archetype::apply_data_archetype_dispatch;

// Re-export so call sites that say `super::MapEntity` (handlers, tests) keep
// working unchanged. The struct itself lives in `scripting::map_entity`.
pub(crate) use crate::scripting::map_entity::MapEntity;

/// Built-in classname handler. Reads KVPs from `entity`, applies any defaults,
/// spawns one ECS entity (with a `Transform` at `entity.origin`), copies tags,
/// and attaches the configured component(s).
///
/// Returns the spawned entity's id on success. Returns `None` only when the
/// registry is exhausted — handlers themselves do not fail on bad KVP data;
/// per the spec they log-and-fall-back so `level_load` keeps going.
pub(crate) type ClassnameHandler = fn(
    entity: &MapEntity,
    registry: &mut EntityRegistry,
) -> Option<crate::scripting::registry::EntityId>;

/// Engine-wide dispatch table of built-in classname handlers. Built once at
/// engine init via [`register_builtins`]; survives level unload — built-in
/// handlers are never cleared.
#[derive(Default)]
pub(crate) struct ClassnameDispatch {
    handlers: HashMap<&'static str, ClassnameHandler>,
}

impl ClassnameDispatch {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// Register a handler for a classname. Panics on duplicate registration —
    /// the built-in table is built once at engine init from a fixed list, so a
    /// duplicate indicates a programming error, not a recoverable runtime
    /// condition.
    pub(crate) fn register(&mut self, classname: &'static str, handler: ClassnameHandler) {
        let prior = self.handlers.insert(classname, handler);
        assert!(
            prior.is_none(),
            "[Loader] duplicate classname handler registered: {classname}"
        );
    }

    pub(crate) fn lookup(&self, classname: &str) -> Option<ClassnameHandler> {
        self.handlers.get(classname).copied()
    }

    /// Test-only: count of registered handlers. Used by unit tests asserting
    /// that `register_builtins` covers the expected set.
    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.handlers.len()
    }
}

/// Populate `dispatch` with every built-in classname handler. Called once at
/// engine init, before any level loads. The level loader consults the
/// resulting dispatch table via [`apply_classname_dispatch`] when sweeping
/// map entities.
pub(crate) fn register_builtins(dispatch: &mut ClassnameDispatch) {
    dispatch.register(
        billboard_emitter::CLASSNAME,
        billboard_emitter::handle as ClassnameHandler,
    );
}

/// Walk `entities` and dispatch each one through the built-in classname
/// handler table. Unregistered classnames are skipped with a `log::debug!` —
/// not an error: a level legitimately may carry classnames the engine
/// doesn't handle natively (mod-defined types, future engine types). Per the
/// data-script-setup error policy, the loader logs and continues; bad data
/// in one entity must not fail the level load.
///
/// # Returns
///
/// The set of classnames *claimed* by a built-in handler — every classname
/// for which a handler exists in `dispatch`, regardless of whether the handler
/// successfully spawned an entity (i.e., regardless of whether it returned
/// `Some` or `None`). A classname that appears in this set must not be
/// re-processed by [`apply_data_archetype_dispatch`]; built-in dispatch always
/// wins, even when the registry was exhausted and no entity actually landed.
/// Contrast with [`apply_data_archetype_dispatch`], which returns only
/// classnames for which at least one entity was materialized.
pub(crate) fn apply_classname_dispatch(
    entities: &[MapEntity],
    dispatch: &ClassnameDispatch,
    registry: &mut EntityRegistry,
) -> HashSet<String> {
    let mut handled: HashSet<String> = HashSet::new();
    for entity in entities {
        match dispatch.lookup(&entity.classname) {
            Some(handler) => {
                // Mark the classname owned by the built-in *before* invoking
                // the handler. Even if the handler returns `None` (registry
                // exhausted, etc.), we must not let the data-archetype sweep
                // re-handle this classname — built-in dispatch wins.
                handled.insert(entity.classname.clone());
                // Uniform KVP-write point for all built-in handlers. Writing
                // the KVP bag for every map-spawned entity (even those with no
                // KVPs) keeps `getEntityProperty` consistent across spawn paths
                // and lets callers distinguish map-spawned entities (always
                // have a kvp_table entry) from runtime-spawned ones (never do).
                // New built-in handlers inherit this behavior without needing
                // to call `set_map_kvps` themselves. (The data-archetype path
                // in `data_archetype.rs` is separate and handles its own KVP
                // write.)
                if let Some(entity_id) = handler(entity, registry) {
                    let _ = registry.set_map_kvps(entity_id, entity.key_values.clone());
                }
            }
            None => {
                log::debug!(
                    "[Loader] unknown classname '{}', skipping",
                    entity.classname,
                );
            }
        }
    }
    handled
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scripting::registry::ComponentKind;
    use glam::Vec3;

    #[test]
    fn register_builtins_registers_billboard_emitter() {
        let mut dispatch = ClassnameDispatch::new();
        register_builtins(&mut dispatch);
        assert!(
            dispatch.lookup("billboard_emitter").is_some(),
            "billboard_emitter should be registered"
        );
        // Lock the count so adding a new built-in is a deliberate, reviewed
        // change rather than an accidental fall-through.
        assert_eq!(dispatch.len(), 1);
    }

    #[test]
    fn unregistered_classname_returns_none() {
        let mut dispatch = ClassnameDispatch::new();
        register_builtins(&mut dispatch);
        assert!(dispatch.lookup("never_registered").is_none());
    }

    #[test]
    fn apply_classname_dispatch_routes_known_classnames() {
        use crate::scripting::components::billboard_emitter::BillboardEmitterComponent;

        let mut dispatch = ClassnameDispatch::new();
        register_builtins(&mut dispatch);

        let mut kv = HashMap::new();
        kv.insert("rate".to_string(), "9.5".to_string());
        let entities = vec![MapEntity {
            classname: "billboard_emitter".to_string(),
            origin: Vec3::new(1.0, 2.0, 3.0),
            key_values: kv,
            angles: Vec3::ZERO,
            tags: vec!["fx".into()],
        }];

        let mut registry = EntityRegistry::new();
        let handled = apply_classname_dispatch(&entities, &dispatch, &mut registry);
        assert_eq!(handled.len(), 1);
        assert!(handled.contains("billboard_emitter"));

        // Confirm the entity actually landed with the configured component.
        let found = registry
            .iter_with_kind(ComponentKind::BillboardEmitter)
            .next();
        let (id, _) = found.expect("a billboard_emitter entity should exist");
        let component = registry
            .get_component::<BillboardEmitterComponent>(id)
            .expect("component should be readable");
        assert_eq!(component.rate, 9.5);
        let tags = registry.get_tags(id).expect("tags should be set");
        assert_eq!(tags, &["fx".to_string()]);
        // Verify the KVP bag was written: `set_map_kvps` runs inside
        // `apply_classname_dispatch` so `getEntityProperty` works uniformly.
        let kvp_rate = registry
            .get_map_kvp(id, "rate")
            .expect("KVP lookup should not fail on a live id");
        assert_eq!(
            kvp_rate.as_deref(),
            Some("9.5"),
            "raw 'rate' KVP should be stored verbatim in the entity's KVP bag"
        );
    }

    #[test]
    fn apply_classname_dispatch_skips_unregistered_classnames() {
        let mut dispatch = ClassnameDispatch::new();
        register_builtins(&mut dispatch);

        let entities = vec![MapEntity {
            classname: "never_registered".to_string(),
            origin: Vec3::ZERO,
            key_values: HashMap::new(),
            angles: glam::Vec3::ZERO,
            tags: vec![],
        }];

        let mut registry = EntityRegistry::new();
        let handled = apply_classname_dispatch(&entities, &dispatch, &mut registry);
        // No handler ran; no entity was spawned. Logged at debug level.
        assert!(handled.is_empty());
        assert!(
            registry
                .iter_with_kind(ComponentKind::BillboardEmitter)
                .next()
                .is_none()
        );
    }

    #[test]
    #[should_panic(expected = "duplicate classname handler")]
    fn duplicate_registration_panics() {
        let mut dispatch = ClassnameDispatch::new();
        dispatch.register(
            billboard_emitter::CLASSNAME,
            billboard_emitter::handle as ClassnameHandler,
        );
        dispatch.register(
            billboard_emitter::CLASSNAME,
            billboard_emitter::handle as ClassnameHandler,
        );
    }
}
