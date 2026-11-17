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
/// handles, `initial` is the root's `initial` expression, `transitions` adds
/// row lists after the authored ones, and `mesh` is a `mesh` component entry
/// (with its trailing separator) before the behavior.
struct Brute<'a> {
    stance: &'a str,
    slam: &'a str,
    initial: &'a str,
    transitions: &'a str,
    mesh: &'a str,
}

const JS_BRUTE: Brute<'static> = Brute {
    stance: "",
    slam: r#", release: { animation: "smash_strike" }"#,
    initial: r#""approach""#,
    transitions: "",
    mesh: "",
};

const LUA_BRUTE: Brute<'static> = Brute {
    stance: "",
    slam: r#", release = { animation = "smash_strike" }"#,
    initial: r#""approach""#,
    transitions: "",
    mesh: "",
};

fn js_source(brute: &Brute) -> String {
    format!(
        r#"(() => {{
            const overhead = {{ kind: "stance", name: "overhead", animation: "smash_windup"{stance} }};
            const slam = {{ kind: "attack", name: "slam", damage: 10, maxRange: 3, cooldownMs: 1000, beginningStance: overhead{slam} }};
            return {{ canonicalName: "brute", components: {{ {mesh}behavior: {{
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
        mesh = brute.mesh,
    )
}

fn lua_source(brute: &Brute) -> String {
    format!(
        r#"local overhead = {{ kind = "stance", name = "overhead", animation = "smash_windup"{stance} }}
        local slam = {{ kind = "attack", name = "slam", damage = 10, maxRange = 3, cooldownMs = 1000, beginningStance = overhead{slam} }}
        return {{ canonicalName = "brute", components = {{ {mesh}behavior = {{
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
        mesh = brute.mesh,
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

/// A brute mesh declaring `jab_quick`, looping or not, beside the stance's
/// and the recover's states.
fn jab_mesh_js(looping: bool) -> String {
    format!(
        r#"mesh: {{ model: "brute", defaultState: "walk", animations: {{
            walk: {{ clip: "walk", loop: true }},
            smash_windup: {{ clip: "smash_windup", loop: true }},
            jab_quick: {{ clip: "jab_quick", loop: {looping} }},
            idle: {{ clip: "idle", loop: true }}
        }} }}, "#
    )
}

fn jab_mesh_lua(looping: bool) -> String {
    format!(
        r#"mesh = {{ model = "brute", defaultState = "walk", animations = {{
            walk = {{ clip = "walk", loop = true }},
            smash_windup = {{ clip = "smash_windup", loop = true }},
            jab_quick = {{ clip = "jab_quick", loop = {looping} }},
            idle = {{ clip = "idle", loop = true }}
        }} }}, "#
    )
}

const JS_COMBO: &str = r#", queueRange: 6, release: { animation: "jab_quick", repeat: 3 }, recover: { animation: "idle" }"#;
// `repeat` is a Luau keyword, so a Luau table spells the key `["repeat"]`.
const LUA_COMBO: &str = r#", queueRange = 6, release = { animation = "jab_quick", ["repeat"] = 3 }, recover = { animation = "idle" }"#;

// A release's `repeat` and an attack's `queueRange` load alike in
// both runtimes: the release carries its pass count, the attack its queue
// distance, and `maxRange` stays the hit distance.
#[test]
fn a_repeating_release_and_a_queue_range_load_in_both_runtimes() {
    let js_mesh = jab_mesh_js(true);
    let lua_mesh = jab_mesh_lua(true);
    let js = js_loaded(&Brute {
        slam: JS_COMBO,
        mesh: &js_mesh,
        ..JS_BRUTE
    })
    .behavior
    .expect("behavior");
    let lua = lua_loaded(&Brute {
        slam: LUA_COMBO,
        mesh: &lua_mesh,
        ..LUA_BRUTE
    })
    .behavior
    .expect("behavior");
    assert_eq!(js, lua);
    assert_eq!(js.envelope.activities["slam_release"].passes, Some(3));
    assert_eq!(js.envelope.activities["slam_recover"].passes, None);
    let slam = &js.attacks["slam"];
    assert_eq!((slam.queue_range, slam.max_range), (Some(6.0), Some(3.0)));

    // `repeat: 1` is the default, and lowers to no pass count.
    let single = js_loaded(&Brute {
        slam: r#", release: { animation: "smash_strike", repeat: 1 }"#,
        ..JS_BRUTE
    })
    .behavior
    .expect("behavior");
    assert_eq!(single, js_loaded(&JS_BRUTE).behavior.expect("behavior"));
}

#[test]
fn a_malformed_repeat_or_queue_range_rejects_load_in_both_runtimes() {
    let release = "`components.behavior.transitions.overhead[0].to.release.repeat`";
    for (js_value, lua_value) in [("0", "0"), ("9", "9"), ("2.5", "2.5"), (r#""3""#, r#""3""#)] {
        assert_rejects_in_both(
            &format!("repeat {js_value}"),
            &Brute {
                slam: &format!(
                    r#", release: {{ animation: "jab_quick", repeat: {js_value} }}, recover: {{ animation: "idle" }}"#
                ),
                ..JS_BRUTE
            },
            &Brute {
                slam: &format!(
                    r#", release = {{ animation = "jab_quick", ["repeat"] = {lua_value} }}, recover = {{ animation = "idle" }}"#
                ),
                ..LUA_BRUTE
            },
            &[release],
        );
    }
    assert_rejects_in_both(
        "repeat on entry",
        &Brute {
            slam: r#", release: { animation: "jab_quick", at: false, repeat: 3 }, recover: { animation: "idle" }"#,
            ..JS_BRUTE
        },
        &Brute {
            slam: r#", release = { animation = "jab_quick", at = false, ["repeat"] = 3 }, recover = { animation = "idle" }"#,
            ..LUA_BRUTE
        },
        &[release, "fires on entry (`at: false`)"],
    );
    assert_rejects_in_both(
        "repeat without a recover animation",
        &Brute {
            slam: r#", release: { animation: "jab_quick", repeat: 3 }"#,
            ..JS_BRUTE
        },
        &Brute {
            slam: r#", release = { animation = "jab_quick", ["repeat"] = 3 }"#,
            ..LUA_BRUTE
        },
        &[
            "`components.behavior.transitions.overhead[0].to.recover.animation` is missing",
            "repeats 3 passes on a looping state",
        ],
    );
    for motion in ["chaseTarget", "strafe", "reposition"] {
        let moves = format!("repeats 3 passes but moves (`motion: \"{motion}\"`)");
        assert_rejects_in_both(
            &format!("repeat while moving ({motion})"),
            &Brute {
                slam: &format!(
                    r#", release: {{ animation: "jab_quick", motion: "{motion}", repeat: 3 }}, recover: {{ animation: "idle" }}"#
                ),
                ..JS_BRUTE
            },
            &Brute {
                slam: &format!(
                    r#", release = {{ animation = "jab_quick", motion = "{motion}", ["repeat"] = 3 }}, recover = {{ animation = "idle" }}"#
                ),
                ..LUA_BRUTE
            },
            &[release, &moves, "A repeating release is planted"],
        );
    }
    // `motion: "hold"` is the default, and a repeating release may name it.
    let js_mesh = jab_mesh_js(true);
    js_loaded(&Brute {
        slam: r#", release: { animation: "jab_quick", motion: "hold", repeat: 3 }, recover: { animation: "idle" }"#,
        mesh: &js_mesh,
        ..JS_BRUTE
    });
    assert_rejects_in_both(
        "repeat recovering on the release's own state",
        &Brute {
            slam: r#", release: { animation: "jab_quick", repeat: 3 }, recover: { animation: "jab_quick" }"#,
            ..JS_BRUTE
        },
        &Brute {
            slam: r#", release = { animation = "jab_quick", ["repeat"] = 3 }, recover = { animation = "jab_quick" }"#,
            ..LUA_BRUTE
        },
        &[
            "`components.behavior.transitions.overhead[0].to.recover.animation`",
            r#"repeats 3 passes on "jab_quick", and its recover plays that same looping state"#,
        ],
    );
    assert_rejects_in_both(
        "queue range under the reach",
        &Brute {
            slam: r#", queueRange: 2, release: { animation: "smash_strike" }"#,
            ..JS_BRUTE
        },
        &Brute {
            slam: r#", queueRange = 2, release = { animation = "smash_strike" }"#,
            ..LUA_BRUTE
        },
        &[
            "`components.behavior.attacks.slam.queueRange` must be a finite value >= `components.behavior.attacks.slam.maxRange` (3)",
        ],
    );
}

// A repeating release must play a looping state, checked where the
// mesh and the lowered behavior meet: the entity readers.
#[test]
fn a_repeating_release_on_a_one_shot_state_rejects_load_in_both_runtimes() {
    let js_mesh = jab_mesh_js(false);
    let lua_mesh = jab_mesh_lua(false);
    assert_rejects_in_both(
        "one-shot release state",
        &Brute {
            slam: JS_COMBO,
            mesh: &js_mesh,
            ..JS_BRUTE
        },
        &Brute {
            slam: LUA_COMBO,
            mesh: &lua_mesh,
            ..LUA_BRUTE
        },
        &[
            r#"attack "slam"'s release repeats 3 passes on animation state "jab_quick", which does not loop"#,
            "`components.mesh.animations.jab_quick`",
        ],
    );
    // A single pass may play a one-shot.
    let js_mesh = jab_mesh_js(false);
    js_loaded(&Brute {
        slam: r#", release: { animation: "jab_quick" }"#,
        mesh: &js_mesh,
        ..JS_BRUTE
    });
}

// A leaping release loads alike in both runtimes: its release activity
// carries `motion: "leap"`, its `speed` (the leap's cap) and the timed `hit`
// it arrives by. A leap needs a moment to arrive by, and has one.
#[test]
fn a_leaping_release_loads_and_rejects_with_its_path_in_both_runtimes() {
    let js = js_loaded(&Brute {
        slam: r#", release: { animation: "smash_strike", motion: "leap", speed: 14 }, recover: { animation: "idle" }"#,
        ..JS_BRUTE
    })
    .behavior
    .expect("behavior");
    let lua = lua_loaded(&Brute {
        slam: r#", release = { animation = "smash_strike", motion = "leap", speed = 14 }, recover = { animation = "idle" }"#,
        ..LUA_BRUTE
    })
    .behavior
    .expect("behavior");
    assert_eq!(js, lua);
    let release = &js.envelope.activities["slam_release"];
    assert_eq!(release.motion, Some(postretro_foundation::MotionVerb::Leap));
    assert_eq!(release.speed, Some(14.0));
    assert!(matches!(
        release.action,
        Some(postretro_foundation::ActionVerb::Attack { at: Some(ref at), .. }) if at == "hit"
    ));

    assert_rejects_in_both(
        "leap on entry",
        &Brute {
            slam: r#", release: { animation: "smash_strike", motion: "leap", at: false }, recover: { animation: "idle" }"#,
            ..JS_BRUTE
        },
        &Brute {
            slam: r#", release = { animation = "smash_strike", motion = "leap", at = false }, recover = { animation = "idle" }"#,
            ..LUA_BRUTE
        },
        &[
            "`components.behavior.transitions.overhead[0].to.release.motion`",
            "leaps (`motion: \"leap\"`), which arrives at its moment, but fires on entry (`at: false`)",
        ],
    );
    assert_rejects_in_both(
        "repeating leap",
        &Brute {
            slam: r#", release: { animation: "jab_quick", motion: "leap", repeat: 3 }, recover: { animation: "idle" }"#,
            ..JS_BRUTE
        },
        &Brute {
            slam: r#", release = { animation = "jab_quick", motion = "leap", ["repeat"] = 3 }, recover = { animation = "idle" }"#,
            ..LUA_BRUTE
        },
        &[
            "`components.behavior.transitions.overhead[0].to.release.repeat`",
            "repeats 3 passes but leaps (`motion: \"leap\"`), which arrives at one moment",
        ],
    );
    // A leap moves, so its recover needs its own animation.
    assert_rejects_in_both(
        "leap without a recover animation",
        &Brute {
            slam: r#", release: { animation: "smash_strike", motion: "leap" }"#,
            ..JS_BRUTE
        },
        &Brute {
            slam: r#", release = { animation = "smash_strike", motion = "leap" }"#,
            ..LUA_BRUTE
        },
        &["`components.behavior.transitions.overhead[0].to.recover` is missing"],
    );
    // A `recover` present but naming no animation is rejected the same way.
    assert_rejects_in_both(
        "leap with a recover without animation",
        &Brute {
            slam: r#", release: { animation: "smash_strike", motion: "leap" }, recover: { ms: 600 }"#,
            ..JS_BRUTE
        },
        &Brute {
            slam: r#", release = { animation = "smash_strike", motion = "leap" }, recover = { ms = 600 }"#,
            ..LUA_BRUTE
        },
        &[
            "`components.behavior.transitions.overhead[0].to.recover.animation` is missing",
            "release moves",
        ],
    );
}
