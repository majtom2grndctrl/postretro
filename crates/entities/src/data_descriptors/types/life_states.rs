//! Authored player life-state names and their lowering into the mod name table.
//! See: context/lib/scripting.md §12.

use std::collections::BTreeMap;

use postretro_foundation::LifeStateProfile;

#[derive(Debug, Clone, PartialEq)]
pub struct LifeStatesDescriptor {
    pub initial: String,
    pub states: BTreeMap<String, LifeStateProfile>,
    pub rejoin: BTreeMap<String, String>,
    pub state_ids: BTreeMap<String, u8>,
    pub table_revision: u8,
}

impl LifeStatesDescriptor {
    pub fn state(&self, name: &str) -> Option<(u8, &LifeStateProfile)> {
        Some((*self.state_ids.get(name)?, self.states.get(name)?))
    }

    pub fn profile(&self, id: u8) -> Option<&LifeStateProfile> {
        let name = self
            .state_ids
            .iter()
            .find_map(|(name, &state_id)| (state_id == id).then_some(name))?;
        self.states.get(name)
    }

    /// Parsed blocks always declare `initial`; registry lowering assigns its id
    /// before any pawn can materialize from the descriptor.
    pub fn initial_state(&self) -> (u8, &LifeStateProfile) {
        self.state(&self.initial)
            .expect("validated life-state initial is lowered")
    }
}
