// `settings.toml` as a document: per-field reads that degrade one field at a
// time, and a round-trip save that preserves what this build cannot read.
// See: context/lib/player_options.md §2

use std::collections::BTreeSet;
use std::path::Path;

use serde::Serialize;
use serde::de::DeserializeOwned;
use toml::{Table, Value};

/// The settings file as loaded. Saving rewrites this table rather than a fresh
/// serialization, so a key this build does not know, or a value it cannot
/// read, survives until the player writes that field.
#[derive(Debug, Clone, Default)]
pub(super) struct StoredDocument {
    table: Table,
    /// Dotted keys (`fog_quality`, `accessibility.reduce_motion`) whose stored
    /// value this build could not read. Their text is kept on save until the
    /// player writes the field.
    unrecognized: BTreeSet<String>,
    /// False when the file exists but could not be read or parsed. Such a file
    /// is never replaced: not by a settled save, not by the first-launch record.
    read_only: bool,
}

impl StoredDocument {
    pub(super) fn read_only() -> Self {
        Self {
            read_only: true,
            ..Self::default()
        }
    }

    pub(super) fn from_table(table: Table) -> Self {
        Self {
            table,
            ..Self::default()
        }
    }

    pub(super) fn is_read_only(&self) -> bool {
        self.read_only
    }

    pub(super) fn table(&self) -> &Table {
        &self.table
    }

    pub(super) fn note_unrecognized(&mut self, key: String) {
        self.unrecognized.insert(key);
    }

    /// The player wrote `key`: the next save replaces whatever text it held.
    pub(super) fn clear_unrecognized(&mut self, key: &str) {
        self.unrecognized.remove(key);
    }

    pub(super) fn is_unrecognized(&self, key: &str) -> bool {
        self.unrecognized.contains(key)
    }
}

/// Reads fields out of a loaded table one at a time. A present value that does
/// not deserialize warns once, is recorded as unrecognized, and reads as absent,
/// so the caller falls back for that field alone.
pub(super) struct FieldReader<'a> {
    document: &'a mut StoredDocument,
    path: &'a Path,
}

impl<'a> FieldReader<'a> {
    pub(super) fn new(document: &'a mut StoredDocument, path: &'a Path) -> Self {
        Self { document, path }
    }

    /// A top-level field.
    pub(super) fn read<T: DeserializeOwned>(&mut self, key: &str) -> Option<T> {
        let value = self.document.table.get(key)?.clone();
        self.convert(value, key.to_string())
    }

    /// A field inside the `[group]` table. A `group` key that is not a table
    /// reads every field as absent; the next save replaces it with a table.
    pub(super) fn read_in<T: DeserializeOwned>(&mut self, group: &str, key: &str) -> Option<T> {
        let value = match self.document.table.get(group)? {
            Value::Table(table) => table.get(key)?.clone(),
            _ => return None,
        };
        self.convert(value, format!("{group}.{key}"))
    }

    /// Warn once when `group` exists but is not a table.
    pub(super) fn check_group(&self, group: &str) {
        if let Some(value) = self.document.table.get(group)
            && !value.is_table()
        {
            log::warn!(
                "[Options] {}: `{group}` is not a table; its settings use their defaults",
                self.path.display()
            );
        }
    }

    fn convert<T: DeserializeOwned>(&mut self, value: Value, dotted: String) -> Option<T> {
        match value.try_into::<T>() {
            Ok(parsed) => Some(parsed),
            Err(err) => {
                log::warn!(
                    "[Options] {}: unrecognized value for `{dotted}` ({err}); using its default",
                    self.path.display()
                );
                self.document.note_unrecognized(dotted);
                None
            }
        }
    }
}

/// Builds the document a save writes: the loaded table with each known field
/// replaced by its current value. A field whose stored value was unrecognized
/// keeps that text until the player writes it.
pub(super) struct DocumentWriter<'a> {
    document: &'a StoredDocument,
    table: Table,
}

impl<'a> DocumentWriter<'a> {
    pub(super) fn new(document: &'a StoredDocument) -> Self {
        Self {
            document,
            table: document.table.clone(),
        }
    }

    /// Write a top-level field; `None` removes the key.
    pub(super) fn put<T: Serialize>(&mut self, key: &str, value: Option<&T>) {
        if self.document.is_unrecognized(key) {
            return;
        }
        put_value(&mut self.table, key, value);
    }

    /// Write a field inside `[group]`, creating the table (or replacing a
    /// non-table value) as needed; `None` removes the key.
    pub(super) fn put_in<T: Serialize>(&mut self, group: &str, key: &str, value: Option<&T>) {
        if self.document.is_unrecognized(&format!("{group}.{key}")) {
            return;
        }
        let entry = self
            .table
            .entry(group.to_string())
            .or_insert_with(|| Value::Table(Table::new()));
        if !entry.is_table() {
            *entry = Value::Table(Table::new());
        }
        if let Value::Table(group_table) = entry {
            put_value(group_table, key, value);
        }
    }

    pub(super) fn finish(self) -> Table {
        self.table
    }
}

fn put_value<T: Serialize>(table: &mut Table, key: &str, value: Option<&T>) {
    match value.map(Value::try_from) {
        Some(Ok(value)) => {
            table.insert(key.to_string(), value);
        }
        Some(Err(err)) => {
            // Every field type serializes to TOML; reaching here is a bug in
            // the field table, not a player-data condition.
            log::warn!("[Options] could not serialize `{key}`: {err}");
        }
        None => {
            table.remove(key);
        }
    }
}
