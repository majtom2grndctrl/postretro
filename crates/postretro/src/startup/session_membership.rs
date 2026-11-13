//! Session membership: a connected client's leave, which every host-side
//! ending also takes, and the role-resolved membership and navigation verbs.
//! See: context/lib/networking.md §Slot lifecycle (Leaving and ending a session)
//! · context/lib/scripting.md §10.4 (Session membership)

use postretro_entities::reactions::system_commands::{FireOrigin, MachineSource};

use crate::App;
use crate::netcode;
use crate::session::role_state::{RoleBoundState, SessionRole, apply_role};
use crate::startup::BootState;

/// Why a connected client's session ends. Every cause lands it where its own
/// leave does; only its own leave sends the disconnect.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LeaveCause {
    /// Its own `leaveSession`, `ui.quitToMenu` or exit to desktop.
    Own,
    /// The host ended the session.
    SessionEnded,
    /// The host refused it at admission.
    Refused,
    /// The established connection ended without a cause from the host.
    LostHost,
    /// No connect attempt reached the host.
    NeverConnected,
}

/// How loudly a refused verb reports. The rule the scheduler applies to a
/// client's dropped tail steps: a mistake when this machine's own input or
/// state fired it, routine when the host's timeline or a shared crossing did.
fn refusal_is_own(origin: &FireOrigin) -> bool {
    matches!(origin, FireOrigin::EveryMachine(MachineSource::OwnState))
}

impl App {
    /// The role this session runs as now.
    pub(crate) fn session_role(&self) -> SessionRole {
        SessionRole::of(
            self.session
                .as_ref()
                .and_then(|session| session.net_endpoint.as_ref()),
        )
    }

    /// Leave a connected client's session: save the player's character,
    /// close the connection (sending the disconnect only for its own leave),
    /// become single-player through the role derivation a session builds
    /// with, drop everything the host steered, and land at the mod's
    /// frontend. Returns whether this machine was a client that left; a
    /// second leave in one frame changes nothing.
    pub(crate) fn leave_session(&mut self, cause: LeaveCause) -> bool {
        if self.session_role() != SessionRole::Client {
            return false;
        }
        match cause {
            LeaveCause::Own => log::info!("[Net] leaving the session"),
            LeaveCause::SessionEnded => log::info!("[Net] the host ended the session"),
            // The transport or the Control drain has already named the cause.
            LeaveCause::Refused | LeaveCause::LostHost | LeaveCause::NeverConnected => {}
        }
        let session_seat = self.save_character_on_leave();
        let Some(session) = self.session.as_mut() else {
            return false;
        };
        let endpoint = session.net_endpoint.take();
        if cause == LeaveCause::Own
            && let Some(netcode::NetEndpoint::Client { mut client, .. }) = endpoint
        {
            client.disconnect();
        }
        let registry = session.scripting.script_ctx.registry.clone();
        let released = apply_role(
            &mut session.scripting,
            &mut session.seat_table,
            &mut registry.borrow_mut(),
            &RoleBoundState::derive(SessionRole::SinglePlayer, None),
        );
        debug_assert!(released.is_empty(), "a client holds no seat ledger");
        self.restore_store_after_leave(session_seat);
        self.forget_host_steering();
        if self.level_is_installed_state() {
            self.level_stopped_by_session_end = true;
        }
        // Before boot clears the splash or the first-launch hold, the boot
        // destination presents the frontend; presenting it now would tear
        // down the accessibility panel the player has not closed.
        if !matches!(
            self.boot_state,
            BootState::Splash | BootState::FirstLaunchHold
        ) {
            self.navigate_to_frontend();
        }
        log::info!("[Net] returned to the frontend as single-player");
        true
    }

    /// End a listen host's session (`endSession`): each client that has
    /// participated receives its final character, then every slot the
    /// session-ended cause; every remote pawn goes and no seat is held. The
    /// host becomes single-player at once and its level keeps running with
    /// its own seat, pawn binding and carried loadout. Its transport survives
    /// only to deliver the closes ([`App::pump_closing_host_transport`]).
    pub(crate) fn end_session(&mut self) {
        if self.session_role() != SessionRole::Host {
            return;
        }
        let Some(session) = self.session.as_mut() else {
            return;
        };
        let script_ctx = session.scripting.script_ctx.clone();
        let identity = session.scripting.script_runtime.store_identity().cloned();
        let committed = session
            .scripting
            .script_runtime
            .committed_store_slots()
            .clone();
        let life_state_names = script_ctx.data_registry.borrow().life_state_names.clone();
        let closing = {
            let mut registry = script_ctx.registry.borrow_mut();
            netcode::end_host_session(
                &mut session.net_endpoint,
                &mut registry,
                session.seat_table.as_mut(),
                &life_state_names,
                |seat| {
                    let (slots, warnings) =
                        crate::scripting::state_persistence::final_character_for_seat(
                            &script_ctx.slot_table.borrow(),
                            identity.as_ref(),
                            &committed,
                            seat,
                        );
                    for warning in warnings {
                        log::warn!("[State] {warning}");
                    }
                    slots
                },
            )
        };
        let Some(closing) = closing else {
            return;
        };
        session.closing_host_transport = Some(closing);
        let released = {
            let mut registry = script_ctx.registry.borrow_mut();
            apply_role(
                &mut session.scripting,
                &mut session.seat_table,
                &mut registry,
                &RoleBoundState::derive(SessionRole::SinglePlayer, None),
            )
        };
        netcode::clear_released_seat_slot_values(&mut script_ctx.slot_table.borrow_mut(), released);
        self.published_level_identity = None;
        // The departed seats no longer reserve respawn starts.
        if self.level_is_installed_state() {
            self.refresh_respawn_placements();
        }
        log::info!("[Net] the session ended; continuing as single-player");
    }

    /// Before the process exits: a client leaves and a host ends its session,
    /// then any closing transport is pumped until every slot acknowledges
    /// its cause or [`postretro_net::transport::SESSION_END_CLOSE_WINDOW`] of
    /// wall time passes, so exit never waits on a silent peer.
    pub(crate) fn leave_or_end_for_exit(&mut self) {
        match self.session_role() {
            SessionRole::Client => {
                self.leave_session(LeaveCause::Own);
            }
            SessionRole::Host => self.end_session(),
            SessionRole::SinglePlayer => {}
        }
        let deadline =
            std::time::Instant::now() + postretro_net::transport::SESSION_END_CLOSE_WINDOW;
        let mut last = std::time::Instant::now();
        while self
            .session
            .as_ref()
            .is_some_and(|session| session.closing_host_transport.is_some())
        {
            let now = std::time::Instant::now();
            if now >= deadline {
                break;
            }
            self.pump_closing_host_transport(now - last);
            last = now;
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        if let Some(session) = self.session.as_mut()
            && let Some(mut server) = session.closing_host_transport.take()
        {
            server.disconnect_remaining();
        }
    }

    /// Advance an ended host's closing transport by one frame, dropping it
    /// once every slot has acknowledged its cause or the close window has
    /// passed. Runs in every boot state.
    pub(crate) fn pump_closing_host_transport(&mut self, frame_dt: std::time::Duration) {
        let Some(session) = self.session.as_mut() else {
            return;
        };
        let Some(server) = session.closing_host_transport.as_mut() else {
            return;
        };
        if let Err(err) = server.update(frame_dt) {
            log::warn!("[Net] closing transport failed ({err}); dropping it");
            session.closing_host_transport = None;
            return;
        }
        if server.closing_done() {
            server.disconnect_remaining();
            session.closing_host_transport = None;
            log::info!("[Net] closing transport dropped");
        }
    }

    /// Save a leaving client's character before its endpoint drops: the
    /// final values its host returned, else the values it last received. A
    /// client that never participated on this connection received no
    /// character, so its saved one stays untouched. Returns the session seat
    /// holding its character, and whether that seat holds it, for the restore.
    fn save_character_on_leave(&mut self) -> (Option<postretro_foundation::Seat>, bool) {
        let final_character = self.received_final_character.take();
        let participated_before = std::mem::take(&mut self.client_participated);
        let Some(session) = self.session.as_mut() else {
            return (None, false);
        };
        let Some((_, participating, local_seat)) = session
            .net_endpoint
            .as_ref()
            .and_then(netcode::NetEndpoint::client_per_owner_save_context)
        else {
            return (None, false);
        };
        let participated = participated_before || participating || final_character.is_some();
        if let (Some(slots), Some(seat)) = (final_character, local_seat) {
            let identity = session.scripting.script_runtime.store_identity().cloned();
            let committed = session
                .scripting
                .script_runtime
                .committed_store_slots()
                .clone();
            let outcome = crate::scripting::state_persistence::apply_join_seed(
                &mut session.scripting.script_ctx.slot_table.borrow_mut(),
                identity.as_ref(),
                &committed,
                seat,
                slots,
            );
            for warning in outcome.warnings {
                log::warn!("[State] final character {warning}");
            }
        }
        if participated {
            crate::save_connected_client_per_owner_state(session);
        }
        (local_seat, participated)
    }

    /// Restore a left client's store as single-player: the host's replicated
    /// world and faction sentiment return to the client's boot world, its
    /// character lands on the single-player seat, and slots that never
    /// replicate keep its writes. Crossing watchers re-arm on the restored
    /// values, so the restore fires none.
    fn restore_store_after_leave(
        &mut self,
        (session_seat, character_on_seat): (Option<postretro_foundation::Seat>, bool),
    ) {
        let Some(session) = self.session.as_mut() else {
            return;
        };
        let identity = session.scripting.script_runtime.store_identity().cloned();
        let committed = session
            .scripting
            .script_runtime
            .committed_store_slots()
            .clone();
        let script_ctx = session.scripting.script_ctx.clone();
        let warnings = crate::scripting::state_persistence::restore_store_after_session(
            &mut script_ctx.slot_table.borrow_mut(),
            session.persisted_state.as_ref(),
            identity.as_ref(),
            &committed,
            session_seat,
            character_on_seat,
            session.player_options.player_id,
        );
        for warning in warnings {
            log::warn!("[State] {warning}");
        }
        {
            let factions = script_ctx.data_registry.borrow();
            let mut sentiment = script_ctx.faction_sentiment.borrow_mut();
            *sentiment = Default::default();
            if let Some(boot) = session.persisted_state.as_ref().filter(|boot| {
                crate::scripting::state_persistence::persisted_state_version_is_supported(boot)
            }) {
                for warning in
                    crate::scripting::state_persistence::overlay_persisted_faction_sentiment(
                        &mut sentiment,
                        &factions.factions,
                        boot,
                    )
                {
                    log::warn!("[State] {warning}");
                }
            }
        }
        session
            .crossing_detector
            .rebaseline(&script_ctx.slot_table.borrow());
    }

    /// Take the leave path when a client's connection is over without a
    /// cause from its host: a lost host, or a connect whose attempts all
    /// failed. A first connect still retrying has not ended.
    pub(crate) fn follow_client_connection_end(&mut self) {
        let end = match self
            .session
            .as_ref()
            .and_then(|session| session.net_endpoint.as_ref())
        {
            Some(netcode::NetEndpoint::Client { client, .. }) => client.connection_end(),
            _ => return,
        };
        match end {
            Some(postretro_net::transport::ConnectionEnd::Lost) => {
                self.leave_session(LeaveCause::LostHost);
            }
            Some(postretro_net::transport::ConnectionEnd::NeverConnected) => {
                self.leave_session(LeaveCause::NeverConnected);
            }
            Some(postretro_net::transport::ConnectionEnd::Left) | None => {}
        }
    }

    /// Nothing the host named outlives the connection: no relevel record,
    /// host-named load (queued or in flight) or host-named install. The
    /// spawn-pose hint went with the endpoint.
    fn forget_host_steering(&mut self) {
        self.last_relevel = None;
        self.level_requests.clear();
        if self.level_load.as_ref().is_some_and(|load| load.host_named) {
            self.cancel_beside_level_load();
            self.abandon_level_load_for_frontend();
        }
        if let Some(load) = self.level_load.as_mut() {
            load.host_named = false;
        }
        self.active_level_host_named = false;
        self.boot_host_level_missing = false;
    }

    /// Whether a client's session ended this frame with its level still
    /// installed: the frame returns before that level's next game logic.
    pub(crate) fn level_stopped_by_session_end(&self) -> bool {
        self.level_stopped_by_session_end
    }

    /// `ui.quitToMenu`: a client leaves, a host ends its session and then
    /// returns to its frontend, and single-player returns as it always has.
    pub(crate) fn quit_to_menu(&mut self) {
        match self.session_role() {
            SessionRole::Client => {
                self.leave_session(LeaveCause::Own);
            }
            SessionRole::Host => {
                self.end_session();
                self.navigate_to_frontend();
            }
            SessionRole::SinglePlayer => self.navigate_to_frontend(),
        }
    }

    /// The `returnToFrontend` verb: navigation outside a session. A host and
    /// a connected client refuse it, naming the membership verbs.
    pub(crate) fn run_return_to_frontend_verb(&mut self, source: String, origin: FireOrigin) {
        match self.session_role() {
            SessionRole::SinglePlayer => self.navigate_to_frontend(),
            SessionRole::Host => self.report_refused_verb(
                "returnToFrontend",
                source,
                true,
                "on a host: the frontend is outside every session; end it first with `endSession()`, then `returnToFrontend()`",
            ),
            SessionRole::Client => self.report_refused_verb(
                "returnToFrontend",
                source,
                refusal_is_own(&origin),
                "on a connected client: the frontend is outside every session; a client leaves with `leaveSession()`, and its host ends a session with `endSession()`",
            ),
        }
    }

    /// The `leaveSession` verb. Returns whether this machine left: the rest
    /// of the drain it ran in then drops.
    pub(crate) fn run_leave_session_verb(&mut self, source: String) -> bool {
        match self.session_role() {
            SessionRole::Client => self.leave_session(LeaveCause::Own),
            SessionRole::Host => {
                self.report_refused_verb(
                    "leaveSession",
                    source,
                    true,
                    "on a host: a host ends its session with `endSession()`",
                );
                false
            }
            SessionRole::SinglePlayer => {
                log::debug!("[Scripting] leaveSession from `{source}`: no session to leave");
                false
            }
        }
    }

    /// The `endSession` verb.
    pub(crate) fn run_end_session_verb(&mut self, source: String, origin: FireOrigin) {
        match self.session_role() {
            SessionRole::Host => self.end_session(),
            SessionRole::Client => self.report_refused_verb(
                "endSession",
                source,
                refusal_is_own(&origin),
                "on a connected client: only the host ends a session; a client leaves with `leaveSession()`",
            ),
            SessionRole::SinglePlayer => {
                log::debug!("[Scripting] endSession from `{source}`: no session to end");
            }
        }
    }

    /// Report a verb this role refuses: once per firing source and verb as a
    /// warning when `own`, else at debug.
    fn report_refused_verb(&mut self, verb: &'static str, source: String, own: bool, why: &str) {
        let first = own
            && self.session.as_mut().is_some_and(|session| {
                session
                    .scripting
                    .refused_verb_warnings
                    .insert((source.clone(), verb))
            });
        if first {
            log::warn!("[Scripting] {verb} from `{source}` ignored {why}");
        } else {
            log::debug!("[Scripting] {verb} from `{source}` ignored {why}");
        }
    }
}
