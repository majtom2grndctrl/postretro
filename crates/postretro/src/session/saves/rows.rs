// The engine-written `saves.*` slots: one row per declared slot and the selection.
// See: context/lib/scripting.md §5 (Engine State SDK)

use postretro_entities::engine_state_collections::{
    LIST_PRESENT_FIELD, SAVES_CHARACTERS_LIST, SAVES_RUNS_LIST,
};
use postretro_entities::slot_table::{SlotTable, SlotValue};
use postretro_sim::scripting::save_files::{SlotFile, format_saved_at};

use crate::session::Session;

/// Publish every `saves.*` row and the selection. Runs only when the slots
/// or selection change, never per frame; unchanged values are not rewritten,
/// so crossings see real changes only.
pub(crate) fn publish_save_rows(session: &mut Session) {
    let saves = &session.saves;
    let maps = session
        .scripting
        .script_ctx
        .data_registry
        .borrow()
        .maps
        .clone();
    let mut table = session.scripting.script_ctx.slot_table.borrow_mut();
    let characters = saves.character_slots();
    for row in 0..SAVES_CHARACTERS_LIST.capacity {
        let slot = characters.get(row);
        let name = slot
            .and_then(SlotFile::document)
            .and_then(|document| document.seats.first())
            .map_or(String::new(), |record| record.name.clone());
        write_row(
            &mut table,
            |field| SAVES_CHARACTERS_LIST.slot_name(row, field),
            [
                (LIST_PRESENT_FIELD, SlotValue::Boolean(slot.is_some())),
                (
                    "occupied",
                    SlotValue::Boolean(slot.is_some_and(SlotFile::is_occupied)),
                ),
                (
                    "readable",
                    SlotValue::Boolean(slot.is_some_and(|slot| slot.document().is_some())),
                ),
                ("name", SlotValue::String(name)),
            ],
        );
    }
    let runs = saves.run_slots();
    for row in 0..SAVES_RUNS_LIST.capacity {
        let slot = runs.get(row);
        let document = slot.and_then(SlotFile::document);
        let level_name = document
            .and_then(|document| document.level.as_ref())
            .map_or(String::new(), |level| {
                maps.iter()
                    .find(|entry| entry.id == level.catalog_id)
                    .map_or_else(|| level.catalog_id.clone(), |entry| entry.name.clone())
            });
        let character_name = document
            .and_then(|document| document.run.as_ref())
            .and_then(|run| {
                characters
                    .iter()
                    .filter_map(SlotFile::document)
                    .filter_map(|document| document.seats.first())
                    .find(|record| record.claim == run.character)
            })
            .map_or(String::new(), |record| record.name.clone());
        let saved_at = document
            .and_then(|document| document.run.as_ref())
            .map_or(String::new(), |run| format_saved_at(run.saved_at));
        write_row(
            &mut table,
            |field| SAVES_RUNS_LIST.slot_name(row, field),
            [
                (LIST_PRESENT_FIELD, SlotValue::Boolean(slot.is_some())),
                (
                    "occupied",
                    SlotValue::Boolean(slot.is_some_and(SlotFile::is_occupied)),
                ),
                ("readable", SlotValue::Boolean(document.is_some())),
                ("levelName", SlotValue::String(level_name)),
                ("characterName", SlotValue::String(character_name)),
                ("savedAt", SlotValue::String(saved_at)),
            ],
        );
    }
    let selection = saves.selection();
    for (name, index) in [
        ("saves.character", selection.character),
        ("saves.run", selection.run),
    ] {
        write_if_changed(&mut table, name, SlotValue::Number(index as f32));
    }
    drop(table);
    super::set_seat_zero_name_from_selection(session);
}

fn write_row<const N: usize>(
    table: &mut SlotTable,
    slot_name: impl Fn(&str) -> String,
    fields: [(&str, SlotValue); N],
) {
    for (field, value) in fields {
        write_if_changed(table, &slot_name(field), value);
    }
}

fn write_if_changed(table: &mut SlotTable, name: &str, value: SlotValue) {
    if let Some(slot) = table.get_mut(name)
        && slot.value.as_ref() != Some(&value)
    {
        slot.write_value(Some(value));
    }
}
