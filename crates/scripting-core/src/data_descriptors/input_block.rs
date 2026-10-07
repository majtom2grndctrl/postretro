// VM-agnostic resolution of the manifest's optional `input` block, shared by
// the QuickJS and Luau drains so both runtimes degrade and warn identically.
// The drains only classify raw VM values into `AuthoredValue`; every shape rule
// and warning lives here. Engine vocabulary is validated by the engine.
// See: context/lib/scripting.md §1 · context/lib/input.md §2

use super::{ModInputBinding, ModInputBlock, ModInputCommand, ModInputGlyphs};

/// Deepest container the drains classify, counting the block itself as depth
/// 0: `input` → `commands` → command → device list → binding. Anything nested
/// deeper is never a valid field value, so it classifies as
/// [`AuthoredValue::Other`]; the cap also bounds recursion on cyclic values.
pub(crate) const MAX_INPUT_CONTAINER_DEPTH: usize = 4;

/// One authored value as a VM drain classifies it.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum AuthoredValue {
    /// Missing, `null`, `undefined`, or `nil`.
    Absent,
    Bool(bool),
    Number(f64),
    String(String),
    Array(Vec<AuthoredValue>),
    /// Keyed entries in the order the drain presents them: authored order
    /// from QuickJS, sorted by key from Luau.
    Object(Vec<(String, AuthoredValue)>),
    /// A Luau `{}`, which is both an empty array and an empty object.
    EmptyTable,
    /// Any other value: a function, a mixed or sparse Luau table, a
    /// non-UTF-8 string, or a container past [`MAX_INPUT_CONTAINER_DEPTH`].
    Other,
}

impl AuthoredValue {
    /// Entries of an object-shaped value, or `None` when it is not one.
    fn into_entries(self) -> Option<Vec<(String, AuthoredValue)>> {
        match self {
            Self::Object(entries) => Some(entries),
            Self::EmptyTable => Some(Vec::new()),
            _ => None,
        }
    }

    /// Items of an array-shaped value, or `None` when it is not one.
    fn into_items(self) -> Option<Vec<AuthoredValue>> {
        match self {
            Self::Array(items) => Some(items),
            Self::EmptyTable => Some(Vec::new()),
            _ => None,
        }
    }
}

/// Remove and return `key`'s value from `entries`, or `Absent`.
fn take(entries: &mut Vec<(String, AuthoredValue)>, key: &str) -> AuthoredValue {
    match entries.iter().position(|(name, _)| name == key) {
        Some(index) => entries.swap_remove(index).1,
        None => AuthoredValue::Absent,
    }
}

/// Resolve the authored `input` value. `Absent` yields `None` (no block, the
/// engine table applies). Every malformed part warns once naming its path and
/// degrades; this never rejects the manifest.
pub(crate) fn resolve_authored_input_block(
    scope: &str,
    raw: AuthoredValue,
) -> Option<ModInputBlock> {
    if matches!(raw, AuthoredValue::Absent) {
        return None;
    }
    let Some(mut block) = raw.into_entries() else {
        log::warn!("[Scripting] {scope}: `input` must be an object; ignoring the input block");
        return None;
    };
    Some(ModInputBlock {
        commands: resolve_commands(scope, take(&mut block, "commands")),
        glyphs: resolve_glyphs(scope, take(&mut block, "glyphs")),
    })
}

fn resolve_commands(scope: &str, raw: AuthoredValue) -> Vec<ModInputCommand> {
    if matches!(raw, AuthoredValue::Absent) {
        return Vec::new();
    }
    let Some(entries) = raw.into_entries() else {
        log::warn!(
            "[Scripting] {scope}: `input.commands` must be an object keyed by command ID; no author commands apply"
        );
        return Vec::new();
    };
    let mut commands = Vec::with_capacity(entries.len());
    for (id, value) in entries {
        if matches!(value, AuthoredValue::Absent) {
            continue;
        }
        let Some(mut fields) = value.into_entries() else {
            log::warn!(
                "[Scripting] {scope}: `input.commands.{id}` must be an object; skipping the command"
            );
            continue;
        };
        let path = format!("input.commands.{id}");
        commands.push(ModInputCommand {
            label: string_field(scope, &path, "label", take(&mut fields, "label")),
            category: string_field(scope, &path, "category", take(&mut fields, "category")),
            order: match take(&mut fields, "order") {
                AuthoredValue::Absent => None,
                AuthoredValue::Number(order) => Some(order),
                _ => {
                    log::warn!("[Scripting] {scope}: `{path}.order` must be a number; ignoring it");
                    None
                }
            },
            show: match take(&mut fields, "show") {
                AuthoredValue::Absent => None,
                AuthoredValue::Bool(show) => Some(show),
                _ => {
                    log::warn!("[Scripting] {scope}: `{path}.show` must be a boolean; ignoring it");
                    None
                }
            },
            keyboard_mouse: device_bindings(
                scope,
                &path,
                "keyboardMouse",
                take(&mut fields, "keyboardMouse"),
            ),
            gamepad: device_bindings(scope, &path, "gamepad", take(&mut fields, "gamepad")),
            id,
        });
    }
    commands
}

fn string_field(scope: &str, path: &str, key: &str, raw: AuthoredValue) -> Option<String> {
    match raw {
        AuthoredValue::Absent => None,
        AuthoredValue::String(value) => Some(value),
        _ => {
            log::warn!("[Scripting] {scope}: `{path}.{key}` must be a string; ignoring it");
            None
        }
    }
}

/// A device class's binding list. Absent keeps the engine default (`None`);
/// a non-array warns and is treated as absent.
fn device_bindings(
    scope: &str,
    path: &str,
    key: &str,
    raw: AuthoredValue,
) -> Option<Vec<ModInputBinding>> {
    if matches!(raw, AuthoredValue::Absent) {
        return None;
    }
    let Some(items) = raw.into_items() else {
        log::warn!(
            "[Scripting] {scope}: `{path}.{key}` must be an array of bindings; the engine default applies"
        );
        return None;
    };
    Some(items.into_iter().map(binding).collect())
}

/// One binding entry. Malformed parts are kept in a form the engine diagnoses
/// (empty input, empty activator, NaN threshold) rather than warned here, so
/// each malformed binding yields exactly one diagnostic, from the engine. An
/// empty input stands for a binding that is not an object or has no `input`
/// string, and the engine names it so.
fn binding(raw: AuthoredValue) -> ModInputBinding {
    let Some(mut fields) = raw.into_entries() else {
        return ModInputBinding::default();
    };
    ModInputBinding {
        input: match take(&mut fields, "input") {
            AuthoredValue::String(input) => input,
            _ => String::new(),
        },
        activator: match take(&mut fields, "activator") {
            AuthoredValue::Absent => None,
            AuthoredValue::String(activator) => Some(activator),
            _ => Some(String::new()),
        },
        threshold: match take(&mut fields, "threshold") {
            AuthoredValue::Absent => None,
            AuthoredValue::Number(threshold) => Some(threshold),
            _ => Some(f64::NAN),
        },
    }
}

fn resolve_glyphs(scope: &str, raw: AuthoredValue) -> ModInputGlyphs {
    if matches!(raw, AuthoredValue::Absent) {
        return ModInputGlyphs::default();
    }
    let Some(mut families) = raw.into_entries() else {
        log::warn!("[Scripting] {scope}: `input.glyphs` must be an object; no glyph art applies");
        return ModInputGlyphs::default();
    };
    let mut family = |key: &str| string_field(scope, "input.glyphs", key, take(&mut families, key));
    ModInputGlyphs {
        keyboard_mouse: family("keyboardMouse"),
        xbox: family("xbox"),
        playstation: family("playstation"),
        nintendo: family("nintendo"),
    }
}
