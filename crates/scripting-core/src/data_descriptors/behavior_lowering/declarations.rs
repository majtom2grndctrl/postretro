// Stance and attack declarations parsed from their handles, each field checked with its path.
// See: context/lib/scripting.md §2 (Attack and stance declarations)

use std::collections::BTreeMap;

use postretro_foundation::is_clip_event_name;
use postretro_foundation::stance_defaults::DEFAULT_HOLD_MS;
use serde_json::{Map, Value};

use super::{Declared, DescriptorError, kind_of, shape};

/// The release motions a strike may take. A position goal cannot carry an
/// attack, so `moveToAnchor`, `moveToLastKnown` and `patrol` are left out.
const RELEASE_MOTIONS: &[&str] = &["hold", "chaseTarget", "strafe", "reposition"];

const STANCE_OPTIONS: &[&str] = &[
    "kind",
    "name",
    "animation",
    "sound",
    "stance",
    "holdMs",
    "breakOn",
];
const BREAK_ON_FIELDS: &[&str] = &["damage", "to"];
const RELEASE_FIELDS: &[&str] = &["animation", "ms", "at", "motion", "speed"];
const RECOVER_FIELDS: &[&str] = &["animation", "ms"];

/// Every stance and attack a behavior's handles declare, by name.
pub(super) struct Catalog {
    pub(super) stances: BTreeMap<String, StanceDecl>,
    /// Attacks that begin in a stance, by name: the ones a row may strike.
    pub(super) strikes: BTreeMap<String, StrikeDecl>,
}

pub(super) struct StanceDecl {
    pub(super) animation: String,
    pub(super) sound: Option<String>,
    /// `[min, max]` milliseconds a hold draws its release from.
    pub(super) hold_ms: [f64; 2],
    /// Whether `hold_ms` was authored rather than defaulted.
    pub(super) hold_authored: bool,
    pub(super) break_on: Option<BreakOn>,
    /// Where the stance's handle was first seen, for its errors.
    pub(super) path: String,
}

pub(super) struct BreakOn {
    pub(super) damage: f64,
    pub(super) to: String,
}

/// An attack's strike: the stances it begins and ends in, its release and its
/// optional recover.
pub(super) struct StrikeDecl {
    pub(super) beginning: String,
    pub(super) ending: String,
    pub(super) release: Release,
    pub(super) recover: Option<Recover>,
}

pub(super) struct Release {
    pub(super) animation: String,
    pub(super) ms: Option<f64>,
    pub(super) at: At,
    pub(super) motion: Option<String>,
    pub(super) speed: Option<f64>,
}

/// When a release fires.
pub(super) enum At {
    /// Omitted: at `hit` when the clip carries it, else on entry (`atDefault`).
    Default,
    Named(String),
    /// `at: false`: on entry.
    OnEntry,
}

pub(super) struct Recover {
    pub(super) animation: Option<String>,
    pub(super) ms: Option<f64>,
}

impl Catalog {
    /// Parse every stance and attack declaration the behavior's handles carry.
    pub(super) fn parse(
        stances: &BTreeMap<String, Declared>,
        attacks: &BTreeMap<String, Declared>,
    ) -> Result<Self, DescriptorError> {
        let mut catalog = Catalog {
            stances: BTreeMap::new(),
            strikes: BTreeMap::new(),
        };
        for (name, declared) in stances {
            catalog
                .stances
                .insert(name.clone(), parse_stance(name, declared)?);
        }
        for (name, declared) in attacks {
            if let Some(strike) = parse_strike(name, declared, &catalog)? {
                catalog.strikes.insert(name.clone(), strike);
            }
        }
        Ok(catalog)
    }
}

fn parse_stance(name: &str, declared: &Declared) -> Result<StanceDecl, DescriptorError> {
    let path = &declared.path;
    let fields = &declared.declaration;
    reject_unknown(fields, STANCE_OPTIONS, path, || {
        format!("stance \"{name}\" takes `animation`, `sound`, `stance`, `holdMs` and `breakOn`")
    })?;
    let animation = required_string(fields, "animation", path, || format!("stance \"{name}\""))?;
    let sound = optional_string(fields, "sound", path)?;
    if present(fields, "stance").is_some() {
        // Seam: `E10--enemy-cover` defines the activity `stance` property this
        // option sets; whichever of the two briefs lands second wires it.
        return Err(shape(format!(
            "`{path}.stance`: stance \"{name}\" sets the body stance, which needs crouching \
             enemies, which the engine does not support yet; remove the option"
        )));
    }
    let (hold_ms, hold_authored) = match present(fields, "holdMs") {
        None => (DEFAULT_HOLD_MS.map(f64::from), false),
        Some(value) => (hold_range(value, &format!("{path}.holdMs"), name)?, true),
    };
    let break_on = match present(fields, "breakOn") {
        None => None,
        Some(value) => Some(break_on(value, &format!("{path}.breakOn"), name)?),
    };
    Ok(StanceDecl {
        animation,
        sound,
        hold_ms,
        hold_authored,
        break_on,
        path: path.clone(),
    })
}

/// `holdMs`: one value or `[min, max]`, never negative, min at most max.
fn hold_range(value: &Value, path: &str, stance: &str) -> Result<[f64; 2], DescriptorError> {
    let [min, max] = match value {
        Value::Number(_) => {
            let fixed = number(value, path)?;
            [fixed, fixed]
        }
        Value::Array(bounds) if bounds.len() == 2 => [
            number(&bounds[0], &format!("{path}[0]"))?,
            number(&bounds[1], &format!("{path}[1]"))?,
        ],
        other => {
            return Err(shape(format!(
                "`{path}`: stance \"{stance}\"'s `holdMs` must be one number of milliseconds or `[min, max]`, found {}",
                kind_of(other)
            )));
        }
    };
    if min < 0.0 || max < 0.0 {
        return Err(shape(format!(
            "`{path}`: stance \"{stance}\"'s `holdMs` bounds must be >= 0, got [{min}, {max}]"
        )));
    }
    if min > max {
        return Err(shape(format!(
            "`{path}`: stance \"{stance}\"'s `holdMs` minimum {min} exceeds its maximum {max}"
        )));
    }
    Ok([min, max])
}

fn break_on(value: &Value, path: &str, stance: &str) -> Result<BreakOn, DescriptorError> {
    let Value::Object(fields) = value else {
        return Err(shape(format!(
            "`{path}`: stance \"{stance}\"'s `breakOn` must be `{{ damage, to }}`, found {}",
            kind_of(value)
        )));
    };
    reject_unknown(fields, BREAK_ON_FIELDS, path, || {
        "`breakOn` takes `damage` and `to`".to_string()
    })?;
    let damage_path = format!("{path}.damage");
    let damage = match present(fields, "damage") {
        Some(value) => number(value, &damage_path)?,
        None => {
            return Err(shape(format!(
                "`{damage_path}` is missing: stance \"{stance}\"'s `breakOn` needs the damage that breaks it"
            )));
        }
    };
    if damage <= 0.0 {
        return Err(shape(format!(
            "`{damage_path}`: stance \"{stance}\"'s `breakOn.damage` must be > 0, got {damage}"
        )));
    }
    let to = required_string(fields, "to", path, || {
        format!("stance \"{stance}\"'s `breakOn`")
    })?;
    Ok(BreakOn { damage, to })
}

/// An attack's strike, or `None` for an attack that begins in no stance.
fn parse_strike(
    name: &str,
    declared: &Declared,
    catalog: &Catalog,
) -> Result<Option<StrikeDecl>, DescriptorError> {
    let path = &declared.path;
    let fields = &declared.declaration;
    let beginning = present(fields, "beginningStance").map(stance_name);
    let ending = present(fields, "endingStance").map(stance_name);
    let release = present(fields, "release");
    let recover = present(fields, "recover");
    let Some(beginning) = beginning else {
        for (field, given) in [
            ("release", release.is_some()),
            ("endingStance", ending.is_some()),
            ("recover", recover.is_some()),
        ] {
            if given {
                return Err(shape(format!(
                    "`{path}.{field}`: attack \"{name}\" has a `{field}` but no `beginningStance`; a strike begins in a stance"
                )));
            }
        }
        return Ok(None);
    };
    let Some(release) = release else {
        return Err(shape(format!(
            "`{path}.release` is missing: attack \"{name}\" begins in stance \"{beginning}\", so it needs a `release`"
        )));
    };
    let release = parse_release(release, &format!("{path}.release"), name)?;
    let recover = match recover {
        None => None,
        Some(value) => Some(parse_recover(value, &format!("{path}.recover"), name)?),
    };
    // A recover without its own animation holds the release's state, which
    // rate-scales only while an activity steers: after a moving release its
    // stride would play in place. A planted release may fall back.
    if release
        .motion
        .as_deref()
        .is_some_and(|motion| motion != "hold")
    {
        match &recover {
            None => {
                return Err(shape(format!(
                    "`{path}.recover` is missing: attack \"{name}\"'s release moves, and the default recover holds the release's state, so its stride would play in place; give it a `recover`"
                )));
            }
            Some(recover) if recover.animation.is_none() => {
                return Err(shape(format!(
                    "`{path}.recover.animation` is missing: attack \"{name}\"'s release moves, and a recover without `animation` holds the release's state, so its stride would play in place; give the recover its own animation"
                )));
            }
            Some(_) => {}
        }
    }
    if catalog
        .stances
        .get(&beginning)
        .is_some_and(|stance| stance.animation == release.animation)
    {
        return Err(shape(format!(
            "`{path}.release.animation`: attack \"{name}\"'s release plays \"{}\", its beginning stance's own state; its clip time would include the wait in line, so give the release its own state",
            release.animation
        )));
    }
    Ok(Some(StrikeDecl {
        ending: ending.unwrap_or_else(|| beginning.clone()),
        beginning,
        release,
        recover,
    }))
}

fn parse_release(value: &Value, path: &str, attack: &str) -> Result<Release, DescriptorError> {
    let Value::Object(fields) = value else {
        return Err(shape(format!(
            "`{path}`: attack \"{attack}\"'s `release` must be an object, found {}",
            kind_of(value)
        )));
    };
    reject_unknown(fields, RELEASE_FIELDS, path, || {
        "`release` takes `animation`, `ms`, `at`, `motion` and `speed`".to_string()
    })?;
    let animation = required_string(fields, "animation", path, || {
        format!("attack \"{attack}\"'s `release`")
    })?;
    let ms = optional_duration(fields, "ms", path)?;
    let at = match present(fields, "at") {
        None => At::Default,
        Some(Value::Bool(false)) => At::OnEntry,
        Some(Value::String(event)) if is_clip_event_name(event) => At::Named(event.clone()),
        Some(Value::String(event)) => {
            return Err(shape(format!(
                "`{path}.at` (\"{event}\") must name a clip event matching `[A-Za-z_][A-Za-z0-9_]*`"
            )));
        }
        Some(other) => {
            return Err(shape(format!(
                "`{path}.at` must name a clip event, or be `false` to fire on entry; found {}",
                describe(other)
            )));
        }
    };
    let motion = match present(fields, "motion") {
        None => None,
        Some(Value::String(motion)) if RELEASE_MOTIONS.contains(&motion.as_str()) => {
            Some(motion.clone())
        }
        Some(other) => {
            return Err(shape(format!(
                "`{path}.motion` must be one of {}, found {}; a position goal cannot carry an attack",
                RELEASE_MOTIONS.join(", "),
                describe(other)
            )));
        }
    };
    let speed = match present(fields, "speed") {
        None => None,
        Some(value) => {
            let speed = number(value, &format!("{path}.speed"))?;
            if speed <= 0.0 {
                return Err(shape(format!(
                    "`{path}.speed` must be a multiplier > 0, got {speed}"
                )));
            }
            Some(speed)
        }
    };
    Ok(Release {
        animation,
        ms,
        at,
        motion,
        speed,
    })
}

fn parse_recover(value: &Value, path: &str, attack: &str) -> Result<Recover, DescriptorError> {
    let Value::Object(fields) = value else {
        return Err(shape(format!(
            "`{path}`: attack \"{attack}\"'s `recover` must be an object, found {}",
            kind_of(value)
        )));
    };
    reject_unknown(fields, RECOVER_FIELDS, path, || {
        "`recover` takes `animation` and `ms`".to_string()
    })?;
    Ok(Recover {
        animation: optional_string(fields, "animation", path)?,
        ms: optional_duration(fields, "ms", path)?,
    })
}

/// A field that is present and not nil.
fn present<'a>(fields: &'a Map<String, Value>, key: &str) -> Option<&'a Value> {
    fields.get(key).filter(|value| !value.is_null())
}

fn stance_name(handle: &Value) -> String {
    handle
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

fn reject_unknown(
    fields: &Map<String, Value>,
    allowed: &[&str],
    path: &str,
    takes: impl Fn() -> String,
) -> Result<(), DescriptorError> {
    match fields.keys().find(|key| !allowed.contains(&key.as_str())) {
        Some(key) => Err(shape(format!(
            "`{path}.{key}` is not an option: {}",
            takes()
        ))),
        None => Ok(()),
    }
}

fn required_string(
    fields: &Map<String, Value>,
    key: &str,
    path: &str,
    owner: impl Fn() -> String,
) -> Result<String, DescriptorError> {
    match optional_string(fields, key, path)? {
        Some(text) => Ok(text),
        None => Err(shape(format!(
            "`{path}.{key}` is missing: {} needs one",
            owner()
        ))),
    }
}

fn optional_string(
    fields: &Map<String, Value>,
    key: &str,
    path: &str,
) -> Result<Option<String>, DescriptorError> {
    match present(fields, key) {
        None => Ok(None),
        Some(Value::String(text)) if !text.is_empty() => Ok(Some(text.clone())),
        Some(other) => Err(shape(format!(
            "`{path}.{key}` must be a non-empty string, found {}",
            describe(other)
        ))),
    }
}

/// An optional millisecond duration, never negative.
fn optional_duration(
    fields: &Map<String, Value>,
    key: &str,
    path: &str,
) -> Result<Option<f64>, DescriptorError> {
    let Some(value) = present(fields, key) else {
        return Ok(None);
    };
    let field = format!("{path}.{key}");
    let ms = number(value, &field)?;
    if ms < 0.0 {
        return Err(shape(format!(
            "`{field}` must be >= 0 milliseconds, got {ms}"
        )));
    }
    Ok(Some(ms))
}

/// A number finite at the runtime's `f32`. The JSON bridge already rejects
/// non-finite numbers with their path; this names a non-number, and one too
/// large for `f32`, which would otherwise lower to an infinite constant.
fn number(value: &Value, path: &str) -> Result<f64, DescriptorError> {
    match value.as_f64() {
        Some(number) if (number as f32).is_finite() => Ok(number),
        Some(number) => Err(shape(format!(
            "`{path}` must be a finite number, found {number}, which overflows single precision"
        ))),
        None => Err(shape(format!(
            "`{path}` must be a finite number, found {}",
            describe(value)
        ))),
    }
}

fn describe(value: &Value) -> String {
    match value {
        Value::Bool(flag) => format!("`{flag}`"),
        Value::String(text) => format!("\"{text}\""),
        other => kind_of(other).to_string(),
    }
}
