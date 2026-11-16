// Load-time lowering of an authored behavior's attack handles into the runtime
// behavior descriptor: one route for both runtimes, run on the behavior's JSON
// before serde, once per entity read (mod load and staged reload).
// See: context/lib/scripting.md §2 (Attack and stance declarations)
//      context/lib/entity_model.md §7c

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{Map, Value};

use super::DescriptorError;

#[cfg(test)]
mod tests;

/// Path every lowering error and log names a behavior field under.
const ROOT: &str = "components.behavior";

/// Fields only lowering writes into the runtime descriptor. An authored copy
/// rejects load wherever it appears.
const LOWERING_ONLY_FIELDS: &[&str] = &[
    "pose",
    "queue",
    "afterMoment",
    "atDefault",
    "releaseAfter",
    "staggerTo",
];

/// An attack declaration's authoring fields: lowering expands them into
/// generated activities (stance lowering) and never writes them into the
/// runtime `attacks` map.
const AUTHORING_ONLY_ATTACK_FIELDS: &[&str] =
    &["kind", "name", "beginningStance", "endingStance", "release", "recover"];

#[cfg(test)]
thread_local! {
    static LOWERING_RUNS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// How many behaviors this thread has lowered; tests prove lowering runs once
/// per entity read, never per level or tick.
#[cfg(test)]
pub(crate) fn lowering_runs() -> usize {
    LOWERING_RUNS.with(std::cell::Cell::get)
}

/// Lower an authored `components.behavior` value in place: resolve attack
/// handles to the names the engine reads, build the name-keyed `attacks` map
/// from their declarations, and drop rows an explicit `attacks` list excludes.
/// `archetype` names the entity in drop logs.
pub(crate) fn lower_behavior(
    behavior: &mut Value,
    archetype: Option<&str>,
) -> Result<(), DescriptorError> {
    #[cfg(test)]
    LOWERING_RUNS.with(|runs| runs.set(runs.get() + 1));

    reject_lowering_only_fields(behavior, ROOT)?;
    let Some(root) = behavior.as_object_mut() else {
        // Not an object: serde names the shape error.
        return Ok(());
    };

    let mut lowering = Lowering::default();
    let listed = take_attack_list(root, &mut lowering)?;
    lowering.collect_envelope(root, ROOT)?;

    if let Some(listed) = &listed {
        for reference in &lowering.non_row_references {
            if !listed.contains(&reference.name) {
                return Err(shape(format!(
                    "`{}` names attack \"{}\", which the behavior's `attacks` list leaves out; list it or remove the reference",
                    reference.path, reference.name
                )));
            }
        }
        prune_to_fixed_point(root, ROOT, listed, archetype);
    }
    lower_row_targets(root, ROOT)?;

    let names = listed.unwrap_or(lowering.referenced);
    let mut attacks = Map::new();
    for name in names {
        let declared = &lowering.declarations[&name];
        attacks.insert(name, runtime_attack_params(&declared.declaration));
    }
    if !attacks.is_empty() {
        root.insert("attacks".to_string(), Value::Object(attacks));
    }
    Ok(())
}

#[derive(Default)]
struct Lowering {
    /// Every attack handle seen, by name, with the path it was first seen at.
    declarations: BTreeMap<String, Declared>,
    /// Attacks a row, action or mark references: the inferred `attacks` set.
    referenced: BTreeSet<String>,
    /// Action and mark references, which an explicit list must cover.
    non_row_references: Vec<Reference>,
}

struct Declared {
    declaration: Map<String, Value>,
    path: String,
}

struct Reference {
    name: String,
    path: String,
}

impl Lowering {
    /// Record one attack handle's declaration. Two handles of one name in one
    /// archetype must declare the same attack: a Luau re-`require` makes
    /// equal copies, while unequal ones would hide which one a name reaches.
    fn declare(&mut self, handle: &Map<String, Value>, path: &str) -> Result<String, DescriptorError> {
        let name = handle_name(handle);
        for field in ["beginningStance", "endingStance"] {
            if let Some(stance) = handle.get(field)
                && !stance.is_null()
            {
                stance_handle(stance, &format!("{path}.{field}"))?;
            }
        }
        match self.declarations.get(&name) {
            Some(first) if first.declaration != *handle => Err(shape(format!(
                "`{path}` declares attack \"{name}\" differently from `{}`; two attack handles of one name must declare the same attack",
                first.path
            ))),
            Some(_) => Ok(name),
            None => {
                self.declarations.insert(
                    name.clone(),
                    Declared {
                        declaration: handle.clone(),
                        path: path.to_string(),
                    },
                );
                Ok(name)
            }
        }
    }

    /// Resolve a non-row reference (an action's `attack`, an `attackTurn`
    /// mark) to its name in place.
    fn resolve_reference(&mut self, slot: &mut Value, path: &str) -> Result<(), DescriptorError> {
        let handle = attack_handle(slot, path)?;
        let name = self.declare(handle, path)?;
        self.referenced.insert(name.clone());
        self.non_row_references.push(Reference {
            name: name.clone(),
            path: path.to_string(),
        });
        *slot = Value::String(name);
        Ok(())
    }

    /// Walk one envelope (the root or a nested graph layer): resolve its
    /// activities' references and record its rows' attack targets.
    fn collect_envelope(
        &mut self,
        envelope: &mut Map<String, Value>,
        path: &str,
    ) -> Result<(), DescriptorError> {
        if let Some(activities) = envelope.get_mut("activities").and_then(Value::as_object_mut) {
            for (name, activity) in activities.iter_mut() {
                if let Some(activity) = activity.as_object_mut() {
                    self.collect_activity(activity, &format!("{path}.activities.{name}"))?;
                }
            }
        }
        for (row_path, row) in rows_mut(envelope, path) {
            if let Some(target) = row.get("to")
                && is_handle_of(target, "attack")
            {
                let handle = attack_handle(target, &format!("{row_path}.to"))?;
                let name = self.declare(handle, &format!("{row_path}.to"))?;
                self.referenced.insert(name);
            }
        }
        Ok(())
    }

    fn collect_activity(
        &mut self,
        activity: &mut Map<String, Value>,
        path: &str,
    ) -> Result<(), DescriptorError> {
        if let Some(action) = activity.get_mut("action").and_then(Value::as_object_mut) {
            self.resolve_action(action, &format!("{path}.action"))?;
        }
        if let Some(mark) = activity.get_mut("attackTurn")
            && !mark.is_null()
        {
            self.resolve_reference(mark, &format!("{path}.attackTurn"))?;
        }
        if let Some(layers) = activity.get_mut("layers").and_then(Value::as_object_mut) {
            for (layer_name, layer) in layers.iter_mut() {
                let layer_path = format!("{path}.layers.{layer_name}");
                match layer {
                    Value::Array(entries) => {
                        for (index, entry) in entries.iter_mut().enumerate() {
                            if let Some(action) =
                                entry.get_mut("action").and_then(Value::as_object_mut)
                            {
                                self.resolve_action(action, &format!("{layer_path}[{index}].action"))?;
                            }
                        }
                    }
                    Value::Object(envelope) => self.collect_envelope(envelope, &layer_path)?,
                    _ => {}
                }
            }
        }
        Ok(())
    }

    fn resolve_action(
        &mut self,
        action: &mut Map<String, Value>,
        path: &str,
    ) -> Result<(), DescriptorError> {
        let attack_path = format!("{path}.attack");
        match action.get_mut("attack") {
            Some(slot) => self.resolve_reference(slot, &attack_path),
            None => Err(missing_handle(&attack_path)),
        }
    }
}

/// Take the authored `attacks` field off the root: absent, the behavior's
/// attacks are inferred from its references; a list of handles is exhaustive.
/// The retired name-keyed map rejects load.
fn take_attack_list(
    root: &mut Map<String, Value>,
    lowering: &mut Lowering,
) -> Result<Option<BTreeSet<String>>, DescriptorError> {
    let path = format!("{ROOT}.attacks");
    let entries = match root.remove("attacks") {
        None | Some(Value::Null) => return Ok(None),
        Some(Value::Array(entries)) => entries,
        // An empty Luau table cannot say whether it is a list or a map.
        Some(Value::Object(map)) if map.is_empty() => Vec::new(),
        Some(Value::Object(map)) => {
            let names = map.keys().cloned().collect::<Vec<_>>().join(", ");
            return Err(shape(format!(
                "`{path}` is a name-keyed map ({names}); declare each attack with `defineAttack` and reference its handle. `attacks` is optional: omitted, a behavior has the attacks its rows, actions and marks reference; present, it is a list of attack handles, `attacks: [slam, jab]`"
            )));
        }
        Some(other) => {
            return Err(shape(format!(
                "`{path}` must be a list of attack handles from `defineAttack`, found {}",
                kind_of(&other)
            )));
        }
    };
    let mut listed = BTreeSet::new();
    for (index, entry) in entries.iter().enumerate() {
        let entry_path = format!("{path}[{index}]");
        let handle = attack_handle(entry, &entry_path)?;
        listed.insert(lowering.declare(handle, &entry_path)?);
    }
    Ok(Some(listed))
}

/// Drop what an explicit `attacks` list excludes, repeating until nothing
/// more drops. Each dropped row logs once and its guard is never checked, so a
/// variant reuses its parent's graph with a shorter list. Stance lowering adds
/// its rule here: the authored rows of a rest no longer reached.
fn prune_to_fixed_point(
    root: &mut Map<String, Value>,
    path: &str,
    listed: &BTreeSet<String>,
    archetype: Option<&str>,
) {
    while drop_rows_to_unlisted_attacks(root, path, listed, archetype) > 0 {}
}

/// Drop every row, in this envelope and those nested in it, whose `to` is an
/// attack the list leaves out. Returns how many rows dropped.
fn drop_rows_to_unlisted_attacks(
    envelope: &mut Map<String, Value>,
    path: &str,
    listed: &BTreeSet<String>,
    archetype: Option<&str>,
) -> usize {
    let mut dropped = 0;
    if let Some(transitions) = envelope.get_mut("transitions").and_then(Value::as_object_mut) {
        for (source, rows) in transitions.iter_mut() {
            let Some(rows) = rows.as_array_mut() else {
                continue;
            };
            let mut index = 0;
            rows.retain(|row| {
                let row_path = format!("{path}.transitions.{source}[{index}]");
                index += 1;
                let Some(name) = row
                    .get("to")
                    .filter(|target| is_handle_of(target, "attack"))
                    .and_then(|target| target.get("name"))
                    .and_then(Value::as_str)
                else {
                    return true;
                };
                if listed.contains(name) {
                    return true;
                }
                log::info!(
                    "[Scripting] {}: `{row_path}` drops its row to attack \"{name}\", which the behavior's `attacks` list leaves out",
                    archetype.unwrap_or("<unnamed archetype>"),
                );
                dropped += 1;
                false
            });
        }
    }
    for_each_nested_envelope(envelope, path, |nested, nested_path| {
        dropped += drop_rows_to_unlisted_attacks(nested, nested_path, listed, archetype);
    });
    dropped
}

/// Lower every remaining row's handle `to` and handle `initial` to the
/// activity name stance lowering generates for it.
fn lower_row_targets(envelope: &mut Map<String, Value>, path: &str) -> Result<(), DescriptorError> {
    if let Some(initial) = envelope.get("initial")
        && let Some(kind) = handle_kind(initial)
    {
        return Err(stance_lowering_pending(&format!("{path}.initial"), kind, initial));
    }
    for (row_path, row) in rows_mut(envelope, path) {
        if let Some(target) = row.get("to")
            && let Some(kind) = handle_kind(target)
        {
            return Err(stance_lowering_pending(&format!("{row_path}.to"), kind, target));
        }
    }
    let mut result = Ok(());
    for_each_nested_envelope(envelope, path, |nested, nested_path| {
        if result.is_ok() {
            result = lower_row_targets(nested, nested_path);
        }
    });
    result
}

/// Seam for stance lowering (E10 stances, Task 7): it expands a row to a
/// stance or an attack, and an `initial` stance, into generated activities.
/// Until then such a target rejects load.
fn stance_lowering_pending(path: &str, kind: &str, target: &Value) -> DescriptorError {
    let name = target.get("name").and_then(Value::as_str).unwrap_or("");
    shape(format!(
        "`{path}` targets {kind} \"{name}\"; rows and `initial` to stances and attacks are not lowered yet, so target an activity name"
    ))
}

/// The runtime `AttackParams` an attack declaration lowers to.
fn runtime_attack_params(declaration: &Map<String, Value>) -> Value {
    let mut params = declaration.clone();
    for field in AUTHORING_ONLY_ATTACK_FIELDS {
        params.remove(*field);
    }
    Value::Object(params)
}

/// Every transition row of one envelope with its path.
fn rows_mut<'a>(
    envelope: &'a mut Map<String, Value>,
    path: &str,
) -> Vec<(String, &'a mut Map<String, Value>)> {
    let mut rows_out = Vec::new();
    if let Some(transitions) = envelope.get_mut("transitions").and_then(Value::as_object_mut) {
        for (source, rows) in transitions.iter_mut() {
            if let Some(rows) = rows.as_array_mut() {
                for (index, row) in rows.iter_mut().enumerate() {
                    if let Some(row) = row.as_object_mut() {
                        rows_out.push((format!("{path}.transitions.{source}[{index}]"), row));
                    }
                }
            }
        }
    }
    rows_out
}

/// Visit each graph layer nested directly under this envelope's activities.
fn for_each_nested_envelope(
    envelope: &mut Map<String, Value>,
    path: &str,
    mut visit: impl FnMut(&mut Map<String, Value>, &str),
) {
    let Some(activities) = envelope.get_mut("activities").and_then(Value::as_object_mut) else {
        return;
    };
    for (name, activity) in activities.iter_mut() {
        let Some(layers) = activity.get_mut("layers").and_then(Value::as_object_mut) else {
            continue;
        };
        for (layer_name, layer) in layers.iter_mut() {
            if let Some(nested) = layer.as_object_mut() {
                visit(nested, &format!("{path}.activities.{name}.layers.{layer_name}"));
            }
        }
    }
}

/// Reject an authored lowering-only field anywhere in the behavior.
fn reject_lowering_only_fields(value: &Value, path: &str) -> Result<(), DescriptorError> {
    match value {
        Value::Object(object) => {
            for (key, child) in object {
                let child_path = format!("{path}.{key}");
                if LOWERING_ONLY_FIELDS.contains(&key.as_str()) {
                    return Err(shape(format!(
                        "`{child_path}` is written only by load-time lowering; remove it"
                    )));
                }
                reject_lowering_only_fields(child, &child_path)?;
            }
            Ok(())
        }
        Value::Array(items) => {
            for (index, child) in items.iter().enumerate() {
                reject_lowering_only_fields(child, &format!("{path}[{index}]"))?;
            }
            Ok(())
        }
        _ => Ok(()),
    }
}

/// `Some(kind)` when the value is an attack or stance handle.
fn handle_kind(value: &Value) -> Option<&str> {
    match value.get("kind").and_then(Value::as_str) {
        Some(kind @ ("attack" | "stance")) if value.is_object() => Some(kind),
        _ => None,
    }
}

fn is_handle_of(value: &Value, kind: &str) -> bool {
    handle_kind(value) == Some(kind)
}

/// The attack handle at `path`, or a pathed error naming what stands there.
fn attack_handle<'a>(value: &'a Value, path: &str) -> Result<&'a Map<String, Value>, DescriptorError> {
    expect_handle(value, path, "attack", "defineAttack")
}

fn stance_handle<'a>(value: &'a Value, path: &str) -> Result<&'a Map<String, Value>, DescriptorError> {
    expect_handle(value, path, "stance", "defineStance")
}

fn expect_handle<'a>(
    value: &'a Value,
    path: &str,
    kind: &str,
    definer: &str,
) -> Result<&'a Map<String, Value>, DescriptorError> {
    let found = match value {
        Value::Object(object)
            if object.get("kind").and_then(Value::as_str) == Some(kind)
                && object
                    .get("name")
                    .and_then(Value::as_str)
                    .is_some_and(|name| !name.is_empty()) =>
        {
            return Ok(object);
        }
        Value::String(text) => format!("`\"{text}\"` is a bare string"),
        Value::Null => "is nil or undefined (in Luau, a misspelled handle reads nil)".to_string(),
        Value::Object(object) => match object.get("kind").and_then(Value::as_str) {
            Some(other) => format!("is a `{other}` handle"),
            None => "is a plain object, not a handle".to_string(),
        },
        other => format!("is {}", kind_of(other)),
    };
    Err(shape(format!(
        "`{path}`: {found}; pass the {kind} handle `{definer}` returned"
    )))
}

fn missing_handle(path: &str) -> DescriptorError {
    shape(format!(
        "`{path}` is missing; pass the attack handle `defineAttack` returned (in Luau, a misspelled handle reads nil)"
    ))
}

fn handle_name(handle: &Map<String, Value>) -> String {
    handle
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

fn kind_of(value: &Value) -> &'static str {
    match value {
        Value::Null => "nil",
        Value::Bool(_) => "a boolean",
        Value::Number(_) => "a number",
        Value::String(_) => "a string",
        Value::Array(_) => "a list",
        Value::Object(_) => "an object",
    }
}

fn shape(reason: String) -> DescriptorError {
    DescriptorError::InvalidShape { reason }
}
