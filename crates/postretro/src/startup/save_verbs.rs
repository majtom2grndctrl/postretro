// The save verbs: select, create and delete characters; continue, start and delete runs.
// See: context/lib/scripting.md §5 (Durable State Store)

use postretro_entities::SystemReactionCommand;
use postretro_entities::slot_table::SlotValue;
use postretro_sim::scripting::save_files::SlotFile;

use super::{LevelRequest, LevelSource};
use crate::App;
use crate::session::role_state::SessionRole;
use crate::session::saves::{
    collect_world_and_run, publish_save_rows, reload_store, set_seat_zero_from_selection,
};

/// The verbs a host at its frontend may run: they start or continue the
/// host character's own run, and its clients follow into the level.
fn hosting_permits(verb: &str) -> bool {
    matches!(verb, "continueRun" | "newRun")
}

impl App {
    /// Run one save verb, or refuse it with a warning, changing nothing.
    pub(crate) fn handle_save_verb(&mut self, command: SystemReactionCommand) {
        let (verb, source, origin, result) = match command {
            SystemReactionCommand::SelectCharacter {
                index,
                dispatch_source,
                origin,
            } => (
                "selectCharacter",
                dispatch_source,
                origin,
                self.gate("selectCharacter")
                    .and_then(|()| self.select_character_verb(index)),
            ),
            SystemReactionCommand::CreateCharacter {
                index,
                name,
                dispatch_source,
                origin,
            } => (
                "createCharacter",
                dispatch_source,
                origin,
                self.gate("createCharacter")
                    .and_then(|()| self.create_character_verb(index, name)),
            ),
            SystemReactionCommand::DeleteCharacter {
                index,
                dispatch_source,
                origin,
            } => (
                "deleteCharacter",
                dispatch_source,
                origin,
                self.gate("deleteCharacter")
                    .and_then(|()| self.delete_character_verb(index)),
            ),
            SystemReactionCommand::ContinueRun {
                index,
                dispatch_source,
                origin,
            } => (
                "continueRun",
                dispatch_source,
                origin,
                self.gate("continueRun")
                    .and_then(|()| self.continue_run_verb(index)),
            ),
            SystemReactionCommand::NewRun {
                index,
                map,
                dispatch_source,
                origin,
            } => (
                "newRun",
                dispatch_source,
                origin,
                self.gate("newRun")
                    .and_then(|()| self.new_run_verb(index, &map)),
            ),
            SystemReactionCommand::DeleteRun {
                index,
                dispatch_source,
                origin,
            } => (
                "deleteRun",
                dispatch_source,
                origin,
                self.gate("deleteRun")
                    .and_then(|()| self.delete_run_verb(index)),
            ),
            SystemReactionCommand::Checkpoint { name, .. } => {
                self.checkpoint_step(&name);
                return;
            }
            _ => return,
        };
        match result {
            Ok(()) => {
                if let Some(session) = self.session.as_mut() {
                    publish_save_rows(session);
                }
            }
            Err(why) => self.report_refused_save_verb(verb, source, &origin, &why),
        }
    }

    /// A refused save verb warns once per firing source, verb and reason when
    /// this machine's own input or state fired it, so a trigger that keeps
    /// firing does not flood the log; any other origin logs at debug.
    fn report_refused_save_verb(
        &mut self,
        verb: &'static str,
        source: String,
        origin: &postretro_entities::FireOrigin,
        why: &str,
    ) {
        let own = super::lifecycle::session_membership::refusal_is_own(origin);
        let first = own
            && self.session.as_mut().is_some_and(|session| {
                session
                    .scripting
                    .refused_verb_warnings
                    .insert((format!("{source}: {why}"), verb))
            });
        if first {
            log::warn!("[State] {verb} refused: {why} (from `{source}`)");
        } else {
            log::debug!("[State] {verb} refused: {why} (from `{source}`)");
        }
    }

    /// Save verbs run only at the frontend of a single player or a host,
    /// never once a run's level is queued, and while hosting only the verbs
    /// that start the host character's own run.
    fn gate(&self, verb: &str) -> Result<(), String> {
        let Some(session) = self.session.as_ref() else {
            return Err("no session".into());
        };
        if !session.saves.is_loaded() {
            return Err("saves are unavailable for this run".into());
        }
        match self.session_role() {
            SessionRole::Client => return Err("a connected client runs no save verbs".into()),
            SessionRole::Host if !hosting_permits(verb) => {
                return Err("only continueRun and newRun run while hosting".into());
            }
            _ => {}
        }
        if session.saves.run_load_queued() {
            return Err("a run's level is already queued".into());
        }
        if !self.at_frontend() {
            return Err("save verbs run only at the frontend".into());
        }
        Ok(())
    }

    fn select_character_verb(&mut self, index: usize) -> Result<(), String> {
        let session = self.session.as_mut().expect("gated on a session");
        match session.saves.character_slots().get(index) {
            Some(SlotFile::Occupied(_)) => {}
            Some(SlotFile::Unreadable) => {
                return Err(format!("character slot {index} cannot be read"));
            }
            Some(SlotFile::Empty) => return Err(format!("character slot {index} is empty")),
            None => return Err(format!("character slot {index} is not declared")),
        }
        session.saves.select_character(Some(index));
        let with_run = session.saves.run_active();
        reload_store(session, with_run);
        set_seat_zero_from_selection(session);
        Ok(())
    }

    /// `name` is `ui.textEntry` as captured when the step fired. Every session
    /// installs the dispatch-time sink, so a capture is always present in play;
    /// without one (a sink-less harness) text edits queue in body order and
    /// the slot read here sees that order.
    fn create_character_verb(&mut self, index: usize, name: Option<String>) -> Result<(), String> {
        let session = self.session.as_mut().expect("gated on a session");
        match session.saves.character_slots().get(index) {
            Some(SlotFile::Empty) => {}
            Some(_) => return Err(format!("character slot {index} is occupied")),
            None => return Err(format!("character slot {index} is not declared")),
        }
        let name = name.unwrap_or_else(|| {
            match session
                .scripting
                .script_ctx
                .slot_table
                .borrow()
                .get(postretro_entities::engine_state_catalog::TEXT_ENTRY_SLOT)
                .and_then(|slot| slot.value.clone())
            {
                Some(SlotValue::String(name)) => name,
                _ => String::new(),
            }
        });
        if !session.saves.create_character(index, name) {
            return Err(format!("character slot {index} could not be written"));
        }
        session.saves.select_character(Some(index));
        let with_run = session.saves.run_active();
        reload_store(session, with_run);
        set_seat_zero_from_selection(session);
        Ok(())
    }

    fn delete_character_verb(&mut self, index: usize) -> Result<(), String> {
        let session = self.session.as_mut().expect("gated on a session");
        match session.saves.character_slots().get(index) {
            Some(SlotFile::Empty) => return Err(format!("character slot {index} is empty")),
            Some(_) => {}
            None => return Err(format!("character slot {index} is not declared")),
        }
        // The second arm catches an unreadable selected slot, which
        // `selected_character_index` skips.
        let selected = session.saves.selected_character_index() == Some(index)
            || session.saves.selection().character == index as i32;
        if !session.saves.delete_character(index) {
            return Err(format!("character slot {index} could not be deleted"));
        }
        if selected {
            session.saves.select_character(None);
            reload_store(session, false);
            set_seat_zero_from_selection(session);
        }
        Ok(())
    }

    fn delete_run_verb(&mut self, index: usize) -> Result<(), String> {
        let session = self.session.as_mut().expect("gated on a session");
        match session.saves.run_slots().get(index) {
            Some(SlotFile::Empty) => return Err(format!("run slot {index} is empty")),
            Some(_) => {}
            None => return Err(format!("run slot {index} is not declared")),
        }
        let selected = session.saves.selection().run == index as i32;
        if !session.saves.delete_run(index) {
            return Err(format!("run slot {index} could not be deleted"));
        }
        if selected {
            session.saves.select_run(None);
        }
        Ok(())
    }

    fn continue_run_verb(&mut self, index: usize) -> Result<(), String> {
        let hosting = self.session_role() == SessionRole::Host;
        let session = self.session.as_mut().expect("gated on a session");
        let document = match session.saves.run_slots().get(index) {
            Some(SlotFile::Occupied(document)) => document.clone(),
            Some(SlotFile::Unreadable) => return Err(format!("run slot {index} cannot be read")),
            Some(SlotFile::Empty) => return Err(format!("run slot {index} is empty")),
            None => return Err(format!("run slot {index} is not declared")),
        };
        let claim = document
            .run
            .as_ref()
            .map(|run| run.character.clone())
            .ok_or_else(|| format!("run slot {index} names no character"))?;
        let Some(character) = session.saves.character_index_for_claim(&claim) else {
            return Err(format!("run slot {index}'s character was deleted"));
        };
        if hosting && session.saves.selected_character_index() != Some(character) {
            return Err(format!(
                "run slot {index} belongs to another character than the host's"
            ));
        }
        let level = document
            .level
            .as_ref()
            .map(|level| level.catalog_id.clone())
            .ok_or_else(|| format!("run slot {index} has reached no level"))?;
        let known = session
            .scripting
            .script_ctx
            .data_registry
            .borrow()
            .maps
            .iter()
            .any(|entry| entry.id == level);
        if !known {
            return Err(format!(
                "run slot {index}'s level `{level}` is not in this mod's catalog"
            ));
        }
        if !hosting {
            session.saves.select_character(Some(character));
        }
        session.saves.select_run(Some(index));
        reload_store(session, true);
        set_seat_zero_from_selection(session);
        session.saves.set_run_active(true);
        session.saves.mark_continue_arrival(&level);
        // Continue arrives at the run's checkpoint, if it reached one.
        session.saves.set_spawn_checkpoint(
            document
                .level
                .as_ref()
                .and_then(|level| level.checkpoint.clone()),
        );
        self.queue_run_level(level);
        Ok(())
    }

    fn new_run_verb(&mut self, index: usize, map: &str) -> Result<(), String> {
        let session = self.session.as_mut().expect("gated on a session");
        let Some(claim) = session
            .saves
            .selected_character()
            .map(|record| record.claim.clone())
        else {
            return Err("no character is selected".into());
        };
        match session.saves.run_slots().get(index) {
            Some(SlotFile::Empty) => {}
            Some(SlotFile::Unreadable) => return Err(format!("run slot {index} cannot be read")),
            Some(SlotFile::Occupied(_)) if session.saves.run_character(index) == Some(&claim) => {}
            Some(SlotFile::Occupied(_)) => {
                return Err(format!("run slot {index} holds another character's run"));
            }
            None => return Err(format!("run slot {index} is not declared")),
        }
        let known = session
            .scripting
            .script_ctx
            .data_registry
            .borrow()
            .maps
            .iter()
            .any(|entry| entry.id == map);
        if !known {
            return Err(format!("level `{map}` is not in this mod's catalog"));
        }
        // A new run: run-scoped globals, faction sentiment and the level
        // reset; world slots and the character as it stands are kept.
        let previous_run = session.saves.selected_run_index();
        let previous_active = session.saves.run_active();
        session.saves.select_run(None);
        reload_store(session, false);
        set_seat_zero_from_selection(session);
        let (_, live) = collect_world_and_run(session);
        // The run file lands before its level load begins. A failed write
        // puts the previous selection and store back.
        if !session.saves.write_new_run(index, claim, map, live) {
            session.saves.select_run(previous_run);
            session.saves.set_run_active(previous_active);
            reload_store(session, previous_active);
            set_seat_zero_from_selection(session);
            return Err(format!("run slot {index} could not be written"));
        }
        session.saves.select_run(Some(index));
        session.saves.set_run_active(true);
        self.queue_run_level(map.to_owned());
        Ok(())
    }

    /// Clear the frontend menu and queue the run's level, as `loadLevel`
    /// does. Every later save verb is refused until the frontend is entered
    /// again.
    fn queue_run_level(&mut self, level: String) {
        if let Some(session) = self.session.as_mut() {
            session.modal_stack.clear_pushed();
            session.saves.set_run_load_queued(true);
        }
        // A return to the frontend queued earlier in this drain would enter
        // the frontend after the run starts and end it; the run's level
        // replaces it, as a played load replaces a backdrop.
        self.level_requests
            .retain(|request| !matches!(request, LevelRequest::Unload));
        self.enqueue_level_request(LevelRequest::Load(LevelSource::Catalog(level)));
    }
}

impl App {
    /// A `checkpoint(name)` step on the host or in single player: the
    /// checkpoint's spawn set takes over for respawn and late joiners, the
    /// active run saves at it, then the character and the world save as they
    /// are now. Outside a run only the character and world save; on a
    /// connected client and in the frontend backdrop it does nothing.
    fn checkpoint_step(&mut self, name: &str) {
        if self.session_role() == SessionRole::Client {
            log::debug!("[State] checkpoint `{name}` ignored on a connected client");
            return;
        }
        if !self.played_level_present() {
            log::debug!("[State] checkpoint `{name}` ignored outside a played level");
            return;
        }
        self.harvest_bound_pawns_for_save();
        // Each guest's loadout, harvested just now, returns to it.
        if let Some(session) = self.session.as_mut() {
            crate::session::store_saves::send_character_loadouts(session);
        }
        self.apply_spawn_checkpoint(Some(name.to_owned()));
        let catalog_id = match self.active_level_source.as_ref() {
            Some(LevelSource::Catalog(id)) => Some(id.clone()),
            _ => None,
        };
        let Some(session) = self.session.as_mut() else {
            return;
        };
        if !session.state_store_lifecycle.can_save() {
            return;
        }
        if session.saves.run_active()
            && let Some(catalog_id) = catalog_id
        {
            let (_, live) = collect_world_and_run(session);
            if !session.saves.save_run_progress(
                &catalog_id,
                crate::session::saves::RunCheckpoint::Set(name.to_owned()),
                live,
            ) {
                log::warn!(
                    "[State] the run did not save at checkpoint `{name}`; keeping the character as it was"
                );
                return;
            }
        }
        let _ = crate::session::progress_save::save_world_and_character(session);
        publish_save_rows(session);
    }

    /// Make `checkpoint`'s spawn set the level's active set. Placements made
    /// against the previous set reset, and respawn republishes against the
    /// new one.
    pub(crate) fn apply_spawn_checkpoint(&mut self, checkpoint: Option<String>) {
        self.host_spawn_points = crate::scripting::builtins::player_spawn::active_spawn_set(
            &self.level_spawn_points,
            checkpoint.as_deref(),
        );
        if let Some(session) = self.session.as_mut() {
            session.saves.set_spawn_checkpoint(checkpoint);
            if let Some(seats) = session.seat_table.as_mut() {
                seats.reset_placements_for_spawn_set();
            }
        }
        self.refresh_respawn_placements();
    }
}
