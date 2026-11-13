// Role-bound session state: the one derivation `Session::build`, a client's
// leave and a host's end share, so a role change cannot drift from a boot.
// See: context/lib/networking.md §Role model

use std::net::SocketAddr;

use postretro_entities::EntityRegistry;
use postretro_entities::slot_table::SlotValue;
use postretro_foundation::Seat;

use super::ScriptingCore;
use crate::netcode;
use crate::scripting_systems::reaction_scheduler::SchedulerRole;

pub(crate) const HOST_ADDRESS_SLOT: &str = "session.hostAddress";
pub(crate) const HOSTING_SLOT: &str = "session.hosting";

/// The role a session runs as. Selected at startup; afterwards it moves one
/// way only, to single-player, when a client leaves or a host ends.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SessionRole {
    SinglePlayer,
    Host,
    Client,
}

impl SessionRole {
    /// The role an endpoint gives a session: the absent endpoint is
    /// single-player.
    pub(crate) fn of(endpoint: Option<&netcode::NetEndpoint>) -> Self {
        match endpoint {
            None => Self::SinglePlayer,
            Some(netcode::NetEndpoint::Host { .. }) => Self::Host,
            Some(netcode::NetEndpoint::Client { .. }) => Self::Client,
        }
    }
}

/// Every value a session derives from its role. [`RoleBoundState::derive`]
/// says what a role holds, [`apply_role`] writes it, and
/// [`RoleBoundState::observe`] reads it back, so a test comparing two
/// observations compares a field added here without being edited.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RoleBoundState {
    /// Runtime spawns materialize only on the authority.
    pub(crate) runtime_spawn_authority: bool,
    /// Mover auto-close countdowns run only on the authority.
    pub(crate) auto_close_timers_enabled: bool,
    pub(crate) scheduler_role: SchedulerRole,
    /// Owner-addressed slot writes apply only on the authority.
    pub(crate) owner_slot_writes_enabled: bool,
    /// `session.hosting`: true only on a live listen host.
    pub(crate) hosting: bool,
    /// `session.hostAddress`: the address players dial, else empty.
    pub(crate) host_address: String,
    /// The seat ledger's seats. The authority keeps a ledger; a client has
    /// none. An ended host keeps only its own seat.
    pub(crate) seats: Option<Vec<Seat>>,
}

impl RoleBoundState {
    /// What `role` holds. `published` is the address a live listen host
    /// publishes; every other role publishes none.
    pub(crate) fn derive(role: SessionRole, published: Option<SocketAddr>) -> Self {
        let authority = role != SessionRole::Client;
        let published = published.filter(|_| role == SessionRole::Host);
        Self {
            runtime_spawn_authority: authority,
            auto_close_timers_enabled: authority,
            scheduler_role: if authority {
                SchedulerRole::Host
            } else {
                SchedulerRole::Client
            },
            owner_slot_writes_enabled: authority,
            hosting: published.is_some(),
            host_address: published
                .map(|address| address.to_string())
                .unwrap_or_default(),
            seats: authority.then(|| vec![Seat(0)]),
        }
    }

    /// The role-bound values a session holds now.
    #[cfg(test)]
    pub(crate) fn observe(
        scripting: &ScriptingCore,
        seat_table: Option<&netcode::SeatTable>,
    ) -> Self {
        let slots = scripting.script_ctx.slot_table.borrow();
        let hosting = matches!(
            slots
                .get(HOSTING_SLOT)
                .and_then(|record| record.value.as_ref()),
            Some(SlotValue::Boolean(true))
        );
        let host_address = match slots
            .get(HOST_ADDRESS_SLOT)
            .and_then(|record| record.value.as_ref())
        {
            Some(SlotValue::String(address)) => address.clone(),
            _ => String::new(),
        };
        Self {
            runtime_spawn_authority: scripting.spawn_context.runtime_spawn_authority(),
            auto_close_timers_enabled: scripting.auto_close_timers.is_enabled(),
            scheduler_role: scripting.scheduler.role(),
            owner_slot_writes_enabled: scripting.script_ctx.owner_slot_writes_enabled.get(),
            hosting,
            host_address,
            seats: seat_table.map(netcode::SeatTable::seats),
        }
    }
}

/// Write `state` into a session. A scheduler role change drops every parked
/// tail. The seat ledger follows `state.seats`: a client drops it, a machine
/// that gains authority starts a fresh one, and an authority keeps only the
/// seats `state` names, releasing the rest. Returns the released seats, whose
/// per-owner slot values the caller clears.
pub(crate) fn apply_role(
    scripting: &mut ScriptingCore,
    seat_table: &mut Option<netcode::SeatTable>,
    registry: &mut EntityRegistry,
    state: &RoleBoundState,
) -> Vec<Seat> {
    scripting
        .spawn_context
        .set_runtime_spawn_authority(state.runtime_spawn_authority);
    scripting
        .auto_close_timers
        .set_enabled(state.auto_close_timers_enabled);
    scripting.scheduler.set_role(state.scheduler_role);
    scripting
        .script_ctx
        .owner_slot_writes_enabled
        .set(state.owner_slot_writes_enabled);
    for (slot, value) in [
        (
            HOST_ADDRESS_SLOT,
            SlotValue::String(state.host_address.clone()),
        ),
        (HOSTING_SLOT, SlotValue::Boolean(state.hosting)),
    ] {
        if let Err(err) = postretro_scripting_core::store_bridge::write_store_slot(
            &scripting.script_ctx,
            slot,
            value,
        ) {
            log::warn!("[Net] failed to write `{slot}`: {err}");
        }
    }
    // Once-per-source refusal latches belong to the role that warned.
    scripting.refused_verb_warnings.clear();

    let Some(keep) = state.seats.as_ref() else {
        *seat_table = None;
        return Vec::new();
    };
    let table = seat_table.get_or_insert_with(|| {
        netcode::SeatTable::new().unwrap_or_else(|err| {
            log::warn!("[Net] seat ledger identity unavailable ({err}); using a local-only ledger");
            netcode::SeatTable::local_only()
        })
    });
    if table.seats() == *keep {
        return Vec::new();
    }
    debug_assert_eq!(keep, &vec![Seat(0)], "an authority keeps only its own seat");
    table.release_remote_seats(registry)
}

#[cfg(test)]
#[path = "role_state_tests.rs"]
mod tests;
