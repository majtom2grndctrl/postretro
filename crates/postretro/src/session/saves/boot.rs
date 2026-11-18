// Boot-time save loading: one-way migration from `state.json`, implicit slots and the stored selection.
// See: context/lib/scripting.md §5 (Durable State Store)

use std::collections::BTreeSet;
use std::path::PathBuf;
use std::time::UNIX_EPOCH;

use postretro_scripting_core::runtime::SavesManifest;
use postretro_sim::scripting::save_files::{
    LedgerDocument, RunRecord, SaveDir, SeatRecord, Selection, SlotFile, now_unix_seconds,
    read_selection, read_slot,
};
use postretro_sim::scripting::state_persistence::{
    encode_player_claim_id, load_persisted_state, persisted_state_version_is_supported,
    split_world_and_run,
};
use postretro_net::wire::PlayerClaimId;

use super::{SaveSlots, write_slot};

/// What boot knows when it loads a mod's saves.
pub(crate) struct BootInputs {
    pub(crate) dir: SaveDir,
    /// The mod's previous single save file, migrated once.
    pub(crate) legacy_state: PathBuf,
    pub(crate) declared: Option<SavesManifest>,
    /// The device's player id: the migrated or implicit character's claim.
    pub(crate) player_id: Option<[u8; 16]>,
    /// Durable keys of the committed `persist: "run"` globals.
    pub(crate) run_keys: BTreeSet<String>,
}

/// Load a mod's save slots, migrating `state.json` first when no world file
/// exists yet. A mod without a `saves` block gets its one character and run
/// created and selected, and its run active.
pub(crate) fn load_save_slots(inputs: BootInputs) -> SaveSlots {
    if !inputs.dir.world_path().exists() && inputs.legacy_state.exists() {
        migrate_legacy_state(&inputs);
    }
    let (character_count, run_count) = inputs
        .declared
        .map_or((1, 1), |saves| (saves.character_slots, saves.run_slots));
    let mut slots = SaveSlots {
        characters: (0..character_count)
            .map(|index| read_slot(&inputs.dir.character_path(index)))
            .collect(),
        runs: (0..run_count)
            .map(|index| read_slot(&inputs.dir.run_path(index)))
            .collect(),
        world: Some(read_slot(&inputs.dir.world_path())),
        selection: Selection::default(),
        run_active: false,
        declared: inputs.declared,
        dir: Some(inputs.dir.clone()),
    };
    if inputs.declared.is_none() {
        create_implicit_slots(&mut slots, &inputs);
    } else {
        slots.selection = validated_selection(&slots, read_selection(&inputs.dir.selection_path()));
    }
    slots
}

/// A stored selection naming an empty, unreadable or undeclared slot boots
/// as `-1` with a warning.
fn validated_selection(slots: &SaveSlots, stored: Selection) -> Selection {
    let check = |index: i32, files: &[SlotFile], kind: &str| -> i32 {
        if index < 0 {
            return -1;
        }
        if SaveSlots::selected_index(index, files).is_some() {
            return index;
        }
        log::warn!(
            "[State] the stored {kind} selection {index} names an empty, unreadable or undeclared slot; booting with none"
        );
        -1
    };
    Selection {
        character: check(stored.character, &slots.characters, "character"),
        run: check(stored.run, &slots.runs, "run"),
    }
}

/// A mod without a `saves` block: its one character and run exist, are
/// selected, and its run is active for the whole process.
fn create_implicit_slots(slots: &mut SaveSlots, inputs: &BootInputs) {
    let claim = match slots.characters[0].document().and_then(|doc| doc.seats.first()) {
        Some(record) => record.claim.clone(),
        None if matches!(slots.characters[0], SlotFile::Unreadable) => {
            log::warn!("[State] the character save cannot be read; progress will not be saved");
            return;
        }
        None => {
            let claim = character_claim(inputs.player_id);
            let document = character_document(claim.clone(), Default::default());
            write_slot(
                &inputs.dir.character_path(0),
                &mut slots.characters[0],
                document,
            );
            claim
        }
    };
    if matches!(slots.runs[0], SlotFile::Empty) {
        let document = LedgerDocument {
            run: Some(RunRecord {
                character: claim,
                saved_at: now_unix_seconds(),
            }),
            ..LedgerDocument::default()
        };
        write_slot(&inputs.dir.run_path(0), &mut slots.runs[0], document);
    }
    slots.selection = Selection {
        character: 0,
        run: if slots.runs[0].document().is_some() { 0 } else { -1 },
    };
    slots.run_active = slots.selected_run().is_some();
}

fn character_claim(player_id: Option<[u8; 16]>) -> String {
    let id = player_id.unwrap_or_else(random_character_id);
    encode_player_claim_id(&PlayerClaimId(id))
}

/// A fresh character's own random id.
pub(crate) fn random_character_id() -> [u8; 16] {
    let mut id = [0; 16];
    if let Err(error) = getrandom::fill(&mut id) {
        log::warn!("[State] could not draw a character id from the OS: {error}");
    }
    id
}

fn character_document(
    claim: String,
    per_owner: std::collections::BTreeMap<
        String,
        postretro_sim::scripting::state_persistence::PersistedValue,
    >,
) -> LedgerDocument {
    LedgerDocument {
        seats: vec![SeatRecord {
            claim,
            name: String::new(),
            per_owner,
            loadout: Default::default(),
        }],
        ..LedgerDocument::default()
    }
}

/// One-way migration from `state.json`. The local player's rows become
/// character slot 0 with the old `player_id` as its claim, and the globals
/// become the world file. Run-scoped globals and faction sentiment go to the
/// implicit run of a mod without a `saves` block and are dropped otherwise.
/// The world file is written last: its presence is what ends migration, so a
/// crash after any earlier write migrates again from the kept source.
fn migrate_legacy_state(inputs: &BootInputs) {
    let legacy = match load_persisted_state(&inputs.legacy_state) {
        Ok(Some(legacy)) if persisted_state_version_is_supported(&legacy) => legacy,
        Ok(Some(_)) => {
            log::warn!(
                "[State] {} has an unsupported version; not migrating it",
                inputs.legacy_state.display()
            );
            return;
        }
        Ok(None) => return,
        Err(error) => {
            log::warn!(
                "[State] cannot migrate {}: {error}",
                inputs.legacy_state.display()
            );
            return;
        }
    };
    let claim = character_claim(inputs.player_id);
    let (world_slots, run_slots) = split_world_and_run(legacy.slots(), &inputs.run_keys);
    let mut ok = true;
    let mut slot = SlotFile::Empty;
    ok &= write_slot(
        &inputs.dir.character_path(0),
        &mut slot,
        character_document(claim.clone(), legacy.per_owner_for(&claim)),
    );
    let implicit = inputs.declared.is_none();
    if implicit {
        // The source's own age keeps a repeated migration byte-identical.
        let saved_at = std::fs::metadata(&inputs.legacy_state)
            .and_then(|meta| meta.modified())
            .ok()
            .and_then(|modified| modified.duration_since(UNIX_EPOCH).ok())
            .map_or(0, |age| age.as_secs());
        let mut slot = SlotFile::Empty;
        ok &= write_slot(
            &inputs.dir.run_path(0),
            &mut slot,
            LedgerDocument {
                run: Some(RunRecord {
                    character: claim,
                    saved_at,
                }),
                slots: run_slots,
                faction_sentiment: legacy.faction_sentiment().clone(),
                ..LedgerDocument::default()
            },
        );
    }
    if let Err(error) = postretro_sim::scripting::save_files::write_selection(
        &inputs.dir.selection_path(),
        Selection {
            character: 0,
            run: if implicit { 0 } else { -1 },
        },
    ) {
        log::warn!("[State] migration could not store the selection: {error}");
        ok = false;
    }
    if !ok {
        log::warn!("[State] migration incomplete; it will run again at the next boot");
        return;
    }
    let mut world = SlotFile::Empty;
    if write_slot(
        &inputs.dir.world_path(),
        &mut world,
        LedgerDocument {
            slots: world_slots,
            ..LedgerDocument::default()
        },
    ) {
        log::info!(
            "[State] migrated {} into character, world and run saves; the source is kept",
            inputs.legacy_state.display()
        );
    }
}
