// Mod-init UI-field drains (uiTrees, theme, fonts) across both runtimes.

use super::*;

// --- G1b Task 1: mod-init UI field drains (cold-boot path) --------------

/// Cold-boot JS: the default manifest export carrying `uiTrees`, `theme`,
/// and `fonts` drains each field via the G1a bridge fns onto the manifest.
#[test]
fn mod_init_quickjs_drains_ui_trees_theme_and_fonts() {
    let (mut rt, _ctx) = runtime();
    let dir = temp_mod_root("js_ui_fields");
    std::fs::write(
            dir.join("start-script.js"),
            r#"
            globalThis.__postretroModManifest = {
                name: "UiMod",
                uiTrees: [
                    { name: "hud", alwaysOn: true,
                      tree: { anchor: "topLeft", offset: [0.0, 0.0],
                              root: { kind: "text", content: "hi", fontSize: 12.0, color: [1.0,1.0,1.0,1.0] } } },
                ],
                theme: { colors: { critical: [1.0, 0.0, 0.0, 1.0] }, spacing: { m: 8.0 } },
                frontend: {
                    menuTree: "mainMenu",
                    backgroundLevel: "menu_backdrop",
                    camera: { position: [4.0, 2.0, 8.0], yaw: -0.6, pitch: -0.1 },
                },
                fonts: { primary: "fonts/inter.ttf" },
            };
            "#,
        )
        .unwrap();

    rt.run_mod_init(&dir).unwrap();
    let manifest = rt.mod_manifest().expect("Some manifest");
    assert_eq!(manifest.ui_trees.len(), 1);
    assert_eq!(manifest.ui_trees[0].name, "hud");
    assert!(manifest.ui_trees[0].always_on);
    assert_eq!(manifest.theme.colors["critical"], [1.0, 0.0, 0.0, 1.0]);
    assert_eq!(manifest.theme.spacing["m"], 8.0);
    let frontend = manifest.frontend.as_ref().expect("frontend drained");
    assert_eq!(frontend.menu_tree, "mainMenu");
    assert_eq!(frontend.background_level.as_deref(), Some("menu_backdrop"));
    assert_eq!(frontend.camera.position, [4.0, 2.0, 8.0]);
    assert_eq!(frontend.camera.yaw, -0.6);
    assert_eq!(frontend.camera.pitch, -0.1);
    assert_eq!(manifest.fonts.families["primary"], "fonts/inter.ttf");
}

#[test]
fn mod_init_quickjs_drains_returned_production_hud_contract() {
    use crate::render::ui::descriptor::{BarMax, Widget};

    let (mut rt, _ctx) = runtime();
    let dir = temp_mod_root("js_production_hud_contract");
    std::fs::write(
            dir.join("start-script.js"),
            r#"
            globalThis.__postretroModManifest = (() => {
                const theme = defineTheme({
                    color: {
                        hud: {
                            panel: [0.018, 0.026, 0.039, 0.82],
                            health: {
                                background: [0.035, 0.045, 0.060, 1.0],
                            },
                            text: [0.82, 0.95, 0.98, 1.0],
                        },
                        critical: [0.86, 0.06, 0.12, 1.0],
                        warning: [0.95, 0.62, 0.12, 1.0],
                        ok: [0.12, 0.72, 0.40, 1.0],
                        panel: {
                            default: [0.018, 0.026, 0.039, 0.92],
                        },
                    },
                    font: {
                        hud: { status: "JetBrains Mono" },
                        primary: "JetBrains Mono",
                        mono: "JetBrains Mono",
                    },
                    spacing: {
                        hud: { gap: 8.0, padding: 14.0, rowGap: 6.0 },
                        m: 8.0,
                        l: 16.0,
                    },
                });
                const tokens = getDesignTokens(theme);
                const player = getGameState().player;
                const healthTree = Tree(
                    { anchor: "bottomLeft", offset: [24.0, -24.0] },
                    VStack({ gap: tokens.spacing.hud.rowGap, padding: tokens.spacing.hud.padding, align: "stretch", fill: tokens.color.hud.panel }, [
                        HStack({ gap: tokens.spacing.hud.gap, align: "center" }, [
                            Text({
                                content: "HP --",
                                color: tokens.color.hud.text,
                                font: tokens.font.hud.status,
                                fontSize: 24.0,
                                bind: bindState(player.health, { format: "HP {}" }),
                            }),
                        ]),
                        Bar({
                            bind: bindState(player.health, {
                                tween: { durationMs: 180.0, easing: "easeOut" },
                            }),
                            max: player.maxHealth,
                            fill: tokens.color.ok,
                            background: tokens.color.hud.health.background,
                            styleRanges: {
                                max: 1.0,
                                entries: [
                                    { upTo: 0.25, color: tokens.color.critical },
                                    { upTo: 0.5, color: tokens.color.warning },
                                    { color: tokens.color.ok },
                                ],
                            },
                        }),
                    ]),
                );
                const reticleTree = Tree(
                    { anchor: "center", offset: [0.0, 0.0] },
                    Text({ content: "+", font: tokens.font.mono }),
                );
                const pauseMenu = Tree(
                    {
                        anchor: "center",
                        offset: [0.0, 0.0],
                        captureMode: "capture",
                        initialFocus: "pauseResume",
                        accessibleName: "Pause menu",
                        role: "group",
                    },
                    VStack({
                        gap: tokens.spacing.m,
                        padding: tokens.spacing.l,
                        align: "stretch",
                        focus: { policy: "linear", wrap: true },
                        fill: tokens.color.panel.default,
                    }, [
                        Text({ content: "PAUSED", font: tokens.font.mono, color: tokens.color.ok }),
                        Button({ id: "pauseResume", label: "RESUME", onPress: CLOSE_DIALOG_ACTION }),
                    ]),
                );
                return {
                    name: "HudMod",
                    uiTrees: [
                        defineUiTree({ name: "hud", tree: healthTree, alwaysOn: true }),
                        defineUiTree({ name: "hud.reticle", tree: reticleTree, alwaysOn: true }),
                        defineUiTree({ name: "pauseMenu", tree: pauseMenu }),
                    ],
                    theme,
                };
            })();
            "#,
        )
        .unwrap();

    rt.run_mod_init(&dir).unwrap();
    let manifest = rt.mod_manifest().expect("Some manifest");
    assert!(
        manifest.store_declarations.is_empty(),
        "production HUD/pause-menu contract no longer needs demo stores",
    );

    assert_eq!(manifest.ui_trees.len(), 3);
    let hud = manifest
        .ui_trees
        .iter()
        .find(|tree| tree.name == "hud")
        .expect("hud tree returned");
    assert!(
        hud.always_on,
        "alwaysOn belongs to the registration envelope"
    );
    let reticle = manifest
        .ui_trees
        .iter()
        .find(|tree| tree.name == "hud.reticle")
        .expect("reticle tree returned");
    assert!(reticle.always_on);
    let Widget::Text(reticle_text) = &reticle.tree.root else {
        panic!("reticle root is text");
    };
    assert_eq!(reticle_text.content, "+");
    assert_eq!(reticle_text.font.as_deref(), Some("mono"));
    let pause_menu = manifest
        .ui_trees
        .iter()
        .find(|tree| tree.name == "pauseMenu")
        .expect("pause menu tree returned");
    assert!(
        !pause_menu.always_on,
        "pause menu is pushed-only, never an always-on layer"
    );
    assert_eq!(
        pause_menu.tree.capture_mode,
        crate::render::ui::descriptor::CaptureMode::Capture
    );
    assert_eq!(
        pause_menu.tree.initial_focus.as_deref(),
        Some("pauseResume")
    );
    assert_eq!(
        pause_menu.tree.accessible_name.as_deref(),
        Some("Pause menu")
    );
    assert_eq!(
        pause_menu.tree.role,
        Some(crate::render::ui::descriptor::Role::Group)
    );
    let Widget::VStack(pause_root) = &pause_menu.tree.root else {
        panic!("pause menu root is a vstack");
    };
    assert_eq!(
        pause_root.focus.as_ref().map(|focus| focus.kind()),
        Some(crate::render::ui::descriptor::FocusKind::Linear)
    );
    assert_eq!(
        pause_root.focus.as_ref().map(|focus| focus.wrap()),
        Some(true)
    );
    assert_eq!(pause_root.children.len(), 2);
    let Widget::Button(resume) = &pause_root.children[1] else {
        panic!("pause menu second child is resume button");
    };
    assert_eq!(resume.id, "pauseResume");
    assert_eq!(resume.on_press, "ui.closeDialog");

    let Widget::VStack(root) = &hud.tree.root else {
        panic!("hud root is a vstack");
    };
    let Widget::HStack(row) = &root.children[0] else {
        panic!("hud first child is a row");
    };
    let Widget::Text(status) = &row.children[0] else {
        panic!("hud status row contains text");
    };
    assert_eq!(
        status.bind.as_ref().and_then(|bind| bind.source.slot()),
        Some("player.health"),
    );
    assert_eq!(
        status.bind.as_ref().and_then(|bind| bind.format.as_deref()),
        Some("HP {}"),
    );

    let Widget::Bar(bar) = &root.children[1] else {
        panic!("hud second child is a bar");
    };
    assert_eq!(bar.bind.source.slot(), Some("player.health"));
    assert_eq!(
        bar.bind.tween.as_ref().map(|tween| tween.duration_ms),
        Some(180.0),
    );
    match &bar.max {
        BarMax::State(reference) => assert_eq!(reference.slot, "player.maxHealth"),
        other => panic!("bar max must be direct player.maxHealth ref, got {other:?}"),
    }
    let ranges = bar
        .style_ranges
        .as_ref()
        .expect("health bar declares normalized ranges");
    assert_eq!(ranges.max, 1.0);
    assert_eq!(ranges.entries[0].up_to, Some(0.25));
    assert_eq!(ranges.entries[1].up_to, Some(0.5));

    assert_eq!(
        manifest.theme.colors["hud.health.background"],
        [0.035, 0.045, 0.060, 1.0]
    );
    assert_eq!(manifest.theme.fonts["hud.status"], "JetBrains Mono");
    assert_eq!(manifest.theme.spacing["hud.padding"], 14.0);
}

/// Cold-boot JS: a malformed `uiTrees` entry (unknown widget kind) is logged
/// and skipped — mod-init still succeeds and other trees survive.
#[test]
fn mod_init_quickjs_malformed_ui_tree_is_skipped_not_aborted() {
    let (mut rt, _ctx) = runtime();
    let dir = temp_mod_root("js_ui_malformed");
    std::fs::write(
            dir.join("start-script.js"),
            r#"
            globalThis.__postretroModManifest = {
                name: "UiMod",
                uiTrees: [
                    { name: "bad", tree: { anchor: "topLeft", offset: [0.0, 0.0], root: { kind: "carousel" } } },
                    { name: "good", tree: { anchor: "topLeft", offset: [0.0, 0.0],
                        root: { kind: "spacer", flexGrow: 1.0 } } },
                ],
            };
            "#,
        )
        .unwrap();

    rt.run_mod_init(&dir)
        .expect("malformed UI tree must not abort mod-init");
    let manifest = rt.mod_manifest().expect("Some manifest");
    assert_eq!(
        manifest.ui_trees.len(),
        1,
        "the malformed tree must be skipped and the good one kept"
    );
    assert_eq!(manifest.ui_trees[0].name, "good");
}

/// Cold-boot Luau: the returned mod manifest carrying `uiTrees`, `theme`,
/// and `fonts` drains each field via the G1a Luau bridge fns onto the manifest.
#[test]
fn mod_init_luau_drains_ui_trees_theme_and_fonts() {
    let (mut rt, _ctx) = runtime();
    let dir = temp_mod_root("luau_ui_fields");
    std::fs::write(
            dir.join("start-script.luau"),
            r#"
            return {
                name = "UiMod",
                uiTrees = {
                    { name = "hud", alwaysOn = true,
                      tree = { anchor = "topLeft", offset = { 0, 0 },
                               root = { kind = "text", content = "hi", fontSize = 12, color = {1,1,1,1} } } },
                },
                theme = { colors = { critical = {1, 0, 0, 1} }, spacing = { m = 8 } },
                frontend = {
                    menuTree = "mainMenu",
                    backgroundLevel = "menu_backdrop",
                    camera = { position = {4, 2, 8}, yaw = -0.6, pitch = -0.1 },
                },
                fonts = { primary = "fonts/inter.ttf" },
            }
            "#,
        )
        .unwrap();

    rt.run_mod_init(&dir).unwrap();
    let manifest = rt.mod_manifest().expect("Some manifest");
    assert_eq!(manifest.ui_trees.len(), 1);
    assert_eq!(manifest.ui_trees[0].name, "hud");
    assert!(manifest.ui_trees[0].always_on);
    assert_eq!(manifest.theme.colors["critical"], [1.0, 0.0, 0.0, 1.0]);
    assert_eq!(manifest.theme.spacing["m"], 8.0);
    let frontend = manifest.frontend.as_ref().expect("frontend drained");
    assert_eq!(frontend.menu_tree, "mainMenu");
    assert_eq!(frontend.background_level.as_deref(), Some("menu_backdrop"));
    assert_eq!(frontend.camera.position, [4.0, 2.0, 8.0]);
    assert_eq!(frontend.camera.yaw, -0.6);
    assert_eq!(frontend.camera.pitch, -0.1);
    assert_eq!(manifest.fonts.families["primary"], "fonts/inter.ttf");
}

/// Cold-boot Luau: a malformed `uiTrees` entry is logged and skipped.
#[test]
fn mod_init_luau_malformed_ui_tree_is_skipped_not_aborted() {
    let (mut rt, _ctx) = runtime();
    let dir = temp_mod_root("luau_ui_malformed");
    std::fs::write(
            dir.join("start-script.luau"),
            r#"
            return {
                name = "UiMod",
                uiTrees = {
                    { name = "bad", tree = { anchor = "topLeft", offset = { 0, 0 }, root = { kind = "carousel" } } },
                    { name = "good", tree = { anchor = "topLeft", offset = { 0, 0 },
                        root = { kind = "spacer", flexGrow = 1 } } },
                },
            }
            "#,
        )
        .unwrap();

    rt.run_mod_init(&dir)
        .expect("malformed UI tree must not abort mod-init");
    let manifest = rt.mod_manifest().expect("Some manifest");
    assert_eq!(manifest.ui_trees.len(), 1);
    assert_eq!(manifest.ui_trees[0].name, "good");
}

#[test]
fn mod_init_luau_require_resolves_from_mod_root() {
    let (mut rt, _ctx) = runtime();
    let dir = temp_mod_root("luau_require");
    // Sub-module returns a descriptor; start-script imports it and folds
    // it into the manifest's `entities` field.
    std::fs::write(
        dir.join("sub.luau"),
        r#"
            return { descriptor = { canonicalName = "smoke_pillar" } }
            "#,
    )
    .unwrap();
    std::fs::write(
        dir.join("start-script.luau"),
        r#"
            local m = require("./sub")
            return { name = "Imported", entities = { m.descriptor } }
            "#,
    )
    .unwrap();

    rt.run_mod_init(&dir).unwrap();
    let manifest = rt.mod_manifest().expect("Some manifest");
    assert_eq!(manifest.name, "Imported");
    assert!(
        manifest
            .entities
            .iter()
            .any(|e| e.canonical_name.as_deref() == Some("smoke_pillar")),
        "domain script imported via require must contribute its entity type to the manifest"
    );
}

#[test]
fn mod_init_luau_virtual_sdk_modules_bypass_shadow_files() {
    let (mut rt, _ctx) = runtime();
    let dir = temp_mod_root("luau_virtual_sdk");
    fs::create_dir_all(dir.join("postretro")).unwrap();
    fs::write(
        dir.join("postretro.luau"),
        r#"error("shadow postretro.luau was read")"#,
    )
    .unwrap();
    fs::write(
        dir.join("postretro/ui.luau"),
        r#"error("shadow postretro/ui.luau was read")"#,
    )
    .unwrap();
    fs::write(
            dir.join("start-script.luau"),
            r#"
            local root = require("postretro")
            local rootAgain = require("postretro")
            local UI = require("postretro/ui")
            local UIAgain = require("postretro/ui")
            assert(root == rootAgain, "postretro must be a singleton")
            assert(UI == UIAgain, "postretro/ui must be a singleton")
            assert(root.shadow == nil, "mod-root postretro.luau shadowed the engine module")
            assert(UI.shadow == nil, "mod-root postretro/ui.luau shadowed the engine module")
            assert(root.Text == nil, "root module must not expose UI factories")
            assert(root.showDialog == nil, "root module must not expose UI reactions")
            assert(UI.Text({ content = "mod" }).kind == "text", "UI.Text must be the SDK factory")
            assert(UI.getGameState().player.health.slot == "player.health", "UI getGameState must expose refs")
            assert(type(Text) == "nil", "bare-global SDK Text must not be installed")
            assert(not pcall(function()
                root.world = nil
            end), "postretro must reject writes")
            assert(not pcall(function()
                UI.Text = nil
            end), "postretro/ui must reject writes")
            return { name = "VirtualUiMod" }
            "#,
        )
        .unwrap();

    rt.run_mod_init(&dir).unwrap();
    let manifest = rt.mod_manifest().expect("Some manifest");
    assert_eq!(manifest.name, "VirtualUiMod");
}

#[test]
fn mod_init_luau_require_rejects_parent_dir_traversal() {
    let (mut rt, _ctx) = runtime();
    let dir = temp_mod_root("luau_require_traversal");
    std::fs::write(
        dir.join("start-script.luau"),
        r#"
            local ok, err = pcall(require, "../escape")
            if ok then error("expected require to reject ../") end
            return { name = "GuardedMod" }
            "#,
    )
    .unwrap();
    rt.run_mod_init(&dir).unwrap();
    let manifest = rt.mod_manifest().expect("Some manifest");
    assert_eq!(manifest.name, "GuardedMod");
}
