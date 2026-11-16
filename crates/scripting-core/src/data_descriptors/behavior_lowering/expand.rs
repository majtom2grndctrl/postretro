// Stance expansion: each level's reached rests and strikes, lowered to activities and rows.
// See: context/lib/entity_model.md §7c (Stances and attacks)

use std::collections::{BTreeMap, BTreeSet};

use postretro_foundation::stance_defaults::{DEFAULT_RECOVERY_MS, STANCE_HOLD_FLOOR_MS};
use postretro_foundation::{
    ATTACK_TURN_HELD_INPUT_PREFIX, BRAIN_ANIMATION_DONE_INPUT, BRAIN_DAMAGE_IN_ACTIVITY_INPUT,
    BRAIN_TIME_IN_ACTIVITY_MS_INPUT,
};
use serde_json::{Map, Value, json};

use super::declarations::{At, Catalog, StanceDecl, StrikeDecl};
use super::floor::warn_below_floor;
use super::{DescriptorError, handle_kind, kind_of, shape};

/// The stances one level rests in and the attacks struck from each rest.
#[derive(Default)]
pub(super) struct Reach {
    pub(super) rests: BTreeSet<String>,
    /// Each rest's struck attacks, in the order its rows first strike them.
    pub(super) strikes: BTreeMap<String, Vec<String>>,
    /// The first row to strike an attack from anywhere but its beginning
    /// stance's rest: `(row path, source, attack)`. Expansion rejects it once
    /// name collisions, which explain such a row better, are ruled out.
    pub(super) misplaced: Option<(String, String, String)>,
}

/// The activity names authored at this level.
pub(super) fn authored_activities(envelope: &Map<String, Value>) -> BTreeSet<String> {
    envelope
        .get("activities")
        .and_then(Value::as_object)
        .map(|activities| activities.keys().cloned().collect())
        .unwrap_or_default()
}

/// Whether `source` keys a stance's rest rows: it names a declared stance and
/// no authored activity.
pub(super) fn is_rest_key(source: &str, authored: &BTreeSet<String>, catalog: &Catalog) -> bool {
    !authored.contains(source) && catalog.stances.contains_key(source)
}

/// The rests this level reaches and the strikes made from them, to a fixed
/// point: the stances `initial` and the authored and `"*"` rows take, then
/// those a reached rest's rows take and every struck attack's ending stance.
pub(super) fn reach(envelope: &Map<String, Value>, path: &str, catalog: &Catalog) -> Reach {
    let authored = authored_activities(envelope);
    let transitions = envelope.get("transitions").and_then(Value::as_object);
    let mut reach = Reach::default();
    let mut pending = Vec::new();
    if let Some(name) = stance_target(envelope.get("initial")) {
        pending.push(name);
    }
    for (source, rows) in transitions.into_iter().flatten() {
        if source != "*" && !authored.contains(source) {
            continue;
        }
        for (index, row) in rows.as_array().into_iter().flatten().enumerate() {
            let target = row.get("to");
            if let Some(name) = stance_target(target) {
                pending.push(name);
            } else if handle_kind(target.unwrap_or(&Value::Null)) == Some("attack") {
                let row_path = format!("{path}.transitions.{source}[{index}]");
                reach.misplace(row_path, source, handle_name(target));
            }
        }
    }
    while let Some(rest) = pending.pop() {
        if !reach.rests.insert(rest.clone()) {
            continue;
        }
        let rows = transitions
            .filter(|_| is_rest_key(&rest, &authored, catalog))
            .and_then(|transitions| transitions.get(&rest))
            .and_then(Value::as_array);
        for (index, row) in rows.into_iter().flatten().enumerate() {
            let target = row.get("to");
            if let Some(name) = stance_target(target) {
                pending.push(name);
                continue;
            }
            if handle_kind(target.unwrap_or(&Value::Null)) != Some("attack") {
                continue;
            }
            let attack = handle_name(target);
            let row_path = format!("{path}.transitions.{rest}[{index}]");
            let strike = match catalog.strikes.get(&attack) {
                Some(strike) if strike.beginning == rest => strike,
                _ => {
                    reach.misplace(row_path, &rest, attack);
                    continue;
                }
            };
            let struck = reach.strikes.entry(rest.clone()).or_default();
            if !struck.contains(&attack) {
                struck.push(attack);
            }
            pending.push(strike.ending.clone());
        }
    }
    reach
}

impl Reach {
    fn misplace(&mut self, row_path: String, source: &str, attack: String) {
        self.misplaced
            .get_or_insert_with(|| (row_path, source.to_string(), attack));
    }
}

/// Expand one level's stances and strikes into activities and rows, and lower
/// its row targets and `initial` to the names it generates. `ancestor_exit`
/// says whether an outer level has a row that can leave this one. Returns the
/// attacks struck at this level.
pub(super) fn expand_level(
    envelope: &mut Map<String, Value>,
    path: &str,
    catalog: &Catalog,
    ancestor_exit: bool,
) -> Result<BTreeSet<String>, DescriptorError> {
    reject_unreadable_activities(envelope, path)?;
    let reach = reach(envelope, path, catalog);
    let authored = authored_activities(envelope);
    let generated = generated_names(&reach, &authored, path)?;
    if let Some((row_path, source, attack)) = &reach.misplaced {
        return Err(strike_outside_its_rest(row_path, source, attack, catalog));
    }
    reject_unreached_rest_rows(envelope, path, &authored, &reach, catalog)?;
    reject_names_of_generated(envelope, path, &generated, &reach)?;
    reject_unknown_sources(envelope, path, &authored, catalog)?;

    let star_targets = star_targets(envelope);
    let mut activities = Map::new();
    let mut rows = Map::new();
    let mut struck = BTreeSet::new();
    for rest in &reach.rests {
        let stance = &catalog.stances[rest];
        let break_row = break_row(stance, rest, path, &authored, &generated)?;
        let authored_rows = take_rows(envelope, rest);
        // The engine skips a `"*"` row into the pose the enemy is in.
        let star_exit = star_targets.iter().any(|target| target != rest);
        if authored_rows.is_empty() && !star_exit && !ancestor_exit {
            return Err(shape(format!(
                "`{path}`: stance \"{rest}\"'s rest has no way out; give it a row of its own (`transitions.{rest}`), or a `\"*\"` or outer-level row to somewhere else (a `\"*\"` row into the stance itself is skipped while the enemy holds it, and its `breakOn` row does not count, since an enemy nobody shoots would hold the pose forever)"
            )));
        }
        activities.insert(rest.clone(), rest_activity(stance, rest));
        let mut rest_rows: Vec<Value> = break_row.iter().cloned().collect();
        rest_rows.extend(authored_rows.into_iter().map(|row| lower_row(row, rest)));
        rows.insert(rest.clone(), Value::Array(rest_rows));

        for attack in reach.strikes.get(rest).into_iter().flatten() {
            let strike = &catalog.strikes[attack];
            if stance.hold_authored && stance.hold_ms[0] < f64::from(STANCE_HOLD_FLOOR_MS) {
                warn_below_floor(rest, stance.hold_ms[0]);
            }
            struck.insert(attack.clone());
            let hold = hold_name(attack, rest);
            let release = format!("{attack}_release");
            let recover = format!("{attack}_recover");
            activities.insert(hold.clone(), hold_activity(stance, rest, attack));
            let mut hold_rows: Vec<Value> = break_row.iter().cloned().collect();
            hold_rows.push(json!({ "to": rest, "when": not(turn_held(attack)) }));
            hold_rows.push(json!({ "to": release, "releaseAfter": stance.hold_ms }));
            rows.insert(hold, Value::Array(hold_rows));
            activities.insert(release.clone(), release_activity(strike, attack));
            rows.insert(release, json!([release_row(&recover, strike.release.ms)]));
            activities.insert(recover.clone(), recover_activity(strike, attack));
            rows.insert(recover, json!([recover_row(&strike.ending, strike)]));
        }
    }

    lower_remaining_targets(envelope);
    if !activities.is_empty() {
        insert_all(envelope, "activities", activities);
        insert_all(envelope, "transitions", rows);
    }
    Ok(struck)
}

/// Rows keyed by a stance no row or `initial` ever reaches are dead: they
/// reject load. Under an `attacks` list, pruning has already dropped the rows
/// of rests only its drops left unreached, and kept these for this check.
fn reject_unreached_rest_rows(
    envelope: &Map<String, Value>,
    path: &str,
    authored: &BTreeSet<String>,
    reach: &Reach,
    catalog: &Catalog,
) -> Result<(), DescriptorError> {
    let transitions = envelope.get("transitions").and_then(Value::as_object);
    for source in transitions.into_iter().flat_map(Map::keys) {
        if is_rest_key(source, authored, catalog) && !reach.rests.contains(source) {
            return Err(shape(format!(
                "`{path}.transitions.{source}` keys rows by stance \"{source}\", which no row or `initial` at this level reaches"
            )));
        }
    }
    Ok(())
}

/// Every name this level generates, with what generates it, checked unique
/// and clear of the authored activities.
fn generated_names(
    reach: &Reach,
    authored: &BTreeSet<String>,
    path: &str,
) -> Result<BTreeMap<String, String>, DescriptorError> {
    let mut generated: BTreeMap<String, String> = BTreeMap::new();
    let mut claim = |name: String, what: String| -> Result<(), DescriptorError> {
        if authored.contains(&name) {
            return Err(shape(format!(
                "`{path}.activities.{name}` collides with {what}, which lowering generates at this level; rename the activity"
            )));
        }
        if let Some(first) = generated.get(&name) {
            return Err(shape(format!(
                "`{path}`: {what} and {first} would both be named \"{name}\"; rename a stance or an attack"
            )));
        }
        generated.insert(name, what);
        Ok(())
    };
    for rest in &reach.rests {
        claim(rest.clone(), format!("stance \"{rest}\"'s rest"))?;
    }
    for (rest, attacks) in &reach.strikes {
        for attack in attacks {
            claim(
                hold_name(attack, rest),
                format!("attack \"{attack}\"'s hold in stance \"{rest}\""),
            )?;
            claim(
                format!("{attack}_release"),
                format!("attack \"{attack}\"'s release"),
            )?;
            claim(
                format!("{attack}_recover"),
                format!("attack \"{attack}\"'s recover"),
            )?;
        }
    }
    Ok(generated)
}

/// Authors reach generated activities only through handles: a string `to` or
/// `initial` naming one, or rows keyed by a hold, release or recover, reject.
fn reject_names_of_generated(
    envelope: &Map<String, Value>,
    path: &str,
    generated: &BTreeMap<String, String>,
    reach: &Reach,
) -> Result<(), DescriptorError> {
    let names = |field: &str, name: &str| {
        generated.get(name).map(|what| {
            shape(format!(
                "`{field}` names \"{name}\", {what}, which lowering generates; target the stance or attack handle instead"
            ))
        })
    };
    if let Some(Value::String(initial)) = envelope.get("initial")
        && let Some(error) = names(&format!("{path}.initial"), initial)
    {
        return Err(error);
    }
    let transitions = envelope.get("transitions").and_then(Value::as_object);
    for (source, rows) in transitions.into_iter().flatten() {
        if !reach.rests.contains(source)
            && let Some(what) = generated.get(source)
        {
            return Err(shape(format!(
                "`{path}.transitions.{source}` keys rows by {what}, which lowering generates with its own rows"
            )));
        }
        for (index, row) in rows.as_array().into_iter().flatten().enumerate() {
            if let Some(Value::String(to)) = row.get("to")
                && let Some(error) = names(&format!("{path}.transitions.{source}[{index}].to"), to)
            {
                return Err(error);
            }
        }
    }
    Ok(())
}

/// A level's `activities` must be an object; only a level that opens on a
/// stance handle may leave it out, and lowering writes it. Without this, a
/// misspelled or mistyped `activities` reads as a level with no authored
/// activities, and the checks after it blame the rows keyed by them
/// (`transitions.idle` naming no activity, a stance's rows never reached)
/// before serde's unknown-field check ever runs.
fn reject_unreadable_activities(
    envelope: &Map<String, Value>,
    path: &str,
) -> Result<(), DescriptorError> {
    match envelope.get("activities") {
        Some(Value::Object(_)) => Ok(()),
        Some(other) => Err(shape(format!(
            "`{path}.activities` must be an object of activities keyed by name, found {}",
            kind_of(other)
        ))),
        None if stance_target(envelope.get("initial")).is_some() => Ok(()),
        None => Err(shape(format!(
            "`{path}.activities` is missing; a level declares its activities there (check the key's spelling), and only one whose `initial` is a stance handle may leave it out"
        ))),
    }
}

/// Rows key an authored activity, a stance's rest or `"*"`. Any other key
/// would leave its rows' handle targets unlowered for serde to reject with no
/// path.
fn reject_unknown_sources(
    envelope: &Map<String, Value>,
    path: &str,
    authored: &BTreeSet<String>,
    catalog: &Catalog,
) -> Result<(), DescriptorError> {
    let transitions = envelope.get("transitions").and_then(Value::as_object);
    for source in transitions.into_iter().flat_map(Map::keys) {
        if source != "*" && !authored.contains(source) && !is_rest_key(source, authored, catalog) {
            // A level that opens on a stance may leave `activities` out, so a
            // misspelled `activities` lands here: say so rather than blame the row.
            let hint = if envelope.contains_key("activities") {
                ""
            } else {
                " (this level declares no `activities`; check that key's spelling)"
            };
            return Err(shape(format!(
                "`{path}.transitions.{source}` does not name a declared activity or stance at this level; key rows by an activity, a stance's name or `\"*\"`{hint}"
            )));
        }
    }
    Ok(())
}

/// The stance's `breakOn` row, first on its rest and every hold: it names a
/// sibling activity the author wrote.
fn break_row(
    stance: &StanceDecl,
    rest: &str,
    path: &str,
    authored: &BTreeSet<String>,
    generated: &BTreeMap<String, String>,
) -> Result<Option<Value>, DescriptorError> {
    let Some(break_on) = &stance.break_on else {
        return Ok(None);
    };
    let field = format!("{}.breakOn.to", stance.path);
    if let Some(what) = generated.get(&break_on.to) {
        return Err(shape(format!(
            "`{field}`: stance \"{rest}\" breaks to \"{}\", {what}, which lowering generates at `{path}`; break to an activity the level declares",
            break_on.to
        )));
    }
    if !authored.contains(&break_on.to) {
        return Err(shape(format!(
            "`{field}`: stance \"{rest}\" breaks to \"{}\", which names no activity at `{path}`; `breakOn.to` takes a sibling activity name",
            break_on.to
        )));
    }
    Ok(Some(json!({
        "to": break_on.to,
        "when": ge(BRAIN_DAMAGE_IN_ACTIVITY_INPUT, break_on.damage),
    })))
}

fn rest_activity(stance: &StanceDecl, rest: &str) -> Value {
    let mut activity = json!({ "animation": stance.animation, "motion": "hold", "pose": rest });
    if let Some(sound) = &stance.sound {
        activity["sound"] = json!(sound);
    }
    activity
}

fn hold_activity(stance: &StanceDecl, rest: &str, attack: &str) -> Value {
    json!({ "animation": stance.animation, "motion": "hold", "pose": rest, "attackTurn": attack })
}

fn release_activity(strike: &StrikeDecl, attack: &str) -> Value {
    let release = &strike.release;
    let action = match &release.at {
        At::Default => json!({ "attack": attack, "at": "hit", "atDefault": true }),
        At::Named(event) => json!({ "attack": attack, "at": event }),
        At::OnEntry => json!({ "attack": attack }),
    };
    let mut activity = json!({
        "animation": release.animation,
        "action": action,
        "attackTurn": attack,
        "motion": release.motion.as_deref().unwrap_or("hold"),
        "staggerTo": strike.ending,
    });
    if let Some(speed) = release.speed {
        activity["speed"] = json!(speed);
    }
    activity
}

fn recover_activity(strike: &StrikeDecl, attack: &str) -> Value {
    let animation = strike
        .recover
        .as_ref()
        .and_then(|recover| recover.animation.as_ref())
        .unwrap_or(&strike.release.animation);
    json!({
        "animation": animation,
        "attackTurn": attack,
        "motion": "hold",
        "staggerTo": strike.ending,
    })
}

/// The release's one row: closed until its moment resolves, or with no
/// moment until its clip plays a pass; an `ms` only holds it longer.
fn release_row(recover: &str, ms: Option<f64>) -> Value {
    let mut row = json!({ "to": recover, "afterMoment": true });
    if let Some(ms) = ms {
        row["when"] = ge(BRAIN_TIME_IN_ACTIVITY_MS_INPUT, ms);
    }
    row
}

/// The recover's one row into the ending stance's rest: after its authored
/// `ms`, or else once its clip plays through or the default duration passes.
fn recover_row(rest: &str, strike: &StrikeDecl) -> Value {
    let when = match strike.recover.as_ref().and_then(|recover| recover.ms) {
        Some(ms) => ge(BRAIN_TIME_IN_ACTIVITY_MS_INPUT, ms),
        None => json!({
            "op": "or",
            "a": input(BRAIN_ANIMATION_DONE_INPUT),
            "b": ge(BRAIN_TIME_IN_ACTIVITY_MS_INPUT, f64::from(DEFAULT_RECOVERY_MS)),
        }),
    };
    json!({ "to": rest, "when": when })
}

/// A rest's authored row, its handle target lowered: a stance to its rest, an
/// attack to its hold in this rest, queued for the attack's turn.
fn lower_row(mut row: Value, rest: &str) -> Value {
    let target = row.get("to");
    let kind = target.and_then(handle_kind).map(str::to_string);
    let name = handle_name(target);
    match kind.as_deref() {
        Some("stance") => row["to"] = json!(name),
        Some("attack") => {
            row["to"] = json!(hold_name(&name, rest));
            row["queue"] = json!(name);
        }
        _ => {}
    }
    row
}

/// Lower the stance handles left on the authored and `"*"` rows and on
/// `initial` to their rests' names.
fn lower_remaining_targets(envelope: &mut Map<String, Value>) {
    if let Some(initial) = envelope.get_mut("initial")
        && let Some(name) = stance_target(Some(initial))
    {
        *initial = json!(name);
    }
    let transitions = envelope
        .get_mut("transitions")
        .and_then(Value::as_object_mut);
    for rows in transitions.into_iter().flat_map(|map| map.values_mut()) {
        for row in rows.as_array_mut().into_iter().flatten() {
            if let Some(target) = row.get_mut("to")
                && let Some(name) = stance_target(Some(target))
            {
                *target = json!(name);
            }
        }
    }
}

/// Remove and return the authored rows keyed by `source`.
fn take_rows(envelope: &mut Map<String, Value>, source: &str) -> Vec<Value> {
    envelope
        .get_mut("transitions")
        .and_then(Value::as_object_mut)
        .and_then(|transitions| transitions.remove(source))
        .and_then(|rows| match rows {
            Value::Array(rows) => Some(rows),
            _ => None,
        })
        .unwrap_or_default()
}

fn insert_all(envelope: &mut Map<String, Value>, field: &str, entries: Map<String, Value>) {
    let slot = envelope
        .entry(field.to_string())
        .or_insert_with(|| Value::Object(Map::new()));
    if let Some(map) = slot.as_object_mut() {
        map.extend(entries);
    }
}

/// The activity names this level's `"*"` rows target, stances by name.
pub(super) fn star_targets(envelope: &Map<String, Value>) -> BTreeSet<String> {
    let rows = envelope
        .get("transitions")
        .and_then(|transitions| transitions.get("*"))
        .and_then(Value::as_array);
    rows.into_iter()
        .flatten()
        .filter_map(|row| match row.get("to") {
            Some(Value::String(name)) => Some(name.clone()),
            target => stance_target(target),
        })
        .collect()
}

/// Whether this level has a non-empty row list keyed by `source`.
pub(super) fn has_rows(envelope: &Map<String, Value>, source: &str) -> bool {
    envelope
        .get("transitions")
        .and_then(|transitions| transitions.get(source))
        .and_then(Value::as_array)
        .is_some_and(|rows| !rows.is_empty())
}

fn strike_outside_its_rest(
    row_path: &str,
    source: &str,
    attack: &str,
    catalog: &Catalog,
) -> DescriptorError {
    match catalog.strikes.get(attack) {
        None => shape(format!(
            "`{row_path}.to` strikes attack \"{attack}\", which names no `beginningStance`; a row strikes only an attack that begins in a stance"
        )),
        Some(strike) => shape(format!(
            "`{row_path}.to` strikes attack \"{attack}\" from `{source}`; a strike leaves only from its beginning stance \"{}\"'s rest, keyed `transitions.{}`",
            strike.beginning, strike.beginning
        )),
    }
}

fn stance_target(target: Option<&Value>) -> Option<String> {
    target
        .filter(|target| handle_kind(target) == Some("stance"))
        .map(|target| handle_name(Some(target)))
}

fn handle_name(handle: Option<&Value>) -> String {
    handle
        .and_then(|handle| handle.get("name"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

fn hold_name(attack: &str, rest: &str) -> String {
    format!("{attack}_{rest}")
}

fn input(name: &str) -> Value {
    json!({ "op": "input", "name": name })
}

fn ge(name: &str, value: f64) -> Value {
    json!({ "op": "ge", "a": input(name), "b": { "op": "const", "value": value } })
}

fn not(node: Value) -> Value {
    json!({ "op": "not", "x": node })
}

fn turn_held(attack: &str) -> Value {
    input(&format!("{ATTACK_TURN_HELD_INPUT_PREFIX}{attack}"))
}
