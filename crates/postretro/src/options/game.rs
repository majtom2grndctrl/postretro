// Game-scoped settings under `[game."<mod_id>"]`, bindings first. Rows are a
// per-mod diff over the author's defaults: keys only, never activators.
// See: context/lib/player_options.md §6

use std::collections::BTreeMap;

use toml::Value;

use super::PlayerOptions;

const GAME: &str = "game";
const BINDINGS: &str = "bindings";

/// One saved binding row: input strings in slot order. `None` marks an element
/// that is not a string; that slot falls back to its author default.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct StoredBindingRow {
    /// `keyboard_mouse` or `gamepad`.
    pub(crate) class_key: String,
    pub(crate) command_id: String,
    pub(crate) inputs: Vec<Option<String>>,
}

/// A row write keyed by mod id, device-class key, and command ID. `None`
/// removes the row, so the command follows the author default again.
pub(crate) type GameBindingWrites = BTreeMap<(String, String, String), Option<Vec<String>>>;

impl PlayerOptions {
    /// The saved binding rows for `mod_id`: the loaded document's rows with
    /// this session's writes applied. A row whose command this build does not
    /// know is returned too; the binding layer ignores it, and the save keeps
    /// it. A row that is not an array is skipped with a warning and kept in the
    /// file.
    pub(crate) fn game_binding_rows(&self, mod_id: &str) -> Vec<StoredBindingRow> {
        let mut rows: BTreeMap<(String, String), Vec<Option<String>>> = BTreeMap::new();
        if let Some(classes) = self.stored.table_at(&[GAME, mod_id, BINDINGS]) {
            for (class_key, class_rows) in classes {
                let Value::Table(class_rows) = class_rows else {
                    log::warn!(
                        "[Options] `game.\"{mod_id}\".bindings.{class_key}` is not a table; \
                         its bindings use the author's defaults"
                    );
                    continue;
                };
                for (command_id, row) in class_rows {
                    let Value::Array(entries) = row else {
                        log::warn!(
                            "[Options] binding row `{command_id}` for `{mod_id}` is not a list; \
                             using the author's default"
                        );
                        continue;
                    };
                    let inputs = entries
                        .iter()
                        .map(|entry| entry.as_str().map(str::to_string))
                        .collect();
                    rows.insert((class_key.clone(), command_id.clone()), inputs);
                }
            }
        }
        for ((write_mod, class_key, command_id), row) in &self.game_binding_writes {
            if write_mod != mod_id {
                continue;
            }
            let key = (class_key.clone(), command_id.clone());
            match row {
                Some(inputs) => {
                    rows.insert(key, inputs.iter().cloned().map(Some).collect());
                }
                None => {
                    rows.remove(&key);
                }
            }
        }
        rows.into_iter()
            .map(|((class_key, command_id), inputs)| StoredBindingRow {
                class_key,
                command_id,
                inputs,
            })
            .collect()
    }

    /// Record the player's binding row for one command on one device class;
    /// `None` resets it to the author default. The next save writes it, and
    /// every other row, known or not, keeps its text.
    pub(crate) fn set_game_binding_row(
        &mut self,
        mod_id: &str,
        class_key: &str,
        command_id: &str,
        inputs: Option<Vec<String>>,
    ) {
        self.game_binding_writes.insert(
            (
                mod_id.to_string(),
                class_key.to_string(),
                command_id.to_string(),
            ),
            inputs,
        );
    }

    /// Apply this session's row writes to the document a save writes.
    pub(super) fn write_game_bindings(&self, writer: &mut super::document::DocumentWriter<'_>) {
        for ((mod_id, class_key, command_id), row) in &self.game_binding_writes {
            writer.put_at(
                &[GAME, mod_id, BINDINGS, class_key],
                command_id,
                row.as_ref(),
            );
        }
    }
}
