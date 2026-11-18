// The host's join-seed apply and a connected client's own character saves.
// See: context/lib/scripting.md §5 (Durable State Store) · context/lib/networking.md §Slot lifecycle

use std::collections::{BTreeMap, BTreeSet};

use postretro_foundation::Seat;

use crate::netcode;
use crate::scripting::state_persistence::{
    apply_join_seed, collect_per_owner_state, collected_per_owner_only_state,
    retain_saved_per_owner_state, save_persisted_state, state_path,
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
    let Some(local_player_id) = session.player_options.player_id else {
        // A process without a durable player identity must not manufacture a
        // per-owner key or write a document it cannot later identify.
        return;
    };
    let Some((mod_id, _)) = session.scripting.script_runtime.committed_mod_identity() else {
        log::warn!("[State] no committed mod manifest; skipping persistent per-owner state save");
        return;
    };
    let mod_id = mod_id.to_owned();
    let Some(state_path) = state_path(session.data_dir.as_deref(), &mod_id) else {
        if session.state_store_lifecycle.disable_persistence() {
            log::warn!(
                "[State] platform data directory is unavailable; persistent state is disabled for this run"
            );
        }
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
    match save_persisted_state(&state_path, &save_state) {
        Ok(()) => {
            // Keep boot-loaded globals in memory for their own lifecycle, but
            // advance the retained per-owner document used by future client
            // saves and join-seed assembly.
            retain_saved_per_owner_state(&mut session.persisted_state, save_state);
            log::info!(
                "[State] saved persistent per-owner slots to {}",
                state_path.display()
            );
        }
        Err(error) => log::warn!(
            "[State] failed to save persistent per-owner slots to {}: {error}",
            state_path.display()
        ),
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
