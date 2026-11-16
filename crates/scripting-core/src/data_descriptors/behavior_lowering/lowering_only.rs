// The rejection of authored lowering-only fields, checked in field positions only.
// See: context/lib/scripting.md §2 (Attack and stance declarations)

use serde_json::{Map, Value};

use super::{DescriptorError, handle_kind, shape};

/// Fields only lowering writes into the runtime descriptor. An authored copy
/// rejects load wherever lowering would write one.
const LOWERING_ONLY_FIELDS: &[&str] = &[
    "pose",
    "queue",
    "afterMoment",
    "atDefault",
    "releaseAfter",
    "staggerTo",
];

/// Reject an authored lowering-only field in any field position: the root and
/// each nested envelope, activity bodies, their actions and selector entries,
/// rows, and the stance and attack declarations handles carry (with their
/// `release`, `recover` and `breakOn`), since lowering expands those into the
/// activities and rows it writes. Keys of the name-keyed `activities`,
/// `transitions` and `layers` maps are names, not fields: an activity or
/// stance may be named `pose`.
pub(super) fn reject_lowering_only_fields(
    behavior: &Value,
    path: &str,
) -> Result<(), DescriptorError> {
    let Some(root) = behavior.as_object() else {
        return Ok(());
    };
    if let Some(Value::Array(entries)) = root.get("attacks") {
        for (index, entry) in entries.iter().enumerate() {
            reject_in_handle(entry, &format!("{path}.attacks[{index}]"))?;
        }
    }
    reject_in_envelope(root, path)
}

fn reject_in_envelope(envelope: &Map<String, Value>, path: &str) -> Result<(), DescriptorError> {
    reject_in_fields(envelope, path)?;
    if let Some(initial) = envelope.get("initial") {
        reject_in_handle(initial, &format!("{path}.initial"))?;
    }
    if let Some(activities) = envelope.get("activities").and_then(Value::as_object) {
        for (name, activity) in activities {
            if let Some(activity) = activity.as_object() {
                reject_in_activity(activity, &format!("{path}.activities.{name}"))?;
            }
        }
    }
    let transitions = envelope.get("transitions").and_then(Value::as_object);
    for (source, rows) in transitions.into_iter().flatten() {
        for (index, row) in rows.as_array().into_iter().flatten().enumerate() {
            let Some(row) = row.as_object() else {
                continue;
            };
            let row_path = format!("{path}.transitions.{source}[{index}]");
            reject_in_fields(row, &row_path)?;
            if let Some(target) = row.get("to") {
                reject_in_handle(target, &format!("{row_path}.to"))?;
            }
        }
    }
    Ok(())
}

fn reject_in_activity(activity: &Map<String, Value>, path: &str) -> Result<(), DescriptorError> {
    reject_in_fields(activity, path)?;
    reject_in_action(activity.get("action"), &format!("{path}.action"))?;
    if let Some(mark) = activity.get("attackTurn") {
        reject_in_handle(mark, &format!("{path}.attackTurn"))?;
    }
    let layers = activity.get("layers").and_then(Value::as_object);
    for (layer_name, layer) in layers.into_iter().flatten() {
        let layer_path = format!("{path}.layers.{layer_name}");
        match layer {
            Value::Object(envelope) => reject_in_envelope(envelope, &layer_path)?,
            Value::Array(entries) => {
                for (index, entry) in entries.iter().enumerate() {
                    if let Some(entry) = entry.as_object() {
                        let entry_path = format!("{layer_path}[{index}]");
                        reject_in_fields(entry, &entry_path)?;
                        reject_in_action(entry.get("action"), &format!("{entry_path}.action"))?;
                    }
                }
            }
            _ => {}
        }
    }
    Ok(())
}

fn reject_in_action(action: Option<&Value>, path: &str) -> Result<(), DescriptorError> {
    let Some(Value::Object(action)) = action else {
        return Ok(());
    };
    reject_in_fields(action, path)?;
    match action.get("attack") {
        Some(attack) => reject_in_handle(attack, &format!("{path}.attack")),
        None => Ok(()),
    }
}

/// A stance or attack handle's declaration, the objects nested in it, and an
/// attack's stances.
fn reject_in_handle(value: &Value, path: &str) -> Result<(), DescriptorError> {
    let (Some(kind), Value::Object(handle)) = (handle_kind(value), value) else {
        return Ok(());
    };
    reject_in_fields(handle, path)?;
    let nested: &[&str] = if kind == "attack" {
        &["release", "recover"]
    } else {
        &["breakOn"]
    };
    for field in nested {
        if let Some(Value::Object(fields)) = handle.get(*field) {
            reject_in_fields(fields, &format!("{path}.{field}"))?;
        }
    }
    if kind == "attack" {
        for field in ["beginningStance", "endingStance"] {
            if let Some(stance) = handle.get(field) {
                reject_in_handle(stance, &format!("{path}.{field}"))?;
            }
        }
    }
    Ok(())
}

fn reject_in_fields(fields: &Map<String, Value>, path: &str) -> Result<(), DescriptorError> {
    match fields
        .keys()
        .find(|key| LOWERING_ONLY_FIELDS.contains(&key.as_str()))
    {
        Some(key) => Err(shape(format!(
            "`{path}.{key}` is written only by load-time lowering; remove it"
        ))),
        None => Ok(()),
    }
}
