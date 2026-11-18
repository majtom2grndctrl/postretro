// The world-and-character save: the entry point the clean exit, a join and
// the other save points share.
// See: context/lib/scripting.md §5 · context/lib/networking.md §Role model

use super::Session;
use super::saves::{collect_character_record, collect_world_and_run, publish_save_rows};
use crate::scripting::state_persistence::PersistedState;

/// What a world-and-character save collected. A write that failed has
/// warned.
pub(crate) struct ProgressSave {
    pub(crate) state: PersistedState,
}

/// Collect the world slots, the run's live state and the selected
/// character, then write the world file and the character file. Returns
/// `None` when nothing may be collected yet: before declarations commit and
/// the boot restore completes, the store holds defaults that could replace a
/// valid file. With persistence unavailable the state is still collected,
/// for the in-memory snapshot a join rebases on, and only the writes are
/// skipped. Only the host or single player calls this; a connected client
/// saves its character alone.
pub(crate) fn save_world_and_character(session: &mut Session) -> Option<ProgressSave> {
    if !session.state_store_lifecycle.can_collect() {
        return None;
    }
    let character = collect_character_record(session);
    let (world, run_state) = collect_world_and_run(session);
    if session.state_store_lifecycle.can_save() {
        let character_saved = character
            .clone()
            .is_none_or(|character| session.saves.save_selected_character(character));
        let world_saved = session.saves.save_world(world.clone());
        // These two lines are named by the join-save tests.
        if character_saved && world_saved {
            log::info!("[State] saved persistent slots to the world and character saves");
        } else {
            log::warn!("[State] failed to save persistent slots; the previous saves are kept");
        }
        publish_save_rows(session);
    }
    Some(ProgressSave {
        state: PersistedState::from_ledger(Some(&world), Some(&run_state), character.as_ref()),
    })
}
