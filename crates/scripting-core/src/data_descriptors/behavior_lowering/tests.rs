// Behavior lowering on the JSON both runtimes hand it: handle resolution, the
// inferred and exhaustive `attacks` set, row drops, and the pathed rejections.
// Runtime parity and the SDK surface are proved through the readers in
// `staged_manifest/attack_handle_tests.rs`.

use log::Level;
use postretro_test_log_capture::LogCapture;
use serde_json::{Value, json};

use super::lower_behavior;

fn jab() -> Value {
    json!({ "kind": "attack", "name": "jab", "damage": 8, "maxRange": 2, "cooldownMs": 400 })
}

fn slam() -> Value {
    json!({ "kind": "attack", "name": "slam", "damage": 14, "maxRange": 3.5, "cooldownMs": 1800 })
}

fn turn_free(attack: &str) -> Value {
    json!({ "op": "input", "name": format!("@attackTurnFree.{attack}") })
}

/// A two-level behavior: `engage` fires `jab` from its own action and nests
/// an offense graph whose `commit` fires and marks `slam`.
fn behavior(attacks: Option<Value>) -> Value {
    let mut behavior = json!({
        "initial": "idle",
        "moveSpeed": 3,
        "activities": {
            "idle": { "animation": "idle" },
            "engage": {
                "animation": "walk",
                "action": { "attack": jab() },
                "layers": { "offense": {
                    "initial": "windup",
                    "activities": {
                        "windup": { "animation": "windup" },
                        "commit": { "animation": "slam", "action": { "attack": slam(), "at": "hit" }, "attackTurn": slam() }
                    },
                    "transitions": { "windup": [{ "to": "commit" }] }
                } }
            }
        },
        "transitions": { "idle": [{ "to": "engage", "when": turn_free("jab") }] }
    });
    if let Some(attacks) = attacks {
        behavior["attacks"] = attacks;
    }
    behavior
}

fn lowered(mut behavior: Value) -> Value {
    lower_behavior(&mut behavior, Some("brute")).expect("the behavior lowers");
    behavior
}

fn rejection(mut behavior: Value) -> String {
    lower_behavior(&mut behavior, Some("brute"))
        .expect_err("the behavior rejects load")
        .to_string()
}

// T2: handles resolve to names everywhere the engine reads them, and the
// `attacks` map is built from their declarations with no authoring field.
#[test]
fn handles_resolve_to_names_and_build_the_attacks_map_from_declarations() {
    let behavior = lowered(behavior(None));
    assert_eq!(
        behavior["activities"]["engage"]["action"],
        json!({ "attack": "jab" })
    );
    let commit = &behavior["activities"]["engage"]["layers"]["offense"]["activities"]["commit"];
    assert_eq!(commit["action"], json!({ "attack": "slam", "at": "hit" }));
    assert_eq!(commit["attackTurn"], json!("slam"));
    assert_eq!(
        behavior["attacks"],
        json!({
            "jab": { "damage": 8, "maxRange": 2, "cooldownMs": 400 },
            "slam": { "damage": 14, "maxRange": 3.5, "cooldownMs": 1800 }
        })
    );
}

// T2: an attack's authoring fields stay out of the runtime map.
#[test]
fn an_attacks_authoring_fields_stay_out_of_the_runtime_map() {
    let overhead = json!({ "kind": "stance", "name": "overhead", "animation": "smash_windup" });
    let mut slam = slam();
    slam["beginningStance"] = overhead.clone();
    slam["endingStance"] = overhead;
    slam["release"] = json!({ "animation": "smash_strike" });
    slam["recover"] = json!({ "animation": "idle" });
    let mut source = behavior(Some(json!([jab(), slam.clone()])));
    let commit = &mut source["activities"]["engage"]["layers"]["offense"]["activities"]["commit"];
    commit["action"]["attack"] = slam.clone();
    commit["attackTurn"] = slam;
    let behavior = lowered(source);
    assert_eq!(
        behavior["attacks"]["slam"],
        json!({ "damage": 14, "maxRange": 3.5, "cooldownMs": 1800 })
    );
}

// T3: without a list, the attacks are exactly those referenced by action and
// mark; a turn-fact guard registers nothing.
#[test]
fn without_a_list_the_attacks_are_those_actions_and_marks_reference() {
    let mut source = behavior(None);
    source["transitions"]["idle"][0]["when"] = turn_free("claw");
    let behavior = lowered(source);
    let names: Vec<&String> = behavior["attacks"].as_object().unwrap().keys().collect();
    assert_eq!(names, ["jab", "slam"]);
}

// T3: a list is exhaustive: a listed attack nothing references still exists.
#[test]
fn a_list_is_exhaustive_and_keeps_an_unreferenced_entry() {
    let claw =
        json!({ "kind": "attack", "name": "claw", "damage": 15, "maxRange": 2, "cooldownMs": 800 });
    let behavior = lowered(behavior(Some(json!([jab(), slam(), claw]))));
    let names: Vec<&String> = behavior["attacks"].as_object().unwrap().keys().collect();
    assert_eq!(names, ["claw", "jab", "slam"]);
}

// T3: an action or mark naming an attack the list leaves out rejects load,
// naming the reference.
#[test]
fn an_action_or_mark_to_an_unlisted_attack_rejects_load() {
    let error = rejection(behavior(Some(json!([slam()]))));
    assert!(
        error
            .contains("`components.behavior.activities.engage.action.attack` names attack \"jab\""),
        "{error}"
    );

    let mut marked = behavior(Some(json!([jab()])));
    marked["activities"]["engage"]["layers"]["offense"]["activities"]["commit"]
        .as_object_mut()
        .unwrap()
        .remove("action");
    let error = rejection(marked);
    assert!(
        error.contains(
            "`components.behavior.activities.engage.layers.offense.activities.commit.attackTurn` names attack \"slam\""
        ),
        "{error}"
    );
}

// T3 / P41 (a): under a list, a row to an unlisted attack drops with one log,
// and its guard (which reads that attack's turn) is never checked.
#[test]
fn a_row_to_an_unlisted_attack_drops_with_one_log_and_its_guard_goes_with_it() {
    let capture = LogCapture::start();
    let mut source = behavior(Some(json!([jab()])));
    source["activities"]["engage"]["layers"]["offense"]["activities"]
        .as_object_mut()
        .unwrap()
        .remove("commit");
    source["activities"]["engage"]["layers"]["offense"]["transitions"] = json!({
        "windup": [
            { "to": slam(), "when": { "op": "input", "name": "@attackTurnHeld.slam" } },
            { "to": "windup" }
        ]
    });
    let behavior = lowered(source);

    let rows = &behavior["activities"]["engage"]["layers"]["offense"]["transitions"]["windup"];
    assert_eq!(
        rows,
        &json!([{ "to": "windup" }]),
        "only the row to `slam` drops"
    );
    assert_eq!(behavior["attacks"].as_object().unwrap().len(), 1);
    capture.assert_logged_once(
        Level::Info,
        "`components.behavior.activities.engage.layers.offense.transitions.windup[0]` drops its row to attack \"slam\"",
    );
}

// T4: a bare string, nil or a handle of the other kind where an attack handle
// is expected rejects load with a pathed error; so does the retired map.
#[test]
fn anything_but_an_attack_handle_rejects_load_with_a_pathed_error() {
    let stance = json!({ "kind": "stance", "name": "overhead" });
    type Mutation = Box<dyn Fn(&mut Value)>;
    let cases: Vec<(Mutation, &str, &str)> = vec![
        (
            Box::new(|b| b["attacks"] = json!(["jab"])),
            "`components.behavior.attacks[0]`",
            "bare string",
        ),
        (
            Box::new(|b| {
                b["attacks"] = json!({ "jab": { "damage": 8, "maxRange": 2, "cooldownMs": 400 } })
            }),
            "`components.behavior.attacks` is a name-keyed map (jab)",
            "defineAttack",
        ),
        (
            Box::new(|b| b["activities"]["engage"]["action"]["attack"] = json!("jab")),
            "`components.behavior.activities.engage.action.attack`",
            "bare string",
        ),
        (
            Box::new(|b| b["activities"]["engage"]["action"]["attack"] = Value::Null),
            "`components.behavior.activities.engage.action.attack`",
            "nil",
        ),
        (
            Box::new(|b| {
                b["activities"]["engage"]["action"]
                    .as_object_mut()
                    .unwrap()
                    .remove("attack");
            }),
            "`components.behavior.activities.engage.action.attack` is missing",
            "defineAttack",
        ),
        (
            Box::new(move |b| b["activities"]["engage"]["action"]["attack"] = stance.clone()),
            "`components.behavior.activities.engage.action.attack`",
            "`stance` handle",
        ),
        (
            Box::new(|b| {
                b["activities"]["engage"]["layers"]["offense"]["activities"]["commit"]["attackTurn"] =
                    json!("slam")
            }),
            "`components.behavior.activities.engage.layers.offense.activities.commit.attackTurn`",
            "bare string",
        ),
        (
            Box::new(|b| {
                b["activities"]["engage"]["action"]["attack"]["beginningStance"] = json!("overhead")
            }),
            "`components.behavior.activities.engage.action.attack.beginningStance`",
            "pass the stance handle `defineStance` returned",
        ),
        (
            Box::new(|b| b["activities"]["engage"]["action"]["attack"]["endingStance"] = jab()),
            "`components.behavior.activities.engage.action.attack.endingStance`",
            "`attack` handle",
        ),
    ];
    for (mutate, path, detail) in cases {
        let mut source = behavior(None);
        mutate(&mut source);
        let error = rejection(source);
        assert!(
            error.contains(path) && error.contains(detail),
            "expected {path} / {detail}: {error}"
        );
    }
}

// Regression: a handle with an empty name (`defineAttack("", ...)`) was
// reported as "a `attack` handle" where an attack handle was expected.
#[test]
fn a_handle_with_an_empty_name_rejects_load_saying_so() {
    let mut source = behavior(None);
    source["activities"]["engage"]["action"]["attack"]["name"] = json!("");
    let error = rejection(source);
    assert!(
        error.contains(
            "`components.behavior.activities.engage.action.attack`: the attack handle has an empty name"
        ),
        "{error}"
    );
}

// Regression: a handle whose `name` is present but not a string (Luau
// `{ kind = "stance", name = 5 }`) was reported as having an empty name.
#[test]
fn a_handle_whose_name_is_not_a_string_rejects_load_saying_so() {
    let mut source = behavior(None);
    source["activities"]["engage"]["action"]["attack"]["name"] = json!(5);
    let error = rejection(source);
    assert!(
        error.contains(
            "`components.behavior.activities.engage.action.attack`: the attack handle's name is a number, not a string"
        ),
        "{error}"
    );
    assert!(!error.contains("empty name"), "{error}");
}

// T4: two handles of one name load when their declarations are equal and
// reject, naming both, when they differ.
#[test]
fn same_name_handles_load_when_equal_and_reject_when_unequal() {
    lowered(behavior(Some(json!([jab(), slam(), slam()]))));

    let mut other = slam();
    other["damage"] = json!(20);
    let error = rejection(behavior(Some(json!([jab(), other]))));
    assert!(
        error
            .contains("declares attack \"slam\" differently from `components.behavior.attacks[1]`"),
        "{error}"
    );
}

// T6: an authored lowering-only field rejects load wherever it stands.
#[test]
fn an_authored_lowering_only_field_rejects_load() {
    for (field, place) in [
        ("pose", "/activities/idle"),
        ("releaseAfter", "/activities/idle"),
        ("staggerTo", "/activities/idle"),
        ("queue", "/transitions/idle/0"),
        ("afterMoment", "/transitions/idle/0"),
        ("atDefault", "/activities/engage/action"),
    ] {
        let mut source = behavior(None);
        source.pointer_mut(place).unwrap()[field] = json!(true);
        let error = rejection(source);
        assert!(
            error.contains(&format!(".{field}` is written only by load-time lowering")),
            "{field}: {error}"
        );
    }
}
