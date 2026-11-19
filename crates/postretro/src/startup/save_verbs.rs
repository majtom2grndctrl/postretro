// Save verbs and `checkpoint` at the app's system-command drain.
// See: context/lib/scripting.md §5

use postretro_entities::SystemReactionCommand;

use crate::App;

impl App {
    /// Run one save verb (`continueRun`, `newRun`, `deleteRun`,
    /// `selectCharacter`, `createCharacter`, `deleteCharacter`) or
    /// `checkpoint` the drain received.
    pub(crate) fn handle_save_verb(&mut self, command: SystemReactionCommand) {
        log::debug!("[State] save verb {:?} not yet handled", command);
    }
}
