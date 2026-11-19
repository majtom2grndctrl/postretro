// The store and seat 0 against the selected saves: boot restore, reloads and the character record.
// See: context/lib/scripting.md §5 (Durable State Store)

use postretro_foundation::Seat;
use postretro_sim::scripting::save_files::{LoadoutRecord, SaveDir, SeatRecord};
use postretro_sim::scripting::state_persistence::{
    collect_seat_per_owner_values, overlay_persisted_faction_sentiment, overlay_persisted_state,
    project_seat_per_owner_values, reset_mod_slots_for_reload, run_scoped_durable_keys, state_path,
};

use super::{BootInputs, load_save_slots, publish_save_rows};
use crate::session::Session;

/// Load the mod's saves once its declarations commit: migrate, select, set seat 0's carry, overlay the
/// store and publish the `saves.*` rows. A connected client overlays only its
/// world and character, never the run's world state.
pub(crate) fn boot_restore(session: &mut Session) {
    let declared = session
        .scripting
        .script_runtime
        .mod_manifest()
        .and_then(|manifest| manifest.saves);
    boot_restore_with(session, declared);
}

/// [`boot_restore`] with the mod's `saves` block given.
pub(crate) fn boot_restore_with(
    session: &mut Session,
    declared: Option<postretro_scripting_core::runtime::SavesManifest>,
) {
    let Some((mod_id, _)) = session.scripting.script_runtime.committed_mod_identity() else {
        return;
    };
    let mod_id = mod_id.to_owned();
    let Some(data_dir) = session.data_dir.clone() else {
        if session.state_store_lifecycle.disable_persistence() {
            log::warn!(
                "[State] platform data directory is unavailable; persistent state is disabled for this run"
            );
        }
        return;
    };
    let runtime = &session.scripting.script_runtime;
    let run_keys = run_scoped_durable_keys(
        runtime.store_identity(),
        runtime.committed_run_scoped_slots(),
    );
    session.saves = load_save_slots(BootInputs {
        dir: SaveDir::new(&data_dir, &mod_id),
        legacy_state: state_path(Some(&data_dir), &mod_id).expect("a data directory was given"),
        declared,
        player_id: session.player_options.player_id,
        run_keys,
    });
    set_seat_zero_carry_from_selection(session);
    let is_client = matches!(
        session.net_endpoint.as_ref(),
        Some(crate::netcode::NetEndpoint::Client { .. })
    );
    overlay_selection(
        session,
        session.saves.run_active() && !is_client,
        !is_client,
    );
    publish_save_rows(session);
}

/// Seat 0 carries the selected character's saved loadout outside play, or
/// nothing with no character selected.
pub(crate) fn set_seat_zero_carry_from_selection(session: &mut Session) {
    let carry = session.saves.selected_carry();
    if let Some(seats) = session.seat_table.as_mut() {
        seats.set_carried_state(Seat(0), carry);
    }
}

/// Reset every mod-owned slot to its default, then overlay the world file,
/// the selected character and, when `with_run`, the selected run. While
/// hosting only seat 0's per-owner values reload. Crossing watchers re-arm on
/// the reloaded values without firing.
pub(crate) fn reload_store(session: &mut Session, with_run: bool) {
    // The world file holds progress that outlives any run: a world slot
    // written at the frontend since the last save point (a difficulty
    // toggle before New game) saves before the reset discards it.
    if session.state_store_lifecycle.can_save() {
        let (world, _) = collect_world_and_run(session);
        session.saves.save_world(world);
    }
    {
        let script_ctx = session.scripting.script_ctx.clone();
        reset_mod_slots_for_reload(&mut script_ctx.slot_table.borrow_mut(), Seat(0));
    }
    overlay_selection(session, with_run, true);
    let script_ctx = session.scripting.script_ctx.clone();
    session
        .crossing_detector
        .rebaseline(&script_ctx.slot_table.borrow());
}

/// A connected client (`with_owner_state` false) overlays only its world
/// here: its character's rows reach its session seat once seated, and the
/// host owns the shared sentiment.
fn overlay_selection(session: &mut Session, with_run: bool, with_owner_state: bool) {
    let document = session.saves.store_document(with_run);
    let identity = session.scripting.script_runtime.store_identity().cloned();
    let committed = session
        .scripting
        .script_runtime
        .committed_store_slots()
        .clone();
    let script_ctx = session.scripting.script_ctx.clone();
    for warning in overlay_persisted_state(
        &mut script_ctx.slot_table.borrow_mut(),
        &document,
        identity.as_ref(),
        &committed,
        session
            .saves
            .selected_character_id()
            .filter(|_| with_owner_state),
        Seat(0),
    ) {
        log::warn!("[State] {warning}");
    }
    if with_owner_state {
        let factions = script_ctx.data_registry.borrow();
        let mut sentiment = script_ctx.faction_sentiment.borrow_mut();
        *sentiment = Default::default();
        for warning in
            overlay_persisted_faction_sentiment(&mut sentiment, &factions.factions, &document)
        {
            log::warn!("[State] {warning}");
        }
    }
    if with_owner_state {
        // UI binds read a per-owner slot's local value, which an overlay of
        // seat 0's values does not move on its own.
        project_seat_per_owner_values(&mut script_ctx.slot_table.borrow_mut(), Seat(0));
    }
    // The retained store document a join seed and a client's leave restore
    // read: it keeps an active run's state even where this overlay skipped
    // it, so a client that boots connected restores its implicit run.
    session.persisted_state = Some(if with_run || !session.saves.run_active() {
        document
    } else {
        session.saves.store_document(true)
    });
}

/// The selected character's record as it is now: its per-owner values on
/// seat 0 and seat 0's carried loadout. A character file never stores a
/// downed or dead state: the life state saves as initial, and health at or
/// below zero restores to full. `None` with no character selected.
pub(crate) fn collect_character_record(session: &Session) -> Option<SeatRecord> {
    let selected = session.saves.selected_character()?;
    let identity = session.scripting.script_runtime.store_identity();
    let committed = session.scripting.script_runtime.committed_store_slots();
    let (per_owner, warnings) = collect_seat_per_owner_values(
        &session.scripting.script_ctx.slot_table.borrow(),
        identity,
        committed,
        Seat(0),
    );
    for warning in warnings {
        log::warn!("[State] {warning}");
    }
    let mut carried = session
        .seat_table
        .as_ref()
        .and_then(|seats| seats.carried_state(Seat(0)).cloned());
    if let Some(carried) = carried.as_mut() {
        carried.life_state = None;
        carried.health_current = carried.health_current.filter(|health| *health > 0.0);
    }
    Some(SeatRecord {
        claim: selected.claim.clone(),
        name: selected.name.clone(),
        per_owner,
        loadout: LoadoutRecord::from_carried(carried.as_ref()),
    })
}

/// The live world and run state: persisted globals split by scope into a
/// world document and a run document, the run's holding faction sentiment.
/// The run document has no level or run record; a save point fills those.
pub(crate) fn collect_world_and_run(
    session: &Session,
) -> (
    postretro_sim::scripting::save_files::LedgerDocument,
    postretro_sim::scripting::save_files::LedgerDocument,
) {
    use postretro_sim::scripting::save_files::LedgerDocument;
    use postretro_sim::scripting::state_persistence::{
        collect_persisted_faction_sentiment, collect_persisted_state, split_world_and_run,
    };
    let runtime = &session.scripting.script_runtime;
    let identity = runtime.store_identity();
    let script_ctx = &session.scripting.script_ctx;
    let mut collected = collect_persisted_state(
        &script_ctx.slot_table.borrow(),
        identity,
        runtime.committed_store_slots(),
    );
    for warning in collected.warnings {
        log::warn!("[State] {warning}");
    }
    {
        let factions = script_ctx.data_registry.borrow();
        let sentiment = script_ctx.faction_sentiment.borrow();
        for warning in collect_persisted_faction_sentiment(
            &mut collected.state,
            &sentiment,
            &factions.factions,
        ) {
            log::warn!("[State] {warning}");
        }
    }
    let run_keys = run_scoped_durable_keys(identity, runtime.committed_run_scoped_slots());
    let (world_slots, run_slots) = split_world_and_run(collected.state.slots(), &run_keys);
    (
        LedgerDocument {
            slots: world_slots,
            ..session.saves.world_document().cloned().unwrap_or_default()
        },
        LedgerDocument {
            slots: run_slots,
            faction_sentiment: collected.state.faction_sentiment().clone(),
            ..LedgerDocument::default()
        },
    )
}
