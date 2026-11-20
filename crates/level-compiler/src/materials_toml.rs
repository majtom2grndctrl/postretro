// The mod's `materials.toml`: content-owned per-texture material values, read
// only by the bake. A broken file warns and never stops the compile.
// See: context/lib/resource_management.md §4.5 · build_pipeline.md §Compiler pipeline step 9

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use postretro_level_format::emissive::EMISSIVE_VALUE_CEILING;
use toml::Spanned;
use toml::de::{DeTable, DeValue};

use crate::texture_mips::normalize_map_texture_name;

/// The file's name under the mod root (`resolve_content_root`).
pub(crate) const MATERIALS_TOML: &str = "materials.toml";

/// The `emissive` key: a texture's glow strength.
const EMISSIVE_KEY: &str = "emissive";

/// One texture table, keyed in [`MaterialsToml`] by its normalized name.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct MaterialEntry {
    /// The table's key as written.
    pub key: String,
    /// 1-based line of the key.
    pub line: usize,
    /// The fixed `emissive` value; `None` when the table sets none or its
    /// value reads as the texture's default.
    pub emissive: Option<f32>,
}

/// A parsed `materials.toml`. Absent or unreadable, it holds no entry and
/// every texture takes its default.
#[derive(Debug, Clone)]
pub(crate) struct MaterialsToml {
    path: PathBuf,
    entries: HashMap<String, MaterialEntry>,
}

impl MaterialsToml {
    /// `<content_root>/materials.toml`. No file is silent; an unreadable one
    /// warns, naming it.
    pub(crate) fn load(content_root: &Path) -> Self {
        let path = content_root.join(MATERIALS_TOML);
        match std::fs::read_to_string(&path) {
            Ok(text) => Self::parse(path, &text),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Self::empty(path),
            Err(error) => {
                log::warn!(
                    "[prl-build] {}: cannot read: {error}; every texture takes its default \
                     emissive strength",
                    path.display()
                );
                Self::empty(path)
            }
        }
    }

    pub(crate) fn empty(path: PathBuf) -> Self {
        Self {
            path,
            entries: HashMap::new(),
        }
    }

    /// Parse `text` as the file at `path`. Each warning names the file and
    /// the line it concerns.
    pub(crate) fn parse(path: PathBuf, text: &str) -> Self {
        let mut parsed = Self::empty(path);
        let root = match DeTable::parse(text) {
            Ok(root) => root,
            Err(error) => {
                let line = error.span().map_or_else(
                    || "?".to_owned(),
                    |span| line_of(text, span.start).to_string(),
                );
                log::warn!(
                    "[prl-build] {}:{line}: syntax error: {}; every texture takes its default \
                     emissive strength",
                    parsed.path.display(),
                    error.message().trim_end()
                );
                return parsed;
            }
        };
        for (key, value) in in_file_order(root.get_ref()) {
            let key_text: &str = key.get_ref();
            let line = line_of(text, key.span().start);
            let DeValue::Table(table) = value.get_ref() else {
                log::warn!(
                    "[prl-build] {}:{line}: top-level key '{key_text}' is not a texture table; \
                     ignored",
                    parsed.path.display()
                );
                continue;
            };
            let entry = MaterialEntry {
                key: key_text.to_owned(),
                line,
                emissive: parsed.read_table(text, key_text, table),
            };
            let normalized = normalize_map_texture_name(key_text);
            if let Some(earlier) = parsed.entries.get(&normalized) {
                log::warn!(
                    "[prl-build] {}: [\"{}\"] (line {}) and [\"{key_text}\"] (line {line}) name \
                     the same texture '{normalized}'; the later, line {line}, wins",
                    parsed.path.display(),
                    earlier.key,
                    earlier.line,
                );
            }
            parsed.entries.insert(normalized, entry);
        }
        parsed
    }

    /// One texture table's `emissive`, every other key warned and ignored.
    fn read_table(&self, text: &str, texture: &str, table: &DeTable<'_>) -> Option<f32> {
        let mut emissive = None;
        for (field, value) in in_file_order(table) {
            let field_name: &str = field.get_ref();
            let line = line_of(text, field.span().start);
            if field_name == EMISSIVE_KEY {
                emissive = self.read_emissive(line, texture, value.get_ref());
            } else {
                log::warn!(
                    "[prl-build] {}:{line}: unknown key '{field_name}' in [\"{texture}\"]; ignored",
                    self.path.display()
                );
            }
        }
        emissive
    }

    /// The `emissive` value, fixed as the volume keys are: negative reads 0,
    /// non-finite reads the default, past the ceiling reads the ceiling.
    fn read_emissive(&self, line: usize, texture: &str, value: &DeValue<'_>) -> Option<f32> {
        let at = || format!("{}:{line}", self.path.display());
        let number = match value {
            DeValue::Integer(integer) => i64::from_str_radix(integer.as_str(), integer.radix())
                .map_or(f64::NAN, |v| v as f64),
            DeValue::Float(float) => float.as_str().parse::<f64>().unwrap_or(f64::NAN),
            other => {
                log::warn!(
                    "[prl-build] {}: [\"{texture}\"] emissive must be a number, found {}; the \
                     texture takes its default",
                    at(),
                    value_kind(other)
                );
                return None;
            }
        };
        if !number.is_finite() {
            log::warn!(
                "[prl-build] {}: [\"{texture}\"] emissive {number} is not finite; the texture \
                 takes its default",
                at()
            );
            return None;
        }
        if number < 0.0 {
            log::warn!(
                "[prl-build] {}: [\"{texture}\"] emissive {number} is negative; reading 0",
                at()
            );
            return Some(0.0);
        }
        if number > f64::from(EMISSIVE_VALUE_CEILING) {
            log::warn!(
                "[prl-build] {}: [\"{texture}\"] emissive {number} is above the ceiling \
                 {EMISSIVE_VALUE_CEILING}; reading {EMISSIVE_VALUE_CEILING}",
                at()
            );
            return Some(EMISSIVE_VALUE_CEILING);
        }
        // `-0` is not negative; adding 0 stores it as 0.
        Some(number as f32 + 0.0)
    }

    /// The entry for a texture whose name normalizes to `normalized`.
    pub(crate) fn entry(&self, normalized: &str) -> Option<&MaterialEntry> {
        self.entries.get(normalized)
    }

    pub(crate) fn path(&self) -> &Path {
        &self.path
    }
}

type Entry<'a, 'i> = (
    &'a Spanned<std::borrow::Cow<'i, str>>,
    &'a Spanned<DeValue<'i>>,
);

/// A table's entries in the order the file writes them: the parser stores
/// them by key, and "the later wins" means later in the file.
fn in_file_order<'a, 'i>(table: &'a DeTable<'i>) -> Vec<Entry<'a, 'i>> {
    let mut entries: Vec<Entry<'a, 'i>> = table.iter().collect();
    entries.sort_by_key(|(key, _)| key.span().start);
    entries
}

/// The 1-based line holding byte `offset`.
fn line_of(text: &str, offset: usize) -> usize {
    let offset = offset.min(text.len());
    text.as_bytes()[..offset]
        .iter()
        .filter(|&&byte| byte == b'\n')
        .count()
        + 1
}

fn value_kind(value: &DeValue<'_>) -> &'static str {
    match value {
        DeValue::String(_) => "a string",
        DeValue::Integer(_) | DeValue::Float(_) => "a number",
        DeValue::Boolean(_) => "a boolean",
        DeValue::Datetime(_) => "a date-time",
        DeValue::Array(_) => "an array",
        DeValue::Table(_) => "a table",
    }
}

#[cfg(test)]
#[path = "materials_toml_tests.rs"]
mod tests;
