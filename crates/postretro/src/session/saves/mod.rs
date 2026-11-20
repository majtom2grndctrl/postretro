// Character, world and run save slots: what is on disk, what is selected, and the run in play.
// See: context/lib/scripting.md §5 (Durable State Store) · context/lib/networking.md §Session-state ledger

mod boot;
mod rows;
mod save_points;
mod store;

#[cfg(test)]
pub(crate) mod test_fixtures;
#[cfg(test)]
mod tests;

pub(crate) use boot::{BootInputs, load_save_slots};
pub(crate) use rows::publish_save_rows;
pub(crate) use save_points::{
    SeamLevelStart, end_run_at_frontend, save_on_frontend_entry, save_on_level_start,
};
#[cfg(test)]
pub(crate) use store::boot_restore_with;
pub(crate) use store::{
    boot_restore, collect_character_record, collect_world_and_run, reload_store,
    set_seat_zero_carry_from_selection,
};

use postretro_combat_model::CarriedState;
use postretro_scripting_core::runtime::SavesManifest;
use postretro_sim::scripting::save_files::{
    LedgerDocument, SaveDir, SeatRecord, Selection, SlotFile, write_document, write_selection,
};
use postretro_sim::scripting::state_persistence::{PersistedState, decode_player_claim_id};

/// A mod's save slots as this process holds them. Every file write goes
/// through here, so an unreadable slot is never written.
#[derive(Debug, Default)]
pub(crate) struct SaveSlots {
    dir: Option<SaveDir>,
    /// The mod's `saves` block; `None` is a mod without one, which gets one
    /// character and one run, selected and active for the whole process.
    declared: Option<SavesManifest>,
    characters: Vec<SlotFile>,
    runs: Vec<SlotFile>,
    world: Option<SlotFile>,
    selection: Selection,
    /// A run is active from Continue or `newRun` until the frontend is entered
    /// again; always, for a mod without a `saves` block.
    run_active: bool,
    /// The catalog id Continue's level load is arriving at, until its level
    /// start writes the run.
    continue_arrival: Option<String>,
    /// Continue or `newRun` queued its level: every later save verb is
    /// refused until the frontend is entered again.
    run_load_queued: bool,
    /// The checkpoint whose spawn set arrival, respawn and late joiners use
    /// in the current level; `None` is the level-start set.
    spawn_checkpoint: Option<String>,
    /// `spawn_checkpoint` is a level seam's name, from a crossing or Continue
    /// from one: with no spawn keyed to it, the level-start set applies
    /// without the unknown-checkpoint warning.
    spawn_checkpoint_seam: bool,
    /// The seam pose Continue arrives at, with the catalog id it is for: the
    /// install of that level takes it for seat 0's spawn. Kept here, not as a
    /// seam crossing's arrival, so the unload of a backdrop before Continue's
    /// level installs cannot drop it.
    continue_seam_pose: Option<ContinueSeamPose>,
    /// On a connected client, the loadout its host last returned: what its
    /// character saves carry. `None` keeps the loadout it joined with.
    client_loadout: Option<postretro_sim::scripting::save_files::LoadoutRecord>,
}

impl SaveSlots {
    /// Whether a save directory was loaded for this mod.
    pub(crate) fn is_loaded(&self) -> bool {
        self.dir.is_some()
    }

    /// Whether the mod declared a `saves` block.
    pub(crate) fn has_saves_block(&self) -> bool {
        self.declared.is_some()
    }

    pub(crate) fn character_slots(&self) -> &[SlotFile] {
        &self.characters
    }

    pub(crate) fn run_slots(&self) -> &[SlotFile] {
        &self.runs
    }

    pub(crate) fn selection(&self) -> Selection {
        self.selection
    }

    pub(crate) fn run_active(&self) -> bool {
        self.run_active
    }

    pub(crate) fn client_loadout(
        &self,
    ) -> Option<&postretro_sim::scripting::save_files::LoadoutRecord> {
        self.client_loadout.as_ref()
    }

    /// Record the loadout a client's host returned, as a character saves it:
    /// never downed or dead. `None` clears it when the session ends.
    pub(crate) fn set_client_loadout(
        &mut self,
        loadout: Option<postretro_sim::scripting::save_files::LoadoutRecord>,
    ) {
        self.client_loadout = loadout.map(|mut loadout| {
            loadout.life_state = None;
            loadout.health = loadout.health.filter(|health| *health > 0.0);
            loadout
        });
    }

    pub(crate) fn spawn_checkpoint(&self) -> Option<&str> {
        self.spawn_checkpoint.as_deref()
    }

    pub(crate) fn set_spawn_checkpoint(&mut self, checkpoint: Option<String>) {
        self.spawn_checkpoint = checkpoint;
        self.spawn_checkpoint_seam = false;
    }

    /// Make level seam `seam`'s name the spawn checkpoint, marked a seam's.
    pub(crate) fn set_seam_spawn_checkpoint(&mut self, seam: String) {
        self.spawn_checkpoint = Some(seam);
        self.spawn_checkpoint_seam = true;
    }

    /// Whether the spawn checkpoint is a level seam's.
    pub(crate) fn spawn_checkpoint_is_seam(&self) -> bool {
        self.spawn_checkpoint.is_some() && self.spawn_checkpoint_seam
    }

    /// The seam pose Continue arrives at in `catalog_id`, taken once. Any
    /// other level's install drops it.
    pub(crate) fn take_continue_seam_pose(
        &mut self,
        catalog_id: Option<&str>,
    ) -> Option<ContinueSeamPose> {
        self.continue_seam_pose
            .take()
            .filter(|pose| Some(pose.catalog_id.as_str()) == catalog_id)
    }

    pub(crate) fn set_continue_seam_pose(&mut self, pose: Option<ContinueSeamPose>) {
        self.continue_seam_pose = pose;
    }

    pub(crate) fn clear_continue_arrival(&mut self) {
        self.continue_arrival = None;
    }

    /// Whether a level start at `catalog_id` is Continue's own arrival.
    pub(crate) fn is_continue_arrival(&self, catalog_id: Option<&str>) -> bool {
        self.continue_arrival.is_some() && self.continue_arrival.as_deref() == catalog_id
    }

    pub(crate) fn run_load_queued(&self) -> bool {
        self.run_load_queued
    }

    pub(crate) fn set_run_load_queued(&mut self, queued: bool) {
        self.run_load_queued = queued;
    }

    pub(crate) fn set_run_active(&mut self, active: bool) {
        // A mod without a `saves` block keeps its one run active throughout.
        self.run_active = active || (self.declared.is_none() && self.selected_run().is_some());
    }

    fn selected_index(index: i32, slots: &[SlotFile]) -> Option<usize> {
        usize::try_from(index).ok().filter(|&index| {
            slots
                .get(index)
                .is_some_and(|slot| slot.document().is_some())
        })
    }

    pub(crate) fn selected_character_index(&self) -> Option<usize> {
        Self::selected_index(self.selection.character, &self.characters)
    }

    pub(crate) fn selected_run_index(&self) -> Option<usize> {
        Self::selected_index(self.selection.run, &self.runs)
    }

    /// The selected character's record.
    pub(crate) fn selected_character(&self) -> Option<&SeatRecord> {
        let index = self.selected_character_index()?;
        self.characters[index].document()?.seats.first()
    }

    /// The selected run's document.
    pub(crate) fn selected_run(&self) -> Option<&LedgerDocument> {
        let index = self.selected_run_index()?;
        self.runs[index].document()
    }

    /// The selected character's claim: the id it joins with and the key its
    /// per-owner rows use.
    pub(crate) fn selected_character_id(&self) -> Option<[u8; 16]> {
        self.selected_character()
            .and_then(|record| decode_player_claim_id(&record.claim))
            .map(|claim| claim.0)
    }

    pub(crate) fn selected_character_name(&self) -> &str {
        self.selected_character()
            .map_or("", |record| record.name.as_str())
    }

    /// The selected character's saved loadout, as seat 0 carries it.
    pub(crate) fn selected_carry(&self) -> Option<CarriedState> {
        let record = self.selected_character()?;
        let mut warnings = Vec::new();
        let carried = record.loadout.to_carried(&mut warnings);
        for warning in warnings {
            log::warn!("[State] character `{}`: {warning}", record.name);
        }
        carried
    }

    pub(crate) fn world_document(&self) -> Option<&LedgerDocument> {
        self.world.as_ref().and_then(SlotFile::document)
    }

    /// The store document the world, the selected character and, when
    /// `with_run`, the selected run compose into.
    pub(crate) fn store_document(&self, with_run: bool) -> PersistedState {
        PersistedState::from_ledger(
            self.world_document(),
            self.selected_run().filter(|_| with_run),
            self.selected_character(),
        )
    }

    /// Write the selected character's file. Refused, changing nothing, with
    /// no selected character or no save directory.
    pub(crate) fn save_selected_character(&mut self, record: SeatRecord) -> bool {
        let Some(index) = self.selected_character_index() else {
            return false;
        };
        let document = LedgerDocument {
            seats: vec![record],
            ..self.characters[index]
                .document()
                .cloned()
                .unwrap_or_default()
        };
        let Some(path) = self.dir.as_ref().map(|dir| dir.character_path(index)) else {
            return false;
        };
        write_slot(&path, &mut self.characters[index], document)
    }

    /// Write the selected run's file.
    pub(crate) fn save_selected_run(&mut self, document: LedgerDocument) -> bool {
        let Some(index) = self.selected_run_index() else {
            return false;
        };
        let Some(path) = self.dir.as_ref().map(|dir| dir.run_path(index)) else {
            return false;
        };
        write_slot(&path, &mut self.runs[index], document)
    }

    /// Write the world file. An unreadable one is never replaced.
    pub(crate) fn save_world(&mut self, document: LedgerDocument) -> bool {
        let Some(path) = self.dir.as_ref().map(SaveDir::world_path) else {
            return false;
        };
        let world = self.world.get_or_insert(SlotFile::Empty);
        write_slot(&path, world, document)
    }

    /// Store the selection in the mod's save directory.
    pub(crate) fn store_selection(&self) {
        let Some(dir) = self.dir.as_ref() else {
            return;
        };
        let path = dir.selection_path();
        if let Err(error) = write_selection(&path, self.selection) {
            log::warn!("[State] failed to save {}: {error}", path.display());
        }
    }
}

/// Write one slot's document, keeping the in-memory copy in step. An
/// unreadable slot is never overwritten; a failed write warns and keeps the
/// previous copy.
fn write_slot(path: &std::path::Path, slot: &mut SlotFile, document: LedgerDocument) -> bool {
    if matches!(slot, SlotFile::Unreadable) {
        log::warn!(
            "[State] {} cannot be read by this build; not overwriting it",
            path.display()
        );
        return false;
    }
    match write_document(path, &document) {
        Ok(()) => {
            *slot = SlotFile::Occupied(document);
            true
        }
        Err(error) => {
            log::warn!("[State] failed to save {}: {error}", path.display());
            false
        }
    }
}

impl SaveSlots {
    /// Record that Continue's own level load is arriving at `catalog_id`: its
    /// level start keeps the run's checkpoint.
    pub(crate) fn mark_continue_arrival(&mut self, catalog_id: &str) {
        self.continue_arrival = Some(catalog_id.to_owned());
    }

    /// Whether this level start is Continue's own arrival at `catalog_id`.
    fn take_continue_arrival(&mut self, catalog_id: &str) -> bool {
        self.continue_arrival
            .take()
            .is_some_and(|arriving| arriving == catalog_id)
    }

    /// Write the selected run at a level start or checkpoint: the level it
    /// reached, the checkpoint, its live run-scoped globals and sentiment, and
    /// the save time. Everything else the run file held, unknown sections
    /// included, is kept.
    pub(crate) fn save_run_progress(
        &mut self,
        catalog_id: &str,
        checkpoint: RunCheckpoint,
        live: LedgerDocument,
    ) -> bool {
        let Some(previous) = self.selected_run().cloned() else {
            return false;
        };
        let keep_checkpoint = matches!(checkpoint, RunCheckpoint::LevelStart)
            && self.take_continue_arrival(catalog_id);
        let kept = || {
            previous
                .level
                .as_ref()
                .filter(|level| level.catalog_id == catalog_id)
        };
        let (checkpoint, seam) = match checkpoint {
            RunCheckpoint::Set(name) => (Some(name), None),
            RunCheckpoint::Seam { name, pose } => (
                Some(name),
                Some(postretro_sim::scripting::save_files::SeamCheckpointRecord { pose }),
            ),
            // Continue's own level start keeps the checkpoint, its seam mark
            // and pose included, so a second Continue arrives there again.
            _ if keep_checkpoint => kept().map_or((None, None), |level| {
                (level.checkpoint.clone(), level.seam.clone())
            }),
            _ => (None, None),
        };
        let run = previous.run.clone().map(|mut run| {
            run.saved_at = postretro_sim::scripting::save_files::now_unix_seconds();
            run
        });
        self.save_selected_run(LedgerDocument {
            level: Some(postretro_sim::scripting::save_files::LevelRecord {
                catalog_id: catalog_id.to_owned(),
                checkpoint,
                seam,
            }),
            run,
            slots: live.slots,
            faction_sentiment: live.faction_sentiment,
            ..previous
        })
    }
}

/// What a run write does to the run's checkpoint.
pub(crate) enum RunCheckpoint {
    /// A level start clears the checkpoint, except Continue's own arrival,
    /// which keeps it.
    LevelStart,
    /// A `checkpoint(name)` step.
    Set(String),
    /// A seam crossing's level start: the checkpoint is the seam's name,
    /// marked a seam's, with seat 0's landmark-relative pose if it crossed.
    Seam {
        name: String,
        pose: Option<postretro_sim::scripting::save_files::SeamPoseRecord>,
    },
}

/// Continue's seam pose for seat 0, for the level it is in.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ContinueSeamPose {
    pub(crate) catalog_id: String,
    pub(crate) seam: String,
    pub(crate) pose: postretro_sim::scripting::save_files::SeamPoseRecord,
}

impl SaveSlots {
    /// Select a character slot, or `None` for none, and store the selection.
    pub(crate) fn select_character(&mut self, index: Option<usize>) {
        self.selection.character = index.map_or(-1, |index| index as i32);
        self.store_selection();
    }

    /// Select a run slot, or `None` for none, and store the selection.
    pub(crate) fn select_run(&mut self, index: Option<usize>) {
        self.selection.run = index.map_or(-1, |index| index as i32);
        if index.is_none() {
            // No run is selected, so none is in play, also for a mod without
            // a `saves` block: its save points then write the character and
            // world alone.
            self.run_active = false;
        }
        self.store_selection();
    }

    /// The character slot holding `claim`, readable.
    pub(crate) fn character_index_for_claim(&self, claim: &str) -> Option<usize> {
        self.characters.iter().position(|slot| {
            slot.document()
                .and_then(|document| document.seats.first())
                .is_some_and(|record| record.claim == claim)
        })
    }

    /// The character claim a run belongs to.
    pub(crate) fn run_character(&self, index: usize) -> Option<&str> {
        self.runs
            .get(index)?
            .document()?
            .run
            .as_ref()
            .map(|run| run.character.as_str())
    }

    /// Write a fresh character into an empty slot.
    pub(crate) fn create_character(&mut self, index: usize, name: String) -> bool {
        let Some(path) = self.dir.as_ref().map(|dir| dir.character_path(index)) else {
            return false;
        };
        let Some(slot) = self.characters.get_mut(index) else {
            return false;
        };
        let claim = postretro_sim::scripting::state_persistence::encode_player_claim_id(
            &postretro_net::wire::PlayerClaimId(boot::random_character_id()),
        );
        write_slot(
            &path,
            slot,
            LedgerDocument {
                seats: vec![SeatRecord {
                    claim,
                    name,
                    ..SeatRecord::default()
                }],
                ..LedgerDocument::default()
            },
        )
    }

    /// Delete a character slot's file, readable or not.
    pub(crate) fn delete_character(&mut self, index: usize) -> bool {
        let Some(path) = self.dir.as_ref().map(|dir| dir.character_path(index)) else {
            return false;
        };
        delete_slot_file(&path, &mut self.characters[index])
    }

    /// Delete a run slot's file, readable or not.
    pub(crate) fn delete_run(&mut self, index: usize) -> bool {
        let Some(path) = self.dir.as_ref().map(|dir| dir.run_path(index)) else {
            return false;
        };
        delete_slot_file(&path, &mut self.runs[index])
    }

    /// Write a new run for `claim` into slot `index`, starting at `catalog_id`
    /// with the live (just reset) run state.
    pub(crate) fn write_new_run(
        &mut self,
        index: usize,
        claim: String,
        catalog_id: &str,
        live: LedgerDocument,
    ) -> bool {
        let Some(path) = self.dir.as_ref().map(|dir| dir.run_path(index)) else {
            return false;
        };
        write_slot(
            &path,
            &mut self.runs[index],
            LedgerDocument {
                level: Some(postretro_sim::scripting::save_files::LevelRecord {
                    catalog_id: catalog_id.to_owned(),
                    checkpoint: None,
                    seam: None,
                }),
                run: Some(postretro_sim::scripting::save_files::RunRecord {
                    character: claim,
                    saved_at: postretro_sim::scripting::save_files::now_unix_seconds(),
                }),
                slots: live.slots,
                faction_sentiment: live.faction_sentiment,
                ..LedgerDocument::default()
            },
        )
    }
}

fn delete_slot_file(path: &std::path::Path, slot: &mut SlotFile) -> bool {
    match postretro_sim::scripting::save_files::delete_slot(path) {
        Ok(()) => {
            *slot = SlotFile::Empty;
            true
        }
        Err(error) => {
            log::warn!("[State] failed to delete {}: {error}", path.display());
            false
        }
    }
}
