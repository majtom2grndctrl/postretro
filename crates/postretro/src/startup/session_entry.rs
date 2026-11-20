//! In-app session entry: a single-player machine at its frontend hosts or
//! joins without a relaunch, through the construction path boot flags take.
//! See: context/lib/networking.md §Role model (Runtime entry) ·
//! context/lib/scripting.md §10.4 (Session membership)

use std::net::SocketAddr;

use postretro_entities::reactions::system_commands::FireOrigin;
use postretro_entities::slot_table::SlotValue;
use postretro_net::wire::{DivergenceReason, HoldingCause, ServerControlMessage};

use super::session_membership::refusal_is_own;
use crate::App;
use crate::netcode;
use crate::session::net_entry::{EntryFailure, enter_role};
use crate::session::progress_save;
use crate::session::role_state::SessionRole;
use crate::startup::LevelRequest;

pub(crate) const JOIN_ADDRESS_SLOT: &str = "session.joinAddress";
/// The password the next `hostSession` challenges joiners for.
pub(crate) const HOST_PASSWORD_SLOT: &str = "session.hostPassword";
/// The password the next `joinSession` answers a challenge with.
pub(crate) const JOIN_PASSWORD_SLOT: &str = "session.joinPassword";
pub(crate) const CONNECTION_SLOT: &str = "session.connection";
pub(crate) const WAIT_REASON_SLOT: &str = "session.waitReason";
pub(crate) const END_CAUSE_SLOT: &str = "session.endCause";

/// `session.connection`: where this machine stands in a session it joins.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ConnectionPhase {
    /// No session, or hosting one (a host reads `session.hosting`).
    Offline,
    /// Dialing, or a press waiting to run.
    Connecting,
    /// Admitted, not participating: the host is at its frontend, loading or
    /// between levels, or content differs.
    Waiting,
    /// Participating in the host's level.
    Playing,
}

impl ConnectionPhase {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Offline => "offline",
            Self::Connecting => "connecting",
            Self::Waiting => "waiting",
            Self::Playing => "playing",
        }
    }
}

/// `session.endCause`: why the last session or attempt ended. Append-only;
/// the mod supplies the wording.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum EndCause {
    SessionEnded,
    HostLost,
    ConnectFailed,
    RefusedMod,
    RefusedPassword,
    BadAddress,
    HostFailed,
}

impl EndCause {
    /// Every value, for coverage checks over the vocabulary. A drift test
    /// holds it equal to the catalog's `SESSION_END_CAUSE_VALUES`.
    #[cfg(test)]
    pub(crate) const ALL: [EndCause; 7] = [
        Self::SessionEnded,
        Self::HostLost,
        Self::ConnectFailed,
        Self::RefusedMod,
        Self::RefusedPassword,
        Self::BadAddress,
        Self::HostFailed,
    ];

    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::SessionEnded => "sessionEnded",
            Self::HostLost => "hostLost",
            Self::ConnectFailed => "connectFailed",
            Self::RefusedMod => "refusedMod",
            Self::RefusedPassword => "refusedPassword",
            Self::BadAddress => "badAddress",
            Self::HostFailed => "hostFailed",
        }
    }
}

/// A host or join press that waits for a left level's unload or an ended
/// host's close. It holds what the press captured, so what runs is what the
/// player saw. Its `Debug` never shows the password.
#[derive(Clone)]
pub(crate) enum PendingEntry {
    Host {
        password: String,
    },
    Join {
        addr: SocketAddr,
        /// The address as the player typed it, remembered once admitted.
        typed: String,
        password: String,
    },
}

impl std::fmt::Debug for PendingEntry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Host { .. } => f.write_str("Host"),
            Self::Join { addr, .. } => write!(f, "Join({addr})"),
        }
    }
}

/// What a client has learned of the session it joined. Reset when a join
/// starts and when the session ends.
#[derive(Debug, Default, Clone)]
pub(crate) struct JoinFacts {
    /// The host has shown it admitted this client: a relevel, a holding
    /// notice, a spawn-pose hint, a roster or a final character arrived.
    pub(crate) admitted: bool,
    /// The latest holding notice since the last participation.
    hold: Option<HoldingCause>,
    /// The host's level is one this client's catalog lacks.
    host_level_missing: bool,
    /// The address the join dialed, as the player typed it: remembered once
    /// admitted.
    address: String,
}

/// `session.waitReason` for a hold. A host level this client lacks reads as
/// `levelDiffers` whatever hold it shows. While this client loads the level
/// its host named (`following`), a level mismatch is the old level against
/// the new one and reads as `host`: it clears when the load declares.
fn wait_reason(
    hold: Option<&HoldingCause>,
    host_level_missing: bool,
    following: bool,
) -> &'static str {
    if host_level_missing {
        return "levelDiffers";
    }
    match hold {
        Some(HoldingCause::ModDigest { .. }) => "modDiffers",
        Some(HoldingCause::LevelIdentity { .. } | HoldingCause::LevelDigest { .. })
            if !following =>
        {
            "levelDiffers"
        }
        Some(HoldingCause::LevelIdentity { .. } | HoldingCause::LevelDigest { .. }) => "host",
        Some(
            HoldingCause::HostLevelAbsent
            | HoldingCause::LevelAbsent { .. }
            | HoldingCause::HostNotRevealed { .. }
            | HoldingCause::ClientNotRevealed { .. },
        )
        | None => "host",
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EntryVerb {
    Host,
    Join,
}

impl EntryVerb {
    const fn name(self) -> &'static str {
        match self {
            Self::Host => "hostSession",
            Self::Join => "joinSession",
        }
    }
}

/// Whether a press may run now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EntryReadiness {
    Ready,
    /// A left level's unload or an ended host's close is pending.
    Waits,
    Refused(&'static str),
}

#[cfg(test)]
thread_local! {
    /// Every `session.connection` value this thread published, in order.
    static PUBLISHED_CONNECTIONS: std::cell::RefCell<Vec<ConnectionPhase>> =
        const { std::cell::RefCell::new(Vec::new()) };
}

/// Take every `session.connection` value this test thread published.
#[cfg(test)]
pub(crate) fn take_published_connections() -> Vec<ConnectionPhase> {
    PUBLISHED_CONNECTIONS.with(|log| std::mem::take(&mut *log.borrow_mut()))
}

impl App {
    /// The `hostSession` verb.
    pub(crate) fn run_host_session_verb(&mut self, source: String, origin: FireOrigin) {
        self.press_entry(EntryVerb::Host, source, origin);
    }

    /// The `joinSession` verb.
    pub(crate) fn run_join_session_verb(&mut self, source: String, origin: FireOrigin) {
        self.press_entry(EntryVerb::Join, source, origin);
    }

    /// A host or join press. Refused with a warning, changing nothing, off the
    /// frontend, in any role but single-player, and while another press
    /// waits. Otherwise it captures what it needs now: it clears the end
    /// cause and takes the address and password. A malformed address ends it
    /// there with `badAddress`. It then runs, or waits for a left level's
    /// unload or an ended host's close, publishing `connecting`.
    fn press_entry(&mut self, verb: EntryVerb, source: String, origin: FireOrigin) {
        let readiness = if self.pending_entry.is_some() {
            EntryReadiness::Refused("while an earlier host or join press waits")
        } else {
            self.entry_readiness()
        };
        if let EntryReadiness::Refused(why) = readiness {
            self.report_refused_verb(verb.name(), source, refusal_is_own(&origin), why);
            return;
        }
        self.publish_end_cause(None);
        let entry = match verb {
            EntryVerb::Host => PendingEntry::Host {
                password: self.read_session_string(HOST_PASSWORD_SLOT),
            },
            EntryVerb::Join => {
                let password = self.read_session_string(JOIN_PASSWORD_SLOT);
                let text = self.read_session_string(JOIN_ADDRESS_SLOT);
                match netcode::parse_join_address(&text) {
                    Ok(addr) => PendingEntry::Join {
                        addr,
                        typed: text.trim().to_string(),
                        password,
                    },
                    Err(err) => {
                        log::warn!("[Net] join refused: {err}");
                        self.publish_end_cause(Some(EndCause::BadAddress));
                        return;
                    }
                }
            }
        };
        match readiness {
            EntryReadiness::Ready => self.run_entry(entry),
            EntryReadiness::Waits => {
                log::info!(
                    "[Net] {} waits for the session being left to close",
                    verb.name()
                );
                self.publish_connection(ConnectionPhase::Connecting);
                self.pending_entry = Some(entry);
            }
            EntryReadiness::Refused(_) => unreachable!("refused above"),
        }
    }

    /// Whether a press may run now, must wait, or is refused.
    fn entry_readiness(&self) -> EntryReadiness {
        match self.session_role() {
            SessionRole::SinglePlayer => {}
            SessionRole::Host => {
                return EntryReadiness::Refused("on a host: it is already in a session");
            }
            SessionRole::Client => {
                return EntryReadiness::Refused(
                    "on a connected client: it is already in a session; leave it first with `leaveSession()`",
                );
            }
        }
        let off_frontend = EntryReadiness::Refused(
            "off the frontend: a machine hosts or joins from its frontend menu, with no played level installed or loading",
        );
        if !self.frontend_menu_is_present() {
            return off_frontend;
        }
        let played = self.played_level_installed();
        let unload_pending = played
            && self
                .level_requests
                .iter()
                .any(|request| matches!(request, LevelRequest::Unload));
        if played && !unload_pending {
            return off_frontend;
        }
        let closing = self
            .session
            .as_ref()
            .is_some_and(|session| session.closing_host_transport.is_some());
        if unload_pending || closing {
            EntryReadiness::Waits
        } else {
            EntryReadiness::Ready
        }
    }

    /// Run a waiting press once nothing holds it: after a frame's level
    /// requests drain, so a left level's unload has run.
    pub(crate) fn resume_pending_entry(&mut self) {
        if self.pending_entry.is_none() {
            return;
        }
        match self.entry_readiness() {
            EntryReadiness::Waits => {}
            EntryReadiness::Ready => {
                let entry = self.pending_entry.take().expect("checked above");
                self.run_entry(entry);
            }
            EntryReadiness::Refused(why) => {
                self.pending_entry = None;
                log::warn!("[Net] a waiting host or join press was dropped {why}");
                self.publish_connection(ConnectionPhase::Offline);
            }
        }
    }

    /// Cancel a waiting press: a later navigation or level load supersedes
    /// it.
    pub(crate) fn cancel_pending_entry(&mut self, by: &str) {
        if self.pending_entry.take().is_some() {
            log::info!("[Net] {by} cancelled the waiting host or join press");
            self.publish_connection(ConnectionPhase::Offline);
        }
    }

    /// Run a captured press.
    fn run_entry(&mut self, entry: PendingEntry) {
        match entry {
            PendingEntry::Host { password } => {
                let port = self.in_app_host_port;
                if self.host_from_frontend_with(port, getrandom::fill, &password) {
                    // The hosted run's password is fixed now, and
                    // `session.passwordProtected` says whether it has one.
                    // Hosting consumes it; a failed build keeps it for a
                    // retry.
                    self.consume_password(HOST_PASSWORD_SLOT);
                } else {
                    self.publish_end_cause(Some(EndCause::HostFailed));
                }
                // A host reads `session.hosting`; a press that waited showed
                // `connecting` until now.
                self.publish_connection(ConnectionPhase::Offline);
            }
            PendingEntry::Join {
                addr,
                typed,
                password,
            } => {
                self.join_facts = JoinFacts {
                    address: typed,
                    ..JoinFacts::default()
                };
                self.write_session_slot(WAIT_REASON_SLOT, SlotValue::Enum(String::new()));
                self.publish_connection(ConnectionPhase::Connecting);
                // The endpoint takes the password before its first poll: the
                // challenge can arrive with the host's first reply.
                if !self.join_from_frontend_with(addr, &password) {
                    self.join_facts = JoinFacts::default();
                    self.publish_end_cause(Some(EndCause::ConnectFailed));
                    self.publish_connection(ConnectionPhase::Offline);
                }
            }
        }
    }

    /// What one host Control message shows this client: admission, a hold,
    /// or a host level it lacks.
    pub(crate) fn observe_client_control(&mut self, control: &ServerControlMessage) {
        match control {
            ServerControlMessage::Relevel { catalog_id, .. } => {
                let known = self.session.as_ref().is_some_and(|session| {
                    session
                        .scripting
                        .script_ctx
                        .data_registry
                        .borrow()
                        .maps
                        .iter()
                        .any(|entry| entry.id == *catalog_id)
                });
                // A new host level: a hold the old level earned is stale. A
                // mod hold is not level-scoped, and the host does not resend
                // an unchanged cause, so it stays.
                self.join_facts.host_level_missing = !known;
                if !matches!(self.join_facts.hold, Some(HoldingCause::ModDigest { .. })) {
                    self.join_facts.hold = None;
                }
                self.note_client_admitted();
                self.publish_wait_reason();
            }
            ServerControlMessage::Divergence(DivergenceReason::Holding(cause)) => {
                // A host with no level installed lacks nothing of ours.
                if matches!(cause, HoldingCause::HostLevelAbsent) {
                    self.join_facts.host_level_missing = false;
                }
                self.join_facts.hold = Some(cause.clone());
                self.note_client_admitted();
                self.publish_connection(ConnectionPhase::Waiting);
                self.publish_wait_reason();
            }
            ServerControlMessage::SpawnPoseHint(_)
            | ServerControlMessage::SessionRoster(_)
            | ServerControlMessage::FinalCharacter { .. }
            | ServerControlMessage::CharacterLoadout { .. } => self.note_client_admitted(),
            _ => {}
        }
    }

    /// The first sign the host admitted this client: it waits, the password
    /// it answered with is consumed, and the address it dialed is remembered.
    fn note_client_admitted(&mut self) {
        if self.join_facts.admitted {
            return;
        }
        self.join_facts.admitted = true;
        self.consume_password(JOIN_PASSWORD_SLOT);
        self.publish_connection(ConnectionPhase::Waiting);
        self.publish_wait_reason();
        let address = self.join_facts.address.clone();
        if !address.is_empty() {
            self.remember_join_address(address);
        }
    }

    /// Store the address of a host that admitted this client, for the next
    /// boot's `session.joinAddress`.
    fn remember_join_address(&mut self, address: String) {
        let Some(session) = self.session.as_mut() else {
            return;
        };
        if session.player_options.last_join_address.as_deref() == Some(address.as_str()) {
            return;
        }
        session.player_options.last_join_address = Some(address);
        session
            .player_options
            .mark_written(crate::options::keys::LAST_JOIN_ADDRESS);
        if let Some(path) = session.settings_path.as_deref()
            && let Err(err) = session.player_options.save(path)
        {
            log::warn!("[Options] failed to save the last join address: {err}");
        }
    }

    /// After a client's Control drain: participating reads `playing`, and an
    /// admitted client that is not reads `waiting`.
    pub(crate) fn publish_client_participation(&mut self) {
        let participating = match self
            .session
            .as_ref()
            .and_then(|session| session.net_endpoint.as_ref())
        {
            Some(netcode::NetEndpoint::Client { client, .. }) => client.is_participating(),
            _ => return,
        };
        if participating {
            self.join_facts.hold = None;
            self.publish_connection(ConnectionPhase::Playing);
            self.publish_wait_reason();
        } else if self.join_facts.admitted {
            self.publish_connection(ConnectionPhase::Waiting);
            self.publish_wait_reason();
        }
    }

    /// `session.waitReason` from the connection phase and the latest hold.
    fn publish_wait_reason(&mut self) {
        let waiting = self.read_session_slot(CONNECTION_SLOT)
            == Some(SlotValue::Enum(
                ConnectionPhase::Waiting.as_str().to_string(),
            ));
        let following = self
            .level_requests
            .iter()
            .any(|request| matches!(request, LevelRequest::HostLevel(_)))
            || (self.level_load_in_flight()
                && self.level_load.as_ref().is_some_and(|load| load.host_named));
        let reason = if waiting {
            wait_reason(
                self.join_facts.hold.as_ref(),
                self.join_facts.host_level_missing,
                following,
            )
        } else {
            ""
        };
        let value = SlotValue::Enum(reason.to_string());
        if self.read_session_slot(WAIT_REASON_SLOT).as_ref() != Some(&value) {
            self.write_session_slot(WAIT_REASON_SLOT, value);
        }
    }

    /// A joined session is over: the client is offline, its wait reason
    /// clears, and the ending's cause publishes. Only admission consumes the
    /// join password: every ending before it, a password refusal included,
    /// keeps it for the player to correct or retry.
    pub(crate) fn finish_join_facts(&mut self, end_cause: Option<EndCause>) {
        self.join_facts = JoinFacts::default();
        self.publish_connection(ConnectionPhase::Offline);
        self.publish_wait_reason();
        if let Some(cause) = end_cause {
            self.publish_end_cause(Some(cause));
        }
    }

    /// A verb consumed the password in `slot`: the slot empties, and an open
    /// field for it cannot write it back.
    fn consume_password(&mut self, slot: &str) {
        self.write_session_slot(slot, SlotValue::String(String::new()));
        self.abandon_text_entry_field_for(slot);
    }

    /// Publish `session.connection` when it changes.
    pub(crate) fn publish_connection(&mut self, phase: ConnectionPhase) {
        let value = SlotValue::Enum(phase.as_str().to_string());
        if self.read_session_slot(CONNECTION_SLOT).as_ref() == Some(&value) {
            return;
        }
        self.write_session_slot(CONNECTION_SLOT, value);
        #[cfg(test)]
        PUBLISHED_CONNECTIONS.with(|log| log.borrow_mut().push(phase));
    }

    /// Publish `session.endCause`, or clear it with `None`.
    pub(crate) fn publish_end_cause(&mut self, cause: Option<EndCause>) {
        let value = cause.map(EndCause::as_str).unwrap_or_default();
        self.write_session_slot(END_CAUSE_SLOT, SlotValue::Enum(value.to_string()));
    }

    fn read_session_slot(&self, slot: &str) -> Option<SlotValue> {
        let session = self.session.as_ref()?;
        let table = session.scripting.script_ctx.slot_table.borrow();
        table.get(slot).and_then(|record| record.value.clone())
    }

    /// A string session slot's value; empty when unset.
    pub(crate) fn read_session_string(&self, slot: &str) -> String {
        match self.read_session_slot(slot) {
            Some(SlotValue::String(value)) => value,
            _ => String::new(),
        }
    }

    pub(crate) fn write_session_slot(&mut self, slot: &str, value: SlotValue) {
        let Some(session) = self.session.as_ref() else {
            return;
        };
        if let Err(err) = postretro_scripting_core::store_bridge::write_store_slot(
            &session.scripting.script_ctx,
            slot,
            value,
        ) {
            log::warn!("[Net] failed to write `{slot}`: {err}");
        }
    }

    /// Boot flags enter a role before the mod's saves load. Once they load:
    /// with a `saves` block and no character selected, a boot host ends its
    /// session and a boot client leaves, as in-app entry would have refused
    /// them; a boot client whose claim is not the selected character's
    /// reconnects with it, so the host keys its seat by the character.
    pub(crate) fn reconcile_boot_role_with_saves(&mut self) {
        let role = self.session_role();
        let Some(session) = self.session.as_mut() else {
            return;
        };
        if !session.saves.is_loaded() || !std::mem::take(&mut session.boot_role_pending) {
            return;
        }
        let boot_connect = session.boot_connect.take();
        if role == SessionRole::SinglePlayer {
            return;
        }
        if !self.character_selected_for_entry() {
            match role {
                SessionRole::Host => {
                    log::warn!(
                        "[Net] --host refused: this mod needs a selected character; ending the session"
                    );
                    self.end_session();
                }
                SessionRole::Client => {
                    log::warn!(
                        "[Net] --connect refused: this mod needs a selected character; leaving the session"
                    );
                    self.leave_session(super::session_membership::LeaveCause::Own);
                }
                SessionRole::SinglePlayer => {}
            }
            return;
        }
        let Some((addr, password)) = boot_connect.filter(|_| role == SessionRole::Client) else {
            return;
        };
        let Some(session) = self.session.as_mut() else {
            return;
        };
        let (claim_id, claim_name) = session.character_claim();
        if claim_id == session.player_options.player_id && claim_name.is_empty() {
            return;
        }
        log::info!("[Net] reconnecting to {addr} as the selected character");
        if let Some(netcode::NetEndpoint::Client { mut client, .. }) = session.net_endpoint.take() {
            client.disconnect();
        }
        if let Some(failure) = self.enter_net_role(
            &netcode::NetRole::Connect { addr },
            getrandom::fill,
            &password,
        ) {
            log::warn!("[Net] reconnecting as the selected character failed: {failure:?}");
        }
    }

    /// With a `saves` block, hosting and joining wait until a character is
    /// selected: the character is the identity a session keys by.
    fn character_selected_for_entry(&self) -> bool {
        self.session.as_ref().is_none_or(|session| {
            !session.saves.has_saves_block() || session.saves.selected_character().is_some()
        })
    }

    /// Whether this machine stands at its frontend: the frontend menu is up
    /// and no level a session plays is installed. A backdrop, installed or
    /// loading, counts as the frontend; a played load cannot be queued or in
    /// flight under the menu, because starting one clears it.
    pub(crate) fn at_frontend(&self) -> bool {
        self.frontend_menu_is_present() && !self.played_level_installed()
    }

    /// A level a session plays is installed: one installed with the menu
    /// cleared, or one a session end stopped, whose counts already left with
    /// the session but which stays installed until its unload drains.
    fn played_level_installed(&self) -> bool {
        self.level_stopped_by_session_end
            || self
                .session
                .as_ref()
                .is_some_and(|session| session.session_players.session_level())
    }

    /// [`Self::join_from_frontend_with`] answering with no password.
    #[cfg(test)]
    pub(crate) fn join_from_frontend(&mut self, addr: SocketAddr) -> bool {
        self.join_from_frontend_with(addr, "")
    }

    /// Join the host at `addr` from the frontend, answering a challenge with
    /// `password`. A backdrop load in flight is cancelled: the host's relevel
    /// would discard it. An installed backdrop stays until the relevel reloads
    /// it as the host's level. Returns whether a client endpoint now exists.
    pub(crate) fn join_from_frontend_with(&mut self, addr: SocketAddr, password: &str) -> bool {
        if self.session_role() != SessionRole::SinglePlayer || !self.at_frontend() {
            log::warn!("[Net] join refused: entry needs a single-player machine at its frontend");
            return false;
        }
        if !self.character_selected_for_entry() {
            log::warn!("[Net] join refused: select a character first");
            return false;
        }
        // A join ends a playing camera route instead (pin P4).
        #[cfg(feature = "dev-tools")]
        if self.playback_abort(self.playback_join_abort_reason(addr)) {
            return false;
        }
        self.cancel_backdrop_load_for_join();
        self.save_progress_for_join();
        let Some(failure) = self.enter_net_role(
            &netcode::NetRole::Connect { addr },
            getrandom::fill,
            password,
        ) else {
            return true;
        };
        log::warn!("[Net] join to {addr} failed before connecting: {failure:?}");
        false
    }

    /// [`Self::host_from_frontend_with`] with OS entropy and no password.
    #[cfg(test)]
    pub(crate) fn host_from_frontend(&mut self, port: u16) -> bool {
        self.host_from_frontend_with(port, getrandom::fill, "")
    }

    /// Host on `port` from the frontend: a new hosted run with a session id
    /// drawn from `mint`, a fresh seat counter and seat 0 keeping its carried
    /// state, challenging joiners for `password` unless it is empty. The
    /// machine stands at its frontend with a live endpoint, and joiners wait
    /// there until it starts a level. Returns whether a host endpoint now
    /// exists.
    pub(crate) fn host_from_frontend_with(
        &mut self,
        port: u16,
        mint: netcode::SessionIdSource,
        password: &str,
    ) -> bool {
        if self.session_role() != SessionRole::SinglePlayer || !self.at_frontend() {
            log::warn!("[Net] host refused: entry needs a single-player machine at its frontend");
            return false;
        }
        if !self.character_selected_for_entry() {
            log::warn!("[Net] host refused: select a character first");
            return false;
        }
        let Some(failure) = self.enter_net_role(&netcode::NetRole::Host { port }, mint, password)
        else {
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
        password: &str,
    ) -> Option<EntryFailure> {
        let Some(session) = self.session.as_mut() else {
            return Some(EntryFailure::Endpoint);
        };
        let (claim_id, claim_name) = session.character_claim();
        let entry = enter_role(
            net_role,
            claim_id.map(|id| (id, claim_name.as_str())),
            &mut session.scripting,
            &mut session.seat_table,
            mint,
            netcode::probe_lan_address,
            password,
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
