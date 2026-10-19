// Tests: `movement.viewFeel.pose` parsing, with JS/Luau parity on every
// accepted shape and every diagnostic.

use super::super::*;
use super::common::*;
use super::movement_view_feel::{js_movement_with_view_feel, lua_movement_with_view_feel};
use postretro_foundation::data_descriptors::{
    LedgeFocus, PoseChannels, PoseParams, PoseRow, VaultPoseRow,
};

/// The Luau spelling of a JS `viewFeel` literal that uses only plain keys,
/// numbers, strings and `null`.
fn to_lua(js: &str) -> String {
    js.replace(": ", " = ")
        .replace("null", "nil")
        .replace("-Infinity", "-math.huge")
        .replace("Infinity", "math.huge")
        .replace("NaN", "(0/0)")
}

fn view_pair(js_body: &str, lua_body: &str) -> [ViewFeelParams; 2] {
    let js = eval_js(&js_movement_with_view_feel(js_body), |ctx, v| {
        entity_descriptor_from_js(ctx, v)
            .unwrap_or_else(|error| panic!("JS rejected {js_body}: {error}"))
    });
    let lua = eval_lua(&lua_movement_with_view_feel(lua_body), |v| {
        entity_descriptor_from_lua(v)
            .unwrap_or_else(|error| panic!("Luau rejected {lua_body}: {error}"))
    });
    [js, lua].map(|descriptor| descriptor.movement.unwrap().view_feel.unwrap())
}

/// The pose both runtimes parse from one body, asserted equal.
#[track_caller]
fn pose_of(js_body: &str, lua_body: &str) -> Option<PoseParams> {
    let [js, lua] = view_pair(js_body, lua_body).map(|view| view.pose);
    assert_eq!(js, lua, "JS and Luau parse {js_body} differently");
    js
}

#[track_caller]
fn same_pose(js_body: &str) -> Option<PoseParams> {
    pose_of(js_body, &to_lua(js_body))
}

/// Both runtimes reject alike: the same message, containing `expected`.
#[track_caller]
fn assert_rejects(js_body: &str, lua_body: &str, expected: &str) {
    let js = eval_js(&js_movement_with_view_feel(js_body), |ctx, v| {
        entity_descriptor_from_js(ctx, v).map(|_| ())
    });
    let lua = eval_lua(&lua_movement_with_view_feel(lua_body), |v| {
        entity_descriptor_from_lua(v).map(|_| ())
    });
    let (Err(js), Err(lua)) = (js, lua) else {
        panic!("both runtimes must reject {js_body} / {lua_body}");
    };
    assert!(
        matches!(js, DescriptorError::InvalidShape { .. }),
        "JS: {js}"
    );
    let (js, lua) = (js.to_string(), lua.to_string());
    assert_eq!(js, lua, "JS and Luau word {js_body} differently");
    assert!(js.contains(expected), "{js_body}: {js}");
}

#[track_caller]
fn assert_same_rejection(js_body: &str, expected: &str) {
    assert_rejects(js_body, &to_lua(js_body), expected);
}

fn channels(pitch: f32, roll: f32, eye_drop: f32, fov: f32) -> PoseChannels {
    PoseChannels {
        pitch,
        roll,
        eye_drop,
        fov,
    }
}

#[test]
fn pose_parses_the_scripting_surface_example_identically_in_both_runtimes() {
    let example = "{ pose: { \
        vault: { ledgeFocus: { strength: 0.7, maxPitch: 30 }, roll: 10, fov: -30, easeIn: 0.33, easeOut: 0.33 }, \
        slide: { eyeDrop: 0.15, fov: 5, enterRate: 18, exitRate: 12 } } }";
    assert_eq!(
        same_pose(example),
        Some(PoseParams {
            slide: Some(PoseRow {
                channels: channels(0.0, 0.0, 0.15, 5.0),
                enter_rate: 18.0,
                exit_rate: 12.0,
            }),
            vault: Some(VaultPoseRow {
                channels: channels(0.0, 10.0, 0.0, -30.0),
                ease_in: 0.33,
                ease_out: 0.33,
                ledge_focus: Some(LedgeFocus {
                    strength: 0.7,
                    max_pitch: 30.0,
                }),
            }),
        })
    );
    // Every channel on both rows, and the vault's shares defaulted.
    assert_eq!(
        same_pose(
            "{ pose: { vault: { pitch: -38.5, eyeDrop: 0.1 }, \
             slide: { pitch: -2, roll: 3, enterRate: 1, exitRate: 2 } } }"
        ),
        Some(PoseParams {
            slide: Some(PoseRow {
                channels: channels(-2.0, 3.0, 0.0, 0.0),
                enter_rate: 1.0,
                exit_rate: 2.0,
            }),
            vault: Some(VaultPoseRow {
                channels: channels(-38.5, 0.0, 0.1, 0.0),
                ease_in: VaultPoseRow::DEFAULT_EASE_IN,
                ease_out: VaultPoseRow::DEFAULT_EASE_OUT,
                ledge_focus: None,
            }),
        })
    );
}

#[test]
fn pose_accepts_every_range_endpoint_in_both_runtimes() {
    let pose = same_pose(
        "{ pose: { \
         slide: { pitch: -90, roll: 90, eyeDrop: 0, fov: -90, enterRate: 0.1, exitRate: 240 }, \
         vault: { pitch: 90, roll: -90, fov: 90, easeIn: 0.8, ledgeFocus: { strength: 0, maxPitch: 89 } } } }",
    )
    .unwrap();
    assert_eq!(
        pose.slide,
        Some(PoseRow {
            channels: channels(-90.0, 90.0, 0.0, -90.0),
            enter_rate: 0.1,
            exit_rate: 240.0,
        })
    );
    let vault = pose.vault.unwrap();
    assert_eq!(vault.channels, channels(90.0, -90.0, 0.0, 90.0));
    // An authored share plus the widened default share fills the whole climb.
    assert_eq!((vault.ease_in, vault.ease_out), (0.8, 0.2));
    assert_eq!(
        vault.ledge_focus,
        Some(LedgeFocus {
            strength: 0.0,
            max_pitch: 89.0,
        })
    );
    let vault = same_pose(
        "{ pose: { vault: { easeIn: 0, easeOut: 1, ledgeFocus: { strength: 1, maxPitch: 0.001 } } } }",
    )
    .unwrap()
    .vault
    .unwrap();
    assert_eq!((vault.ease_in, vault.ease_out), (0.0, 1.0));
    assert_eq!(vault.ledge_focus.unwrap().strength, 1.0);
}

#[test]
fn pose_omitted_or_nil_yields_no_pose_and_an_omitted_row_no_row() {
    for body in ["{}", "{ pose: null }"] {
        assert_eq!(same_pose(body), None, "{body}");
    }
    let [view, _] = view_pair("{ pose: undefined }", "{ pose = nil }");
    assert_eq!(view.pose, None);
    assert_eq!(same_pose("{ pose: {} }"), Some(PoseParams::default()));
    let pose =
        same_pose("{ pose: { slide: null, vault: { ledgeFocus: null, pitch: null } } }").unwrap();
    assert_eq!(pose.slide, None);
    assert_eq!(
        pose.vault,
        Some(VaultPoseRow {
            channels: PoseChannels::default(),
            ease_in: VaultPoseRow::DEFAULT_EASE_IN,
            ease_out: VaultPoseRow::DEFAULT_EASE_OUT,
            ledge_focus: None,
        })
    );
    // A nil entry is absent: Luau cannot hold one, so a nullish unknown key
    // in JS is no key at all.
    assert_eq!(
        pose_of(
            "{ pose: { normal: null, dash: undefined } }",
            "{ pose = {} }"
        ),
        Some(PoseParams::default())
    );
}

#[test]
fn pose_rejects_a_missing_rate_naming_its_path() {
    for (body, expected) in [
        (
            "{ pose: { slide: { eyeDrop: 0.15, enterRate: 18 } } }",
            "`movement.viewFeel.pose.slide.exitRate` is required",
        ),
        (
            "{ pose: { slide: { exitRate: 12, enterRate: null } } }",
            "`movement.viewFeel.pose.slide.enterRate` is required",
        ),
        (
            "{ pose: { slide: {} } }",
            "`movement.viewFeel.pose.slide.enterRate` is required",
        ),
    ] {
        assert_same_rejection(body, expected);
    }
}

#[test]
fn pose_rejects_a_rate_on_vault_and_a_share_on_slide_saying_how_the_row_is_timed() {
    for rate in ["enterRate", "exitRate"] {
        assert_same_rejection(
            &format!("{{ pose: {{ vault: {{ pitch: -10, {rate}: 18 }} }} }}"),
            &format!(
                "`movement.viewFeel.pose.vault.{rate}` is not a vault pose field; the vault row is timed by the climb's progress through `easeIn` and `easeOut` shares, not rates"
            ),
        );
    }
    for share in ["easeIn", "easeOut"] {
        assert_same_rejection(
            &format!("{{ pose: {{ slide: {{ enterRate: 18, exitRate: 12, {share}: 0.2 }} }} }}"),
            &format!(
                "`movement.viewFeel.pose.slide.{share}` is not a slide pose field; the slide row is timed by `enterRate` and `exitRate`, not shares"
            ),
        );
    }
}

#[test]
fn pose_rejects_out_of_range_non_finite_and_non_numeric_values_naming_the_field() {
    const SLIDE: &str = "enterRate: 18, exitRate: 12";
    for (row, rest) in [("slide", SLIDE), ("vault", "")] {
        let path = format!("movement.viewFeel.pose.{row}");
        for (field, values, bounds) in [
            ("pitch", &["-90.5", "90.000001", "1e100"][..], "[-90, 90]"),
            ("roll", &["-90.0000001", "120"][..], "[-90, 90]"),
            ("fov", &["90.000001", "-91"][..], "[-90, 90]"),
            (
                "eyeDrop",
                &["-0.01", "-1e-100", "1e100", "3.5e38"][..],
                "[0, 3.4028234663852886e38]",
            ),
        ] {
            for value in values.iter().chain(&["Infinity", "-Infinity", "NaN"]) {
                let separator = if rest.is_empty() { "" } else { ", " };
                assert_same_rejection(
                    &format!("{{ pose: {{ {row}: {{ {rest}{separator}{field}: {value} }} }} }}"),
                    &format!("`{path}.{field}` must be finite in {bounds}, got "),
                );
            }
            for (js, lua) in [
                ("\"down\"", "\"down\""),
                ("true", "true"),
                ("{}", "{}"),
                ("[1]", "{ 1 }"),
            ] {
                let separator = if rest.is_empty() { "" } else { ", " };
                assert_rejects(
                    &format!("{{ pose: {{ {row}: {{ {rest}{separator}{field}: {js} }} }} }}"),
                    &format!(
                        "{{ pose = {{ {row} = {{ {}{separator}{field} = {lua} }} }} }}",
                        to_lua(rest)
                    ),
                    &format!("`{path}.{field}` must be a number"),
                );
            }
        }
    }
    for (body, expected) in [
        (
            "{ pose: { vault: { pitch: 90.000001 } } }",
            "`movement.viewFeel.pose.vault.pitch` must be finite in [-90, 90], got 90.000001",
        ),
        (
            "{ pose: { vault: { roll: Infinity } } }",
            "`movement.viewFeel.pose.vault.roll` must be finite in [-90, 90], got inf",
        ),
        (
            "{ pose: { slide: { enterRate: 18, exitRate: 12, eyeDrop: 1e100 } } }",
            "`movement.viewFeel.pose.slide.eyeDrop` must be finite in [0, 3.4028234663852886e38], got 1e100",
        ),
        // Rates and shares round onto their bounds as f32, so the f64 check
        // must see them first.
        (
            "{ pose: { slide: { enterRate: 0.0999999999, exitRate: 12 } } }",
            "`movement.viewFeel.pose.slide.enterRate` must be finite in [0.1, 240], got 0.0999999999",
        ),
        (
            "{ pose: { slide: { enterRate: 18, exitRate: 240.000001 } } }",
            "`movement.viewFeel.pose.slide.exitRate` must be finite in [0.1, 240], got 240.000001",
        ),
        (
            "{ pose: { slide: { enterRate: 0, exitRate: 12 } } }",
            "`movement.viewFeel.pose.slide.enterRate` must be finite in [0.1, 240], got 0",
        ),
        (
            "{ pose: { slide: { enterRate: NaN, exitRate: 12 } } }",
            "`movement.viewFeel.pose.slide.enterRate` must be finite in [0.1, 240], got NaN",
        ),
        (
            "{ pose: { vault: { easeIn: -1e-100 } } }",
            "`movement.viewFeel.pose.vault.easeIn` must be finite in [0, 1], got -1e-100",
        ),
        (
            "{ pose: { vault: { easeOut: 1.5 } } }",
            "`movement.viewFeel.pose.vault.easeOut` must be finite in [0, 1], got 1.5",
        ),
        (
            "{ pose: { vault: { ledgeFocus: { strength: 1.0000001, maxPitch: 30 } } } }",
            "`movement.viewFeel.pose.vault.ledgeFocus.strength` must be finite in [0, 1], got 1.0000001",
        ),
        (
            "{ pose: { vault: { ledgeFocus: { strength: 0.5, maxPitch: 0 } } } }",
            "`movement.viewFeel.pose.vault.ledgeFocus.maxPitch` must be finite in (0, 89], got 0",
        ),
        (
            "{ pose: { vault: { ledgeFocus: { strength: 0.5, maxPitch: 89.0000001 } } } }",
            "`movement.viewFeel.pose.vault.ledgeFocus.maxPitch` must be finite in (0, 89], got 89.0000001",
        ),
        (
            "{ pose: { vault: { ledgeFocus: { strength: 0.5, maxPitch: -Infinity } } } }",
            "`movement.viewFeel.pose.vault.ledgeFocus.maxPitch` must be finite in (0, 89], got -inf",
        ),
        (
            "{ pose: { vault: { ledgeFocus: { strength: \"full\", maxPitch: 30 } } } }",
            "`movement.viewFeel.pose.vault.ledgeFocus.strength` must be a number",
        ),
    ] {
        assert_same_rejection(body, expected);
    }
}

#[test]
fn pose_rejects_vault_shares_summing_past_one() {
    assert_same_rejection(
        "{ pose: { vault: { easeIn: 0.6, easeOut: 0.5 } } }",
        "`movement.viewFeel.pose.vault.easeIn` plus `movement.viewFeel.pose.vault.easeOut` must be at most 1, got 1.1",
    );
    // An authored share against the other's default.
    assert_same_rejection(
        "{ pose: { vault: { easeIn: 0.9 } } }",
        "`movement.viewFeel.pose.vault.easeIn` plus `movement.viewFeel.pose.vault.easeOut` must be at most 1, got 1.1",
    );
}

#[test]
fn pose_rejects_normal_and_other_unknown_rows() {
    for row in ["normal", "dash", "Slide", "wallRun"] {
        assert_same_rejection(
            &format!("{{ pose: {{ {row}: {{ pitch: -5 }} }} }}"),
            &format!("`movement.viewFeel.pose.{row}` is not a pose row; expected slide or vault"),
        );
    }
    // A bad value under an unknown row still names the row.
    assert_same_rejection(
        "{ pose: { normal: 3 } }",
        "`movement.viewFeel.pose.normal` is not a pose row",
    );
}

#[test]
fn pose_rejects_unknown_fields_in_rows_and_ledge_focus() {
    for (body, expected) in [
        (
            "{ pose: { vault: { pich: -10 } } }",
            "`movement.viewFeel.pose.vault.pich` is not a vault pose field; expected pitch, roll, eyeDrop, fov, easeIn, easeOut or ledgeFocus",
        ),
        (
            "{ pose: { slide: { enterRate: 18, exitRate: 12, drop: 0.1 } } }",
            "`movement.viewFeel.pose.slide.drop` is not a slide pose field; expected pitch, roll, eyeDrop, fov, enterRate or exitRate",
        ),
        (
            "{ pose: { slide: { enterRate: 18, exitRate: 12, fovIncrease: 5 } } }",
            "`movement.viewFeel.pose.slide.fovIncrease` has been renamed; author `movement.viewFeel.pose.slide.fov` instead",
        ),
        (
            "{ pose: { vault: { fovIncrease: 5 } } }",
            "`movement.viewFeel.pose.vault.fovIncrease` has been renamed; author `movement.viewFeel.pose.vault.fov` instead",
        ),
        (
            "{ pose: { vault: { ledgeFocus: { strength: 0.5, maxPitch: 30, max: 20 } } } }",
            "`movement.viewFeel.pose.vault.ledgeFocus.max` is not a ledge focus field; expected strength or maxPitch",
        ),
        // The smallest unknown key is named, whatever order a runtime keeps.
        (
            "{ pose: { vault: { zeta: 1, pitch: 2, alpha: 3, mid: 4 } } }",
            "`movement.viewFeel.pose.vault.alpha` is not a vault pose field",
        ),
    ] {
        assert_same_rejection(body, expected);
    }
}

#[test]
fn pose_rejects_ledge_focus_on_a_non_vault_row() {
    assert_same_rejection(
        "{ pose: { slide: { enterRate: 18, exitRate: 12, ledgeFocus: { strength: 0.5, maxPitch: 30 } } } }",
        "`movement.viewFeel.pose.slide.ledgeFocus` is not a slide pose field; `ledgeFocus` is vault-only",
    );
}

#[test]
fn pose_rejects_an_incomplete_ledge_focus_naming_the_missing_field() {
    for (focus, missing) in [
        ("{ strength: 0.7 }", "maxPitch"),
        ("{ maxPitch: 30 }", "strength"),
        ("{ strength: 0.7, maxPitch: null }", "maxPitch"),
        ("{}", "strength"),
    ] {
        assert_same_rejection(
            &format!("{{ pose: {{ vault: {{ ledgeFocus: {focus} }} }} }}"),
            &format!("`movement.viewFeel.pose.vault.ledgeFocus.{missing}` is required"),
        );
    }
}

#[test]
fn pose_rejects_blocks_that_are_not_objects() {
    for (js, lua, path) in [
        ("{ pose: 3 }", "{ pose = 3 }", "movement.viewFeel.pose"),
        (
            "{ pose: \"slide\" }",
            "{ pose = \"slide\" }",
            "movement.viewFeel.pose",
        ),
        ("{ pose: [] }", "{ pose = { 1 } }", "movement.viewFeel.pose"),
        (
            "{ pose: () => ({}) }",
            "{ pose = function() return {} end }",
            "movement.viewFeel.pose",
        ),
        (
            "{ pose: { vault: [1] } }",
            "{ pose = { vault = { -6, 2 } } }",
            "movement.viewFeel.pose.vault",
        ),
        (
            "{ pose: { vault: { [Symbol.iterator]: 1, pitch: -6 } } }",
            "{ pose = { vault = { [1.5] = 2, pitch = -6 } } }",
            "movement.viewFeel.pose.vault",
        ),
        (
            "{ pose: { slide: true } }",
            "{ pose = { slide = true } }",
            "movement.viewFeel.pose.slide",
        ),
        (
            "{ pose: { vault: { ledgeFocus: 0.5 } } }",
            "{ pose = { vault = { ledgeFocus = 0.5 } } }",
            "movement.viewFeel.pose.vault.ledgeFocus",
        ),
    ] {
        assert_rejects(js, lua, &format!("`{path}` must be an object"));
    }
}

#[test]
fn pose_rejects_an_inherited_pose_block_row_or_field_naming_it() {
    for (js, lua, expected) in [
        // `viewFeel` inherits `pose`, by a table or a function `__index`.
        (
            "Object.create({ pose: { vault: { pitch: -10 } } })",
            "setmetatable({}, { __index = { pose = { vault = { pitch = -10 } } } })",
            "`movement.viewFeel.pose` is inherited; author it directly on `movement.viewFeel`",
        ),
        (
            "Object.create({ get pose() { return {}; } })",
            "setmetatable({}, { __index = function(_, key) if key == \"pose\" then return {} end end })",
            "`movement.viewFeel.pose` is inherited; author it directly on `movement.viewFeel`",
        ),
        // `pose` inherits a row.
        (
            "{ pose: Object.create({ vault: { pitch: -10 } }) }",
            "{ pose = setmetatable({}, { __index = { vault = { pitch = -10 } } }) }",
            "`movement.viewFeel.pose.vault` is inherited; author it directly on `movement.viewFeel.pose`",
        ),
        // A row inherits a field, even one it also owns; the smallest
        // inherited key is named.
        (
            "{ pose: { vault: Object.assign(Object.create({ roll: 5, pitch: -10 }), { pitch: -2 }) } }",
            "{ pose = { vault = setmetatable({ pitch = -2 }, { __index = { roll = 5, pitch = -10 } }) } }",
            "`movement.viewFeel.pose.vault.pitch` is inherited; author it directly on `movement.viewFeel.pose.vault`",
        ),
        // Through a two-step chain.
        (
            "{ pose: { slide: Object.assign(Object.create(Object.create({ fov: 5 })), { enterRate: 18, exitRate: 12 }) } }",
            "{ pose = { slide = setmetatable({ enterRate = 18, exitRate = 12 }, { __index = setmetatable({}, { __index = { fov = 5 } }) }) } }",
            "`movement.viewFeel.pose.slide.fov` is inherited; author it directly on `movement.viewFeel.pose.slide`",
        ),
        (
            "{ pose: { vault: { ledgeFocus: Object.assign(Object.create({ maxPitch: 30 }), { strength: 0.5 }) } } }",
            "{ pose = { vault = { ledgeFocus = setmetatable({ strength = 0.5 }, { __index = { maxPitch = 30 } }) } } }",
            "`movement.viewFeel.pose.vault.ledgeFocus.maxPitch` is inherited; author it directly on `movement.viewFeel.pose.vault.ledgeFocus`",
        ),
        // A chain past the walk bound cannot be named.
        (
            "{ pose: { vault: (() => { let o = {}; for (let i = 0; i < 20; i++) o = Object.create(o); return o; })() } }",
            "{ pose = { vault = (function() local t = {} for _ = 1, 20 do t = setmetatable({}, { __index = t }) end return t end)() } }",
            "`movement.viewFeel.pose.vault` inherits fields through its prototype or `__index`; author them directly on `movement.viewFeel.pose.vault`",
        ),
    ] {
        assert_rejects(js, lua, expected);
    }
    // Luau alone can hide what it supplies behind a function `__index`.
    let error = eval_lua(
        &lua_movement_with_view_feel(
            "{ pose = { vault = setmetatable({}, { __index = function() return -10 end }) } }",
        ),
        |v| entity_descriptor_from_lua(v).unwrap_err(),
    );
    assert!(
        error.to_string().contains(
            "`movement.viewFeel.pose.vault` inherits fields through its prototype or `__index`; author them directly on `movement.viewFeel.pose.vault`"
        ),
        "{error}"
    );
    // Nothing inherited: a metatable with no `__index`, an empty `__index`
    // table, a nullish prototype entry, and the built-in prototypes.
    assert_eq!(
        pose_of(
            "{ pose: { vault: Object.assign(Object.create({ roll: undefined, fov: null }), { pitch: -2 }) } }",
            "{ pose = { vault = setmetatable({ pitch = -2 }, { __index = {} }) } }",
        ),
        pose_of(
            "{ pose: Object.assign(Object.create(null), { vault: { pitch: -2 } }) }",
            "{ pose = setmetatable({ vault = { pitch = -2 } }, { __newindex = function() end }) }",
        ),
    );
}

#[test]
fn view_feel_rejects_leftover_slide_and_vault_blocks_naming_their_pose_replacements() {
    for (body, expected) in [
        (
            "{ slide: { eyeDrop: 0.15, fovIncrease: 5, enterRate: 18, exitRate: 12 } }",
            "`movement.viewFeel.slide` has moved; author `movement.viewFeel.pose.slide` instead",
        ),
        (
            "{ vault: { pitch: -38.5 }, pose: { vault: { pitch: -10 } } }",
            "`movement.viewFeel.vault` has moved; author `movement.viewFeel.pose.vault` instead",
        ),
        (
            "{ vault: 3 }",
            "`movement.viewFeel.vault` has moved; author `movement.viewFeel.pose.vault` instead",
        ),
    ] {
        assert_same_rejection(body, expected);
    }
    // A nil leftover is no block.
    assert_eq!(same_pose("{ slide: null, vault: null }"), None);
}
