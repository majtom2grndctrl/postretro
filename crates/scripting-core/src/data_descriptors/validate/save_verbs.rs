// Save verbs and `checkpoint`: the argument check shared by reaction parsing
// in both runtimes and the system-reaction registry.
// See: context/lib/scripting.md §10.4 · context/lib/scripting.md §5

use postretro_entities::engine_state_collections::SAVE_SLOT_CAPACITY;
use postretro_entities::reactions::system_commands::SystemReactionKind;
use serde_json::Value;

/// How the authoring runtime counts `forEach` rows. A row index always crosses
/// 0-based (the Luau SDK subtracts one); the base only decides how a refusal
/// shows the value, so a Luau author reads back the number they wrote.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthoredRowBase {
    Zero,
    One,
}

/// The parsed arguments of a save verb or `checkpoint`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SaveVerbArgs {
    /// `continueRun`, `deleteRun`, `selectCharacter`, `createCharacter`,
    /// `deleteCharacter`: a 0-based slot row.
    Row(usize),
    NewRun {
        index: usize,
        map: String,
    },
    Checkpoint {
        name: String,
    },
}

/// Parse the wire `args` of a save verb or `checkpoint`. `None` when
/// `primitive` is neither. A refusal names the verb and the offending value.
pub fn parse_save_verb_args(
    primitive: &str,
    args: &Value,
    base: AuthoredRowBase,
) -> Option<Result<SaveVerbArgs, String>> {
    use SystemReactionKind::*;
    let kind = SystemReactionKind::from_primitive_name(primitive)?;
    let parsed = match kind {
        ContinueRun | DeleteRun | SelectCharacter | CreateCharacter | DeleteCharacter => {
            row_index(primitive, args, base).map(SaveVerbArgs::Row)
        }
        NewRun => row_index(primitive, args, base).and_then(|index| {
            non_empty_string(primitive, args, "map", "level (a catalog map id)")
                .map(|map| SaveVerbArgs::NewRun { index, map })
        }),
        Checkpoint => non_empty_string(primitive, args, "name", "name")
            .map(|name| SaveVerbArgs::Checkpoint { name }),
        _ => return None,
    };
    Some(parsed)
}

/// Reaction parsing's check: a save verb or `checkpoint` body or step whose
/// args do not parse is refused, naming the reaction, the site, the verb and
/// the value.
pub fn validate_save_verb_reaction_args(
    reaction: &str,
    site: &str,
    primitive: &str,
    args: &Value,
    base: AuthoredRowBase,
) -> Result<(), String> {
    match parse_save_verb_args(primitive, args, base) {
        Some(Err(detail)) => Err(format!("reaction `{reaction}` {site}: {detail}")),
        _ => Ok(()),
    }
}

fn row_index(primitive: &str, args: &Value, base: AuthoredRowBase) -> Result<usize, String> {
    let raw = args.get("index");
    let whole = raw
        .and_then(Value::as_f64)
        .filter(|index| index.fract() == 0.0 && *index >= 0.0)
        .filter(|index| *index < SAVE_SLOT_CAPACITY as f64);
    if let Some(index) = whole {
        return Ok(index as usize);
    }
    let (first, last, note) = match base {
        AuthoredRowBase::Zero => (0, SAVE_SLOT_CAPACITY - 1, ""),
        AuthoredRowBase::One => (1, SAVE_SLOT_CAPACITY, " (Luau rows count from 1)"),
    };
    let shown = match raw {
        None => "no index".to_string(),
        Some(value) => match (value.as_f64(), base) {
            (Some(number), AuthoredRowBase::One) => show_number(number + 1.0),
            (Some(number), AuthoredRowBase::Zero) => show_number(number),
            (None, _) => value.to_string(),
        },
    };
    Err(format!(
        "`{primitive}` row index must be a whole number from {first} to {last}{note}, got {shown}"
    ))
}

fn non_empty_string(
    primitive: &str,
    args: &Value,
    field: &str,
    what: &str,
) -> Result<String, String> {
    match args.get(field) {
        Some(Value::String(text)) if !text.is_empty() => Ok(text.clone()),
        Some(value) => Err(format!(
            "`{primitive}` {what} must be a non-empty string, got {value}"
        )),
        None => Err(format!(
            "`{primitive}` {what} must be a non-empty string, got nothing"
        )),
    }
}

/// A number as an author wrote it: whole values without a trailing `.0`.
fn show_number(number: f64) -> String {
    if number.fract() == 0.0 && number.abs() < 1e15 {
        format!("{}", number as i64)
    } else {
        format!("{number}")
    }
}
