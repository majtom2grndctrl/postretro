// Tests: UI bridge: factories, manifest drains, theme.

use super::super::*;
use super::common::*;

#[test]
fn js_bridge_capture_envelope_and_interactive_widgets_round_trip() {
    // A capture-mode tree with initialFocus + a grid of interactive widgets,
    // covering button/slider/bar, color tokens, binds, and styleRanges.
    let src = r#"({
        anchor: "center", offset: [0.0, 0.0], captureMode: "capture", initialFocus: "resume",
        root: {
            kind: "vstack", gap: "m", padding: "s", align: "center", focus: "linear",
            children: [
                { kind: "button", id: "resume", label: "Resume", onPress: "resumeGame",
                  focusNeighbors: { down: "vol" } },
                { kind: "slider", id: "vol", label: "Volume", bind: { slot: "audio.master" },
                  min: 0.0, max: 1.0, step: 0.1, capturesNav: ["nav.left", "nav.right"] },
                { kind: "bar", bind: { slot: "player.health" }, max: 100.0,
                  fill: "ok", background: [0.1, 0.1, 0.1, 1.0],
                  styleRanges: { max: 100.0, entries: [ { upTo: 0.25, color: "critical" }, { color: "ok" } ] } }
            ]
        }
    })"#;
    let tree = eval_js(src, |ctx, v| {
        anchored_tree_from_js_value(ctx, v).expect("must convert")
    });
    assert_eq!(tree.capture_mode, CaptureMode::Capture);
    assert_eq!(tree.initial_focus.as_deref(), Some("resume"));

    let expected = r#"{"anchor":"center","offset":[0.0,0.0],"root":{"kind":"vstack","gap":"m","padding":"s","align":"center","focus":"linear","children":[{"kind":"button","id":"resume","label":"Resume","onPress":"resumeGame","focusNeighbors":{"down":"vol"}},{"kind":"slider","id":"vol","label":"Volume","bind":{"slot":"audio.master"},"min":0.0,"max":1.0,"step":0.1,"capturesNav":["nav.left","nav.right"]},{"kind":"bar","bind":{"slot":"player.health"},"max":100.0,"fill":"ok","background":[0.1,0.1,0.1,1.0],"styleRanges":{"max":100.0,"entries":[{"upTo":0.25,"color":"critical"},{"color":"ok"}]}}]},"captureMode":"capture","initialFocus":"resume"}"#;
    assert_eq!(serde_json::to_string(&tree).unwrap(), expected);
}

#[test]
fn js_bridge_malformed_tree_surfaces_named_error_not_panic() {
    // Unknown widget kind → InvalidShape (a named DescriptorError), no panic.
    let bad_kind = r#"({ anchor: "center", offset: [0.0, 0.0], root: { kind: "carousel" } })"#;
    let err = eval_js(bad_kind, |ctx, v| {
        anchored_tree_from_js_value(ctx, v).unwrap_err()
    });
    assert!(matches!(err, DescriptorError::InvalidShape { .. }));

    // Missing required `root` → MissingField.
    let no_root = r#"({ anchor: "center", offset: [0.0, 0.0] })"#;
    let err = eval_js(no_root, |ctx, v| {
        anchored_tree_from_js_value(ctx, v).unwrap_err()
    });
    assert_eq!(err, DescriptorError::MissingField { field: "root" });

    // Bad anchor literal → InvalidShape.
    let bad_anchor =
        r#"({ anchor: "middle", offset: [0.0, 0.0], root: { kind: "spacer", flexGrow: 1.0 } })"#;
    let err = eval_js(bad_anchor, |ctx, v| {
        anchored_tree_from_js_value(ctx, v).unwrap_err()
    });
    assert!(matches!(err, DescriptorError::InvalidShape { .. }));
}

/// The end-to-end AC: run the ACTUAL Luau `widgets`/`layout`/`tree` factories
/// under mlua, pass the produced descriptor value through the bridge, and
/// assert the typed result + a byte-identical re-serialization. This is the
/// validation that ties Tasks 3/4/5 together for the Luau runtime.
#[test]
fn luau_factories_through_bridge_round_trip_byte_identically() {
    const WIDGETS_SRC: &str = include_str!("../../../../../../sdk/lib/ui/widgets.luau");
    const LAYOUT_SRC: &str = include_str!("../../../../../../sdk/lib/ui/layout.luau");
    const TREE_SRC: &str = include_str!("../../../../../../sdk/lib/ui/tree.luau");

    let lua = mlua::Lua::new();
    install_ui_theme_token_validator(&lua);
    let widgets: mlua::Table = lua.load(WIDGETS_SRC).eval().unwrap();
    let layout: mlua::Table = lua.load(LAYOUT_SRC).eval().unwrap();
    let tree_mod: mlua::Table = lua.load(TREE_SRC).eval().unwrap();
    lua.globals().set("W", widgets).unwrap();
    lua.globals().set("L", layout).unwrap();
    lua.globals().set("T", tree_mod).unwrap();

    // Build the same all-kinds tree via the factories. (gap/padding/align are
    // authored explicitly to match the byte-identical wire fixture.)
    let src = r#"return T.Tree({ anchor = "center", offset = { 10, -20 } },
        L.VStack({ gap = 4, padding = 8, align = "start" }, {
            W.Text({ content = "hello", fontSize = 18, color = {1,1,1,1} }),
            W.Panel({ fill = {0.1,0.2,0.3,1}, border = { texture = "ui/frame", slice = {8,8,8,8}, tint = {1,1,1,1} } }),
            L.HStack({ gap = 2, padding = 0, align = "center" }, {
                -- M13 G2: the bridge now enforces image name-XOR-decorative. The
                -- SDK Image factory gains a `decorative`/`label` arg in Task 4
                -- (SDK factories+typedefs); until then these decorative icons are
                -- authored as raw tables so this round-trip fixture stays valid.
                { kind = "image", asset = "ui/logo", decorative = true },
                W.Spacer({ flexGrow = 1 }),
            }),
            L.Grid({ gap = 1, padding = 3, align = "stretch", cols = 2 }, {
                { kind = "image", asset = "ui/icon", decorative = true },
            }),
        }))"#;
    let value: mlua::Value = lua.load(src).eval().expect("factories must build a tree");
    let tree = anchored_tree_from_lua_value(value).expect("bridge must convert factory output");

    assert_eq!(tree.anchor, Anchor::Center);
    let Widget::VStack(root) = &tree.root else {
        panic!("root must be a vstack");
    };
    assert_eq!(root.children.len(), 4);

    assert_eq!(serde_json::to_string(&tree).unwrap(), UI_ALL_KINDS_WIRE);
}

#[test]
fn luau_modder_component_is_a_plain_function_nesting_inside_an_sdk_container() {
    // The modder-component convention (M13 G1b, Task 6): a modder component is
    // a PLAIN FUNCTION returning a descriptor subtree — no defineComponent, no
    // decorator, no inheritance. It takes the same props-first(-then-children)
    // shape as an SDK factory and nests inside SDK containers. Here a plain
    // `Labeled` function returns an `HStack` subtree; it is called with a props
    // object exactly like `L.HStack`, and its result nests inside an `L.VStack`
    // — then the whole mixed tree passes through the SAME G1a bridge an
    // all-factory tree does, proving the component is callable and nestable with
    // no special machinery.
    const WIDGETS_SRC: &str = include_str!("../../../../../../sdk/lib/ui/widgets.luau");
    const LAYOUT_SRC: &str = include_str!("../../../../../../sdk/lib/ui/layout.luau");
    const TREE_SRC: &str = include_str!("../../../../../../sdk/lib/ui/tree.luau");

    let lua = mlua::Lua::new();
    install_ui_theme_token_validator(&lua);
    let widgets: mlua::Table = lua.load(WIDGETS_SRC).eval().unwrap();
    let layout: mlua::Table = lua.load(LAYOUT_SRC).eval().unwrap();
    let tree_mod: mlua::Table = lua.load(TREE_SRC).eval().unwrap();
    lua.globals().set("W", widgets).unwrap();
    lua.globals().set("L", layout).unwrap();
    lua.globals().set("T", tree_mod).unwrap();

    // A modder component: a plain function, props-first, returning a subtree
    // built from SDK factories. No registration, no base class.
    let src = r#"
        -- modder component: plain function, props-first, returns a subtree.
        local function Labeled(props)
            return L.HStack({ gap = 2, padding = 0, align = "center" }, {
                W.Text({ content = props.label, fontSize = 14, color = {1,1,1,1} }),
                W.Text({ content = props.value, fontSize = 14, color = {1,1,1,1} }),
            })
        end

        return T.Tree({ anchor = "topLeft", offset = { 0, 0 } },
            L.VStack({ gap = 4, padding = 8, align = "start" }, {
                -- the modder component nests inside the SDK container exactly
                -- like a factory call would.
                Labeled({ label = "HP", value = "100" }),
                W.Spacer({ flexGrow = 1 }),
            }))
    "#;
    let value: mlua::Value = lua.load(src).eval().expect("mixed tree must build");
    let tree = anchored_tree_from_lua_value(value)
        .expect("bridge converts the mixed factory+component tree");

    // The root SDK container holds the modder component's subtree as its first
    // child, structurally identical to an inline `HStack` — the bridge sees no
    // difference between a factory call and a plain-function component.
    let Widget::VStack(root) = &tree.root else {
        panic!("root must be a vstack");
    };
    assert_eq!(
        root.children.len(),
        2,
        "container + component child + spacer"
    );
    let Widget::HStack(labeled) = &root.children[0] else {
        panic!(
            "the modder component returned an hstack subtree, got {:?}",
            root.children[0]
        );
    };
    assert_eq!(labeled.children.len(), 2, "the component's two text runs");
    assert!(matches!(labeled.children[0], Widget::Text(_)));
    assert!(matches!(root.children[1], Widget::Spacer(_)));

    // The mixed tree re-serializes byte-identically through the descriptor's
    // own serde — the component's output is indistinguishable from inline
    // factory output on the wire.
    let expected = r#"{"anchor":"topLeft","offset":[0.0,0.0],"root":{"kind":"vstack","gap":4.0,"padding":8.0,"align":"start","children":[{"kind":"hstack","gap":2.0,"padding":0.0,"align":"center","children":[{"kind":"text","content":"HP","fontSize":14.0,"color":[1.0,1.0,1.0,1.0]},{"kind":"text","content":"100","fontSize":14.0,"color":[1.0,1.0,1.0,1.0]}]},{"kind":"spacer","flexGrow":1.0}]}}"#;
    assert_eq!(serde_json::to_string(&tree).unwrap(), expected);
}

#[test]
fn luau_empty_container_parses_as_explicit_children_array() {
    // The critical Task 3 note: an EMPTY Luau container `children` table would
    // serialize to `{}` (not `[]`) through the generic lua_to_json walker. The
    // bridge reads `children` straight into a `Vec<Widget>`, so an empty
    // container parses cleanly and re-serializes with the required `"children":[]`.
    const LAYOUT_SRC: &str = include_str!("../../../../../../sdk/lib/ui/layout.luau");
    const TREE_SRC: &str = include_str!("../../../../../../sdk/lib/ui/tree.luau");
    let lua = mlua::Lua::new();
    install_ui_theme_token_validator(&lua);
    let layout: mlua::Table = lua.load(LAYOUT_SRC).eval().unwrap();
    let tree_mod: mlua::Table = lua.load(TREE_SRC).eval().unwrap();
    lua.globals().set("L", layout).unwrap();
    lua.globals().set("T", tree_mod).unwrap();

    let src = r#"return T.Tree({ anchor = "topLeft", offset = { 0, 0 } },
        L.VStack({ gap = 0, padding = 0, align = "start" }, {}))"#;
    let value: mlua::Value = lua.load(src).eval().unwrap();
    let tree = anchored_tree_from_lua_value(value).expect("empty container must convert");

    let Widget::VStack(root) = &tree.root else {
        panic!("root must be a vstack");
    };
    assert!(root.children.is_empty(), "children must be empty");

    let expected = r#"{"anchor":"topLeft","offset":[0.0,0.0],"root":{"kind":"vstack","gap":0.0,"padding":0.0,"align":"start","children":[]}}"#;
    let reserialized = serde_json::to_string(&tree).unwrap();
    assert_eq!(reserialized, expected);
    assert!(
        reserialized.contains(r#""children":[]"#),
        "empty container must serialize children as an empty ARRAY, got: {reserialized}"
    );
}

#[test]
fn luau_bridge_malformed_tree_surfaces_named_error_not_panic() {
    // A non-table root → InvalidShape (named error, no panic).
    let lua = mlua::Lua::new();
    let value: mlua::Value = lua
        .load(r#"return { anchor = "center", offset = { 0, 0 }, root = 42 }"#)
        .eval()
        .unwrap();
    let err = anchored_tree_from_lua_value(value).unwrap_err();
    assert!(matches!(err, DescriptorError::InvalidShape { .. }));

    // A missing root → MissingField.
    let value: mlua::Value = lua
        .load(r#"return { anchor = "center", offset = { 0, 0 } }"#)
        .eval()
        .unwrap();
    let err = anchored_tree_from_lua_value(value).unwrap_err();
    assert_eq!(err, DescriptorError::MissingField { field: "root" });

    // An unknown widget kind → InvalidShape.
    let value: mlua::Value = lua
        .load(r#"return { anchor = "center", offset = { 0, 0 }, root = { kind = "carousel" } }"#)
        .eval()
        .unwrap();
    let err = anchored_tree_from_lua_value(value).unwrap_err();
    assert!(matches!(err, DescriptorError::InvalidShape { .. }));
}

#[test]
fn js_and_luau_bridges_agree_on_one_tree() {
    // The same authored tree through both runtimes yields byte-identical wire
    // output — the cross-runtime parity guarantee for the bridge.
    const WIDGETS_SRC: &str = include_str!("../../../../../../sdk/lib/ui/widgets.luau");
    const TREE_SRC: &str = include_str!("../../../../../../sdk/lib/ui/tree.luau");
    let lua = mlua::Lua::new();
    install_ui_theme_token_validator(&lua);
    let widgets: mlua::Table = lua.load(WIDGETS_SRC).eval().unwrap();
    let tree_mod: mlua::Table = lua.load(TREE_SRC).eval().unwrap();
    lua.globals().set("W", widgets).unwrap();
    lua.globals().set("T", tree_mod).unwrap();
    let lua_value: mlua::Value = lua
        .load(
            r#"return T.Tree({ anchor = "center", offset = { 0, 0 } },
                W.Text({ content = "hi", fontSize = 12, color = {1,1,1,1} }))"#,
        )
        .eval()
        .unwrap();
    let lua_tree = anchored_tree_from_lua_value(lua_value).unwrap();
    let lua_wire = serde_json::to_string(&lua_tree).unwrap();

    let js_tree = eval_js(
        r#"({ anchor: "center", offset: [0.0, 0.0], root: { kind: "text", content: "hi", fontSize: 12.0, color: [1.0, 1.0, 1.0, 1.0] } })"#,
        |ctx, v| anchored_tree_from_js_value(ctx, v).unwrap(),
    );
    let js_wire = serde_json::to_string(&js_tree).unwrap();

    assert_eq!(lua_wire, js_wire);
}

// ======================================================================
// Level-manifest `uiTrees` drain (both level parsers)
// ======================================================================

#[test]
fn level_manifest_js_drains_ui_trees() {
    let manifest = eval_js(
        r#"({
            reactions: [],
            uiTrees: [
                { name: "objective", alwaysOn: true,
                  tree: { anchor: "top", offset: [0.0, 8.0],
                          root: { kind: "spacer", flexGrow: 1.0 } } },
            ],
        })"#,
        |ctx, v| LevelManifest::from_js_value(ctx, v).expect("must parse"),
    );
    assert_eq!(manifest.ui_trees.len(), 1);
    assert_eq!(manifest.ui_trees[0].name, "objective");
    assert!(manifest.ui_trees[0].always_on);
}

#[test]
fn level_manifest_js_skips_malformed_ui_tree() {
    let manifest = eval_js(
        r#"({
            reactions: [],
            uiTrees: [
                { name: "bad", tree: { anchor: "top", offset: [0.0, 0.0], root: { kind: "carousel" } } },
                { name: "good", tree: { anchor: "top", offset: [0.0, 0.0], root: { kind: "spacer", flexGrow: 1.0 } } },
            ],
        })"#,
        |ctx, v| LevelManifest::from_js_value(ctx, v).expect("malformed entry must not abort"),
    );
    assert_eq!(manifest.ui_trees.len(), 1);
    assert_eq!(manifest.ui_trees[0].name, "good");
}

#[test]
fn level_manifest_luau_drains_ui_trees() {
    let lua = mlua::Lua::new();
    let value: mlua::Value = lua
        .load(
            r#"return {
                reactions = {},
                uiTrees = {
                    { name = "objective", alwaysOn = true,
                      tree = { anchor = "top", offset = { 0, 8 },
                               root = { kind = "spacer", flexGrow = 1 } } },
                },
            }"#,
        )
        .eval()
        .unwrap();
    let manifest = LevelManifest::from_lua_value(value).expect("must parse");
    assert_eq!(manifest.ui_trees.len(), 1);
    assert_eq!(manifest.ui_trees[0].name, "objective");
    assert!(manifest.ui_trees[0].always_on);
}

#[test]
fn level_manifest_luau_skips_malformed_ui_tree() {
    let lua = mlua::Lua::new();
    let value: mlua::Value = lua
        .load(
            r#"return {
                reactions = {},
                uiTrees = {
                    { name = "bad", tree = { anchor = "top", offset = { 0, 0 }, root = { kind = "carousel" } } },
                    { name = "good", tree = { anchor = "top", offset = { 0, 0 }, root = { kind = "spacer", flexGrow = 1 } } },
                },
            }"#,
        )
        .eval()
        .unwrap();
    let manifest = LevelManifest::from_lua_value(value).expect("malformed entry must not abort");
    assert_eq!(manifest.ui_trees.len(), 1);
    assert_eq!(manifest.ui_trees[0].name, "good");
}

// ======================================================================
// Fix A: Luau `buildBind` accepts `{ local }` presentation-cell binds
// ======================================================================

/// Drive the REAL `widgets.luau` factory with a `{ local }` bind (not a
/// `{ slot }` bind), then pass the result through `anchored_tree_from_lua_value`.
/// Asserts the bridge yields a `BindSource::Local`, mirroring the TS twin which
/// accepts either `{ slot }` or `{ local }`.
#[test]
fn luau_factory_local_bind_yields_bind_source_local() {
    const WIDGETS_SRC: &str = include_str!("../../../../../../sdk/lib/ui/widgets.luau");
    const TREE_SRC: &str = include_str!("../../../../../../sdk/lib/ui/tree.luau");

    let lua = mlua::Lua::new();
    install_ui_theme_token_validator(&lua);
    let widgets: mlua::Table = lua.load(WIDGETS_SRC).eval().unwrap();
    let tree_mod: mlua::Table = lua.load(TREE_SRC).eval().unwrap();
    lua.globals().set("W", widgets).unwrap();
    lua.globals().set("T", tree_mod).unwrap();

    // Use W.Text with a `{ ["local"] = "count" }` bind — the Lua keyword
    // `local` must be escaped as a string key.
    let src = r#"return T.Tree({ anchor = "center", offset = { 0, 0 } },
        W.Text({ content = "0", fontSize = 18, color = {1,1,1,1},
                 bind = { ["local"] = "count" } }))"#;
    let value: mlua::Value = lua
        .load(src)
        .eval()
        .expect("factory with {local} bind must succeed");
    let tree = anchored_tree_from_lua_value(value).expect("bridge must convert {local} bind tree");

    let Widget::Text(t) = &tree.root else {
        panic!("root must be a text widget");
    };
    assert_eq!(
        t.bind.as_ref().map(|b| &b.source),
        Some(&BindSource::Local {
            local: "count".into()
        }),
        "bind source must be Local{{local: \"count\"}}"
    );
}

// ======================================================================
// Fix B: malformed Luau theme token is skipped, good token survives
// ======================================================================

// A `theme.colors` map with one malformed token (a string instead of
// [r,g,b,a]) and one valid token: the bad token must be skipped (with a
// logged warn) and the good token must survive in the drained result.
// This verifies per-token log-and-skip rather than per-token abort.
// ======================================================================
// Fix A: JS theme sub-map that is present but not an object degrades to
// empty (warn + continue), matching the Luau twin behavior.
// ======================================================================

/// A JS `theme` with `colors: 5` (non-object sub-map) must NOT abort the
/// mod manifest drain. The `colors` sub-map degrades to empty, and the rest
/// of the theme (fonts, spacing) is still drained correctly.
#[test]
fn drain_theme_js_degrades_non_object_sub_map_to_empty() {
    let tokens = eval_js(
        r#"({
            theme: {
                colors: 5,
                fonts: { primary: "Inter" },
                spacing: { m: 8 },
            },
        })"#,
        |_ctx, v| {
            let obj = Object::from_value(v).expect("must be an object");
            drain_theme_js(&obj, "test").expect("non-object colors must not abort the drain")
        },
    );
    assert!(
        tokens.colors.is_empty(),
        "colors must degrade to empty when the sub-map is not an object"
    );
    assert_eq!(
        tokens.fonts.get("primary").map(String::as_str),
        Some("Inter"),
        "fonts must still be drained when colors is bad"
    );
    assert_eq!(
        tokens.spacing.get("m").copied(),
        Some(8.0f32),
        "spacing must still be drained when colors is bad"
    );
}

#[test]
fn drain_theme_lua_skips_bad_token_and_keeps_good_token() {
    let lua = mlua::Lua::new();
    let value: mlua::Value = lua
        .load(
            r#"return {
                theme = {
                    colors = {
                        bad  = "not-an-rgba-array",
                        good = { 1.0, 0.0, 0.0, 1.0 },
                    },
                },
            }"#,
        )
        .eval()
        .unwrap();
    let LuaValue::Table(table) = value else {
        panic!("expected table");
    };
    let tokens = drain_theme_lua(&table, "test").expect("bad token must not abort the whole drain");
    assert!(
        !tokens.colors.contains_key("bad"),
        "the malformed color token must be skipped"
    );
    assert_eq!(
        tokens.colors.get("good"),
        Some(&[1.0f32, 0.0, 0.0, 1.0]),
        "the valid color token must survive"
    );
}
