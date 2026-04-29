// Built-in classname dispatch: map FGD `classname` → handler that spawns an
// engine-native entity with components configured from FGD KVPs.
// See: context/plans/in-progress/scripting-foundation/plan-3-emitter-entity.md §Sub-plan 6

use std::collections::HashMap;

use glam::Vec3;

use crate::scripting::registry::EntityRegistry;

pub(crate) mod billboard_emitter;

/// One map entity as it is presented to a built-in classname handler. Mirrors
/// the FGD KVP shape: a flat string→string map plus an origin and an optional
/// `_tags` list (data-script-setup convention).
///
/// The runtime side of the engine doesn't yet have its own `MapEntity` struct;
/// this is the shape sub-plan 8 will populate from the level-load sweep. Until
/// then, this type also drives the unit tests for sub-plan 6.
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
/// engine init, before any level loads. Sub-plan 8 will wire the level loader
/// to consult the resulting dispatch table when sweeping map entities.
pub(crate) fn register_builtins(dispatch: &mut ClassnameDispatch) {
    dispatch.register(
        billboard_emitter::CLASSNAME,
        billboard_emitter::handle as ClassnameHandler,
    );
}

#[cfg(test)]
mod tests {
    use super::*;

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
