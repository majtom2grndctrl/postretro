//! One closed life-state validator for both authoring VMs.
//! See: context/lib/scripting.md §12

use std::collections::{BTreeMap, BTreeSet};

use postretro_entities::LifeStatesDescriptor;
use postretro_foundation::{
    ACTIVATION_IR_CAP_REASON, ASSIST_HOLD_MS_LITERAL_RULE, ASSIST_RADIUS_LITERAL_RULE, Assistable,
    LifeStatePosture, LifeStateProfile, MAX_ACTIVATION_IR_DEPTH, MAX_ACTIVATION_IR_NODES,
    NumberOrIr, PlayerMovementDescriptor, assist_hold_ms_literal_valid,
    assist_radius_literal_valid, check_activation_ir_caps, ir_node_from_json,
};
use serde_json::{Map, Value};

use super::{DescriptorError, EntityTypeDescriptor, JsValue, LuaValue, js_err, lua_err};

const ROOT: &str = "components.lifeStates";

fn invalid(path: &str, message: &str) -> DescriptorError {
    DescriptorError::InvalidShape {
        reason: format!("`{path}` {message}"),
    }
}

fn object<'a>(value: &'a Value, path: &str) -> Result<&'a Map<String, Value>, DescriptorError> {
    value
        .as_object()
        .ok_or_else(|| invalid(path, "must be an object"))
}

fn keys(value: &Map<String, Value>, path: &str, allowed: &[&str]) -> Result<(), DescriptorError> {
    for key in value.keys() {
        if !allowed.contains(&key.as_str()) {
            return Err(invalid(&format!("{path}.{key}"), "is not a supported key"));
        }
    }
    Ok(())
}

fn string<'a>(value: Option<&'a Value>, path: &str) -> Result<&'a str, DescriptorError> {
    value
        .and_then(Value::as_str)
        .ok_or_else(|| invalid(path, "must be a string"))
}

fn validate_profile(
    value: &Value,
    path: &str,
    crouch: bool,
) -> Result<LifeStateProfile, DescriptorError> {
    let fields = object(value, path)?;
    keys(
        fields,
        path,
        &[
            "movement",
            "look",
            "fire",
            "switchWeapon",
            "reload",
            "use",
            "touch",
            "occupancy",
            "damageable",
            "assistable",
        ],
    )?;
    for (key, value) in fields {
        let field = format!("{path}.{key}");
        if key != "movement" && key != "assistable" && !value.is_boolean() {
            return Err(invalid(&field, "must be a boolean"));
        }
    }
    if let Some(movement) = fields.get("movement") {
        let path = format!("{path}.movement");
        let movement = object(movement, &path)?;
        keys(movement, &path, &["speedScale", "abilities", "posture"])?;
        if let Some(speed) = movement.get("speedScale") {
            let speed = speed.as_f64().ok_or_else(|| {
                invalid(
                    &format!("{path}.speedScale"),
                    "must be a finite number in [0, 1]",
                )
            })?;
            if !speed.is_finite() || !(0.0..=1.0).contains(&speed) {
                return Err(invalid(
                    &format!("{path}.speedScale"),
                    "must be a finite number in [0, 1]",
                ));
            }
        }
        if movement
            .get("abilities")
            .is_some_and(|value| !value.is_boolean())
        {
            return Err(invalid(&format!("{path}.abilities"), "must be a boolean"));
        }
        if let Some(posture) = movement.get("posture") {
            match posture.as_str() {
                Some("stand") => {}
                Some("crouch") if crouch => {}
                Some("crouch") => {
                    return Err(invalid(
                        &format!("{path}.posture"),
                        "requires components.movement.crouch",
                    ));
                }
                _ => {
                    return Err(invalid(
                        &format!("{path}.posture"),
                        "must be `stand` or `crouch`",
                    ));
                }
            }
        }
    }
    let assistable = fields
        .get("assistable")
        .map(|value| validate_assistable(value, &format!("{path}.assistable")))
        .transpose()?;
    let mut flags = fields.clone();
    flags.remove("assistable");
    let mut profile: LifeStateProfile = serde_json::from_value(Value::Object(flags))
        .map_err(|error| invalid(path, &error.to_string()))?;
    profile.assistable = assistable;
    debug_assert!(profile.movement.posture != LifeStatePosture::Crouch || crouch);
    Ok(profile)
}

/// Route each field explicitly, as the dash parsers do: an object is an
/// expression, a number a literal. Expressions are checked only against the
/// activation caps here; their inputs bind where store schemas are known.
fn validate_assistable(value: &Value, path: &str) -> Result<Assistable, DescriptorError> {
    let fields = object(value, path)?;
    keys(fields, path, &["radius", "holdMs"])?;
    let mut assistable = Assistable::default();
    if let Some(radius) = fields.get("radius") {
        assistable.radius = assist_field(
            radius,
            &format!("{path}.radius"),
            assist_radius_literal_valid,
            ASSIST_RADIUS_LITERAL_RULE,
        )?;
    }
    if let Some(hold_ms) = fields.get("holdMs") {
        assistable.hold_ms = assist_field(
            hold_ms,
            &format!("{path}.holdMs"),
            assist_hold_ms_literal_valid,
            ASSIST_HOLD_MS_LITERAL_RULE,
        )?;
    }
    Ok(assistable)
}

fn assist_field(
    value: &Value,
    path: &str,
    literal_valid: fn(f32) -> bool,
    rule: &str,
) -> Result<NumberOrIr, DescriptorError> {
    match value {
        Value::Object(_) => {
            let node = ir_node_from_json(value.clone(), path)?;
            check_activation_ir_caps(&node).map_err(|reason| invalid(path, reason))?;
            Ok(NumberOrIr::Ir(node))
        }
        Value::Number(number) => {
            // Checked after narrowing, so a value that only fits as f64 (an
            // overflow to infinity, an underflow to 0) is judged as stored.
            let literal = number.as_f64().map(|value| value as f32);
            match literal {
                Some(literal) if literal_valid(literal) => Ok(NumberOrIr::Literal(literal)),
                _ => Err(invalid(path, rule)),
            }
        }
        _ => Err(invalid(path, "must be a number or a runtime expression")),
    }
}

fn validate(
    value: Value,
    movement: Option<&PlayerMovementDescriptor>,
) -> Result<LifeStatesDescriptor, DescriptorError> {
    let movement = movement.ok_or_else(|| invalid(ROOT, "requires components.movement"))?;
    let block = object(&value, ROOT)?;
    keys(block, ROOT, &["initial", "states", "rejoin"])?;
    let initial = string(block.get("initial"), &format!("{ROOT}.initial"))?.to_string();
    let states_path = format!("{ROOT}.states");
    let states = object(
        block
            .get("states")
            .ok_or_else(|| invalid(&states_path, "is required"))?,
        &states_path,
    )?;
    let mut profiles = BTreeMap::new();
    for (name, value) in states {
        let path = format!("{states_path}.{name}");
        postretro_foundation::validate_ascii_identifier(&path, name)?;
        profiles.insert(
            name.clone(),
            validate_profile(value, &path, movement.crouch.is_some())?,
        );
    }
    if !profiles.contains_key(&initial) {
        return Err(invalid(
            &format!("{ROOT}.initial"),
            "must name a declared state",
        ));
    }
    let mut rejoin = BTreeMap::new();
    if let Some(value) = block.get("rejoin") {
        for (source, target) in object(value, &format!("{ROOT}.rejoin"))? {
            let path = format!("{ROOT}.rejoin.{source}");
            let target = string(Some(target), &path)?;
            if !profiles.contains_key(source) || !profiles.contains_key(target) {
                return Err(invalid(&path, "key and value must name declared states"));
            }
            rejoin.insert(source.clone(), target.to_owned());
        }
    }
    if profiles.len() > usize::from(u8::MAX) {
        return Err(invalid(&states_path, LIFE_STATE_NAME_LIMIT));
    }
    let state_ids = profiles
        .keys()
        .enumerate()
        .map(|(index, name)| (name.clone(), index as u8))
        .collect();
    Ok(LifeStatesDescriptor {
        initial,
        states: profiles,
        rejoin,
        state_ids,
        table_revision: 0,
    })
}

/// The first name past the 255-name id space, walking descriptors in order:
/// the holding descriptor's index and the name.
pub(crate) fn first_life_state_name_overflow(
    entities: &[EntityTypeDescriptor],
) -> Option<(usize, &str)> {
    let mut names = BTreeSet::new();
    for (index, descriptor) in entities.iter().enumerate() {
        let Some(block) = descriptor.life_states.as_ref() else {
            continue;
        };
        for name in block.states.keys() {
            names.insert(name);
            if names.len() > usize::from(u8::MAX) {
                return Some((index, name.as_str()));
            }
        }
    }
    None
}

pub(crate) const LIFE_STATE_NAME_LIMIT: &str =
    "exceeds the mod limit of 255 distinct life-state names";

/// Validate a parsed manifest's `entities` before any startup/reload commit.
/// Indexes name the manifest array, so pass the authored list only.
pub(crate) fn validate_life_state_name_limit(
    entities: &[EntityTypeDescriptor],
) -> Result<(), DescriptorError> {
    match first_life_state_name_overflow(entities) {
        None => Ok(()),
        Some((index, name)) => Err(invalid(
            &format!("entities[{index}].{ROOT}.states.{name}"),
            LIFE_STATE_NAME_LIMIT,
        )),
    }
}

/// Object nesting allowed below `components.lifeStates` outside expressions.
const MAX_OBJECT_DEPTH: usize = 6;
/// Raw depth of `components.lifeStates.states.<state>.assistable.<field>`.
const ASSIST_FIELD_DEPTH: usize = 4;

/// The walk below an `assistable` field, which follows the activation IR caps
/// rather than the object depth and reports a breach at the field itself.
struct IrBudget {
    field: String,
    /// Raw depth of the field's parent, so the field's own node is IR depth 1.
    base: usize,
    nodes: usize,
}

fn is_assist_field(path: &str, depth: usize) -> bool {
    depth == ASSIST_FIELD_DEPTH
        && path.starts_with(&format!("{ROOT}.states."))
        && (path.ends_with(".assistable.radius") || path.ends_with(".assistable.holdMs"))
}

/// Count an object as one IR node while inside an `assistable` field. The node
/// cap also bounds a shared-reference graph, which would otherwise expand
/// exponentially in the walk.
fn enter_object(depth: usize, ir: &mut Option<IrBudget>) -> Result<(), DescriptorError> {
    let Some(budget) = ir else {
        return Ok(());
    };
    budget.nodes += 1;
    if depth - budget.base > MAX_ACTIVATION_IR_DEPTH || budget.nodes > MAX_ACTIVATION_IR_NODES {
        return Err(invalid(&budget.field, ACTIVATION_IR_CAP_REASON));
    }
    Ok(())
}

/// Walk one child, opening an IR budget when the child is an `assistable` field.
fn descend(
    child: &str,
    depth: usize,
    ir: &mut Option<IrBudget>,
    walk: impl FnOnce(&mut Option<IrBudget>) -> Result<Value, DescriptorError>,
) -> Result<Value, DescriptorError> {
    if ir.is_some() || !is_assist_field(child, depth) {
        return walk(ir);
    }
    let mut budget = Some(IrBudget {
        field: child.to_owned(),
        base: depth - 1,
        nodes: 0,
    });
    walk(&mut budget)
}

// The raw adapters keep empty Luau tables as objects and reject invalid/nonfinite
// values with their full path before JSON can lose their authored shape.
fn raw_js(
    value: JsValue<'_>,
    path: &str,
    depth: usize,
    ir: &mut Option<IrBudget>,
) -> Result<Value, DescriptorError> {
    if ir.is_none() && depth > MAX_OBJECT_DEPTH {
        return Err(invalid(path, "exceeds the life-state object depth"));
    }
    if let Some(value) = value.as_bool() {
        return Ok(Value::Bool(value));
    }
    if let Some(value) = value.as_number() {
        return serde_json::Number::from_f64(value)
            .map(Value::Number)
            .ok_or_else(|| invalid(path, "must be finite"));
    }
    if let Some(value) = value.as_string() {
        return Ok(Value::String(value.to_string().map_err(js_err)?));
    }
    if value.is_null() || value.is_undefined() {
        return Ok(Value::Null);
    }
    if value.is_array() || value.is_function() {
        return Err(invalid(path, "must be a data object"));
    }
    let Some(object) = value.as_object() else {
        return Err(invalid(path, "has an unsupported value"));
    };
    enter_object(depth, ir)?;
    let mut entries = BTreeMap::new();
    for key in object.own_keys::<String>(rquickjs::object::Filter::new().string()) {
        let key = key.map_err(js_err)?;
        entries.insert(key.clone(), object.get::<_, JsValue>(&key).map_err(js_err)?);
    }
    let mut out = Map::new();
    for (key, value) in entries {
        // `undefined` is absence, as a Luau nil key is: an omitted optional,
        // never a value for the closed schema to reject.
        if value.is_undefined() {
            continue;
        }
        let child = format!("{path}.{key}");
        let value = descend(&child, depth + 1, ir, |ir| {
            raw_js(value, &child, depth + 1, ir)
        })?;
        out.insert(key, value);
    }
    Ok(Value::Object(out))
}

fn raw_lua(
    value: LuaValue,
    path: &str,
    depth: usize,
    ir: &mut Option<IrBudget>,
) -> Result<Value, DescriptorError> {
    if ir.is_none() && depth > MAX_OBJECT_DEPTH {
        return Err(invalid(path, "exceeds the life-state object depth"));
    }
    match value {
        LuaValue::Nil => Ok(Value::Null),
        LuaValue::Boolean(value) => Ok(Value::Bool(value)),
        LuaValue::Integer(value) => Ok(Value::Number(value.into())),
        LuaValue::Number(value) => serde_json::Number::from_f64(value)
            .map(Value::Number)
            .ok_or_else(|| invalid(path, "must be finite")),
        LuaValue::String(value) => Ok(Value::String(value.to_str().map_err(lua_err)?.to_string())),
        LuaValue::Table(table) => {
            enter_object(depth, ir)?;
            let mut entries = BTreeMap::new();
            for entry in table.pairs::<LuaValue, LuaValue>() {
                let (key, value) = entry.map_err(lua_err)?;
                let LuaValue::String(key) = key else {
                    return Err(invalid(path, "keys must be strings"));
                };
                entries.insert(key.to_str().map_err(lua_err)?.to_string(), value);
            }
            let mut out = Map::new();
            for (key, value) in entries {
                let child = format!("{path}.{key}");
                let value = descend(&child, depth + 1, ir, |ir| {
                    raw_lua(value, &child, depth + 1, ir)
                })?;
                out.insert(key, value);
            }
            Ok(Value::Object(out))
        }
        _ => Err(invalid(path, "has an unsupported value")),
    }
}

pub(crate) fn life_states_from_js(
    value: JsValue<'_>,
    movement: Option<&PlayerMovementDescriptor>,
) -> Result<LifeStatesDescriptor, DescriptorError> {
    validate(raw_js(value, ROOT, 0, &mut None)?, movement)
}

pub(crate) fn life_states_from_lua(
    value: LuaValue,
    movement: Option<&PlayerMovementDescriptor>,
) -> Result<LifeStatesDescriptor, DescriptorError> {
    validate(raw_lua(value, ROOT, 0, &mut None)?, movement)
}
