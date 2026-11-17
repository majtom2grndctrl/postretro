// The world-and-character save: the one entry point the clean-exit save and
// a join's first save share, so splitting world and character saves later
// changes one place.
// See: context/lib/scripting.md §5 · context/lib/networking.md §Role model

use super::Session;
use crate::scripting::state_persistence::{
    PersistedState, collect_per_owner_state, collect_persisted_faction_sentiment,
    collect_persisted_state, merge_per_owner_state, save_persisted_state, state_path,
};

/// What a world-and-character save collected. A write that failed has
/// warned.
pub(crate) struct ProgressSave {
    pub(crate) state: PersistedState,
}

/// Collect the world slots, faction sentiment and the local player's
/// character, then write them to this mod's state file. Returns `None` when
/// nothing may be collected yet: before declarations commit and the boot
/// restore completes, a snapshot holds defaults that could replace a valid
/// file. With persistence unavailable the state is still collected, for the
/// in-memory snapshot a join rebases on, and only the write is skipped. Every
/// failure warns.
pub(crate) fn save_world_and_character(session: &mut Session) -> Option<ProgressSave> {
    if !session.state_store_lifecycle.can_collect() {
        return None;
    }
    let Some((mod_id, _)) = session.scripting.script_runtime.committed_mod_identity() else {
        log::warn!("[State] no committed mod manifest; skipping persistent-state save");
        return None;
    };
    let mod_id = mod_id.to_owned();
    let identity = session.scripting.script_runtime.store_identity().cloned();
    let committed_store_slots = session
        .scripting
        .script_runtime
        .committed_store_slots()
        .clone();
    let script_ctx = session.scripting.script_ctx.clone();
    let mut collected = collect_persisted_state(
        &script_ctx.slot_table.borrow(),
        identity.as_ref(),
        &committed_store_slots,
    );
    for warning in collected.warnings {
        log::warn!("[State] {warning}");
    }
    {
        let factions = script_ctx.data_registry.borrow();
        let faction_sentiment = script_ctx.faction_sentiment.borrow();
        for warning in collect_persisted_faction_sentiment(
            &mut collected.state,
            &faction_sentiment,
            &factions.factions,
        ) {
            log::warn!("[State] {warning}");
        }
    }
    if let Some(local_player_id) = session.player_options.player_id {
        let per_owner = collect_per_owner_state(
            &script_ctx.slot_table.borrow(),
            identity.as_ref(),
            &committed_store_slots,
            postretro_foundation::Seat(0),
            local_player_id,
        );
        for warning in per_owner.warnings {
            log::warn!("[State] {warning}");
        }
        merge_per_owner_state(&mut collected.state, per_owner.per_owner);
    }
    if !session.state_store_lifecycle.can_save() {
        // Persistence is off for this run; it warned when it turned off.
        return Some(ProgressSave {
            state: collected.state,
        });
    }
    match state_path(session.data_dir.as_deref(), &mod_id) {
        Some(state_path) => match save_persisted_state(&state_path, &collected.state) {
            Ok(()) => {
                log::info!("[State] saved persistent slots to {}", state_path.display());
            }
            Err(error) => log::warn!(
                "[State] failed to save persistent slots to {}: {error}",
                state_path.display()
            ),
        },
        None => {
            if session.state_store_lifecycle.disable_persistence() {
                log::warn!(
                    "[State] platform data directory is unavailable; persistent state is disabled for this run"
                );
            }
        }
    }
    Some(ProgressSave {
        state: collected.state,
    })
}
