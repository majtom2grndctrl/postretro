// Status kinds: the `defineMod({ statuses })` declaration parser, parameter
// ranges and the millisecond-to-fixed-tick quantization the status stage uses.
// See: context/lib/scripting.md §2 (Status declarations) · context/lib/entity_model.md §Components

use serde_json::{Map, Value};

use crate::defense::{HandleRef, validate_defense_name};

/// Most statuses one mod may declare: presence replicates as one `u64`.
pub const MAX_STATUSES: usize = 64;
/// Most live instances one entity holds (`research.md` §Instance cap).
pub const MAX_STATUS_INSTANCES: usize = 16;
/// Literal range of `damage`, per tick per stack.
pub const STATUS_DAMAGE_RANGE: (f32, f32) = (0.0, 1.0e6);
/// Literal range of `tickMs`.
pub const STATUS_TICK_MS_RANGE: (f32, f32) = (1.0, 60_000.0);
/// Upper bound of `durationMs`; its lower bound is the status's `tickMs`.
pub const STATUS_DURATION_MS_MAX: f32 = 600_000.0;
/// Literal range of `maxStacks`.
pub const STATUS_MAX_STACKS_RANGE: (u8, u8) = (1, 64);

/// Prefix of the per-entity status Bools: `@status.<name>` holds while the
/// entity carries a live instance of that registered status.
pub const STATUS_INPUT_PREFIX: &str = "@status.";

/// Prefix of the local-only `player.status.<name>` Bool slots a status
/// handle's `.active` reads.
pub const PLAYER_STATUS_SLOT_PREFIX: &str = "player.status.";

/// Source id of an application whose impact had no damager and that came
/// from a reaction verb rather than an impact (`players().applyStatus`).
pub const APPLY_STATUS_SOURCE_ID: &str = "script.applyStatus";
/// Prefix of a fluid's applier source id, `fluid.<name>`.
pub const FLUID_SOURCE_PREFIX: &str = "fluid.";

/// How a repeat application by the same applier changes its instance.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum StatusStacking {
    /// Resets time left.
    #[default]
    Refresh,
    /// Adds one stack up to `maxStacks` and resets time left.
    Stack,
}

/// What a status draws on each afflicted entity. The emitter stays the
/// SDK builder's JSON here; the presentation layer resolves it.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct StatusPresentation {
    pub emitter: Option<Value>,
    pub tick_sound: Option<String>,
}

/// One registered status kind with its literal parameters, quantized.
#[derive(Debug, Clone, PartialEq)]
pub struct StatusDeclaration {
    pub name: String,
    /// Damage per tick per stack. Zero makes a marker that never enters the
    /// damage chokepoint.
    pub damage: f32,
    /// Registered damage-type index; `None` deals the mod's default type.
    pub damage_type: Option<usize>,
    pub tick_ms: f32,
    /// Whole fixed ticks between damage ticks, at least one.
    pub tick_ticks: u32,
    /// `None` lasts until cleared.
    pub duration_ms: Option<f32>,
    pub duration_ticks: Option<u32>,
    pub stacking: StatusStacking,
    pub max_stacks: u8,
    pub presentation: StatusPresentation,
}

impl StatusDeclaration {
    /// A zero-damage marker: it ticks without entering the chokepoint.
    pub fn is_marker(&self) -> bool {
        self.damage <= 0.0
    }
}

/// Whole fixed ticks `ms` spans, rounded up, at least one: `tickMs` 1 through
/// 16 tick every fixed tick at 60 Hz and 17 every second one.
pub fn status_ticks(ms: f32) -> u32 {
    crate::defense::delay_ticks(ms).clamp(1, u64::from(u32::MAX)) as u32
}

const STATUS_KEYS: [&str; 9] = [
    "kind",
    "name",
    "damage",
    "type",
    "tickMs",
    "durationMs",
    "stacking",
    "maxStacks",
    "presentation",
];

/// Parse the manifest's `statuses` list against the registered damage types.
/// `type_index` resolves a damage-type name. Every failure names its key path.
pub fn parse_statuses(
    value: Option<&Value>,
    type_index: impl Fn(&str) -> Option<usize>,
) -> Result<Vec<StatusDeclaration>, String> {
    let entries = match value {
        None | Some(Value::Null) => return Ok(Vec::new()),
        Some(Value::Array(entries)) => entries,
        // An empty Luau table can arrive as an empty object.
        Some(Value::Object(object)) if object.is_empty() => return Ok(Vec::new()),
        Some(_) => return Err("`statuses` must be an array of handles".to_string()),
    };
    if entries.len() > MAX_STATUSES {
        return Err(format!(
            "`statuses[{MAX_STATUSES}]`: a mod declares at most {MAX_STATUSES} statuses"
        ));
    }
    let mut statuses: Vec<StatusDeclaration> = Vec::with_capacity(entries.len());
    for (index, entry) in entries.iter().enumerate() {
        let path = format!("statuses[{index}]");
        let status = parse_status(&path, entry, &type_index)?;
        if statuses.iter().any(|other| other.name == status.name) {
            return Err(format!(
                "`{path}`: status `{}` is registered twice",
                status.name
            ));
        }
        statuses.push(status);
    }
    Ok(statuses)
}

fn parse_status(
    path: &str,
    entry: &Value,
    type_index: &impl Fn(&str) -> Option<usize>,
) -> Result<StatusDeclaration, String> {
    if let Some(text) = entry.as_str() {
        return Err(format!(
            "`{path}`: `\"{text}\"` is a bare string; list the handle its declaration returned"
        ));
    }
    let object = entry
        .as_object()
        .ok_or_else(|| format!("`{path}`: expected a `status` handle"))?;
    match object.get("kind").and_then(Value::as_str) {
        Some("status") => {}
        Some(kind) => {
            return Err(format!(
                "`{path}`: expected a `status` handle, got a `{kind}` handle"
            ));
        }
        None => {
            return Err(format!(
                "`{path}`: expected a `status` handle, got a value with no `kind`"
            ));
        }
    }
    if let Some(key) = object
        .keys()
        .find(|key| !STATUS_KEYS.contains(&key.as_str()))
    {
        return Err(format!(
            "`{path}.{key}`: `defineStatus` takes no key `{key}`"
        ));
    }
    let name = object
        .get("name")
        .and_then(Value::as_str)
        .ok_or_else(|| format!("`{path}.name` must be a string"))?;
    validate_defense_name(name).map_err(|error| format!("`{path}.name`: {error}"))?;

    let damage = required_number(object, path, "damage", f32_bounds(STATUS_DAMAGE_RANGE))? as f32;
    let damage_type = match object.get("type") {
        None | Some(Value::Null) => None,
        Some(value) => {
            let type_path = format!("{path}.type");
            let handle =
                HandleRef::from_json(value).map_err(|error| format!("`{type_path}`: {error}"))?;
            let type_name = handle
                .expect_damage_type()
                .map_err(|error| format!("`{type_path}`: {error}"))?;
            Some(type_index(type_name).ok_or_else(|| {
                format!(
                    "`{type_path}`: damage type `{type_name}` is not registered in `defineMod({{ damageTypes }})`"
                )
            })?)
        }
    };
    let tick_ms_exact = required_number(object, path, "tickMs", f32_bounds(STATUS_TICK_MS_RANGE))?;
    let tick_ms = tick_ms_exact as f32;
    let duration_ms = match object.get("durationMs") {
        None | Some(Value::Null) => None,
        Some(_) => Some(required_number(
            object,
            path,
            "durationMs",
            (tick_ms_exact, f64::from(STATUS_DURATION_MS_MAX)),
        )? as f32),
    };
    let stacking = match object.get("stacking") {
        None | Some(Value::Null) => StatusStacking::Refresh,
        Some(Value::String(text)) if text == "refresh" => StatusStacking::Refresh,
        Some(Value::String(text)) if text == "stack" => StatusStacking::Stack,
        Some(_) => {
            return Err(format!(
                "`{path}.stacking` must be `\"refresh\"` or `\"stack\"`"
            ));
        }
    };
    let max_stacks = match object.get("maxStacks") {
        None | Some(Value::Null) => 1,
        Some(_) => {
            let (min, max) = STATUS_MAX_STACKS_RANGE;
            let value =
                required_number(object, path, "maxStacks", (f64::from(min), f64::from(max)))?;
            if value.fract() != 0.0 {
                return Err(format!("`{path}.maxStacks` must be a whole number"));
            }
            value as u8
        }
    };
    let presentation =
        parse_presentation(&format!("{path}.presentation"), object.get("presentation"))?;
    Ok(StatusDeclaration {
        name: name.to_string(),
        damage,
        damage_type,
        tick_ms,
        tick_ticks: status_ticks(tick_ms),
        duration_ms,
        duration_ticks: duration_ms.map(status_ticks),
        stacking,
        max_stacks,
        presentation,
    })
}

fn f32_bounds((min, max): (f32, f32)) -> (f64, f64) {
    (f64::from(min), f64::from(max))
}

// Range-checks in f64 so a value a hair past a bound cannot round onto it when
// narrowed to f32; callers narrow after the check.
fn required_number(
    object: &Map<String, Value>,
    path: &str,
    key: &str,
    (min, max): (f64, f64),
) -> Result<f64, String> {
    let value = object
        .get(key)
        .and_then(Value::as_f64)
        .ok_or_else(|| format!("`{path}.{key}` must be a number"))?;
    if !value.is_finite() || value < min || value > max {
        return Err(format!(
            "`{path}.{key}` must be in [{min}, {max}], got {value}"
        ));
    }
    Ok(value)
}

fn parse_presentation(path: &str, value: Option<&Value>) -> Result<StatusPresentation, String> {
    let object = match value {
        None | Some(Value::Null) => return Ok(StatusPresentation::default()),
        Some(Value::Array(entries)) if entries.is_empty() => {
            return Ok(StatusPresentation::default());
        }
        Some(Value::Object(object)) => object,
        Some(_) => return Err(format!("`{path}` must be an object")),
    };
    if let Some(key) = object
        .keys()
        .find(|key| !matches!(key.as_str(), "emitter" | "tickSound"))
    {
        return Err(format!(
            "`{path}.{key}`: a status presentation takes no key `{key}`"
        ));
    }
    let emitter = match object.get("emitter") {
        None | Some(Value::Null) => None,
        Some(Value::Object(emitter))
            if emitter.get("kind").and_then(Value::as_str) == Some("billboard_emitter") =>
        {
            Some(Value::Object(emitter.clone()))
        }
        Some(_) => {
            return Err(format!(
                "`{path}.emitter` must be built with the SDK `emitter()` builder"
            ));
        }
    };
    let tick_sound = match object.get("tickSound") {
        None | Some(Value::Null) => None,
        Some(Value::String(sound)) if !sound.is_empty() => Some(sound.clone()),
        Some(_) => return Err(format!("`{path}.tickSound` must be a non-empty sound key")),
    };
    Ok(StatusPresentation {
        emitter,
        tick_sound,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn status(fields: Value) -> Value {
        let mut base = json!({ "kind": "status", "name": "burning", "damage": 4, "tickMs": 500 });
        base.as_object_mut()
            .unwrap()
            .extend(fields.as_object().unwrap().clone());
        base
    }

    fn fire(name: &str) -> Option<usize> {
        (name == "fire").then_some(0)
    }

    fn parse(entries: Value) -> Result<Vec<StatusDeclaration>, String> {
        parse_statuses(Some(&entries), fire)
    }

    #[test]
    fn status_timing_rounds_up_to_fixed_ticks() {
        assert_eq!(status_ticks(1.0), 1);
        assert_eq!(status_ticks(16.0), 1);
        assert_eq!(status_ticks(17.0), 2);
        assert_eq!(status_ticks(500.0), 30);
        assert_eq!(status_ticks(3000.0), 180);
    }

    #[test]
    fn define_status_parses_declared_parameters() {
        let parsed = parse(json!([status(json!({
            "type": { "kind": "damageType", "name": "fire" },
            "durationMs": 3000, "stacking": "stack", "maxStacks": 3,
            "presentation": {
                "emitter": { "kind": "billboard_emitter", "sprite": "ember" },
                "tickSound": "burn_tick",
            },
        }))]))
        .unwrap();
        let burning = &parsed[0];
        assert_eq!(burning.damage_type, Some(0));
        assert_eq!(
            (burning.tick_ticks, burning.duration_ticks),
            (30, Some(180))
        );
        assert_eq!(burning.stacking, StatusStacking::Stack);
        assert_eq!(burning.max_stacks, 3);
        assert_eq!(
            burning.presentation.tick_sound.as_deref(),
            Some("burn_tick")
        );
        let marker = &parse(json!([status(json!({ "damage": 0 }))])).unwrap()[0];
        assert!(marker.is_marker());
        assert_eq!(marker.damage_type, None);
        assert_eq!(marker.duration_ticks, None, "until cleared");
        assert_eq!(marker.max_stacks, 1);
    }

    #[test]
    fn define_status_rejects_out_of_range_and_names_key_path() {
        let cases = [
            (json!({ "durationMs": 499 }), "statuses[0].durationMs"),
            (json!({ "durationMs": 600_001 }), "statuses[0].durationMs"),
            (json!({ "tickMs": 0.5 }), "statuses[0].tickMs"),
            (json!({ "tickMs": 60_001 }), "statuses[0].tickMs"),
            (json!({ "damage": -1 }), "statuses[0].damage"),
            (json!({ "damage": 1.0e7 }), "statuses[0].damage"),
            (json!({ "maxStacks": 0 }), "statuses[0].maxStacks"),
            (json!({ "maxStacks": 65 }), "statuses[0].maxStacks"),
            (json!({ "chance": 0.5 }), "statuses[0].chance"),
            (
                json!({ "type": { "kind": "damageType", "name": "ice" } }),
                "statuses[0].type",
            ),
            (
                json!({ "type": { "kind": "defenseLayer", "name": "fire" } }),
                "statuses[0].type",
            ),
            (json!({ "type": "fire" }), "statuses[0].type"),
            (json!({ "stacking": "add" }), "statuses[0].stacking"),
            (
                json!({ "presentation": { "glow": true } }),
                "statuses[0].presentation.glow",
            ),
        ];
        for (fields, path) in cases {
            let error = parse(json!([status(fields.clone())])).unwrap_err();
            assert!(error.contains(&format!("`{path}")), "{fields}: {error}");
        }
        let many: Vec<Value> = (0..=MAX_STATUSES)
            .map(|index| status(json!({ "name": format!("s{index}") })))
            .collect();
        let error = parse(Value::Array(many)).unwrap_err();
        assert!(error.contains("`statuses[64]`"), "{error}");
    }

    // Regression: range checks ran after narrowing to f32, so values a hair past a
    // bound rounded onto it and loaded.
    #[test]
    fn define_status_rejects_values_just_past_bounds_in_f64() {
        let cases = [
            (json!({ "damage": 1000000.03 }), "damage"),
            (json!({ "damage": -0.00000001 }), "damage"),
            (json!({ "tickMs": 0.99999999 }), "tickMs"),
            (json!({ "tickMs": 60000.01 }), "tickMs"),
            (json!({ "durationMs": 600000.03 }), "durationMs"),
            (
                json!({ "tickMs": 500, "durationMs": 499.99999999 }),
                "durationMs",
            ),
            (json!({ "maxStacks": 64.00000001 }), "maxStacks"),
            (json!({ "maxStacks": 0.99999999 }), "maxStacks"),
        ];
        for (fields, key) in cases {
            let error = parse(json!([status(fields.clone())])).unwrap_err();
            assert!(
                error.contains(&format!("`statuses[0].{key}`")),
                "{fields}: {error}"
            );
        }
    }

    #[test]
    fn define_status_boundary_values_load() {
        let boundaries = [
            json!({ "tickMs": 1, "durationMs": 1 }),
            json!({ "tickMs": 500, "durationMs": 500 }),
            json!({ "tickMs": 60_000, "durationMs": 600_000 }),
            json!({ "damage": 0 }),
            json!({ "damage": 1.0e6 }),
            json!({ "maxStacks": 1 }),
            json!({ "maxStacks": 64 }),
        ];
        for fields in boundaries {
            assert!(parse(json!([status(fields.clone())])).is_ok(), "{fields}");
        }
        let full: Vec<Value> = (0..MAX_STATUSES)
            .map(|index| status(json!({ "name": format!("s{index}") })))
            .collect();
        assert_eq!(parse(Value::Array(full)).unwrap().len(), MAX_STATUSES);
    }
}
