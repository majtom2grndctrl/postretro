// Per-link state history stamped in host time, read at each member's presentation clock.
// See: context/lib/entity_model.md §7d · context/lib/networking.md §Current contract

use std::collections::VecDeque;

use postretro_entities::GhostLinkComponent;
use postretro_level_format::ghost_links::GhostMembers;

/// Changes kept past the base. A link changes at most once per host tick and the
/// oldest clock trails host-now by the presentation delay, so pruning keeps far
/// fewer; the cap only bounds memory if no frame prunes.
const MAX_CHANGES: usize = 64;

/// What a link shows at one host time.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct LinkState {
    pub armed: bool,
    pub members: GhostMembers,
}

impl LinkState {
    pub(crate) fn from_component(link: &GhostLinkComponent) -> Self {
        Self {
            armed: link.armed,
            // The column holds the validated PRL bits; reserved bits never reach it.
            members: GhostMembers::from_bits_truncate(link.members),
        }
    }

    /// Whether a member of `category` targets full membership under this state.
    pub(crate) fn shows(self, category: GhostMembers) -> bool {
        self.armed && self.members.contains(category)
    }
}

/// A link's state over host time. Each change is stamped with the host tick
/// whose end it describes, in snapshot-stamp units: a change applied during the
/// fixed tick labelled `N` is stamped `N + 1`, the post-increment tick a snapshot
/// of that state carries. A member presented at fractional tick `f` reads the
/// latest change stamped at or before `ceil(f)`, so a state and the first frame
/// of motion toward the same stamp present together.
///
/// The host and single player record their own component state; a connected
/// client holds its local spawn state until received records are pushed, and
/// returns to it on demotion or unload ([`Self::reset_to_spawn`]).
#[derive(Debug, Clone)]
pub(crate) struct LinkHistory {
    /// Local spawn state: the installed component's starting armed state and
    /// categories. Replication returns to it on demotion.
    #[cfg_attr(not(test), allow(dead_code))]
    spawn: LinkState,
    /// The state in force before the first retained change.
    base: LinkState,
    /// Retained changes, ascending by stamp.
    changes: VecDeque<(u32, LinkState)>,
}

impl LinkHistory {
    pub(crate) fn new(spawn: LinkState) -> Self {
        Self {
            spawn,
            base: spawn,
            changes: VecDeque::with_capacity(8),
        }
    }

    /// The newest known state.
    pub(crate) fn latest(&self) -> LinkState {
        self.changes.back().map_or(self.base, |(_, state)| *state)
    }

    /// The state a member presented at fractional host tick `clock` reads.
    pub(crate) fn state_at(&self, clock: f64) -> LinkState {
        let bound = clock.ceil();
        self.changes
            .iter()
            .rev()
            .find(|(stamp, _)| f64::from(*stamp) <= bound)
            .map_or(self.base, |(_, state)| *state)
    }

    /// Record `state` as of host stamp `stamp`. An unchanged state records
    /// nothing. Stamps arrive non-decreasing; a repeated stamp replaces its
    /// change (latest wins), and a lower one means the stamping clock restarted,
    /// so the new state covers all time ([`Self::rebase`]).
    pub(crate) fn push(&mut self, stamp: u32, state: LinkState) {
        if state == self.latest() {
            return;
        }
        match self.changes.back_mut() {
            Some((last, _)) if stamp < *last => self.rebase(state),
            Some((last, last_state)) if stamp == *last => *last_state = state,
            _ => {
                self.changes.push_back((stamp, state));
                if self.changes.len() > MAX_CHANGES
                    && let Some((_, oldest)) = self.changes.pop_front()
                {
                    self.base = oldest;
                }
            }
        }
    }

    /// The stamping clock restarted (a role change): `state` now covers every
    /// host time, since earlier stamps belong to another clock.
    pub(crate) fn rebase(&mut self, state: LinkState) {
        self.changes.clear();
        self.base = state;
    }

    /// Drop changes no clock at or after `oldest_clock` can read: everything
    /// before the change covering `oldest_clock` folds into the base.
    pub(crate) fn prune_before(&mut self, oldest_clock: f64) {
        let bound = oldest_clock.ceil();
        let Some(covering) = self
            .changes
            .iter()
            .rposition(|(stamp, _)| f64::from(*stamp) <= bound)
        else {
            return;
        };
        for (_, state) in self.changes.drain(..=covering) {
            self.base = state;
        }
    }

    /// Forget every recorded or received change and present local spawn state.
    /// Replication calls it on demotion; until then only tests do.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn reset_to_spawn(&mut self) {
        self.rebase(self.spawn);
    }
}

#[cfg(test)]
#[path = "history_tests.rs"]
mod tests;
