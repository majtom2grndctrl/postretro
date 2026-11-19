// Save points: level starts and frontend entry write the run, the character and the world.
// See: context/lib/scripting.md §5 (Durable State Store)

use super::{RunCheckpoint, collect_world_and_run, publish_save_rows};
use crate::session::Session;
use crate::session::progress_save::save_world_and_character;

/// A level start on the host or in single player, after the level-change
/// harvest and before the next level installs. An active run writes first,
/// naming the destination, so a crash between the writes resumes the
/// destination with the earlier gear rather than the earlier level with the
/// later gear; then the character, then the world. A destination with no
/// catalog id (a path load) leaves the run's level unchanged.
pub(crate) fn save_on_level_start(session: &mut Session, destination: Option<&str>) {
    // A level start arrives at its level-start spawns; Continue's own arrival
    // keeps the run's checkpoint set, which Continue chose.
    if !session.saves.is_continue_arrival(destination) {
        session.saves.set_spawn_checkpoint(None);
    }
    if !session.state_store_lifecycle.can_save() {
        return;
    }
    if session.saves.run_active()
        && let Some(catalog_id) = destination
    {
        let (_, live) = collect_world_and_run(session);
        if !session
            .saves
            .save_run_progress(catalog_id, RunCheckpoint::LevelStart, live)
        {
            // Writing the character now would pair the run's earlier level
            // with this level's gear.
            log::warn!(
                "[State] the run did not save at this level start; keeping the character as it was"
            );
            return;
        }
    }
    let _ = save_world_and_character(session);
    publish_save_rows(session);
}

/// The frontend is entered by any path on the host or in single player: the
/// character and the world save when a played level just unloaded, and any
/// run in play ends.
pub(crate) fn save_on_frontend_entry(session: &mut Session, played_level_unloaded: bool) {
    if played_level_unloaded {
        let _ = save_world_and_character(session);
    }
    end_run_at_frontend(session);
}

/// The frontend is entered by any path, a failed run load or a resume
/// included: the run in play ends and its queued-load latch and arrival
/// clear. With a selected character seat 0 keeps what it carries: the gear a
/// played level's unload just harvested, which its save wrote. If that write
/// failed, a level start or the exit writes it later; a verb that reloads the
/// character takes the file's instead. With no character selected (or no
/// saves at all) seat 0 carries nothing, so a level's pickups never reach the
/// next level.
pub(crate) fn end_run_at_frontend(session: &mut Session) {
    session.saves.set_run_active(false);
    session.saves.set_run_load_queued(false);
    session.saves.set_spawn_checkpoint(None);
    session.saves.clear_continue_arrival();
    if session.saves.selected_character().is_none() {
        super::set_seat_zero_carry_from_selection(session);
    }
    publish_save_rows(session);
}
