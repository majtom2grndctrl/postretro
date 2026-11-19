// The host's join-seed apply and a connected client's own character saves.
// See: context/lib/scripting.md §5 (Durable State Store) · context/lib/networking.md §Slot lifecycle

use std::collections::{BTreeMap, BTreeSet};

use postretro_foundation::Seat;

use crate::netcode;
use crate::scripting::state_persistence::{
    apply_join_seed, collect_per_owner_state, collected_per_owner_only_state,
    retain_saved_per_owner_state,
};

/// Apply one host-validated join seed to an admitted seat. The host registry is
/// authoritative: unknown durable keys and any schema-incompatible entries are
/// warned and skipped before owner-private replication observes the values.
pub(crate) fn apply_host_join_seed(
    script_ctx: &postretro_entities::ScriptCtx,
    registry: &mut postretro_entities::EntityRegistry,
    identity: Option<&postretro_scripting_core::store_identity::StoreIdentityLedger>,
    committed_store_slots: &BTreeSet<String>,
    seat: Seat,
    slots: BTreeMap<String, postretro_net::wire::JoinSeedValue>,
) {
    // Bound so the slot-table borrow ends before the layers evaluate.
    let outcome = apply_join_seed(
        &mut script_ctx.slot_table.borrow_mut(),
        identity,
        committed_store_slots,
        seat,
        slots,
    );
    for warning in &outcome.warnings {
        log::warn!("[Net] {warning}");
    }
    if outcome.applied == 0 {
        return;
    }
    // Settle now, as seat bind does: fullness is judged against the pre-seed
    // capacity and refills to the seeded one before a grant, hit or level
    // change can land.
    if let Some(pawn) = registry.pawn_for_seat(seat) {
        use postretro_entities::components::defense_layers::{LayerRefill, settle_layers_now};
        settle_layers_now(registry, pawn, |component| {
            component.mark_unmarked(LayerRefill::IfFull);
        });
    }
}

/// A final character as a closing or answering host returned it: the
/// client's per-owner values and its seat's carried loadout.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ReceivedFinalCharacter {
    pub(crate) slots: BTreeMap<String, postretro_net::wire::JoinSeedValue>,
    pub(crate) loadout: Option<postretro_net::wire::CarriedLoadoutWire>,
}

/// Check a joiner's loadout against this host's mod and make it the seat's
/// carry, so the pawn about to spawn holds it. Each drop or clamp warns. A
/// seat whose pawn already spawned keeps the gear it spawned with. The
/// check's result lives only on the seat: the host never saves a guest's
/// loadout.
pub(crate) fn apply_host_join_loadout(
    script_ctx: &postretro_entities::ScriptCtx,
    registry: &postretro_entities::EntityRegistry,
    seats: Option<&mut netcode::SeatTable>,
    spawn_points: &[postretro_sim::scripting::map_entity::MapEntity],
    seat: Seat,
    loadout: Option<postretro_net::wire::CarriedLoadoutWire>,
) {
    let Some(loadout) = loadout else {
        return;
    };
    let Some(seats) = seats else {
        return;
    };
    if registry.pawn_for_seat(seat).is_some() {
        log::warn!(
            "[Net] seat {}: join loadout arrived after its pawn spawned; the seat keeps the gear it spawned with",
            seat.0
        );
        return;
    }
    let data = script_ctx.data_registry.borrow();
    let gear = netcode::HostGear::new(&data.entities, &data.defense, spawn_points);
    let mut warnings = Vec::new();
    let checked = netcode::check_join_loadout(&loadout, &gear, &mut warnings);
    for warning in &warnings {
        log::warn!("[Net] seat {} join loadout: {warning}", seat.0);
    }
    seats.set_carried_state(seat, checked);
}

/// Answer each final-character request in this poll: harvest the asking
/// seat's live pawn, then send its per-owner values and that loadout.
#[allow(clippy::too_many_arguments)]
pub(crate) fn answer_final_character_requests(
    script_ctx: &postretro_entities::ScriptCtx,
    server: &mut postretro_net::transport::NetServer,
    join_seeds: &netcode::HostJoinSeeds,
    seats: &mut netcode::SeatTable,
    registry: &postretro_entities::EntityRegistry,
    identity: Option<&postretro_scripting_core::store_identity::StoreIdentityLedger>,
    committed_store_slots: &BTreeSet<String>,
    requests: &[u64],
) {
    if requests.is_empty() {
        return;
    }
    let life_state_names = script_ctx.data_registry.borrow().life_state_names.clone();
    netcode::answer_final_character_requests(
        server,
        join_seeds,
        seats,
        registry,
        &life_state_names,
        requests,
        |seat| {
            let (slots, warnings) = crate::scripting::state_persistence::final_character_for_seat(
                &script_ctx.slot_table.borrow(),
                identity,
                committed_store_slots,
                seat,
            );
            for warning in warnings {
                log::warn!("[State] {warning}");
            }
            slots
        },
    );
}

/// At a host level start or checkpoint, after its harvest: return each
/// guest's carried loadout to that guest alone.
pub(crate) fn send_character_loadouts(session: &mut super::Session) {
    if let Some(endpoint) = session.net_endpoint.as_mut() {
        endpoint.send_character_loadouts(session.seat_table.as_ref());
    }
}

/// The joining character's loadout, as the join seed carries it: the selected
/// character's saved one, or `None` for an empty one.
pub(crate) fn join_seed_loadout(
    session: &super::Session,
) -> Option<postretro_net::wire::CarriedLoadoutWire> {
    session
        .saves
        .selected_character()
        .map(|record| &record.loadout)
        .filter(|loadout| !loadout.is_empty())
        .map(netcode::loadout_record_to_wire)
}

/// Save a connected client's own persistent per-owner values, periodically
/// and at its leave. Its document keeps the world it booted with
/// (`collected_per_owner_only_state`): the host's replicated globals are the
/// host's, while the durable player-owned values belong in the local device's
/// document.
pub(crate) fn save_connected_client_per_owner_state(session: &mut super::Session) {
    if !session.state_store_lifecycle.can_save() {
        return;
    }
    let Some((_, _, Some(local_seat))) = session
        .net_endpoint
        .as_ref()
        .and_then(netcode::NetEndpoint::client_per_owner_save_context)
    else {
        return;
    };
    let (Some(local_player_id), _) = session.character_claim() else {
        // A process without a durable player identity must not manufacture a
        // per-owner key or write a document it cannot later identify.
        return;
    };
    let identity = session.scripting.script_runtime.store_identity().cloned();
    let committed_store_slots = session
        .scripting
        .script_runtime
        .committed_store_slots()
        .clone();
    let script_ctx = session.scripting.script_ctx.clone();
    let collected = collect_per_owner_state(
        &script_ctx.slot_table.borrow(),
        identity.as_ref(),
        &committed_store_slots,
        local_seat,
        local_player_id,
    );
    for warning in collected.warnings {
        log::warn!("[State] {warning}");
    }

    let save_state =
        collected_per_owner_only_state(session.persisted_state.as_ref(), collected.per_owner);
    // The character file keeps the loadout it holds; the host returns the
    // live one at each level start and checkpoint.
    let Some(mut record) = session.saves.selected_character().cloned() else {
        return;
    };
    record.per_owner = save_state.per_owner_for(&record.claim);
    if session.saves.save_selected_character(record) {
        // Advance the retained per-owner document used by future client
        // saves and join-seed assembly; the boot world stays as it was.
        retain_saved_per_owner_state(&mut session.persisted_state, save_state);
        log::info!("[State] saved the character's per-owner slots");
    }
}

/// Advance the connected-client private-save cadence after all game-logic work
/// has settled for the frame. The caller owns the exact post-command-drain
/// location; this helper owns only role/participation gating and synchronous I/O.
pub(crate) fn maybe_save_connected_client_per_owner_state(
    session: &mut super::Session,
    frame_dt: std::time::Duration,
) {
    let Some((connected, participating, _)) = session
        .net_endpoint
        .as_ref()
        .and_then(netcode::NetEndpoint::client_per_owner_save_context)
    else {
        return;
    };
    session.per_owner_save_timer.observe_connection(connected);
    if session
        .per_owner_save_timer
        .advance(frame_dt, connected && participating)
    {
        save_connected_client_per_owner_state(session);
    }
}
