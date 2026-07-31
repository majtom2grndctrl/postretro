// Timed state vocabulary shared by the currently weapon-hosted wieldable machine.
// See: context/lib/entity_model.md §2

use serde::{Deserialize, Serialize};

/// The live state of the currently equipped wieldable.
///
/// The machine is hosted on [`WeaponComponent`](super::weapon::WeaponComponent) while
/// weapons are the only wieldable kind. Equip states join this enum as new variants
/// when switching owns their behavior.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum WieldableState {
    #[default]
    Idle,
    Reloading,
    ShellLoading,
    Lowering,
    Raising,
}

impl WieldableState {
    pub const fn allows_fire(self) -> bool {
        match self {
            Self::Idle => true,
            Self::Reloading => false,
            Self::ShellLoading => false,
            Self::Lowering => false,
            Self::Raising => false,
        }
    }

    pub const fn allows_reload(self) -> bool {
        match self {
            Self::Idle => true,
            Self::Reloading => false,
            Self::ShellLoading => false,
            Self::Lowering => false,
            Self::Raising => false,
        }
    }

    pub const fn is_reload_activity(self) -> bool {
        match self {
            Self::Idle => false,
            Self::Reloading => true,
            Self::ShellLoading => true,
            Self::Lowering => false,
            Self::Raising => false,
        }
    }

    pub const fn is_timed_state(self) -> bool {
        match self {
            Self::Idle => false,
            Self::Reloading => true,
            Self::ShellLoading => true,
            Self::Lowering => true,
            Self::Raising => true,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn state_predicates_define_each_shipped_legality_row() {
        assert_eq!(
            (
                WieldableState::Idle.allows_fire(),
                WieldableState::Idle.allows_reload(),
                WieldableState::Idle.is_reload_activity(),
                WieldableState::Idle.is_timed_state(),
            ),
            (true, true, false, false)
        );
        assert_eq!(
            (
                WieldableState::Reloading.allows_fire(),
                WieldableState::Reloading.allows_reload(),
                WieldableState::Reloading.is_reload_activity(),
                WieldableState::Reloading.is_timed_state(),
            ),
            (false, false, true, true)
        );
        assert_eq!(
            (
                WieldableState::ShellLoading.allows_fire(),
                WieldableState::ShellLoading.allows_reload(),
                WieldableState::ShellLoading.is_reload_activity(),
                WieldableState::ShellLoading.is_timed_state(),
            ),
            (false, false, true, true)
        );
        for state in [WieldableState::Lowering, WieldableState::Raising] {
            assert_eq!(
                (
                    state.allows_fire(),
                    state.allows_reload(),
                    state.is_reload_activity(),
                    state.is_timed_state()
                ),
                (false, false, false, true)
            );
        }
    }
}
