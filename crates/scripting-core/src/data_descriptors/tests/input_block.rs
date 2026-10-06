// Tests: manifest `input` block drains, QuickJS and Luau twins.

use super::super::*;
use super::common::*;
use log::Level;
use postretro_test_log_capture::LogCapture;

const SCOPE: &str = "test manifest";

fn drain_js(manifest: &str) -> Option<ModInputBlock> {
    eval_js(&format!("({manifest})"), |_, value| {
        let obj = Object::from_value(value).expect("fixture must evaluate to an object");
        drain_input_block_js(&obj, SCOPE).expect("an input block never rejects the manifest")
    })
}

fn drain_lua(manifest: &str) -> Option<ModInputBlock> {
    eval_lua(&format!("return {manifest}"), |value| {
        let LuaValue::Table(table) = value else {
            panic!("fixture must evaluate to a table");
        };
        drain_input_block_lua(&table, SCOPE).expect("an input block never rejects the manifest")
    })
}

fn drain_lua_chunk(chunk: &str) -> Option<ModInputBlock> {
    eval_lua(chunk, |value| {
        let LuaValue::Table(table) = value else {
            panic!("fixture must evaluate to a table");
        };
        drain_input_block_lua(&table, SCOPE).expect("an input block never rejects the manifest")
    })
}

fn warnings(capture: &LogCapture) -> Vec<String> {
    capture
        .records()
        .into_iter()
        .filter(|record| record.level == Level::Warn)
        .map(|record| record.message)
        .collect()
}

fn assert_not_warned(capture: &LogCapture) {
    let warnings = warnings(capture);
    assert!(
        warnings.is_empty(),
        "expected no warnings, got {warnings:?}"
    );
}

fn binding(input: &str, activator: Option<&str>, threshold: Option<f64>) -> ModInputBinding {
    ModInputBinding {
        input: input.to_string(),
        activator: activator.map(str::to_string),
        threshold,
    }
}

fn command(id: &str) -> ModInputCommand {
    ModInputCommand {
        id: id.to_string(),
        ..ModInputCommand::default()
    }
}

/// The brief's Scripting-surface example, in each runtime's syntax.
const JS_EXAMPLE: &str = r#"{
    id: "acme.neon",
    input: {
        commands: {
            dash: {
                label: "Dash", category: "Movement", order: 30,
                keyboardMouse: [{ input: "ShiftLeft", activator: "tap", threshold: 0.2 }],
                gamepad: [{ input: "left_stick_press" }],
            },
            sprint: { keyboardMouse: [{ input: "ShiftLeft", activator: "hold" }], gamepad: [] },
            alt_fire: { show: false },
        },
        glyphs: { keyboardMouse: "ui/glyphs/kbm", xbox: "ui/glyphs/xbox",
                  playstation: "ui/glyphs/ps", nintendo: "ui/glyphs/nx" },
    },
}"#;

const LUAU_EXAMPLE: &str = r#"{
    id = "acme.neon",
    input = {
        commands = {
            dash = {
                label = "Dash", category = "Movement", order = 30,
                keyboardMouse = { { input = "ShiftLeft", activator = "tap", threshold = 0.2 } },
                gamepad = { { input = "left_stick_press" } },
            },
            sprint = { keyboardMouse = { { input = "ShiftLeft", activator = "hold" } }, gamepad = {} },
            alt_fire = { show = false },
        },
        glyphs = { keyboardMouse = "ui/glyphs/kbm", xbox = "ui/glyphs/xbox",
                   playstation = "ui/glyphs/ps", nintendo = "ui/glyphs/nx" },
    },
}"#;

fn example_dash() -> ModInputCommand {
    ModInputCommand {
        label: Some("Dash".to_string()),
        category: Some("Movement".to_string()),
        order: Some(30.0),
        keyboard_mouse: Some(vec![binding("ShiftLeft", Some("tap"), Some(0.2))]),
        gamepad: Some(vec![binding("left_stick_press", None, None)]),
        ..command("dash")
    }
}

fn example_sprint() -> ModInputCommand {
    ModInputCommand {
        keyboard_mouse: Some(vec![binding("ShiftLeft", Some("hold"), None)]),
        gamepad: Some(Vec::new()),
        ..command("sprint")
    }
}

fn example_alt_fire() -> ModInputCommand {
    ModInputCommand {
        show: Some(false),
        ..command("alt_fire")
    }
}

fn example_glyphs() -> ModInputGlyphs {
    ModInputGlyphs {
        keyboard_mouse: Some("ui/glyphs/kbm".to_string()),
        xbox: Some("ui/glyphs/xbox".to_string()),
        playstation: Some("ui/glyphs/ps".to_string()),
        nintendo: Some("ui/glyphs/nx".to_string()),
    }
}

#[test]
fn input_block_scripting_surface_example_drains_in_both_runtimes() {
    let capture = LogCapture::start();
    assert_eq!(
        drain_js(JS_EXAMPLE),
        Some(ModInputBlock {
            // QuickJS keeps authored key order.
            commands: vec![example_dash(), example_sprint(), example_alt_fire()],
            glyphs: example_glyphs(),
        })
    );
    assert_eq!(
        drain_lua(LUAU_EXAMPLE),
        Some(ModInputBlock {
            // Luau tables carry no key order: sorted by command ID.
            commands: vec![example_alt_fire(), example_dash(), example_sprint()],
            glyphs: example_glyphs(),
        })
    );
    assert_not_warned(&capture);
}

#[test]
fn input_block_absent_or_null_drains_to_none_silently() {
    let capture = LogCapture::start();
    for js in ["{ id: 'none' }", "{ input: undefined }", "{ input: null }"] {
        assert_eq!(drain_js(js), None, "QuickJS fixture `{js}`");
    }
    for luau in ["{ id = 'none' }", "{ input = nil }"] {
        assert_eq!(drain_lua(luau), None, "Luau fixture `{luau}`");
    }
    assert_not_warned(&capture);
}

#[test]
fn input_block_empty_device_list_unbinds_and_absent_list_keeps_the_engine_default() {
    let capture = LogCapture::start();
    let expected = Some(ModInputBlock {
        commands: vec![ModInputCommand {
            keyboard_mouse: Some(Vec::new()),
            gamepad: None,
            ..command("jump")
        }],
        glyphs: ModInputGlyphs::default(),
    });
    assert_eq!(
        drain_js("{ input: { commands: { jump: { keyboardMouse: [] } } } }"),
        expected
    );
    assert_eq!(
        drain_lua("{ input = { commands = { jump = { keyboardMouse = {} } } } }"),
        expected
    );
    assert_not_warned(&capture);
}

#[test]
fn input_block_without_commands_or_glyphs_drains_to_an_empty_block_silently() {
    let capture = LogCapture::start();
    let expected = Some(ModInputBlock::default());
    assert_eq!(drain_js("{ input: {} }"), expected);
    assert_eq!(drain_js("{ input: { commands: {} } }"), expected);
    assert_eq!(drain_lua("{ input = {} }"), expected);
    assert_eq!(drain_lua("{ input = { commands = {} } }"), expected);
    assert_not_warned(&capture);
}

#[test]
fn input_block_that_is_not_an_object_warns_once_and_drains_to_none() {
    let capture = LogCapture::start();
    for js in [
        "{ input: 5 }",
        "{ input: 'dash' }",
        "{ input: [] }",
        "{ input: () => 1 }",
    ] {
        capture.clear();
        assert_eq!(drain_js(js), None, "QuickJS fixture `{js}`");
        capture.assert_logged_once(Level::Warn, "`input` must be an object");
    }
    for luau in [
        "{ input = 5 }",
        "{ input = 'dash' }",
        "{ input = { 1, 2 } }",
    ] {
        capture.clear();
        assert_eq!(drain_lua(luau), None, "Luau fixture `{luau}`");
        capture.assert_logged_once(Level::Warn, "`input` must be an object");
    }
}

#[test]
fn input_block_commands_that_are_not_an_object_warn_once_and_drain_empty() {
    let capture = LogCapture::start();
    let expected = Some(ModInputBlock {
        commands: Vec::new(),
        glyphs: ModInputGlyphs {
            xbox: Some("ui/glyphs/xbox".to_string()),
            ..ModInputGlyphs::default()
        },
    });
    for js in [
        "{ input: { commands: 3, glyphs: { xbox: 'ui/glyphs/xbox' } } }",
        "{ input: { commands: ['dash'], glyphs: { xbox: 'ui/glyphs/xbox' } } }",
    ] {
        capture.clear();
        assert_eq!(drain_js(js), expected, "QuickJS fixture `{js}`");
        capture.assert_logged_once(Level::Warn, "`input.commands` must be an object");
        assert_eq!(warnings(&capture).len(), 1);
    }
    for luau in [
        "{ input = { commands = 3, glyphs = { xbox = 'ui/glyphs/xbox' } } }",
        "{ input = { commands = { 'dash' }, glyphs = { xbox = 'ui/glyphs/xbox' } } }",
    ] {
        capture.clear();
        assert_eq!(drain_lua(luau), expected, "Luau fixture `{luau}`");
        capture.assert_logged_once(Level::Warn, "`input.commands` must be an object");
        assert_eq!(warnings(&capture).len(), 1);
    }
}

#[test]
fn input_block_command_that_is_not_an_object_is_skipped_with_one_warning() {
    let capture = LogCapture::start();
    let expected = Some(ModInputBlock {
        commands: vec![ModInputCommand {
            show: Some(true),
            ..command("jump")
        }],
        glyphs: ModInputGlyphs::default(),
    });
    assert_eq!(
        drain_js("{ input: { commands: { dash: 'ShiftLeft', jump: { show: true } } } }"),
        expected
    );
    capture.assert_logged_once(Level::Warn, "`input.commands.dash` must be an object");
    assert_eq!(warnings(&capture).len(), 1);
    capture.clear();
    assert_eq!(
        drain_lua("{ input = { commands = { dash = 'ShiftLeft', jump = { show = true } } } }"),
        expected
    );
    capture.assert_logged_once(Level::Warn, "`input.commands.dash` must be an object");
    assert_eq!(warnings(&capture).len(), 1);
}

#[test]
fn input_block_wrongly_typed_command_fields_drop_alone_with_one_warning_each() {
    let capture = LogCapture::start();
    let expected = Some(ModInputBlock {
        commands: vec![ModInputCommand {
            gamepad: Some(vec![binding("south", None, None)]),
            ..command("jump")
        }],
        glyphs: ModInputGlyphs::default(),
    });
    let fields = [
        "`input.commands.jump.label` must be a string",
        "`input.commands.jump.category` must be a string",
        "`input.commands.jump.order` must be a number",
        "`input.commands.jump.show` must be a boolean",
    ];
    assert_eq!(
        drain_js(
            "{ input: { commands: { jump: { label: 3, category: true, order: '30', show: 'yes', gamepad: [{ input: 'south' }] } } } }"
        ),
        expected
    );
    for field in fields {
        capture.assert_logged_once(Level::Warn, field);
    }
    assert_eq!(warnings(&capture).len(), fields.len());
    capture.clear();
    assert_eq!(
        drain_lua(
            "{ input = { commands = { jump = { label = 3, category = true, order = '30', show = 'yes', gamepad = { { input = 'south' } } } } } }"
        ),
        expected
    );
    for field in fields {
        capture.assert_logged_once(Level::Warn, field);
    }
    assert_eq!(warnings(&capture).len(), fields.len());
}

#[test]
fn input_block_device_class_that_is_not_an_array_warns_once_and_keeps_the_engine_default() {
    let capture = LogCapture::start();
    let expected = Some(ModInputBlock {
        commands: vec![ModInputCommand {
            keyboard_mouse: None,
            gamepad: Some(vec![binding("south", None, None)]),
            ..command("jump")
        }],
        glyphs: ModInputGlyphs::default(),
    });
    for js in [
        "{ input: { commands: { jump: { keyboardMouse: 'Space', gamepad: [{ input: 'south' }] } } } }",
        "{ input: { commands: { jump: { keyboardMouse: {}, gamepad: [{ input: 'south' }] } } } }",
        "{ input: { commands: { jump: { keyboardMouse: { input: 'Space' }, gamepad: [{ input: 'south' }] } } } }",
    ] {
        capture.clear();
        assert_eq!(drain_js(js), expected, "QuickJS fixture `{js}`");
        capture.assert_logged_once(
            Level::Warn,
            "`input.commands.jump.keyboardMouse` must be an array of bindings",
        );
        assert_eq!(warnings(&capture).len(), 1);
    }
    for luau in [
        "{ input = { commands = { jump = { keyboardMouse = 'Space', gamepad = { { input = 'south' } } } } } }",
        // A keyed table is an object, not a list.
        "{ input = { commands = { jump = { keyboardMouse = { input = 'Space' }, gamepad = { { input = 'south' } } } } } }",
        // A sparse sequence is not a list.
        "{ input = { commands = { jump = { keyboardMouse = { [1] = { input = 'Space' }, [3] = { input = 'KeyJ' } }, gamepad = { { input = 'south' } } } } } }",
    ] {
        capture.clear();
        assert_eq!(drain_lua(luau), expected, "Luau fixture `{luau}`");
        capture.assert_logged_once(
            Level::Warn,
            "`input.commands.jump.keyboardMouse` must be an array of bindings",
        );
        assert_eq!(warnings(&capture).len(), 1);
    }
}

/// Malformed binding entries keep their slot in a form the engine diagnoses,
/// so the drain itself stays silent: one diagnostic per malformed binding.
fn assert_malformed_bindings(bindings: Option<ModInputBlock>, runtime: &str) {
    let block = bindings.unwrap_or_else(|| panic!("{runtime}: block must drain"));
    let [jump] = block.commands.as_slice() else {
        panic!("{runtime}: expected one command, got {:?}", block.commands);
    };
    let entries = jump
        .keyboard_mouse
        .as_ref()
        .unwrap_or_else(|| panic!("{runtime}: keyboardMouse must drain"));
    assert_eq!(entries.len(), 6, "{runtime}: every entry keeps its slot");
    // Not an object.
    assert_eq!(entries[0], binding("", None, None), "{runtime}");
    // Missing input.
    assert_eq!(entries[1], binding("", Some("tap"), None), "{runtime}");
    // Input not a string.
    assert_eq!(entries[2], binding("", None, None), "{runtime}");
    // Activator not a string.
    assert_eq!(entries[3], binding("Space", Some(""), None), "{runtime}");
    // Threshold not a number.
    assert_eq!(entries[4].input, "Space", "{runtime}");
    assert_eq!(entries[4].activator.as_deref(), Some("hold"), "{runtime}");
    assert!(
        entries[4].threshold.is_some_and(f64::is_nan),
        "{runtime}: a non-number threshold drains to NaN, got {:?}",
        entries[4].threshold
    );
    // A well-formed neighbour is untouched.
    assert_eq!(entries[5], binding("KeyJ", None, Some(0.5)), "{runtime}");
}

#[test]
fn input_block_malformed_binding_entries_keep_their_slot_for_engine_diagnosis() {
    let capture = LogCapture::start();
    assert_malformed_bindings(
        drain_js(
            "{ input: { commands: { jump: { keyboardMouse: [
                'Space',
                { activator: 'tap' },
                { input: 32 },
                { input: 'Space', activator: 1 },
                { input: 'Space', activator: 'hold', threshold: '0.3' },
                { input: 'KeyJ', threshold: 0.5 },
            ] } } } }",
        ),
        "QuickJS",
    );
    assert_malformed_bindings(
        drain_lua(
            "{ input = { commands = { jump = { keyboardMouse = {
                'Space',
                { activator = 'tap' },
                { input = 32 },
                { input = 'Space', activator = 1 },
                { input = 'Space', activator = 'hold', threshold = '0.3' },
                { input = 'KeyJ', threshold = 0.5 },
            } } } } }",
        ),
        "Luau",
    );
    assert_not_warned(&capture);
}

#[test]
fn input_block_malformed_glyphs_warn_once_and_drop_alone() {
    let capture = LogCapture::start();
    let not_an_object = Some(ModInputBlock::default());
    assert_eq!(
        drain_js("{ input: { glyphs: 'ui/glyphs' } }"),
        not_an_object
    );
    capture.assert_logged_once(Level::Warn, "`input.glyphs` must be an object");
    capture.clear();
    assert_eq!(
        drain_lua("{ input = { glyphs = 'ui/glyphs' } }"),
        not_an_object
    );
    capture.assert_logged_once(Level::Warn, "`input.glyphs` must be an object");

    let bad_family = Some(ModInputBlock {
        commands: Vec::new(),
        glyphs: ModInputGlyphs {
            keyboard_mouse: Some("ui/glyphs/kbm".to_string()),
            xbox: None,
            ..ModInputGlyphs::default()
        },
    });
    capture.clear();
    assert_eq!(
        drain_js("{ input: { glyphs: { keyboardMouse: 'ui/glyphs/kbm', xbox: 7 } } }"),
        bad_family
    );
    capture.assert_logged_once(Level::Warn, "`input.glyphs.xbox` must be a string");
    assert_eq!(warnings(&capture).len(), 1);
    capture.clear();
    assert_eq!(
        drain_lua("{ input = { glyphs = { keyboardMouse = 'ui/glyphs/kbm', xbox = 7 } } }"),
        bad_family
    );
    capture.assert_logged_once(Level::Warn, "`input.glyphs.xbox` must be a string");
    assert_eq!(warnings(&capture).len(), 1);
}

#[test]
fn input_block_cyclic_binding_value_terminates_as_an_empty_input() {
    let capture = LogCapture::start();
    let expected = Some(ModInputBlock {
        commands: vec![ModInputCommand {
            keyboard_mouse: Some(vec![binding("", None, None)]),
            ..command("jump")
        }],
        glyphs: ModInputGlyphs::default(),
    });
    let js = eval_js(
        "const loop = {}; loop.input = loop; ({ input: { commands: { jump: { keyboardMouse: [loop] } } } })",
        |_, value| {
            let obj = Object::from_value(value).expect("fixture must evaluate to an object");
            drain_input_block_js(&obj, SCOPE).expect("an input block never rejects the manifest")
        },
    );
    assert_eq!(js, expected);
    assert_eq!(
        drain_lua_chunk(
            "local loop = {}; loop.input = loop; return { input = { commands = { jump = { keyboardMouse = { loop } } } } }"
        ),
        expected
    );
    assert_not_warned(&capture);
}
