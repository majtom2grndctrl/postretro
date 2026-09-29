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

    /// Write a top-level `f32` field. See `put_f32_value` for why this must
    /// not go through `put`'s generic `Value::try_from` path. `None` removes
    /// the key.
    pub(super) fn put_f32(&mut self, key: &str, value: Option<&f32>) {
        if self.document.is_unrecognized(key) {
            return;
        }
        put_f32_value(&mut self.table, key, value);
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

    /// Group-scoped counterpart to `put_f32`, for an `f32` field inside
    /// `[group]`.
    pub(super) fn put_f32_in(&mut self, group: &str, key: &str, value: Option<&f32>) {
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
            put_f32_value(group_table, key, value);
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

/// Store `value` at `key` as the exact TOML float for its f32 bits, or remove
/// the key when `value` is `None`.
///
/// This bypasses `put_value`'s `Value::try_from` (which goes through serde's
/// `serialize_f32`) because toml 1.x's `Value` has no narrower-than-`f64`
/// float variant: `serialize_f32` widens with `value as f64`, which is exact
/// bit-for-bit but not decimal-for-decimal — `0.002_f32 as f64` is
/// `0.0020000000949949026`. Every f32 setting would round-trip fine but save
/// with that widening noise, and any hand-typed value (`0.85`) would be
/// rewritten to noise on the very next save. `f32_to_toml_float` writes the
/// float the player actually sees instead.
fn put_f32_value(table: &mut Table, key: &str, value: Option<&f32>) {
    match value {
        Some(value) => {
            table.insert(key.to_string(), f32_to_toml_float(*value));
        }
        None => {
            table.remove(key);
        }
    }
}

/// The `Value::Float` for `value`'s shortest round-trip decimal text. `f32`'s
/// `Display` produces the shortest decimal string that reparses to the exact
/// same f32 bits (e.g. `0.002`, not `0.0020000000949949026`); reparsing that
/// text as `f64` keeps the stored value exact instead of exposing f32-to-f64
/// widening. `PlayerOptions::sanitize` and `AccessibilityOptions::sanitize`
/// keep every persisted f32 finite, so `value` is never NaN/inf here in
/// practice; `f32::to_string` still reparses cleanly for a non-finite value
/// (`f64::from_str` accepts `"NaN"`/`"inf"`/`"-inf"`), matching how toml's own
/// `serialize_f32` handles NaN, so this never panics.
fn f32_to_toml_float(value: f32) -> Value {
    Value::Float(
        value
            .to_string()
            .parse()
            .expect("f32's Display output always reparses as f64"),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn put_f32_writes_the_exact_f32_decimal_not_widened_f64_noise() {
        let document = StoredDocument::default();
        let mut writer = DocumentWriter::new(&document);
        writer.put_f32("mouse_sensitivity", Some(&0.002_f32));
        writer.put_f32("view_feel_scale", Some(&0.85_f32));
        let table = writer.finish();

        let text = toml::to_string(&table).unwrap();
        assert!(
            text.contains("mouse_sensitivity = 0.002\n"),
            "Value::try_from(0.002_f32) would write 0.0020000000949949026 \
             (serialize_f32 widens through `as f64`); got:\n{text}"
        );
        assert!(text.contains("view_feel_scale = 0.85\n"), "got:\n{text}");
    }

    #[test]
    fn put_f32_none_removes_the_key() {
        let mut initial = Table::new();
        initial.insert("mouse_sensitivity".to_string(), Value::Float(0.002));
        let document = StoredDocument::from_table(initial);
        let mut writer = DocumentWriter::new(&document);
        writer.put_f32("mouse_sensitivity", None);
        assert!(!writer.finish().contains_key("mouse_sensitivity"));
    }
}
