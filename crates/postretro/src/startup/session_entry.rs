//! In-app session entry: a single-player machine at its frontend hosts or
//! joins without a relaunch, through the construction path boot flags take.
//! See: context/lib/networking.md §Role model (Runtime entry) ·
//! context/lib/scripting.md §10.4 (Session membership)

use std::net::SocketAddr;

use crate::App;
use crate::netcode;
use crate::session::net_entry::{EntryFailure, enter_role};
use crate::session::progress_save;
use crate::session::role_state::SessionRole;
use crate::startup::LevelRequest;

impl App {
    /// Whether this machine stands at its frontend: the frontend menu is up
    /// and no level a session plays is installed. A backdrop, installed or
    /// loading, counts as the frontend; a played load cannot be queued or in
    /// flight under the menu, because starting one clears it.
    pub(crate) fn at_frontend(&self) -> bool {
        self.frontend_menu_is_present()
            && !self
                .session
                .as_ref()
                .is_some_and(|session| session.session_players.session_level())
    }

    /// Join the host at `addr` from the frontend. A backdrop load in flight is
    /// cancelled: the host's relevel would discard it. An installed backdrop
    /// stays until the relevel reloads it as the host's level. Returns whether
    /// a client endpoint now exists.
    pub(crate) fn join_from_frontend(&mut self, addr: SocketAddr) -> bool {
        if self.session_role() != SessionRole::SinglePlayer || !self.at_frontend() {
            log::warn!(
                "[Net] join refused: entry needs a single-player machine at its frontend"
            );
            return false;
        }
        self.cancel_backdrop_load_for_join();
        self.save_progress_for_join();
        let Some(failure) =
            self.enter_net_role(&netcode::NetRole::Connect { addr }, getrandom::fill)
        else {
            return true;
        };
        log::warn!("[Net] join to {addr} failed before connecting: {failure:?}");
        false
    }

    /// Host on `port` from the frontend: a new hosted run with a fresh session
    /// id and seat counter, seat 0 keeping its carried state. The machine
    /// stands at its frontend with a live endpoint, and joiners wait there
    /// until it starts a level. Returns whether a host endpoint now exists.
    pub(crate) fn host_from_frontend(&mut self, port: u16) -> bool {
        self.host_from_frontend_with(port, getrandom::fill)
    }

    /// [`Self::host_from_frontend`] with the session id drawn from `mint`.
    pub(crate) fn host_from_frontend_with(
        &mut self,
        port: u16,
        mint: netcode::SessionIdSource,
    ) -> bool {
        if self.session_role() != SessionRole::SinglePlayer || !self.at_frontend() {
            log::warn!(
                "[Net] host refused: entry needs a single-player machine at its frontend"
            );
            return false;
        }
        let Some(failure) = self.enter_net_role(&netcode::NetRole::Host { port }, mint) else {
            return true;
        };
        log::warn!("[Net] hosting on port {port} failed; staying single-player: {failure:?}");
        false
    }

    /// Save the world and character before the first connect packet, then
    /// rebase the leave-restore snapshot, and with it the join seed, on what
    /// the save collected: a later leave restores the world as it was at
    /// join. A failed write has warned and rebases all the same.
    fn save_progress_for_join(&mut self) {
        let Some(session) = self.session.as_mut() else {
            return;
        };
        if let Some(save) = progress_save::save_world_and_character(session) {
            session.persisted_state = Some(save.state);
        }
    }

    /// Drop a queued backdrop load and cancel one in flight. The frontend
    /// menu stays up.
    fn cancel_backdrop_load_for_join(&mut self) {
        self.level_requests
            .retain(|request| !matches!(request, LevelRequest::Load(_)));
        if self.level_load_in_flight() && !self.cancel_beside_level_load() {
            self.abandon_level_load_for_frontend();
        }
    }

    /// Enter `net_role` through the shared construction path, then seed the
    /// new endpoint with this mod's admission identity and parity digest, as
    /// mod init seeds a boot endpoint. Returns the failure that left the
    /// machine single-player, if any.
    fn enter_net_role(
        &mut self,
        net_role: &netcode::NetRole,
        mint: netcode::SessionIdSource,
    ) -> Option<EntryFailure> {
        let session = self.session.as_mut()?;
        let entry = enter_role(
            net_role,
            session.player_options.player_id,
            &mut session.scripting,
            &mut session.seat_table,
            mint,
            netcode::probe_lan_address,
        );
        netcode::clear_released_seat_slot_values(
            &mut session.scripting.script_ctx.slot_table.borrow_mut(),
            entry.released,
        );
        session.net_endpoint = entry.endpoint;
        self.install_network_mod_content();
        entry.failure
    }
}
