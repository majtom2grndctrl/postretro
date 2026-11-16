// Load-time lowering of a behavior's attack and stance handles into its runtime descriptor.
// See: context/lib/scripting.md §2 (Attack and stance declarations)

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{Map, Value};

use super::DescriptorError;

mod declarations;
mod expand;
mod floor;
mod lowering_only;
mod prune;

use declarations::Catalog;
pub(crate) use floor::stance_floor_warning_scope;
use lowering_only::reject_lowering_only_fields;
use prune::prune_to_fixed_point;

#[cfg(test)]
mod tests;

#[cfg(test)]
mod stance_tests;

/// Path every lowering error and log names a behavior field under.
const ROOT: &str = "components.behavior";

/// An attack declaration's authoring fields: lowering expands them into
/// generated activities (stance lowering) and never writes them into the
/// runtime `attacks` map.
const AUTHORING_ONLY_ATTACK_FIELDS: &[&str] = &[
    "kind",
    "name",
    "beginningStance",
    "endingStance",
    "release",
    "recover",
];

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
/// handles to the names the engine reads, drop rows an explicit `attacks`
/// list excludes, expand each level's stances and strikes into activities,
/// and build the name-keyed `attacks` map from their declarations.
/// `archetype` names the entity in drop logs.
pub(crate) fn lower_behavior(
    behavior: &mut Value,
    archetype: Option<&str>,
) -> Result<(), DescriptorError> {
    #[cfg(test)]
    LOWERING_RUNS.with(|runs| runs.set(runs.get() + 1));
    // Inside a mod load's scope this joins it; alone, it dedupes per behavior.
    let _floor_warnings = stance_floor_warning_scope();

    reject_lowering_only_fields(behavior, ROOT)?;
    let Some(root) = behavior.as_object_mut() else {
        // Not an object: serde names the shape error.
        return Ok(());
    };

    let mut lowering = Lowering::default();
    let listed = take_attack_list(root, &mut lowering)?;
    lowering.collect_envelope(root, ROOT)?;
    let catalog = Catalog::parse(&lowering.stances, &lowering.declarations)?;

    if let Some(listed) = &listed {
        for reference in &lowering.non_row_references {
            if !listed.contains(&reference.name) {
                return Err(shape(format!(
                    "`{}` names attack \"{}\", which the behavior's `attacks` list leaves out; list it or remove the reference",
                    reference.path, reference.name
                )));
            }
        }
        prune_to_fixed_point(root, ROOT, listed, &catalog, archetype);
    }
    let struck = expand_envelope(root, ROOT, &catalog, false)?;

    // Without a list, struck attacks count as referenced.
    let names = listed.unwrap_or_else(|| lowering.referenced.union(&struck).cloned().collect());
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
    /// Every stance handle seen, by name, with the path it was first seen at:
    /// row targets, `initial`, and attacks' beginning and ending stances.
    stances: BTreeMap<String, Declared>,
    /// Attacks an action or mark references; with the attacks rows strike,
    /// the inferred `attacks` set.
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
    /// Record one attack handle's declaration, and the stances it begins and
    /// ends in, by [`declare_into`]'s equal-copies rule.
    fn declare(
        &mut self,
        handle: &Map<String, Value>,
        path: &str,
    ) -> Result<String, DescriptorError> {
        let name = handle_name(handle);
        for field in ["beginningStance", "endingStance"] {
            if let Some(stance) = handle.get(field)
                && !stance.is_null()
            {
                let field_path = format!("{path}.{field}");
                let stance = stance_handle(stance, &field_path)?;
                self.declare_stance(stance, &field_path)?;
            }
        }
        declare_into(&mut self.declarations, "attack", handle, path)?;
        Ok(name)
    }

    /// Record one stance handle's declaration, by [`declare_into`]'s rule.
    fn declare_stance(
        &mut self,
        handle: &Map<String, Value>,
        path: &str,
    ) -> Result<(), DescriptorError> {
        declare_into(&mut self.stances, "stance", handle, path)
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
        if let Some(activities) = envelope
            .get_mut("activities")
            .and_then(Value::as_object_mut)
        {
            for (name, activity) in activities.iter_mut() {
                if let Some(activity) = activity.as_object_mut() {
                    self.collect_activity(activity, &format!("{path}.activities.{name}"))?;
                }
            }
        }
        // Anything but a name or the handles lowering resolves would otherwise
        // reach serde, whose type error names no path.
        if let Some(initial) = envelope.get("initial")
            && !initial.is_string()
        {
            let initial_path = format!("{path}.initial");
            if !is_handle_of(initial, "stance") {
                return Err(shape(format!(
                    "`{initial_path}`: {}; `initial` takes an activity name or a stance handle from `defineStance`",
                    found(initial)
                )));
            }
            self.declare_stance(stance_handle(initial, &initial_path)?, &initial_path)?;
        }
        for (row_path, row) in rows_mut(envelope, path) {
            let target_path = format!("{row_path}.to");
            let Some(target) = row.get("to") else {
                return Err(shape(format!(
                    "`{target_path}` is missing; a row targets an activity name, a stance handle or an attack handle (in Luau, a misspelled handle reads nil)"
                )));
            };
            match handle_kind(target) {
                Some("attack") => {
                    self.declare(attack_handle(target, &target_path)?, &target_path)?;
                }
                Some("stance") => {
                    self.declare_stance(stance_handle(target, &target_path)?, &target_path)?;
                }
                _ if target.is_string() => {}
                _ => {
                    return Err(shape(format!(
                        "`{target_path}`: {}; a row's `to` takes an activity name, a stance handle from `defineStance` or an attack handle from `defineAttack`",
                        found(target)
                    )));
                }
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
                                self.resolve_action(
                                    action,
                                    &format!("{layer_path}[{index}].action"),
                                )?;
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

/// Expand this envelope's stances and strikes, then each nested envelope's.
/// A nested level may leave through an outer row: the enclosing level's
/// `"*"` rows to anywhere but the composite holding it (the engine skips a
/// `"*"` row into the current activity), its rows out of the composite, or an
/// exit further out. Returns every attack struck.
fn expand_envelope(
    envelope: &mut Map<String, Value>,
    path: &str,
    catalog: &Catalog,
    ancestor_exit: bool,
) -> Result<BTreeSet<String>, DescriptorError> {
    let mut struck = expand::expand_level(envelope, path, catalog, ancestor_exit)?;
    let star_targets = expand::star_targets(envelope);
    let exits: BTreeSet<String> = expand::authored_activities(envelope)
        .into_iter()
        .filter(|name| expand::has_rows(envelope, name))
        .collect();
    let mut result = Ok(());
    for_each_nested_envelope_of(envelope, path, |activity, nested, nested_path| {
        if result.is_ok() {
            let star_exit = star_targets.iter().any(|target| target != activity);
            let exit = ancestor_exit || star_exit || exits.contains(activity);
            match expand_envelope(nested, nested_path, catalog, exit) {
                Ok(more) => struck.extend(more),
                Err(error) => result = Err(error),
            }
        }
    });
    result.map(|()| struck)
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
    if let Some(transitions) = envelope
        .get_mut("transitions")
        .and_then(Value::as_object_mut)
    {
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
    for_each_nested_envelope_of(envelope, path, |_, nested, nested_path| {
        visit(nested, nested_path);
    });
}

/// [`for_each_nested_envelope`], naming the activity each layer belongs to.
fn for_each_nested_envelope_of(
    envelope: &mut Map<String, Value>,
    path: &str,
    mut visit: impl FnMut(&str, &mut Map<String, Value>, &str),
) {
    let Some(activities) = envelope
        .get_mut("activities")
        .and_then(Value::as_object_mut)
    else {
        return;
    };
    for (name, activity) in activities.iter_mut() {
        let Some(layers) = activity.get_mut("layers").and_then(Value::as_object_mut) else {
            continue;
        };
        for (layer_name, layer) in layers.iter_mut() {
            if let Some(nested) = layer.as_object_mut() {
                visit(
                    name,
                    nested,
                    &format!("{path}.activities.{name}.layers.{layer_name}"),
                );
            }
        }
    }
}

/// Record a handle's declaration under its name. Two handles of one name in
/// one archetype must declare the same thing: a Luau re-`require` makes equal
/// copies, while unequal ones would hide which one a name reaches.
fn declare_into(
    declarations: &mut BTreeMap<String, Declared>,
    kind: &str,
    handle: &Map<String, Value>,
    path: &str,
) -> Result<(), DescriptorError> {
    let name = handle_name(handle);
    match declarations.get(&name) {
        Some(first) if first.declaration != *handle => Err(shape(format!(
            "`{path}` declares {kind} \"{name}\" differently from `{}`; two {kind} handles of one name must declare the same {kind}",
            first.path
        ))),
        Some(_) => Ok(()),
        None => {
            declarations.insert(
                name,
                Declared {
                    declaration: handle.clone(),
                    path: path.to_string(),
                },
            );
            Ok(())
        }
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
fn attack_handle<'a>(
    value: &'a Value,
    path: &str,
) -> Result<&'a Map<String, Value>, DescriptorError> {
    expect_handle(value, path, "attack", "defineAttack")
}

fn stance_handle<'a>(
    value: &'a Value,
    path: &str,
) -> Result<&'a Map<String, Value>, DescriptorError> {
    expect_handle(value, path, "stance", "defineStance")
}

fn expect_handle<'a>(
    value: &'a Value,
    path: &str,
    kind: &str,
    definer: &str,
) -> Result<&'a Map<String, Value>, DescriptorError> {
    if let Value::Object(object) = value
        && object.get("kind").and_then(Value::as_str) == Some(kind)
    {
        if object
            .get("name")
            .and_then(Value::as_str)
            .is_some_and(|name| !name.is_empty())
        {
            return Ok(object);
        }
        return Err(shape(format!(
            "`{path}`: the {kind} handle has an empty name; give `{definer}` a non-empty name"
        )));
    }
    Err(shape(format!(
        "`{path}`: {}; pass the {kind} handle `{definer}` returned",
        found(value)
    )))
}

/// What stands where a handle was expected, for a rejection.
fn found(value: &Value) -> String {
    match value {
        Value::String(text) => format!("`\"{text}\"` is a bare string"),
        Value::Null => "is nil or undefined (in Luau, a misspelled handle reads nil)".to_string(),
        Value::Object(object) => match object.get("kind").and_then(Value::as_str) {
            Some(other) => format!("is a `{other}` handle"),
            None => "is a plain object, not a handle".to_string(),
        },
        other => format!("is {}", kind_of(other)),
    }
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
