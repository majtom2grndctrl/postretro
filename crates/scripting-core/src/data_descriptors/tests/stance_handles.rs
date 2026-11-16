// Stance and attack handles through both entity readers: the pathed
// rejections lowering owes a misplaced handle, and the raw Luau checks that
// run before the JSON bridge launders a value into `null`.
// See: context/lib/scripting.md §2 (Attack and stance declarations)

use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use super::super::*;
use super::common::*;

/// One authoring of a brute that takes `overhead` and strikes `slam` from it.
/// Each fragment is spliced in verbatim: `stance` and `slam` extend the two
/// handles, `initial` is the root's `initial` expression, and `transitions`
/// adds row lists after the authored ones.
struct Brute<'a> {
    stance: &'a str,
    slam: &'a str,
    initial: &'a str,
    transitions: &'a str,
}

const JS_BRUTE: Brute<'static> = Brute {
    stance: "",
    slam: r#", release: { animation: "smash_strike" }"#,
    initial: r#""approach""#,
    transitions: "",
};

const LUA_BRUTE: Brute<'static> = Brute {
    stance: "",
    slam: r#", release = { animation = "smash_strike" }"#,
    initial: r#""approach""#,
    transitions: "",
};

fn js_source(brute: &Brute) -> String {
    format!(
        r#"(() => {{
            const overhead = {{ kind: "stance", name: "overhead", animation: "smash_windup"{stance} }};
            const slam = {{ kind: "attack", name: "slam", damage: 10, maxRange: 3, cooldownMs: 1000, beginningStance: overhead{slam} }};
            return {{ canonicalName: "brute", components: {{ behavior: {{
                initial: {initial}, moveSpeed: 3,
                activities: {{ approach: {{ animation: "walk", motion: "chaseTarget" }} }},
                transitions: {{
                    approach: [{{ to: overhead }}],
                    overhead: [
                        {{ to: slam }},
                        {{ to: "approach", when: {{ op: "ge", a: {{ op: "input", name: "@brain.timeInActivityMs" }}, b: {{ op: "const", value: 2000 }} }} }}
                    ]{transitions}
                }}
            }} }} }};
        }})()"#,
        stance = brute.stance,
        slam = brute.slam,
        initial = brute.initial,
        transitions = brute.transitions,
    )
}

fn lua_source(brute: &Brute) -> String {
    format!(
        r#"local overhead = {{ kind = "stance", name = "overhead", animation = "smash_windup"{stance} }}
        local slam = {{ kind = "attack", name = "slam", damage = 10, maxRange = 3, cooldownMs = 1000, beginningStance = overhead{slam} }}
        return {{ canonicalName = "brute", components = {{ behavior = {{
            initial = {initial}, moveSpeed = 3,
            activities = {{ approach = {{ animation = "walk", motion = "chaseTarget" }} }},
            transitions = {{
                approach = {{ {{ to = overhead }} }},
                overhead = {{
                    {{ to = slam }},
                    {{ to = "approach", when = {{ op = "ge", a = {{ op = "input", name = "@brain.timeInActivityMs" }}, b = {{ op = "const", value = 2000 }} }} }}
                }}{transitions}
            }}
        }} }} }}"#,
        stance = brute.stance,
        slam = brute.slam,
        initial = brute.initial,
        transitions = brute.transitions,
    )
}

fn js_loaded(brute: &Brute) -> EntityTypeDescriptor {
    eval_js(&js_source(brute), |ctx, value| {
        entity_descriptor_from_js(ctx, value).expect("the TypeScript brute loads")
    })
}

fn lua_loaded(brute: &Brute) -> EntityTypeDescriptor {
    eval_lua(&lua_source(brute), |value| {
        entity_descriptor_from_lua(value).expect("the Luau brute loads")
    })
}

fn js_rejection(brute: &Brute) -> String {
    eval_js(&js_source(brute), |ctx, value| {
        entity_descriptor_from_js(ctx, value)
            .expect_err("the TypeScript brute rejects load")
            .to_string()
    })
}

fn lua_rejection(brute: &Brute) -> String {
    eval_lua(&lua_source(brute), |value| {
        entity_descriptor_from_lua(value)
            .expect_err("the Luau brute rejects load")
            .to_string()
    })
}

/// Both runtimes reject the authoring with an error naming every needle.
fn assert_rejects_in_both(label: &str, js: &Brute, lua: &Brute, needles: &[&str]) {
    for (runtime, error) in [("js", js_rejection(js)), ("luau", lua_rejection(lua))] {
        for needle in needles {
            assert!(
                error.contains(needle),
                "{label} ({runtime}): expected `{needle}` in: {error}"
            );
        }
    }
}

#[test]
fn the_brute_authoring_loads_in_both_runtimes() {
    let js = js_loaded(&JS_BRUTE).behavior.expect("behavior");
    let lua = lua_loaded(&LUA_BRUTE).behavior.expect("behavior");
    assert_eq!(js, lua);
    assert!(js.envelope.activities.contains_key("slam_release"));
}

// Regression: a moving release whose `recover` named no animation fell back
// to the release's state, so its stride played in place. A planted release
// keeps that fallback.
#[test]
fn a_moving_release_needs_a_recover_animation_in_both_runtimes() {
    assert_rejects_in_both(
        "moving release, recover without animation",
        &Brute {
            slam: r#", release: { animation: "run", motion: "chaseTarget", at: false }, recover: { ms: 600 }"#,
            ..JS_BRUTE
        },
        &Brute {
            slam: r#", release = { animation = "run", motion = "chaseTarget", at = false }, recover = { ms = 600 }"#,
            ..LUA_BRUTE
        },
        &[
            "`components.behavior.transitions.overhead[0].to.recover.animation` is missing",
            "release moves",
        ],
    );

    let js = js_loaded(&Brute {
        slam: r#", release: { animation: "smash_strike" }, recover: { ms: 600 }"#,
        ..JS_BRUTE
    });
    let lua = lua_loaded(&Brute {
        slam: r#", release = { animation = "smash_strike" }, recover = { ms = 600 }"#,
        ..LUA_BRUTE
    });
    for descriptor in [js, lua] {
        let behavior = descriptor.behavior.expect("behavior");
        assert_eq!(
            behavior.envelope.activities["slam_recover"]
                .animation
                .as_deref(),
            Some("smash_strike"),
            "a planted release's recover holds the release's state"
        );
    }
}

// Regression: these handle mistakes reached serde, which named no path
// ("invalid type: map, expected a string").
#[test]
fn a_misplaced_handle_rejects_load_with_its_path_in_both_runtimes() {
    let cases: [(&str, Brute, Brute, &[&str]); 5] = [
        (
            "an attack handle as `initial`",
            Brute {
                initial: "slam",
                ..JS_BRUTE
            },
            Brute {
                initial: "slam",
                ..LUA_BRUTE
            },
            &[
                "`components.behavior.initial`: is a `attack` handle",
                "`initial` takes an activity name or a stance handle",
            ],
        ),
        (
            "a handle row under a misspelled source key",
            Brute {
                transitions: ", idel: [{ to: slam }]",
                ..JS_BRUTE
            },
            Brute {
                transitions: ", idel = { { to = slam } }",
                ..LUA_BRUTE
            },
            &["`components.behavior.transitions.idel` does not name a declared activity or stance"],
        ),
        (
            "a damage-type handle as a row's `to`",
            Brute {
                transitions: r#", "*": [{ to: { kind: "damageType", name: "fire" } }]"#,
                ..JS_BRUTE
            },
            Brute {
                transitions: r#", ["*"] = { { to = { kind = "damageType", name = "fire" } } }"#,
                ..LUA_BRUTE
            },
            &[
                "`components.behavior.transitions.*[0].to`: is a `damageType` handle",
                "a row's `to` takes an activity name",
            ],
        ),
        (
            "an attack-token handle as a row's `to`",
            Brute {
                transitions: r#", "*": [{ to: { kind: "attackToken", name: "melee" } }]"#,
                ..JS_BRUTE
            },
            Brute {
                transitions: r#", ["*"] = { { to = { kind = "attackToken", name = "melee" } } }"#,
                ..LUA_BRUTE
            },
            &["`components.behavior.transitions.*[0].to`: is a `attackToken` handle"],
        ),
        (
            "a plain object as a row's `to`",
            Brute {
                transitions: r#", "*": [{ to: { name: "approach" } }]"#,
                ..JS_BRUTE
            },
            Brute {
                transitions: r#", ["*"] = { { to = { name = "approach" } } }"#,
                ..LUA_BRUTE
            },
            &["`components.behavior.transitions.*[0].to`: is a plain object, not a handle"],
        ),
    ];
    for (label, js, lua, needles) in &cases {
        assert_rejects_in_both(label, js, lua, needles);
    }
}

// Regression: a function-valued stance `sound`, `release.at` or
// `recover.animation` crossed Luau's JSON bridge as null and read as omitted
// (no sound, the defaulted `hit`, the release's state), where QuickJS rejects.
#[test]
fn a_function_valued_stance_or_strike_field_is_rejected_in_luau() {
    let error = lua_rejection(&Brute {
        stance: ", sound = function() end",
        ..LUA_BRUTE
    });
    assert!(
        error.contains("`components.behavior.transitions.")
            && error.contains(".sound` must be a string"),
        "{error}"
    );

    for (slam, field) in [
        (
            r#", release = { animation = "smash_strike", at = function() end }"#,
            "release.at",
        ),
        (
            r#", release = { animation = "smash_strike" }, recover = { animation = function() end }"#,
            "recover.animation",
        ),
    ] {
        let error = lua_rejection(&Brute { slam, ..LUA_BRUTE });
        assert!(
            error.contains(&format!(
                "`components.behavior.transitions.overhead[0].to.{field}` must be a string, number, boolean or table, got function"
            )),
            "{field}: {error}"
        );
    }
}

// Regression: the raw Luau sound-key walk followed every table to a fixed
// depth with no visited set, so a behavior table with two back-edges to
// itself took exponential time and hung mod load.
#[test]
fn a_self_referencing_luau_behavior_table_rejects_load_promptly() {
    const SOURCE: &str = r#"
        local behavior = {
            initial = "idle", moveSpeed = 3,
            activities = { idle = { animation = "idle", motion = "hold" } },
            transitions = {},
        }
        behavior.self = behavior
        behavior.again = behavior
        return { components = { behavior = behavior } }
    "#;
    let (sender, receiver) = mpsc::channel();
    thread::spawn(move || {
        let error = eval_lua(SOURCE, |value| {
            entity_descriptor_from_lua(value)
                .expect_err("a cyclic behavior rejects load")
                .to_string()
        });
        let _ = sender.send(error);
    });
    let error = receiver
        .recv_timeout(Duration::from_secs(30))
        .expect("a cyclic behavior table rejects load promptly, not by hanging");
    assert!(
        error.contains("maximum conversion depth"),
        "the JSON bridge names the cycle's depth: {error}"
    );
}
