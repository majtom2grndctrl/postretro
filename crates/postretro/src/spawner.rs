//! Shared, VM-free state for fixed-tick `entity_spawner` execution.
//!
//! The context is session-owned because reaction registries retain closures
//! across level reloads. Its per-level interior is replaced atomically during
//! lifecycle install, leaving the later fixed-tick executor no reason to enter
//! a script context or data registry.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use postretro_foundation::NavAgentParams;
use postretro_scripting_core::data_descriptors::EntityTypeDescriptor;

#[derive(Debug, Default)]
pub(crate) struct SpawnContextState {
    pub(crate) resolved_enemy_descriptors: HashMap<String, EntityTypeDescriptor>,
    pub(crate) agent_params: Option<NavAgentParams>,
    /// Task 2 uses this for one warning per missing spawner tag per level.
    pub(crate) warned_zero_match_tags: HashSet<String>,
}

/// Session-built shared handle supplied to both trigger and app-side command
/// routes. It is intentionally not an ECS component and therefore is not part
/// of the serde component vocabulary.
#[derive(Debug, Clone, Default)]
pub(crate) struct SpawnContext {
    state: Rc<RefCell<SpawnContextState>>,
}

impl SpawnContext {
    pub(crate) fn replace_level_data(
        &self,
        resolved_enemy_descriptors: HashMap<String, EntityTypeDescriptor>,
        agent_params: Option<NavAgentParams>,
    ) {
        *self.state.borrow_mut() = SpawnContextState {
            resolved_enemy_descriptors,
            agent_params,
            warned_zero_match_tags: HashSet::new(),
        };
    }

    pub(crate) fn clear(&self) {
        *self.state.borrow_mut() = SpawnContextState::default();
    }

    pub(crate) fn state(&self) -> std::cell::Ref<'_, SpawnContextState> {
        self.state.borrow()
    }

}
