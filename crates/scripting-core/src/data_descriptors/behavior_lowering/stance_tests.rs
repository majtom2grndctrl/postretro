// Stance lowering on the JSON both runtimes hand it, from the fixture to the pathed rejections.
// See: context/lib/entity_model.md §7c (Stances and attacks)

use log::Level;
use postretro_foundation::BehaviorGraphDescriptor;
use postretro_foundation::stance_defaults::{DEFAULT_HOLD_MS, STANCE_HOLD_FLOOR_MS};
use postretro_test_log_capture::LogCapture;
use serde_json::{Value, json};

use super::{lower_behavior, stance_floor_warning_scope};

const FLOOR_WARNING: &str = "below the co-op floor";

fn stance(name: &str, options: Value) -> Value {
    let mut handle = json!({ "kind": "stance", "name": name });
    handle
        .as_object_mut()
        .unwrap()
        .extend(options.as_object().unwrap().clone());
    handle
}

fn attack(name: &str, declaration: Value) -> Value {
    let mut handle =
        json!({ "kind": "attack", "name": name, "damage": 10, "maxRange": 3, "cooldownMs": 1000 });
    handle
        .as_object_mut()
        .unwrap()
        .extend(declaration.as_object().unwrap().clone());
    handle
}

fn overhead() -> Value {
    stance(
        "overhead",
        json!({ "animation": "smash_windup", "breakOn": { "damage": 25, "to": "flinch" } }),
    )
}

fn coil() -> Value {
    stance("coil", json!({ "animation": "swipe_windup" }))
}

fn slam() -> Value {
    attack(
        "slam",
        json!({ "beginningStance": overhead(), "endingStance": coil(), "release": { "animation": "smash_strike" } }),
    )
}

fn sweep() -> Value {
    attack(
        "sweep",
        json!({ "beginningStance": overhead(), "release": { "animation": "swipe_strike" } }),
    )
}

fn lunge() -> Value {
    attack(
        "lunge",
        json!({ "beginningStance": coil(), "release": { "animation": "swipe_strike" } }),
    )
}

fn ms(value: f64) -> Value {
    json!({ "op": "ge", "a": { "op": "input", "name": "@brain.timeInActivityMs" }, "b": { "op": "const", "value": value } })
}

/// A root level whose `approach` takes `overhead`, `overhead` striking `slam`
/// (landing in `coil`) and `sweep`, `coil` lunging, each rest with a way back.
fn offense() -> Value {
    json!({
        "initial": "approach",
        "moveSpeed": 3,
        "activities": {
            "approach": { "animation": "walk", "motion": "chaseTarget" },
            "flinch": { "animation": "flinch", "motion": "hold" }
        },
        "transitions": {
            "approach": [{ "to": overhead() }],
            "overhead": [{ "to": slam() }, { "to": sweep() }, { "to": "approach", "when": ms(2000.0) }],
            "coil": [{ "to": lunge() }, { "to": "approach", "when": ms(800.0) }],
            "flinch": [{ "to": "approach", "when": ms(450.0) }]
        }
    })
}

/// Lower, then parse and validate as the runtime descriptor, as both entity
/// readers do.
fn load(mut behavior: Value) -> Result<Value, String> {
    lower_behavior(&mut behavior, Some("brute")).map_err(|error| error.to_string())?;
    serde_json::from_value::<BehaviorGraphDescriptor>(behavior.clone())
        .map_err(|error| error.to_string())?
        .validate()
        .map_err(|error| error.to_string())?;
    Ok(behavior)
}

fn loaded(behavior: Value) -> Value {
    load(behavior).expect("the behavior loads")
}

fn rejection(behavior: Value) -> String {
    load(behavior).expect_err("the behavior rejects load")
}

/// Every number as `f64`: authored integers and lowered floats compare alike.
fn normalized(value: Value) -> Value {
    match value {
        Value::Number(number) => json!(number.as_f64().expect("finite")),
        Value::Array(items) => Value::Array(items.into_iter().map(normalized).collect()),
        Value::Object(map) => {
            Value::Object(map.into_iter().map(|(k, v)| (k, normalized(v))).collect())
        }
        other => other,
    }
}

fn activity_names(behavior: &Value) -> Vec<&str> {
    behavior["activities"]
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect()
}

fn floor_warnings(capture: &LogCapture) -> Vec<String> {
    capture
        .records()
        .into_iter()
        .filter(|record| record.level == Level::Warn && record.message.contains(FLOOR_WARNING))
        .map(|record| record.message)
        .collect()
}

// S2: with a fixed `holdMs` and a `breakOn`, a level whose rows take
// `overhead` and strike `slam` from it lowers to the checked-in hand-written
// fixture: the rest, the hold, the release and the recover, their lowering-only
// fields, and their rows, row order included.
#[test]
fn a_stance_and_its_strike_lower_to_the_hand_written_fixture() {
    let authored: Value =
        serde_json::from_str(include_str!("fixtures/stance_overhead_slam.authored.json"))
            .expect("the authored fixture parses");
    let expected: Value =
        serde_json::from_str(include_str!("fixtures/stance_overhead_slam.lowered.json"))
            .expect("the lowered fixture parses");
    let lowered = loaded(authored);
    assert_eq!(normalized(lowered), normalized(expected));
}

// S3: a level gains the rests its rows and `initial` reach, directly or through
// a struck attack's `endingStance`, and the strikes made from them; a stance
// nothing reaches is not generated, and a level that targets nothing gains
// nothing.
#[test]
fn a_level_gains_only_the_rests_and_strikes_it_reaches() {
    let behavior = loaded(offense());
    assert_eq!(
        activity_names(&behavior),
        [
            "approach",
            "coil",
            "flinch",
            "lunge_coil",
            "lunge_recover",
            "lunge_release",
            "overhead",
            "slam_overhead",
            "slam_recover",
            "slam_release",
            "sweep_overhead",
            "sweep_recover",
            "sweep_release",
        ],
        "`coil` is reached only through `slam`'s ending stance"
    );

    // An attack whose stance no row reaches lowers to its `attacks` entry
    // alone; neither level gains an activity.
    let charge = attack(
        "charge",
        json!({ "beginningStance": stance("bellow", json!({ "animation": "roar" })), "release": { "animation": "run", "at": false } }),
    );
    let untouched = json!({
        "initial": "idle",
        "moveSpeed": 3,
        "activities": {
            "idle": { "animation": "idle", "motion": "hold" },
            "engage": { "animation": "walk", "layers": { "offense": {
                "initial": "stalk",
                "activities": { "stalk": { "animation": "walk", "action": { "attack": charge } } },
                "transitions": {}
            } } }
        },
        "transitions": { "idle": [{ "to": "engage" }] }
    });
    let behavior = loaded(untouched.clone());
    let mut expected = untouched;
    expected["activities"]["engage"]["layers"]["offense"]["activities"]["stalk"]["action"] =
        json!({ "attack": "charge" });
    expected["attacks"] = json!({ "charge": { "damage": 10, "maxRange": 3, "cooldownMs": 1000 } });
    assert_eq!(
        behavior, expected,
        "a level that targets nothing gains nothing"
    );
}

// S3: an authored activity whose name collides with a generated one rejects
// load naming both; so does a string `to` or `initial` naming a generated
// activity.
#[test]
fn a_collision_or_a_string_naming_a_generated_activity_rejects_load() {
    let mut collides = offense();
    collides["activities"]["slam_release"] = json!({ "animation": "idle" });
    let error = rejection(collides);
    assert!(
        error.contains(
            "`components.behavior.activities.slam_release` collides with attack \"slam\"'s release"
        ),
        "{error}"
    );

    let mut rest = offense();
    rest["activities"]["coil"] = json!({ "animation": "idle" });
    let error = rejection(rest);
    assert!(
        error
            .contains("`components.behavior.activities.coil` collides with stance \"coil\"'s rest"),
        "{error}"
    );

    let mut string_to = offense();
    string_to["transitions"]["flinch"][0]["to"] = json!("slam_overhead");
    let error = rejection(string_to);
    assert!(
        error.contains("`components.behavior.transitions.flinch[0].to` names \"slam_overhead\", attack \"slam\"'s hold in stance \"overhead\""),
        "{error}"
    );

    let mut string_initial = offense();
    string_initial["initial"] = json!("overhead");
    let error = rejection(string_initial);
    assert!(
        error.contains(
            "`components.behavior.initial` names \"overhead\", stance \"overhead\"'s rest"
        ),
        "{error}"
    );
}

// S4 (lowering half): `{ to: overhead }` enters the rest, which holds no turn
// and carries the stance sound; an `initial` stance starts there.
#[test]
fn a_row_and_an_initial_to_a_stance_lower_to_its_unmarked_rest() {
    let sounded = stance(
        "overhead",
        json!({ "animation": "smash_windup", "sound": "brute/stance_overhead" }),
    );
    let behavior = loaded(json!({
        "initial": sounded,
        "moveSpeed": 3,
        "activities": { "approach": { "animation": "walk" } },
        "transitions": {
            "approach": [{ "to": sounded }],
            "overhead": [{ "to": "approach", "when": ms(500.0) }]
        }
    }));
    assert_eq!(behavior["initial"], json!("overhead"));
    assert_eq!(
        behavior["transitions"]["approach"][0]["to"],
        json!("overhead")
    );
    assert_eq!(
        behavior["activities"]["overhead"],
        json!({ "animation": "smash_windup", "sound": "brute/stance_overhead", "motion": "hold", "pose": "overhead" }),
        "the rest holds no turn and plays the stance sound"
    );
}

// S6: two attacks sharing a stance get separate holds in the stance's pose,
// with the stance's animation, hold range and `breakOn` row, each marked for
// its own attack.
#[test]
fn two_attacks_sharing_a_stance_get_their_own_holds_in_its_pose() {
    let behavior = loaded(offense());
    let break_row = json!({
        "to": "flinch",
        "when": { "op": "ge", "a": { "op": "input", "name": "@brain.damageInActivity" }, "b": { "op": "const", "value": 25.0 } }
    });
    for attack in ["slam", "sweep"] {
        let hold = format!("{attack}_overhead");
        assert_eq!(
            behavior["activities"][&hold],
            json!({ "animation": "smash_windup", "motion": "hold", "pose": "overhead", "attackTurn": attack }),
            "{hold}"
        );
        let rows = &behavior["transitions"][&hold];
        assert_eq!(
            normalized(rows[0].clone()),
            normalized(break_row.clone()),
            "{hold}"
        );
        assert_eq!(
            rows[2]["releaseAfter"],
            json!(DEFAULT_HOLD_MS.map(f64::from))
        );
    }
    let rest_rows = &behavior["transitions"]["overhead"];
    assert_eq!(normalized(rest_rows[0].clone()), normalized(break_row));
    assert_eq!(
        rest_rows[1],
        json!({ "to": "slam_overhead", "queue": "slam" })
    );
    assert_eq!(
        rest_rows[2],
        json!({ "to": "sweep_overhead", "queue": "sweep" })
    );
}

// S7: the body `stance` option waits on the enemy cover brief; until then it
// rejects load with a pathed error.
#[test]
fn the_body_stance_option_rejects_load_until_enemy_cover_lands() {
    let error = rejection(with_overhead(|s| s["stance"] = json!("crouch")));
    assert!(
        error.contains("`components.behavior.transitions.approach[0].to.stance`"),
        "{error}"
    );
}

// S8: a rest with no row of its own and no `"*"` or outer row rejects load
// naming the stance, with or without a `breakOn`; a `"*"` or an outer row is a
// way out.
#[test]
fn a_rest_with_no_way_out_rejects_load_naming_the_stance() {
    for options in [
        json!({ "animation": "smash_windup" }),
        json!({ "animation": "smash_windup", "breakOn": { "damage": 25, "to": "flinch" } }),
    ] {
        let behavior = json!({
            "initial": "approach",
            "moveSpeed": 3,
            "activities": {
                "approach": { "animation": "walk" },
                "flinch": { "animation": "flinch", "motion": "hold" }
            },
            "transitions": {
                "approach": [{ "to": stance("overhead", options.clone()) }],
                "flinch": [{ "to": "approach" }]
            }
        });
        let error = rejection(behavior.clone());
        assert!(
            error.contains("stance \"overhead\"'s rest has no way out"),
            "{error}"
        );

        let mut wildcard = behavior.clone();
        wildcard["transitions"]["*"] = json!([{ "to": "flinch", "when": ms(5000.0) }]);
        loaded(wildcard);

        // Nested under `engage`, whose own row leaves it.
        let nested = json!({
            "initial": "idle",
            "moveSpeed": 3,
            "activities": {
                "idle": { "animation": "idle", "motion": "hold" },
                "engage": { "animation": "walk", "layers": { "offense": behavior.as_object().unwrap().iter()
                    .filter(|(key, _)| *key != "moveSpeed")
                    .map(|(key, value)| (key.clone(), value.clone()))
                    .collect::<serde_json::Map<_, _>>() } }
            },
            "transitions": { "idle": [{ "to": "engage" }], "engage": [{ "to": "idle", "when": ms(9000.0) }] }
        });
        loaded(nested);
    }
}

// S9 (lowering half): an attack's release and recover route to its ending
// stance's rest, which defaults to its beginning stance.
#[test]
fn a_strike_lands_in_its_ending_stance_which_defaults_to_its_beginning() {
    let behavior = loaded(offense());
    assert_eq!(behavior["activities"]["slam_release"]["staggerTo"], "coil");
    assert_eq!(behavior["activities"]["slam_recover"]["staggerTo"], "coil");
    assert_eq!(behavior["transitions"]["slam_recover"][0]["to"], "coil");
    assert_eq!(
        behavior["activities"]["sweep_recover"]["staggerTo"],
        "overhead"
    );
    assert_eq!(
        behavior["transitions"]["sweep_recover"][0]["to"],
        "overhead"
    );
}

// S10: every generated rest, hold, release and recover is planted; a release's
// `motion` and `speed` replace that on the release alone.
#[test]
fn generated_activities_are_planted_and_a_moving_release_moves_alone() {
    let behavior = loaded(offense());
    for (name, activity) in behavior["activities"].as_object().unwrap() {
        if name != "approach" {
            assert_eq!(activity["motion"], "hold", "{name}");
            assert!(activity.get("speed").is_none(), "{name}");
        }
    }

    let mut charging = offense();
    let bellow = stance("bellow", json!({ "animation": "roar" }));
    let charge = attack(
        "charge",
        json!({
            "beginningStance": bellow,
            "release": { "animation": "run", "motion": "chaseTarget", "speed": 2, "ms": 1500, "at": false },
            "recover": { "animation": "idle", "ms": 600 }
        }),
    );
    charging["transitions"]["approach"]
        .as_array_mut()
        .unwrap()
        .push(json!({ "to": bellow }));
    charging["transitions"]["bellow"] =
        json!([{ "to": charge }, { "to": "approach", "when": ms(1500.0) }]);
    let behavior = loaded(charging);
    let release = &behavior["activities"]["charge_release"];
    assert_eq!(release["motion"], "chaseTarget");
    assert_eq!(release["speed"], json!(2.0));
    assert_eq!(
        release["action"],
        json!({ "attack": "charge" }),
        "`at: false` fires on entry"
    );
    for planted in ["bellow", "charge_bellow", "charge_recover"] {
        assert_eq!(
            behavior["activities"][planted]["motion"], "hold",
            "{planted}"
        );
        assert!(
            behavior["activities"][planted].get("speed").is_none(),
            "{planted}"
        );
    }
    assert_eq!(
        behavior["activities"]["charge_recover"]["animation"],
        "idle"
    );
    assert_eq!(
        normalized(behavior["transitions"]["charge_recover"].clone()),
        normalized(json!([{ "to": "bellow", "when": ms(600.0) }])),
        "an authored `recover.ms` sets its duration"
    );
}

// S11 / F2: a stance given only `animation` and an attack given only its
// engine fields, `beginningStance` and `release` load silently; the hold takes
// the default range, at or above the floor, the release fires at a defaulted
// `hit`, and the recover holds the release's state until its clip plays
// through or the default duration passes.
#[test]
fn a_minimal_stance_and_attack_load_silently_with_the_defaults() {
    let capture = LogCapture::start();
    let minimal = stance("overhead", json!({ "animation": "smash_windup" }));
    let jab = attack(
        "jab",
        json!({ "beginningStance": minimal, "release": { "animation": "punch_strike" } }),
    );
    let behavior = loaded(json!({
        "initial": "approach",
        "moveSpeed": 3,
        "activities": { "approach": { "animation": "walk" } },
        "transitions": {
            "approach": [{ "to": minimal }],
            "overhead": [{ "to": jab }, { "to": "approach", "when": ms(2000.0) }]
        }
    }));
    capture.assert_nothing_above(Level::Info);

    let release_after = behavior["transitions"]["jab_overhead"][1]["releaseAfter"].clone();
    let [min, max] = [
        release_after[0].as_f64().unwrap(),
        release_after[1].as_f64().unwrap(),
    ];
    assert_eq!([min, max], DEFAULT_HOLD_MS.map(f64::from));
    assert!(min >= f64::from(STANCE_HOLD_FLOOR_MS));
    assert_eq!(
        behavior["activities"]["jab_release"]["action"],
        json!({ "attack": "jab", "at": "hit", "atDefault": true })
    );
    assert_eq!(
        behavior["transitions"]["jab_release"],
        json!([{ "to": "jab_recover", "afterMoment": true }])
    );
    assert_eq!(
        behavior["activities"]["jab_recover"]["animation"],
        "punch_strike"
    );
    assert_eq!(
        normalized(behavior["transitions"]["jab_recover"].clone()),
        normalized(json!([{ "to": "overhead", "when": {
            "op": "or",
            "a": { "op": "input", "name": "@brain.animationDone" },
            "b": ms(600.0)
        } }]))
    );
}

// F2: a stance whose authored minimum falls below the floor warns once per mod
// load naming it, `holdMs: 0` included, however many strikes and levels lower
// it; one at or above the floor loads silently.
#[test]
fn a_hold_below_the_floor_warns_once_per_mod_load_naming_the_stance() {
    let fast = |hold: Value| {
        let mut behavior = offense();
        let overhead = stance(
            "overhead",
            json!({ "animation": "smash_windup", "holdMs": hold, "breakOn": { "damage": 25, "to": "flinch" } }),
        );
        behavior["transitions"]["approach"][0]["to"] = overhead.clone();
        for row in behavior["transitions"]["overhead"].as_array_mut().unwrap() {
            if let Some(begins) = row["to"].get_mut("beginningStance") {
                *begins = overhead.clone();
            }
        }
        behavior
    };

    for hold in [json!(0), json!([100, 900])] {
        let capture = LogCapture::start();
        let load = stance_floor_warning_scope();
        // Two archetypes in one mod load, each striking twice from the stance.
        loaded(fast(hold.clone()));
        loaded(fast(hold.clone()));
        drop(load);
        let warnings = floor_warnings(&capture);
        assert_eq!(warnings.len(), 1, "{hold}: {warnings:?}");
        assert!(warnings[0].contains("stance \"overhead\""), "{warnings:?}");

        // The next mod load warns again.
        let _next_load = stance_floor_warning_scope();
        loaded(fast(hold));
        assert_eq!(floor_warnings(&capture).len(), 2);
    }

    let capture = LogCapture::start();
    loaded(fast(json!([STANCE_HOLD_FLOOR_MS, 1200])));
    loaded(fast(json!(STANCE_HOLD_FLOOR_MS + 1.0)));
    assert!(
        floor_warnings(&capture).is_empty(),
        "at or above the floor is silent"
    );
}

// S20: a release on its beginning stance's own state rejects load naming the
// attack; one on its own state loads.
#[test]
fn a_release_on_its_stances_state_rejects_load_naming_the_attack() {
    let mut behavior = offense();
    behavior["transitions"]["overhead"][1]["to"]["release"]["animation"] = json!("smash_windup");
    let error = rejection(behavior);
    assert!(
        error.contains("`components.behavior.transitions.overhead[1].to.release.animation`")
            && error.contains("attack \"sweep\"'s release plays \"smash_windup\""),
        "{error}"
    );
}

/// Edit the `overhead` stance everywhere it appears in [`offense`]: its row
/// target, and `slam`'s and `sweep`'s beginning stance.
fn with_overhead(edit: impl Fn(&mut Value)) -> Value {
    let mut behavior = offense();
    edit(&mut behavior["transitions"]["approach"][0]["to"]);
    for index in 0..2 {
        edit(&mut behavior["transitions"]["overhead"][index]["to"]["beginningStance"]);
    }
    behavior
}

/// [`offense`] with `sweep`'s declaration edited.
fn with_sweep(edit: impl Fn(&mut Value)) -> Value {
    let mut behavior = offense();
    edit(&mut behavior["transitions"]["overhead"][1]["to"]);
    behavior
}

// S26: each malformed stance, attack or row rejects load with a pathed error.
#[test]
fn malformed_stances_attacks_and_rows_reject_load_with_pathed_errors() {
    let approach_row = "components.behavior.transitions.approach[0].to";
    let sweep_row = "components.behavior.transitions.overhead[1].to";
    let cases: Vec<(&str, Value, Vec<String>)> = vec![
        (
            "breakOn.to names no sibling",
            with_overhead(|s| s["breakOn"]["to"] = json!("stagger")),
            vec!["breakOn.to".to_string(), "breaks to \"stagger\", which names no activity".to_string()],
        ),
        (
            "string to naming a generated activity",
            {
                let mut b = offense();
                b["transitions"]["flinch"][0]["to"] = json!("sweep_release");
                b
            },
            vec!["transitions.flinch[0].to` names \"sweep_release\"".to_string()],
        ),
        (
            "row to an attack outside its stance's rest",
            {
                let mut b = offense();
                b["transitions"]["approach"].as_array_mut().unwrap().push(json!({ "to": sweep() }));
                b
            },
            vec!["transitions.approach[1].to` strikes attack \"sweep\" from `approach`".to_string()],
        ),
        (
            "row to an attack from another stance's rest",
            {
                let mut b = offense();
                b["transitions"]["coil"].as_array_mut().unwrap().insert(0, json!({ "to": sweep() }));
                b
            },
            vec!["transitions.coil[0].to` strikes attack \"sweep\" from `coil`".to_string()],
        ),
        (
            "row to an attack with no beginningStance",
            {
                let mut b = offense();
                b["transitions"]["overhead"].as_array_mut().unwrap().insert(0, json!({ "to": attack("jab", json!({})) }));
                b
            },
            vec!["transitions.overhead[0].to` strikes attack \"jab\", which names no `beginningStance`".to_string()],
        ),
        (
            "holdMs min > max",
            with_overhead(|s| s["holdMs"] = json!([900, 500])),
            vec![format!("{approach_row}.holdMs"), "minimum 900 exceeds its maximum 500".to_string()],
        ),
        (
            "holdMs negative",
            with_overhead(|s| s["holdMs"] = json!(-1)),
            vec![format!("{approach_row}.holdMs"), "must be >= 0".to_string()],
        ),
        (
            "breakOn.damage zero",
            with_overhead(|s| s["breakOn"]["damage"] = json!(0)),
            vec![format!("{approach_row}.breakOn.damage"), "must be > 0".to_string()],
        ),
        (
            "breakOn.damage negative",
            with_overhead(|s| s["breakOn"]["damage"] = json!(-5)),
            vec![format!("{approach_row}.breakOn.damage"), "must be > 0".to_string()],
        ),
        (
            "negative release.ms",
            with_sweep(|a| a["release"]["ms"] = json!(-1)),
            vec![format!("{sweep_row}.release.ms"), "must be >= 0".to_string()],
        ),
        (
            "negative recover.ms",
            with_sweep(|a| a["recover"] = json!({ "ms": -1 })),
            vec![format!("{sweep_row}.recover.ms"), "must be >= 0".to_string()],
        ),
        (
            "release.at breaking the clip event rule",
            with_sweep(|a| a["release"]["at"] = json!("1hit")),
            vec![format!("{sweep_row}.release.at"), "must name a clip event".to_string()],
        ),
        (
            "release.at true",
            with_sweep(|a| a["release"]["at"] = json!(true)),
            vec![format!("{sweep_row}.release.at"), "found `true`".to_string()],
        ),
        (
            "release.at a number",
            with_sweep(|a| a["release"]["at"] = json!(3)),
            vec![format!("{sweep_row}.release.at"), "found a number".to_string()],
        ),
        (
            "release.speed zero",
            with_sweep(|a| a["release"]["speed"] = json!(0)),
            vec![format!("{sweep_row}.release.speed"), "must be a multiplier > 0".to_string()],
        ),
        (
            "activity speed zero",
            {
                let mut b = offense();
                b["activities"]["approach"]["speed"] = json!(0);
                b
            },
            vec!["components.behavior.activities.approach.speed".to_string(), "> 0.0".to_string()],
        ),
        (
            "release.motion a position goal",
            with_sweep(|a| a["release"]["motion"] = json!("patrol")),
            vec![format!("{sweep_row}.release.motion"), "found \"patrol\"".to_string()],
        ),
        (
            "moving release without recover",
            with_sweep(|a| a["release"]["motion"] = json!("chaseTarget")),
            vec![format!("{sweep_row}.recover` is missing"), "release moves".to_string()],
        ),
        (
            "beginningStance without release",
            with_sweep(|a| {
                a.as_object_mut().unwrap().remove("release");
            }),
            vec![format!("{sweep_row}.release` is missing")],
        ),
        (
            "release without beginningStance",
            {
                let mut b = offense();
                b["activities"]["approach"]["action"] = json!({ "attack": attack("jab", json!({ "release": { "animation": "punch" } })) });
                b
            },
            vec!["activities.approach.action.attack.release`".to_string(), "no `beginningStance`".to_string()],
        ),
        (
            "endingStance without beginningStance",
            {
                let mut b = offense();
                b["activities"]["approach"]["action"] = json!({ "attack": attack("jab", json!({ "endingStance": coil() })) });
                b
            },
            vec!["activities.approach.action.attack.endingStance`".to_string(), "no `beginningStance`".to_string()],
        ),
        (
            "a misspelled stance option",
            with_overhead(|s| s["holdMS"] = json!(500)),
            vec![format!("{approach_row}.holdMS` is not an option")],
        ),
        (
            "an authored lowering-only field on a stance",
            with_overhead(|s| s["pose"] = json!("overhead")),
            vec![format!("{approach_row}.pose` is written only by load-time lowering")],
        ),
    ];
    for (label, behavior, needles) in cases {
        let error = rejection(behavior);
        for needle in &needles {
            assert!(
                error.contains(needle.as_str()),
                "{label}: expected `{needle}` in: {error}"
            );
        }
    }
}

// S26: any non-finite value rejects load, naming its path, at the JSON bridge
// both runtimes lower through.
#[test]
fn a_non_finite_stance_value_rejects_load_with_its_path() {
    let runtime = rquickjs::Runtime::new().expect("runtime");
    let context = rquickjs::Context::full(&runtime).expect("context");
    let error = context.with(|ctx| {
        let value: rquickjs::Value = ctx
            .eval(
                r#"({ components: { behavior: {
                    initial: "approach", moveSpeed: 3,
                    activities: { approach: { animation: "walk" } },
                    transitions: { approach: [{ to: { kind: "stance", name: "overhead", animation: "smash_windup", holdMs: [Infinity, 900] } }] }
                } } })"#,
            )
            .expect("the source evaluates");
        crate::data_descriptors::entity_descriptor_from_js(&ctx, value)
            .expect_err("a non-finite hold rejects load")
            .to_string()
    });
    assert!(error.contains("holdMs"), "{error}");
}

// T3 / P37 (a): under `attacks: [sweep]`, a reached rest whose only row struck
// `slam` drops it, and is left with no way out: load rejects naming it.
#[test]
fn a_reached_rest_whose_only_row_drops_rejects_load_naming_it() {
    let capture = LogCapture::start();
    let mut behavior = offense();
    behavior["attacks"] = json!([sweep()]);
    behavior["transitions"]["overhead"] = json!([{ "to": slam() }]);
    behavior["transitions"]
        .as_object_mut()
        .unwrap()
        .remove("coil");
    let error = rejection(behavior);
    assert!(
        error.contains("stance \"overhead\"'s rest has no way out"),
        "{error}"
    );
    capture.assert_logged_once(
        Level::Info,
        "`components.behavior.transitions.overhead[0]` drops its row to attack \"slam\"",
    );
}

// T3 / P37 (b), P41 (c): under `attacks: [sweep]`, `coil`, reached only through
// `slam`'s ending stance, is not generated; its authored rows drop with a log,
// one of them reading `slam`'s turn, which is never checked.
#[test]
fn a_rest_reached_only_through_a_dropped_attack_drops_its_rows() {
    let capture = LogCapture::start();
    let mut behavior = offense();
    behavior["attacks"] = json!([sweep()]);
    behavior["transitions"]["coil"]
        .as_array_mut()
        .unwrap()
        .push(
            json!({ "to": "approach", "when": { "op": "input", "name": "@attackTurnFree.slam" } }),
        );
    let behavior = loaded(behavior);
    assert!(
        !activity_names(&behavior)
            .iter()
            .any(|name| name.contains("coil") || name.contains("slam") || name.contains("lunge")),
        "{:?}",
        activity_names(&behavior)
    );
    assert!(behavior["transitions"].get("coil").is_none());
    assert_eq!(behavior["attacks"].as_object().unwrap().len(), 1);
    capture.assert_logged_once(
        Level::Info,
        "`components.behavior.transitions.coil[0]` drops its row to attack \"lunge\"",
    );
    for index in [1, 2] {
        capture.assert_logged_once(
            Level::Info,
            &format!(
                "`components.behavior.transitions.coil[{index}]` drops with stance \"coil\"'s rest"
            ),
        );
    }
}

// T3 / P40: drops repeat to a fixed point. `coil`, reached only through `slam`,
// holds `bellow`'s only route in, and `bellow`'s only row strikes `charge`;
// neither rest is generated, every dropped row logs once, and load succeeds.
#[test]
fn drops_repeat_to_a_fixed_point() {
    let capture = LogCapture::start();
    let bellow = stance("bellow", json!({ "animation": "roar" }));
    let charge = attack(
        "charge",
        json!({ "beginningStance": bellow, "release": { "animation": "run", "at": false } }),
    );
    let mut behavior = offense();
    behavior["attacks"] = json!([sweep()]);
    behavior["transitions"]["coil"] = json!([{ "to": bellow }]);
    behavior["transitions"]["bellow"] = json!([{ "to": charge }]);
    let behavior = loaded(behavior);
    let names = activity_names(&behavior);
    assert!(
        !names.contains(&"coil") && !names.contains(&"bellow"),
        "{names:?}"
    );
    for (row, what) in [
        ("overhead[0]", "drops its row to attack \"slam\""),
        ("bellow[0]", "drops its row to attack \"charge\""),
        ("coil[0]", "drops with stance \"coil\"'s rest"),
    ] {
        capture.assert_logged_once(
            Level::Info,
            &format!("`components.behavior.transitions.{row}` {what}"),
        );
    }
}

// Regression: rows keyed by a stance nothing reached even before the list's
// drops were dropped with a log blaming the list; they reject load, as they do
// without a list.
#[test]
fn rows_keyed_by_a_stance_nothing_ever_reaches_reject_load_under_an_attacks_list() {
    let capture = LogCapture::start();
    let bellow = stance("bellow", json!({ "animation": "roar" }));
    let charge = attack(
        "charge",
        json!({ "beginningStance": bellow, "release": { "animation": "run", "at": false } }),
    );
    let mut behavior = offense();
    behavior["attacks"] = json!([slam(), sweep(), lunge(), charge]);
    behavior["transitions"]["bellow"] = json!([{ "to": "approach", "when": ms(500.0) }]);
    let error = rejection(behavior);
    assert!(
        error.contains(
            "`components.behavior.transitions.bellow` keys rows by stance \"bellow\", which no row or `initial` at this level reaches"
        ),
        "{error}"
    );
    assert!(
        !capture
            .records()
            .iter()
            .any(|record| record.message.contains("drops with stance")),
        "a never-reached rest's rows are not the list's drops"
    );
}

// Regression: a `"*"` row into the stance itself counted as its rest's way
// out, though the engine skips a `"*"` row into the pose the enemy holds, so
// the rest never left. A `"*"` row to a sibling still counts.
#[test]
fn a_star_row_into_the_stance_itself_is_no_way_out_of_its_rest() {
    let overhead = stance("overhead", json!({ "animation": "smash_windup" }));
    let level = |star: Value| {
        json!({
            "initial": "approach",
            "activities": {
                "approach": { "animation": "walk" },
                "flinch": { "animation": "flinch", "motion": "hold" }
            },
            "transitions": {
                "approach": [{ "to": overhead }],
                "flinch": [{ "to": "approach" }],
                "*": star
            }
        })
    };
    let root = |star: Value| {
        let mut root = level(star);
        root["moveSpeed"] = json!(3);
        root
    };
    let error = rejection(root(json!([{ "to": overhead, "when": ms(5000.0) }])));
    assert!(
        error.contains("stance \"overhead\"'s rest has no way out"),
        "{error}"
    );
    loaded(root(json!([{ "to": "flinch", "when": ms(5000.0) }])));
    loaded(root(json!([
        { "to": overhead, "when": ms(5000.0) },
        { "to": "flinch", "when": ms(5000.0) }
    ])));

    // Nested: an outer `"*"` row into the composite holding the level is
    // skipped while the enemy is inside it; one to a sibling leaves.
    let outer = |star: Value| {
        json!({
            "initial": "idle",
            "moveSpeed": 3,
            "activities": {
                "idle": { "animation": "idle", "motion": "hold" },
                "engage": { "animation": "walk", "layers": { "offense": level(json!([])) } }
            },
            "transitions": { "idle": [{ "to": "engage" }], "*": star }
        })
    };
    let error = rejection(outer(json!([{ "to": "engage", "when": ms(9000.0) }])));
    assert!(
        error.contains("stance \"overhead\"'s rest has no way out"),
        "{error}"
    );
    loaded(outer(json!([{ "to": "idle", "when": ms(9000.0) }])));
}

// Regression: a moving release whose `recover` named no animation fell back
// to the release's state, so its stride played in place. A planted release
// keeps the fallback.
#[test]
fn a_moving_release_needs_a_recover_with_its_own_animation() {
    let error = rejection(with_sweep(|a| {
        a["release"]["motion"] = json!("chaseTarget");
        a["recover"] = json!({ "ms": 600 });
    }));
    assert!(
        error.contains(
            "`components.behavior.transitions.overhead[1].to.recover.animation` is missing"
        ) && error.contains("release moves"),
        "{error}"
    );

    let behavior = loaded(with_sweep(|a| a["recover"] = json!({ "ms": 600 })));
    assert_eq!(
        behavior["activities"]["sweep_recover"]["animation"],
        "swipe_strike"
    );
}

// Regression: a number finite as f64 but infinite as f32 lowered to an
// infinite constant or failed later at a generated path.
#[test]
fn a_number_too_large_for_single_precision_rejects_load_with_its_path() {
    let approach_row = "components.behavior.transitions.approach[0].to";
    let sweep_row = "components.behavior.transitions.overhead[1].to";
    let cases = [
        (
            with_sweep(|a| a["release"]["ms"] = json!(1e39)),
            format!("`{sweep_row}.release.ms`"),
        ),
        (
            with_sweep(|a| a["recover"] = json!({ "ms": 1e39 })),
            format!("`{sweep_row}.recover.ms`"),
        ),
        (
            with_sweep(|a| a["release"]["speed"] = json!(1e39)),
            format!("`{sweep_row}.release.speed`"),
        ),
        (
            with_overhead(|s| s["breakOn"]["damage"] = json!(1e39)),
            format!("`{approach_row}.breakOn.damage`"),
        ),
        (
            with_overhead(|s| s["holdMs"] = json!([400, 1e39])),
            format!("`{approach_row}.holdMs[1]`"),
        ),
    ];
    for (behavior, path) in cases {
        let error = rejection(behavior);
        assert!(
            error.contains(&path) && error.contains("overflows single precision"),
            "{path}: {error}"
        );
    }
}

// Regression: the floor warning deduped by stance name alone, so a second
// stance of that name in another archetype of the mod never warned.
#[test]
fn two_stances_sharing_a_name_across_archetypes_each_warn_below_the_floor() {
    let with_hold = |hold: Value| with_overhead(move |s| s["holdMs"] = hold.clone());
    let capture = LogCapture::start();
    let load = stance_floor_warning_scope();
    loaded(with_hold(json!([100, 900])));
    loaded(with_hold(json!([200, 900])));
    loaded(with_hold(json!([100, 900])));
    drop(load);
    let warnings = floor_warnings(&capture);
    assert_eq!(warnings.len(), 2, "{warnings:?}");
    assert!(
        warnings
            .iter()
            .all(|warning| warning.contains("stance \"overhead\"")),
        "{warnings:?}"
    );
}

// Regression: the lowering-only field check matched every key at every depth,
// so an activity, layer or stance named like a lowering-only field rejected
// load as if it were that field.
#[test]
fn an_activity_or_stance_named_like_a_lowering_only_field_loads() {
    let pose = stance("pose", json!({ "animation": "smash_windup" }));
    let behavior = loaded(json!({
        "initial": "queue",
        "moveSpeed": 3,
        "activities": {
            "queue": { "animation": "walk", "layers": { "staggerTo": {
                "initial": "atDefault",
                "activities": { "atDefault": { "animation": "walk" } },
                "transitions": {}
            } } },
            "afterMoment": { "animation": "idle", "motion": "hold" }
        },
        "transitions": {
            "queue": [{ "to": pose }],
            "pose": [{ "to": "afterMoment", "when": ms(500.0) }],
            "afterMoment": [{ "to": "queue", "when": ms(500.0) }]
        }
    }));
    assert_eq!(behavior["activities"]["pose"]["pose"], "pose");
    assert_eq!(behavior["transitions"]["queue"][0]["to"], "pose");
}

// T6: an authored lowering-only field still rejects load in every field
// position, a stance or attack handle's declaration included.
#[test]
fn an_authored_lowering_only_field_in_a_body_or_handle_rejects_load() {
    let approach_row = "components.behavior.transitions.approach[0].to";
    let sweep_row = "components.behavior.transitions.overhead[1].to";
    let cases = [
        (
            {
                let mut behavior = offense();
                behavior["activities"]["approach"]["pose"] = json!("approach");
                behavior
            },
            "`components.behavior.activities.approach.pose`".to_string(),
        ),
        (
            with_overhead(|s| s["pose"] = json!("overhead")),
            format!("`{approach_row}.pose`"),
        ),
        (
            with_overhead(|s| s["breakOn"]["queue"] = json!("slam")),
            format!("`{approach_row}.breakOn.queue`"),
        ),
        (
            with_sweep(|a| a["atDefault"] = json!(true)),
            format!("`{sweep_row}.atDefault`"),
        ),
        (
            with_sweep(|a| a["release"]["afterMoment"] = json!(true)),
            format!("`{sweep_row}.release.afterMoment`"),
        ),
        (
            with_sweep(|a| a["recover"] = json!({ "staggerTo": "coil" })),
            format!("`{sweep_row}.recover.staggerTo`"),
        ),
        (
            with_sweep(|a| a["beginningStance"]["releaseAfter"] = json!([0, 1])),
            format!("`{sweep_row}.beginningStance.releaseAfter`"),
        ),
    ];
    for (behavior, path) in cases {
        let error = rejection(behavior);
        assert!(
            error.contains(&format!("{path} is written only by load-time lowering")),
            "{path}: {error}"
        );
    }
}
