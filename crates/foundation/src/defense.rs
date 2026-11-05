// Defense layers and damage types: the handle reference both SDKs serialize,
// the vocabulary name grammar, layer parameter ranges and the pure drain rule.
// See: context/lib/scripting.md §2, §11 (Defense layers) · context/lib/entity_model.md §Components

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// Longest layer or damage-type name, in bytes.
pub const DEFENSE_NAME_MAX_BYTES: usize = 64;

/// Literal and clamp range of a layer's capacity, in points.
pub const LAYER_CAPACITY_RANGE: (f32, f32) = (0.0, 1.0e6);
/// Literal and clamp range of a layer's recharge delay, in milliseconds.
pub const LAYER_DELAY_MS_RANGE: (f32, f32) = (0.0, 60_000.0);
/// Literal and clamp range of a layer's recharge rate, in points per second.
pub const LAYER_RATE_RANGE: (f32, f32) = (0.0, 1.0e6);
/// Literal and clamp range of a damage-type multiplier or a resist value.
pub const MULTIPLIER_RANGE: (f32, f32) = (0.0, 64.0);

/// Recharge delay an entry that omits `rechargeDelayMs` uses.
pub const DEFAULT_RECHARGE_DELAY_MS: f32 = 3_000.0;
/// Recharge rate an entry that omits `rechargeRate` uses: never recharges.
pub const DEFAULT_RECHARGE_RATE: f32 = 0.0;
/// Resist value a type missing from a `resist` map uses.
pub const DEFAULT_RESIST: f32 = 1.0;

/// The pool name a matrix or resist key uses for health itself.
pub const HEALTH_POOL: &str = "health";

/// Prefix of the owner-private player layer slots.
pub const PLAYER_DEFENSE_SLOT_PREFIX: &str = "player.defense.";
/// Suffix of a player layer's capacity slot.
pub const DEFENSE_CAPACITY_SUFFIX: &str = ".capacity";
/// Prefix of the per-entity layer IR inputs.
pub const DEFENSE_INPUT_PREFIX: &str = "@defense.";

/// A layer or damage-type handle as it crosses from either SDK: exactly its
/// `kind` and `name`. A bare string fails this parse; the other `kind`
/// parses, and each surface then checks the variant it expects.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", deny_unknown_fields)]
pub enum HandleRef {
    #[serde(rename = "defenseLayer")]
    DefenseLayer { name: String },
    #[serde(rename = "damageType")]
    DamageType { name: String },
}

impl HandleRef {
    pub fn name(&self) -> &str {
        match self {
            HandleRef::DefenseLayer { name } | HandleRef::DamageType { name } => name,
        }
    }

    /// The layer name, or a diagnostic naming what was found instead.
    pub fn expect_layer(&self) -> Result<&str, String> {
        match self {
            HandleRef::DefenseLayer { name } => Ok(name),
            HandleRef::DamageType { name } => Err(format!(
                "expected a defense layer handle, got damage type `{name}`"
            )),
        }
    }

    /// The damage-type name, or a diagnostic naming what was found instead.
    pub fn expect_damage_type(&self) -> Result<&str, String> {
        match self {
            HandleRef::DamageType { name } => Ok(name),
            HandleRef::DefenseLayer { name } => Err(format!(
                "expected a damage type handle, got defense layer `{name}`"
            )),
        }
    }

    /// Parse a JSON value as a handle, naming a bare string or any other
    /// shape as a bad reference.
    pub fn from_json(value: &serde_json::Value) -> Result<Self, String> {
        if let Some(text) = value.as_str() {
            return Err(format!(
                "`\"{text}\"` is a bare string; pass the handle its declaration returned"
            ));
        }
        serde_json::from_value(value.clone())
            .map_err(|error| format!("not a defense layer or damage type handle: {error}"))
    }
}

/// Validate a layer or damage-type name: 1–64 bytes of `[A-Za-z0-9_-]`. `.`
/// and `:` are excluded because names compose into dotted slot and input
/// paths (`player.defense.<name>.capacity`).
pub fn validate_defense_name(name: &str) -> Result<(), String> {
    if name.is_empty() || name.len() > DEFENSE_NAME_MAX_BYTES {
        return Err(format!(
            "name `{name}` must be 1–{DEFENSE_NAME_MAX_BYTES} bytes"
        ));
    }
    if let Some(bad) = name
        .chars()
        .find(|c| !(c.is_ascii_alphanumeric() || *c == '_' || *c == '-'))
    {
        return Err(format!(
            "name `{name}` contains `{bad}`; names use only [A-Za-z0-9_-]"
        ));
    }
    Ok(())
}

/// One pool's multiplier for a hit: a scale in [0, 64], or a bypass that
/// passes the pool untouched. Only layers bypass.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PoolMultiplier {
    Scale(f32),
    Bypass,
}

impl PoolMultiplier {
    pub const ONE: PoolMultiplier = PoolMultiplier::Scale(1.0);

    /// This pool's multiplier after its own resist. A bypass stays a bypass.
    pub fn resisted(self, resist: f32) -> PoolMultiplier {
        match self {
            PoolMultiplier::Scale(scale) => PoolMultiplier::Scale(scale * resist),
            PoolMultiplier::Bypass => PoolMultiplier::Bypass,
        }
    }
}

/// What one hit did, after the layers drained in order and health took the
/// remainder. `points_dealt` is `absorbed + health_loss`; `health_loss` is
/// unfloored, as `healthAfter` is.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct DrainOutcome {
    pub raw: f32,
    pub absorbed: f32,
    pub health_loss: f32,
    pub points_dealt: f32,
    /// Some layer went from above zero to zero on this hit.
    pub broke: bool,
}

/// Drain `raw` through `layers` in order, then health.
///
/// Each entry is a layer's current value and its multiplier for this hit. A
/// bypass layer is passed untouched, and a layer at or below zero is skipped
/// whatever its multiplier. A layer above zero takes `raw × m`; when that
/// empties it, only the raw it could not absorb moves on, so a type's bonus
/// never carries past the pool it targets. A layer at multiplier 0 above zero
/// absorbs all the raw that reaches it and drains nothing. Health takes the
/// raw remainder × `health_multiplier`.
pub fn drain_layers<'a>(
    raw: f32,
    layers: impl IntoIterator<Item = (&'a mut f32, PoolMultiplier)>,
    health_multiplier: f32,
) -> DrainOutcome {
    let mut outcome = DrainOutcome {
        raw,
        ..DrainOutcome::default()
    };
    let mut remaining = raw;
    for (value, multiplier) in layers {
        if remaining <= 0.0 {
            break;
        }
        let PoolMultiplier::Scale(scale) = multiplier else {
            continue;
        };
        if *value <= 0.0 {
            continue;
        }
        if scale <= 0.0 {
            remaining = 0.0;
            break;
        }
        let scaled = remaining * scale;
        if scaled < *value {
            *value -= scaled;
            outcome.absorbed += scaled;
            remaining = 0.0;
            break;
        }
        let held = *value;
        outcome.absorbed += held;
        *value = 0.0;
        outcome.broke = true;
        remaining = (remaining - held / scale).max(0.0);
    }
    outcome.health_loss = remaining * health_multiplier;
    outcome.points_dealt = outcome.absorbed + outcome.health_loss;
    outcome
}

/// One fixed tick of recharge for one layer: nothing until `delay_ms` has
/// passed in whole ticks since `last_hit_tick` (a layer never hit recharges at
/// once), then `rate × dt`, stopping at `capacity`. `tick` is the tick being
/// started; a hit stamps the tick it landed in or after.
pub fn recharged_value(
    value: f32,
    capacity: f32,
    delay_ms: f32,
    rate: f32,
    last_hit_tick: Option<u64>,
    tick: u64,
    dt_seconds: f32,
) -> f32 {
    if rate <= 0.0 || value >= capacity {
        return value.min(capacity);
    }
    if let Some(hit) = last_hit_tick
        && tick.saturating_sub(hit) < delay_ticks(delay_ms)
    {
        return value;
    }
    (value + rate * dt_seconds).min(capacity)
}

/// Clamp an evaluated or literal parameter into its range. `None` for a
/// non-finite value, which is a fault.
pub fn clamp_parameter(value: f32, range: (f32, f32)) -> Option<f32> {
    value.is_finite().then(|| value.clamp(range.0, range.1))
}

/// Whole fixed ticks a recharge delay spans. Expiry counts ticks since the
/// last hit against this, never accumulated float time, so it stays exact
/// after long uptime.
pub fn delay_ticks(delay_ms: f32) -> u64 {
    let tick_ms = crate::TICK_DURATION.as_secs_f64() * 1_000.0;
    // A tiny tolerance keeps 3000 ms at 180 ticks despite the 16.667 ms step.
    ((f64::from(delay_ms.max(0.0)) / tick_ms) - 1.0e-6)
        .ceil()
        .max(0.0) as u64
}

/// One registered damage type: how much one raw point takes from each pool
/// kind. A missing pool is ×1.
#[derive(Debug, Clone, PartialEq)]
pub struct DamageTypeDeclaration {
    pub name: String,
    /// Keyed by layer name or [`HEALTH_POOL`].
    pub matrix: BTreeMap<String, PoolMultiplier>,
}

/// A mod's registered damage vocabulary: layer kinds and damage types in
/// `defineMod` list order, and the optional default type.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct DefenseVocabulary {
    pub layers: Vec<String>,
    pub damage_types: Vec<DamageTypeDeclaration>,
    /// Index into `damage_types` dealt by a producer that names no type.
    pub default_damage_type: Option<usize>,
}

impl DefenseVocabulary {
    pub fn is_empty(&self) -> bool {
        self.layers.is_empty() && self.damage_types.is_empty()
    }

    pub fn layer_index(&self, name: &str) -> Option<usize> {
        self.layers.iter().position(|layer| layer == name)
    }

    pub fn type_index(&self, name: &str) -> Option<usize> {
        self.damage_types.iter().position(|ty| ty.name == name)
    }

    /// Parse the manifest's `defenseLayers`, `damageTypes` and
    /// `defaultDamageType` values (each absent when `None`). Every failure
    /// names its key path.
    pub fn from_manifest(
        layers: Option<&serde_json::Value>,
        damage_types: Option<&serde_json::Value>,
        default_damage_type: Option<&serde_json::Value>,
    ) -> Result<Self, String> {
        let mut vocabulary = DefenseVocabulary::default();
        for (index, entry) in list_entries("defenseLayers", layers)?.iter().enumerate() {
            let path = format!("defenseLayers[{index}]");
            let (name, object) = declaration_entry(&path, entry, "defenseLayer")?;
            check_reserved_options(&path, object, "defineDefenseLayer")?;
            if name == HEALTH_POOL {
                return Err(format!("`{path}`: a layer cannot be named `{HEALTH_POOL}`"));
            }
            if vocabulary.layer_index(&name).is_some() {
                return Err(format!("`{path}`: layer `{name}` is registered twice"));
            }
            vocabulary.layers.push(name);
        }
        for (index, entry) in list_entries("damageTypes", damage_types)?
            .iter()
            .enumerate()
        {
            let path = format!("damageTypes[{index}]");
            let (name, object) = declaration_entry(&path, entry, "damageType")?;
            check_reserved_options(&path, object, "defineDamageType")?;
            if vocabulary.type_index(&name).is_some() {
                return Err(format!(
                    "`{path}`: damage type `{name}` is registered twice"
                ));
            }
            let matrix = parse_matrix(
                &format!("{path}.matrix"),
                object.get("matrix"),
                &vocabulary.layers,
            )?;
            vocabulary
                .damage_types
                .push(DamageTypeDeclaration { name, matrix });
        }
        if let Some(value) = default_damage_type.filter(|value| !value.is_null()) {
            let handle = HandleRef::from_json(value)
                .map_err(|error| format!("`defaultDamageType`: {error}"))?;
            let name = handle
                .expect_damage_type()
                .map_err(|error| format!("`defaultDamageType`: {error}"))?;
            let index = vocabulary.type_index(name).ok_or_else(|| {
                format!("`defaultDamageType`: damage type `{name}` is not listed in `damageTypes`")
            })?;
            vocabulary.default_damage_type = Some(index);
        }
        Ok(vocabulary)
    }
}

impl DefenseVocabulary {
    /// Check a descriptor's `health` block against the registered names:
    /// every layer handle names a registered layer, and every `resist` key,
    /// on health or on a layer entry, names a registered type. `path` is the
    /// block's key path.
    pub fn check_health(&self, path: &str, health: &crate::HealthDescriptor) -> Result<(), String> {
        for (index, entry) in health.layers.iter().enumerate() {
            let entry_path = format!("{path}.layers[{index}]");
            let handle = entry.layer.handle(&format!("{entry_path}.layer"))?;
            let name = handle
                .expect_layer()
                .map_err(|error| format!("`{entry_path}.layer`: {error}"))?;
            if self.layer_index(name).is_none() {
                return Err(format!(
                    "`{entry_path}.layer`: layer `{name}` is not registered in `defineMod({{ defenseLayers }})`"
                ));
            }
            self.check_resist_keys(&format!("{entry_path}.resist"), entry.resist.keys())?;
        }
        self.check_resist_keys(&format!("{path}.resist"), health.resist.keys())
    }

    fn check_resist_keys<'a>(
        &self,
        path: &str,
        keys: impl Iterator<Item = &'a String>,
    ) -> Result<(), String> {
        for key in keys {
            if self.type_index(key).is_none() {
                return Err(format!(
                    "`{path}.{key}`: `{key}` names no registered damage type"
                ));
            }
        }
        Ok(())
    }
}

fn list_entries<'a>(
    key: &str,
    value: Option<&'a serde_json::Value>,
) -> Result<&'a [serde_json::Value], String> {
    match value {
        None | Some(serde_json::Value::Null) => Ok(&[]),
        Some(serde_json::Value::Array(entries)) => Ok(entries),
        Some(_) => Err(format!("`{key}` must be an array of handles")),
    }
}

/// A `defineMod` list entry: the declaration `defineMod` resolved from a
/// handle, `{ kind, name, ... }`. A bare string or a handle of the other kind
/// is rejected naming `path`.
fn declaration_entry<'a>(
    path: &str,
    entry: &'a serde_json::Value,
    expected_kind: &str,
) -> Result<(String, &'a serde_json::Map<String, serde_json::Value>), String> {
    if let Some(text) = entry.as_str() {
        return Err(format!(
            "`{path}`: `\"{text}\"` is a bare string; list the handle its declaration returned"
        ));
    }
    let object = entry
        .as_object()
        .ok_or_else(|| format!("`{path}`: expected a handle"))?;
    let kind = object
        .get("kind")
        .and_then(|kind| kind.as_str())
        .unwrap_or("");
    if kind != expected_kind {
        let found = if kind.is_empty() {
            "a value with no `kind`".to_string()
        } else {
            format!("a `{kind}` handle")
        };
        return Err(format!(
            "`{path}`: expected a `{expected_kind}` handle, got {found}"
        ));
    }
    let name = object
        .get("name")
        .and_then(|name| name.as_str())
        .ok_or_else(|| format!("`{path}.name` must be a string"))?;
    validate_defense_name(name).map_err(|error| format!("`{path}.name`: {error}"))?;
    Ok((name.to_string(), object))
}

/// The options argument of both definers is reserved: empty today, and an
/// unknown key rejects load.
fn check_reserved_options(
    path: &str,
    object: &serde_json::Map<String, serde_json::Value>,
    definer: &str,
) -> Result<(), String> {
    match object.get("options") {
        None | Some(serde_json::Value::Null) => Ok(()),
        // Luau encodes an empty table as an empty array.
        Some(serde_json::Value::Array(entries)) if entries.is_empty() => Ok(()),
        Some(serde_json::Value::Object(options)) => match options.keys().next() {
            None => Ok(()),
            Some(key) => Err(format!(
                "`{path}.options.{key}`: `{definer}` takes no option `{key}`"
            )),
        },
        Some(_) => Err(format!("`{path}.options` must be an object")),
    }
}

fn parse_matrix(
    path: &str,
    value: Option<&serde_json::Value>,
    layers: &[String],
) -> Result<BTreeMap<String, PoolMultiplier>, String> {
    let object = match value {
        None | Some(serde_json::Value::Null) => return Ok(BTreeMap::new()),
        Some(serde_json::Value::Array(entries)) if entries.is_empty() => return Ok(BTreeMap::new()),
        Some(serde_json::Value::Object(object)) => object,
        Some(_) => {
            return Err(format!(
                "`{path}` must be an object keyed by layer name or `health`"
            ));
        }
    };
    let mut matrix = BTreeMap::new();
    for (key, entry) in object {
        let entry_path = format!("{path}.{key}");
        let is_health = key == HEALTH_POOL;
        if !is_health && !layers.iter().any(|layer| layer == key) {
            return Err(format!(
                "`{entry_path}`: `{key}` names no registered layer and is not `{HEALTH_POOL}`"
            ));
        }
        let multiplier = match entry {
            serde_json::Value::String(text) if text == "bypass" => {
                if is_health {
                    return Err(format!(
                        "`{entry_path}`: `health` cannot be \"bypass\"; use 0"
                    ));
                }
                PoolMultiplier::Bypass
            }
            serde_json::Value::Number(number) => {
                let scale = number.as_f64().unwrap_or(f64::NAN) as f32;
                if !scale.is_finite() || !(MULTIPLIER_RANGE.0..=MULTIPLIER_RANGE.1).contains(&scale)
                {
                    return Err(format!(
                        "`{entry_path}` must be a multiplier in [{}, {}] or \"bypass\", got {scale}",
                        MULTIPLIER_RANGE.0, MULTIPLIER_RANGE.1
                    ));
                }
                PoolMultiplier::Scale(scale)
            }
            other => {
                return Err(format!(
                    "`{entry_path}` must be a multiplier in [{}, {}] or \"bypass\", got {other}",
                    MULTIPLIER_RANGE.0, MULTIPLIER_RANGE.1
                ));
            }
        };
        matrix.insert(key.clone(), multiplier);
    }
    Ok(matrix)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pairs(
        layers: &mut [(f32, PoolMultiplier)],
    ) -> impl Iterator<Item = (&mut f32, PoolMultiplier)> {
        layers
            .iter_mut()
            .map(|(value, multiplier)| (value, *multiplier))
    }

    fn approx(a: f32, b: f32) -> bool {
        (a - b).abs() < 1.0e-3
    }

    #[test]
    fn untyped_hit_spills_in_layer_order() {
        let mut layers = [(30.0, PoolMultiplier::ONE), (50.0, PoolMultiplier::ONE)];
        let outcome = drain_layers(100.0, pairs(&mut layers), 1.0);
        assert_eq!(layers[0].0, 0.0);
        assert_eq!(layers[1].0, 0.0);
        assert!(approx(outcome.health_loss, 20.0));
        assert!(approx(outcome.absorbed, 80.0));
        assert!(outcome.broke);
    }

    #[test]
    fn spillover_carries_raw_not_scaled_remainder() {
        // Shock 20 against shield 30 (×2) and health ×0.75.
        let mut layers = [(30.0, PoolMultiplier::Scale(2.0))];
        let outcome = drain_layers(20.0, pairs(&mut layers), 0.75);
        assert_eq!(layers[0].0, 0.0);
        assert!(approx(outcome.health_loss, 3.75));
        assert!(approx(outcome.points_dealt, 33.75));
        assert!(approx(outcome.absorbed, 30.0));
    }

    #[test]
    fn ballistic_default_spills_raw_remainder() {
        // Shield 30 ×1, armor 50 ×0.8, health 100; 100 raw.
        let mut layers = [
            (30.0, PoolMultiplier::ONE),
            (50.0, PoolMultiplier::Scale(0.8)),
        ];
        let outcome = drain_layers(100.0, pairs(&mut layers), 1.0);
        assert!(approx(outcome.health_loss, 7.5));
        assert!(approx(outcome.points_dealt, 87.5));
        assert!(approx(outcome.absorbed, 80.0));
    }

    #[test]
    fn empty_layer_is_skipped_and_never_breaks() {
        let mut layers = [(0.0, PoolMultiplier::ONE)];
        let outcome = drain_layers(20.0, pairs(&mut layers), 1.0);
        assert!(!outcome.broke);
        assert!(approx(outcome.health_loss, 20.0));
        assert_eq!(outcome.absorbed, 0.0);
    }

    #[test]
    fn bypass_passes_layer_untouched_whatever_its_resist() {
        let mut layers = [
            (30.0, PoolMultiplier::Bypass.resisted(0.0)),
            (0.0, PoolMultiplier::Scale(0.0)),
        ];
        let outcome = drain_layers(20.0, pairs(&mut layers), 1.0);
        assert_eq!(layers[0].0, 30.0);
        assert!(approx(outcome.health_loss, 20.0));
        assert_eq!(outcome.absorbed, 0.0);
        assert!(!outcome.broke);
    }

    #[test]
    fn zero_multiplier_layer_stops_the_rest_of_the_hit() {
        // Flechette against shield 30, armor 50: bypass then ×0.
        let mut layers = [
            (30.0, PoolMultiplier::Bypass),
            (50.0, PoolMultiplier::Scale(0.0)),
        ];
        let outcome = drain_layers(20.0, pairs(&mut layers), 1.0);
        assert_eq!((layers[0].0, layers[1].0), (30.0, 50.0));
        assert_eq!(outcome.points_dealt, 0.0);
        assert!(!outcome.broke);
        // Shield 10 ×1, armor 50 ×0, a 30 hit: the shield's drain stands.
        let mut layers = [
            (10.0, PoolMultiplier::ONE),
            (50.0, PoolMultiplier::Scale(0.0)),
        ];
        let outcome = drain_layers(30.0, pairs(&mut layers), 1.0);
        assert_eq!((layers[0].0, layers[1].0), (0.0, 50.0));
        assert!(approx(outcome.points_dealt, 10.0));
        assert!(approx(outcome.absorbed, 10.0));
        assert!(outcome.broke);
        assert_eq!(outcome.health_loss, 0.0);
    }

    #[test]
    fn layer_resist_scales_that_layer_only() {
        // Shock ×2 with a shield resist of 0.5, health ×0.75, a 40 hit.
        let mut layers = [(30.0, PoolMultiplier::Scale(2.0).resisted(0.5))];
        let outcome = drain_layers(40.0, pairs(&mut layers), 0.75);
        assert!(approx(outcome.health_loss, 7.5));
        assert!(approx(outcome.points_dealt, 37.5));
    }

    #[test]
    fn health_resist_scales_health_only() {
        // Armor 120 ×0.5, health ×0.75 × 0.5, a shock 300.
        let mut layers = [(120.0, PoolMultiplier::Scale(0.5))];
        let outcome = drain_layers(300.0, pairs(&mut layers), 0.75 * 0.5);
        assert_eq!(layers[0].0, 0.0);
        assert!(approx(outcome.health_loss, 22.5));
        assert!(approx(outcome.points_dealt, 142.5));
    }

    #[test]
    fn delay_ticks_are_whole_ticks_at_60_hz() {
        assert_eq!(delay_ticks(3_000.0), 180);
        assert_eq!(delay_ticks(2_500.0), 150);
        assert_eq!(delay_ticks(500.0), 30);
        assert_eq!(delay_ticks(0.0), 0);
    }

    #[test]
    fn layer_recharges_after_delay_at_rate_and_stops_at_capacity() {
        let dt = crate::TICK_DURATION.as_secs_f32();
        // Hit on tick 100, 3000 ms delay: nothing through tick 279.
        assert_eq!(
            recharged_value(0.0, 100.0, 3_000.0, 60.0, Some(100), 279, dt),
            0.0
        );
        let first = recharged_value(0.0, 100.0, 3_000.0, 60.0, Some(100), 280, dt);
        assert!(approx(first, 1.0), "{first}");
        // The same delay after long uptime.
        let late = recharged_value(0.0, 100.0, 3_000.0, 60.0, Some(100_000), 100_180, dt);
        assert!(approx(late, 1.0));
        assert_eq!(recharged_value(99.5, 100.0, 0.0, 60.0, None, 5, dt), 100.0);
        assert_eq!(recharged_value(10.0, 100.0, 0.0, 0.0, None, 5, dt), 10.0);
        // Never hit: recharges at once. Delay 0 recharges the tick after.
        assert!(approx(
            recharged_value(0.0, 80.0, 3_000.0, 60.0, None, 7, dt),
            1.0
        ));
        assert!(approx(
            recharged_value(0.0, 80.0, 0.0, 60.0, Some(7), 8, dt),
            1.0
        ));
    }

    #[test]
    fn defense_names_reject_dots_colons_and_empty() {
        assert!(validate_defense_name("shield").is_ok());
        assert!(validate_defense_name("armor-2_b").is_ok());
        for bad in ["", "a.b", "a:b", "a@b", &"x".repeat(65)] {
            assert!(validate_defense_name(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn handle_ref_parses_both_kinds_and_rejects_strings_and_extras() {
        let layer =
            HandleRef::from_json(&serde_json::json!({"kind": "defenseLayer", "name": "shield"}));
        assert_eq!(layer.unwrap().expect_layer().unwrap(), "shield");
        let ty = HandleRef::from_json(&serde_json::json!({"kind": "damageType", "name": "shock"}))
            .unwrap();
        assert!(ty.expect_layer().is_err());
        assert!(HandleRef::from_json(&serde_json::json!("shield")).is_err());
        assert!(
            HandleRef::from_json(&serde_json::json!({"kind": "damageType", "name": "s", "x": 1}))
                .is_err()
        );
    }

    #[test]
    fn vocabulary_parses_registered_lists_and_default() {
        let vocabulary = DefenseVocabulary::from_manifest(
            Some(&serde_json::json!([
                {"kind": "defenseLayer", "name": "shield", "options": {}},
                {"kind": "defenseLayer", "name": "armor"}
            ])),
            Some(&serde_json::json!([
                {"kind": "damageType", "name": "flechette", "matrix": {"shield": "bypass", "armor": 0, "health": 1.5}, "options": {}}
            ])),
            Some(&serde_json::json!({"kind": "damageType", "name": "flechette"})),
        )
        .unwrap();
        assert_eq!(vocabulary.layers, ["shield", "armor"]);
        assert_eq!(vocabulary.default_damage_type, Some(0));
        let matrix = &vocabulary.damage_types[0].matrix;
        assert_eq!(matrix["shield"], PoolMultiplier::Bypass);
        assert_eq!(matrix["armor"], PoolMultiplier::Scale(0.0));
    }

    #[test]
    fn vocabulary_load_failures_name_the_key_path() {
        let layer = |name: &str| serde_json::json!({"kind": "defenseLayer", "name": name});
        let ty = |name: &str, matrix: serde_json::Value| serde_json::json!({"kind": "damageType", "name": name, "matrix": matrix});
        let cases: Vec<(
            Option<serde_json::Value>,
            Option<serde_json::Value>,
            Option<serde_json::Value>,
            &str,
        )> = vec![
            (
                Some(serde_json::json!([layer("health")])),
                None,
                None,
                "defenseLayers[0]",
            ),
            (
                Some(serde_json::json!([layer("shield"), layer("shield")])),
                None,
                None,
                "defenseLayers[1]",
            ),
            (
                Some(serde_json::json!(["shield"])),
                None,
                None,
                "bare string",
            ),
            (
                Some(serde_json::json!([ty("shock", serde_json::json!({}))])),
                None,
                None,
                "expected a `defenseLayer` handle",
            ),
            (
                None,
                Some(serde_json::json!([layer("shield")])),
                None,
                "expected a `damageType` handle",
            ),
            (
                None,
                Some(serde_json::json!([
                    ty("a", serde_json::json!({})),
                    ty("a", serde_json::json!({}))
                ])),
                None,
                "damageTypes[1]",
            ),
            (
                Some(serde_json::json!([layer("a.b")])),
                None,
                None,
                "defenseLayers[0].name",
            ),
            (
                Some(serde_json::json!([layer("")])),
                None,
                None,
                "defenseLayers[0].name",
            ),
            (
                None,
                Some(serde_json::json!([ty(
                    "s",
                    serde_json::json!({"shield": 2})
                )])),
                None,
                "damageTypes[0].matrix.shield",
            ),
            (
                None,
                Some(serde_json::json!([ty(
                    "s",
                    serde_json::json!({"health": "bypass"})
                )])),
                None,
                "damageTypes[0].matrix.health",
            ),
            (
                None,
                Some(serde_json::json!([ty(
                    "s",
                    serde_json::json!({"health": 65})
                )])),
                None,
                "damageTypes[0].matrix.health",
            ),
            (
                Some(
                    serde_json::json!([{"kind": "defenseLayer", "name": "s", "options": {"recharge": 1}}]),
                ),
                None,
                None,
                "defenseLayers[0].options.recharge",
            ),
            (
                None,
                None,
                Some(serde_json::json!("shock")),
                "defaultDamageType",
            ),
            (
                Some(serde_json::json!([layer("shield")])),
                None,
                Some(serde_json::json!({"kind": "defenseLayer", "name": "shield"})),
                "defaultDamageType",
            ),
            (
                None,
                Some(serde_json::json!([ty("s", serde_json::json!({}))])),
                Some(serde_json::json!({"kind": "damageType", "name": "t"})),
                "not listed",
            ),
        ];
        for (layers, types, default, needle) in cases {
            let error =
                DefenseVocabulary::from_manifest(layers.as_ref(), types.as_ref(), default.as_ref())
                    .expect_err(needle);
            assert!(
                error.contains(needle),
                "`{error}` should mention `{needle}`"
            );
        }
    }
}
