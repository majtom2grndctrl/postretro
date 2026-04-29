// Built-in classname dispatch: map FGD `classname` → handler that spawns an
// engine-native entity with components configured from FGD KVPs.
// See: context/lib/scripting.md

use std::collections::HashMap;

use glam::Vec3;

use crate::scripting::registry::EntityRegistry;

pub(crate) mod billboard_emitter;

/// One map entity as it is presented to a built-in classname handler. Mirrors
/// the FGD KVP shape: a flat string→string map plus an origin and an optional
/// `_tags` list (data-script-setup convention).
///
/// The level loader calls [`apply_classname_dispatch`] on this list at level
/// load. Until the PRL wire format gains a generic map-entity section, this
/// list is always empty at load time — populating it is the only remaining
/// step.
#[derive(Debug, Clone)]
pub(crate) struct MapEntity {
    pub(crate) classname: String,
    pub(crate) origin: Vec3,
    pub(crate) key_values: HashMap<String, String>,
    pub(crate) tags: Vec<String>,
}

impl MapEntity {
    /// Convenience: assemble the diagnostic prefix used by handlers when they
    /// log warnings about a malformed key value. Format mirrors classic
    /// `id Tech` baker logs: `classname @ (x, y, z)`.
    pub(crate) fn diagnostic_origin(&self) -> String {
        format!(
            "{} @ ({:.3}, {:.3}, {:.3})",
            self.classname, self.origin.x, self.origin.y, self.origin.z
        )
    }
}

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
/// Returns the count of successfully dispatched entities (handler returned
/// `Some(EntityId)`), for diagnostics.
pub(crate) fn apply_classname_dispatch(
    entities: &[MapEntity],
    dispatch: &ClassnameDispatch,
    registry: &mut EntityRegistry,
) -> usize {
    let mut spawned = 0usize;
    for entity in entities {
        match dispatch.lookup(&entity.classname) {
            Some(handler) => {
                if handler(entity, registry).is_some() {
                    spawned += 1;
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
    spawned
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scripting::registry::ComponentKind;

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
            tags: vec!["fx".into()],
        }];

        let mut registry = EntityRegistry::new();
        let spawned = apply_classname_dispatch(&entities, &dispatch, &mut registry);
        assert_eq!(spawned, 1);

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
    }

    #[test]
    fn apply_classname_dispatch_skips_unregistered_classnames() {
        let mut dispatch = ClassnameDispatch::new();
        register_builtins(&mut dispatch);

        let entities = vec![MapEntity {
            classname: "never_registered".to_string(),
            origin: Vec3::ZERO,
            key_values: HashMap::new(),
            tags: vec![],
        }];

        let mut registry = EntityRegistry::new();
        let spawned = apply_classname_dispatch(&entities, &dispatch, &mut registry);
        // No handler ran; no entity was spawned. Logged at debug level.
        assert_eq!(spawned, 0);
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
