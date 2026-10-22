// Received link state: replicated records into link history, and the drop back to spawn state.
// See: context/lib/networking.md §Current contract · context/lib/entity_model.md §7d

use postretro_level_format::ghost_links::GhostMembers;

use super::GhostLinks;
use super::history::LinkState;
use crate::netcode::{GhostLinkUpdate, NetEndpoint};

impl GhostLinks {
    /// Push one received link state into its link's history at the stamp of the
    /// snapshot that carried it. Netcode bound the record to this link by
    /// map-order index and source entity; a repeated state changes nothing.
    pub(crate) fn apply_received(&mut self, update: GhostLinkUpdate) {
        let Some(history) = self.history_mut(update.state.index) else {
            return;
        };
        history.push(
            update.server_tick,
            LinkState {
                armed: update.state.armed,
                // The net crate rejected reserved bits before apply.
                members: GhostMembers::from_bits_truncate(update.state.members),
            },
        );
    }

    /// Drop every received state: each link returns to its local spawn state,
    /// presented through the membership ramp. Demotion calls this; a level
    /// unload drops the whole table.
    pub(crate) fn reset_received_state(&mut self) {
        for link in &mut self.links {
            link.history.reset_to_spawn();
        }
    }
}

impl crate::App {
    /// On a connected client, move this frame's received link states into link
    /// history before membership reads it. Every state is kept, so one that
    /// lasted only between two frames still presents at its own host time.
    pub(crate) fn drain_received_ghost_link_state(&mut self) {
        let Some(NetEndpoint::Client { replication, .. }) = self
            .session
            .as_mut()
            .and_then(|session| session.net_endpoint.as_mut())
        else {
            return;
        };
        for update in replication.drain_ghost_link_updates() {
            self.ghost_links.apply_received(update);
        }
    }
}

#[cfg(test)]
#[path = "replication_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "replication_test_fixtures.rs"]
mod test_fixtures;

#[cfg(test)]
#[path = "replication_loopback_tests.rs"]
mod loopback_tests;

#[cfg(test)]
#[path = "replication_mover_tests.rs"]
mod mover_tests;
