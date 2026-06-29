// Runtime registry types for named reaction dispatch.
// See: context/lib/scripting.md §10

use std::collections::HashMap;

use thiserror::Error;

pub use postretro_entities::reactions::system_commands::{
    SystemCommandQueue, SystemReactionCommand,
};
use postretro_entities::registry::{EntityId, EntityRegistry};

/// Errors a reaction-primitive dispatcher may return. Dispatchers log per-target
/// failures inline, so the `Result` exists for invariant violations.
#[derive(Debug, Error, PartialEq)]
pub enum ReactionError {
    #[error("invalid argument: {reason}")]
    InvalidArgument { reason: String },
}

pub type ReactionPrimitiveFn =
    Box<dyn Fn(&mut EntityRegistry, &[EntityId], &serde_json::Value) -> Result<(), ReactionError>>;

#[derive(Default)]
pub struct ReactionPrimitiveRegistry {
    handlers: HashMap<String, ReactionPrimitiveFn>,
}

impl ReactionPrimitiveRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register<F>(&mut self, name: impl Into<String>, handler: F)
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

    pub fn contains(&self, name: &str) -> bool {
        self.handlers.contains_key(name)
    }

    pub fn get(&self, name: &str) -> Option<&ReactionPrimitiveFn> {
        self.handlers.get(name)
    }

    pub fn dispatch(
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

pub type SystemReactionFn =
    Box<dyn Fn(&serde_json::Value, &SystemCommandQueue) -> Result<(), ReactionError>>;

#[derive(Default)]
pub struct SystemReactionRegistry {
    handlers: HashMap<String, SystemReactionFn>,
}

impl SystemReactionRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register<F>(&mut self, name: impl Into<String>, handler: F)
    where
        F: Fn(&serde_json::Value, &SystemCommandQueue) -> Result<(), ReactionError> + 'static,
    {
        let name = name.into();
        if self.handlers.contains_key(&name) {
            debug_assert!(false, "duplicate system reaction registration: {name}");
            log::warn!(
                "[Scripting] SystemReactionRegistry: overwriting existing handler for '{name}'"
            );
        }
        self.handlers.insert(name, Box::new(handler));
    }

    pub fn contains(&self, name: &str) -> bool {
        self.handlers.contains_key(name)
    }

    pub fn dispatch(
        &self,
        name: &str,
        args: &serde_json::Value,
        queue: &SystemCommandQueue,
    ) -> Result<bool, ReactionError> {
        let Some(handler) = self.handlers.get(name) else {
            return Ok(false);
        };
        handler(args, queue).map(|_| true)
    }
}

impl std::fmt::Debug for SystemReactionRegistry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SystemReactionRegistry")
            .field("handlers", &self.handlers.keys().collect::<Vec<_>>())
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reaction_registry_registers_and_dispatches_handler() {
        let mut registry = ReactionPrimitiveRegistry::new();
        registry.register("noop", |_reg, _targets, _args| Ok(()));

        let mut entities = EntityRegistry::new();
        let dispatched = registry
            .dispatch("noop", &mut entities, &[], &serde_json::Value::Null)
            .unwrap();

        assert!(dispatched);
        assert!(
            !registry
                .dispatch("missing", &mut entities, &[], &serde_json::Value::Null)
                .unwrap()
        );
    }
}
