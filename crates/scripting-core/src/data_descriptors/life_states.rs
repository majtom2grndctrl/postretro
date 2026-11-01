//! One closed life-state validator for both authoring VMs.

use std::collections::{BTreeMap, BTreeSet};

use postretro_entities::LifeStatesDescriptor;
use postretro_foundation::{LifeStatePosture, LifeStateProfile, PlayerMovementDescriptor};
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
        ],
    )?;
    for (key, value) in fields {
        let field = format!("{path}.{key}");
        if key != "movement" && !value.is_boolean() {
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
    let profile: LifeStateProfile =
        serde_json::from_value(value.clone()).map_err(|error| invalid(path, &error.to_string()))?;
    debug_assert!(profile.movement.posture != LifeStatePosture::Crouch || crouch);
    Ok(profile)
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
        return Err(invalid(
            &states_path,
            "exceeds the mod limit of 255 distinct life-state names",
        ));
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

/// Validate across the whole parsed manifest before any startup/reload commit.
pub(crate) fn validate_life_state_name_limit(
    entities: &[EntityTypeDescriptor],
) -> Result<(), DescriptorError> {
    let mut names = BTreeSet::new();
    for (index, descriptor) in entities.iter().enumerate() {
        if let Some(block) = descriptor.life_states.as_ref() {
            for name in block.states.keys() {
                names.insert(name);
                if names.len() > usize::from(u8::MAX) {
                    return Err(invalid(
                        &format!("entities[{index}].{ROOT}.states.{name}"),
                        "exceeds the mod limit of 255 distinct life-state names",
                    ));
                }
            }
        }
    }
    Ok(())
}

// The raw adapters keep empty Luau tables as objects and reject invalid/nonfinite
// values with their full path before JSON can lose their authored shape.
#[derive(Clone, Copy)]
enum LifeStateShape {
    Block,
    States,
    Profile,
    Movement,
    Other,
}

impl LifeStateShape {
    fn child(self, key: &str) -> Self {
        match (self, key) {
            (Self::Block, "states") => Self::States,
            (Self::States, _) => Self::Profile,
            (Self::Profile, "movement") => Self::Movement,
            _ => Self::Other,
        }
    }

    fn optional(self, key: &str) -> bool {
        match self {
            Self::Block => key == "rejoin",
            Self::Profile => matches!(
                key,
                "movement"
                    | "look"
                    | "fire"
                    | "switchWeapon"
                    | "reload"
                    | "use"
                    | "touch"
                    | "occupancy"
                    | "damageable"
            ),
            Self::Movement => matches!(key, "speedScale" | "abilities" | "posture"),
            Self::States | Self::Other => false,
        }
    }
}

fn raw_js(
    value: JsValue<'_>,
    path: &str,
    depth: u8,
    shape: LifeStateShape,
) -> Result<Value, DescriptorError> {
    if depth > 6 {
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
    let mut entries = BTreeMap::new();
    for key in object.own_keys::<String>(rquickjs::object::Filter::new().string()) {
        let key = key.map_err(js_err)?;
        entries.insert(key.clone(), object.get::<_, JsValue>(&key).map_err(js_err)?);
    }
    let mut out = Map::new();
    for (key, value) in entries {
        // Luau nil removes an optional field. Keep unknown undefined keys so
        // the closed-schema validator still rejects misspellings.
        if value.is_undefined() && shape.optional(&key) {
            continue;
        }
        let child_shape = shape.child(&key);
        out.insert(
            key.clone(),
            raw_js(value, &format!("{path}.{key}"), depth + 1, child_shape)?,
        );
    }
    Ok(Value::Object(out))
}

fn raw_lua(value: LuaValue, path: &str, depth: u8) -> Result<Value, DescriptorError> {
    if depth > 6 {
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
                out.insert(
                    key.clone(),
                    raw_lua(value, &format!("{path}.{key}"), depth + 1)?,
                );
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
    validate(raw_js(value, ROOT, 0, LifeStateShape::Block)?, movement)
}

pub(crate) fn life_states_from_lua(
    value: LuaValue,
    movement: Option<&PlayerMovementDescriptor>,
) -> Result<LifeStatesDescriptor, DescriptorError> {
    validate(raw_lua(value, ROOT, 0)?, movement)
}
