// G1b cross-cutting lifecycle + render suite (M13 G1b, Task 6): the
// register -> resolve -> render story over the PRODUCTION path, where the
// per-module tests cover only one slice each.
//
// What this file pins that the inline/sibling tests do not:
//   - The full COLD-LAUNCH chain: a `RegisteredUiTree` drained into the tiered
//     registry (as `setupMod` produces) resolves BY NAME and the resolved
//     descriptor renders to draw data — proving the registry and the renderer
//     meet, not just that each works alone.
//   - The always-on COMPOSE -> render chain and its removal-next-frame behavior.
//   - A mod theme override (the `ModThemeTokens` -> `ThemeDescriptor` -> merge
//     shape the `main.rs` drain builds) reaching a RENDERED widget's color.
//   - A runtime-registered font asset becoming usable by a `text` widget's
//     `font` (the net-new runtime font path, render side).
//   - `localState` on a MIXED tree (store-bound + local-bound widgets together)
//     rendering both, with the gameplay recompute counter flat on a settled
//     frame (the live cell rides the snapshot, not the compared descriptor).
//
// Pure CPU — `UiTree`/`UiTheme`/`FontSystem` are all GPU-free. The renderer's
// `set_ui_theme`/`register_ui_font` are thin GPU-owning wrappers over exactly the
// data logic exercised here (theme merge + `FontSystem::load_font_data`), so the
// CPU suite covers the data that produces render data per testing_guide.md §3.
//
// See: context/lib/ui.md · context/lib/scripting.md §11

use std::collections::HashMap;

use super::descriptor::{
    Align, AnchoredTree, BindSource, CaptureMode, ColorValue, ContainerWidget, LocalState,
    PanelBind, PanelWidget, SpacingValue, TextBind, TextWidget, Widget,
};
use super::layout::Anchor;
use super::modal_stack::{ModalStack, ScopeTier};
use super::theme::{ThemeDescriptor, UiTheme};
use super::tree::{CellValues, ImageSizes, UiTree};
use crate::scripting::data_descriptors::{ModThemeTokens, RegisteredUiTree};
use crate::scripting::slot_table::SlotValue;

fn fs() -> glyphon::FontSystem {
    super::text::build_font_system()
}

fn no_images() -> ImageSizes {
    ImageSizes::new()
}

fn no_slots() -> HashMap<String, SlotValue> {
    HashMap::new()
}

/// Render a descriptor through the production retained build and return the draw
/// data. Mirrors what `UiPass::layout_gameplay_tree` does per layer (build a
/// `UiTree` from the descriptor, resolve binds against the snapshot), minus the
/// GPU encode.
fn render(
    tree: &AnchoredTree,
    theme: &UiTheme,
    slots: &HashMap<String, SlotValue>,
    cells: &CellValues,
) -> super::tree::UiDrawData {
    let mut ui = UiTree::from_descriptor(tree, theme);
    let mut fs = fs();
    ui.build_draw_data_retained([1280, 720], &mut fs, &no_images(), slots, cells, 0.0)
}

/// A passthrough single-text tree, the minimal renderable HUD-shaped descriptor.
fn text_tree(content: &str, color: ColorValue, font: Option<String>) -> AnchoredTree {
    AnchoredTree {
        anchor: Anchor::TopLeft,
        offset: [0.0, 0.0],
        root: Widget::Text(TextWidget {
            content: content.into(),
            font_size: 18.0,
            color,
            font,
            bind: None,
            style_ranges: None,
            id: None,
            focus_neighbors: Default::default(),
        }),
        capture_mode: CaptureMode::Passthrough,
        initial_focus: None,
        text_entry_target: None,
    }
}

/// A `RegisteredUiTree` envelope as `setupMod`/`setupLevel` produce it.
fn registered(name: &str, tree: AnchoredTree, always_on: bool) -> RegisteredUiTree {
    RegisteredUiTree {
        name: name.to_string(),
        tree,
        always_on,
    }
}

// --- Cold-launch: drain -> resolve by name -> render -------------------------

#[test]
fn setup_mod_tree_resolves_by_name_and_renders_on_cold_launch() {
    // The cold-boot chain end to end: a tree drained from a `setupMod` return
    // into the tiered registry (the `register_script_trees` drain `main.rs` runs
    // after `run_mod_init`) resolves BY NAME through the `&self` seam, and the
    // resolved descriptor renders to non-empty draw data carrying its content.
    // This proves the registry-to-renderer handoff the production frame makes,
    // not just that the registry holds the entry.
    let mut stack = ModalStack::new();
    stack.register_script_trees(
        vec![registered(
            "objectiveBoard",
            text_tree("OBJECTIVE", ColorValue::Literal([1.0, 1.0, 1.0, 1.0]), None),
            false,
        )],
        ScopeTier::Mod,
    );

    let resolved = stack
        .tree("objectiveBoard")
        .expect("a setupMod tree resolves by name after the drain");
    let data = render(
        resolved,
        &UiTheme::engine_default(),
        &no_slots(),
        &CellValues::new(),
    );
    assert!(
        data.texts.iter().any(|t| t.content == "OBJECTIVE"),
        "the resolved-by-name tree renders its content on a cold launch",
    );
}

#[test]
fn mod_hud_shadow_renders_the_mod_tree_not_the_engine_hud() {
    // The reskin path, end to end: an engine HUD registered at boot, then a mod
    // tree drained under the SAME name shadows it (last-wins + the one-line warn
    // emitted by `UiTreeRegistry::register`). Resolving the HUD name now renders
    // the MOD tree's content — the shadow takes effect on the render path.
    let mut stack = ModalStack::new();
    stack.registry_mut().register(
        "hud",
        text_tree("ENGINE HUD", ColorValue::Literal([1.0; 4]), None),
        ScopeTier::Engine,
        true,
    );
    // Drain a mod tree under the same name (the warn fires inside register).
    stack.register_script_trees(
        vec![registered(
            "hud",
            text_tree("MOD HUD", ColorValue::Literal([1.0; 4]), None),
            true,
        )],
        ScopeTier::Mod,
    );

    let resolved = stack.tree("hud").expect("hud resolves");
    let data = render(
        resolved,
        &UiTheme::engine_default(),
        &no_slots(),
        &CellValues::new(),
    );
    assert!(
        data.texts.iter().any(|t| t.content == "MOD HUD"),
        "the shadowing mod tree renders in the HUD slot",
    );
    assert!(
        !data.texts.iter().any(|t| t.content == "ENGINE HUD"),
        "the shadowed engine HUD must not render",
    );
}

// --- Always-on compose -> render, and removal next frame ---------------------

#[test]
fn always_on_layer_composes_and_renders_at_its_anchored_placement() {
    // An always-on registered tree (not the HUD, not pushed) composes as a base
    // layer and renders at its declared anchor/offset. Mirrors the `main.rs`
    // compose step: `always_on_layers()` ++ `entries()`, each layer rendered in
    // turn. The compose is a pure function of registry contents — the per-frame
    // `always_on_layers()` resolves whatever is currently registered, so an entry
    // that is absent from the registry never enters the composed set (the
    // removal-next-frame property; the registry's compose read is stateless).
    let mut overlay_tree = text_tree("OVERLAY", ColorValue::Literal([1.0; 4]), None);
    overlay_tree.anchor = Anchor::BottomRight;
    overlay_tree.offset = [-8.0, -8.0];

    let mut stack = ModalStack::new();
    stack
        .registry_mut()
        .register("scanlines", overlay_tree, ScopeTier::Mod, true);

    let layers = stack.always_on_layers();
    assert!(
        layers.iter().any(|e| e.name == "scanlines"),
        "the always-on overlay composes as a base layer",
    );
    let overlay = layers.iter().find(|e| e.name == "scanlines").unwrap();
    // It renders at its declared anchored placement (bottom-right): the text's
    // device position is in the lower-right quadrant of the 1280x720 backbuffer.
    let data = render(
        &overlay.descriptor,
        &UiTheme::engine_default(),
        &no_slots(),
        &CellValues::new(),
    );
    let drawn = data
        .texts
        .iter()
        .find(|t| t.content == "OVERLAY")
        .expect("the composed always-on layer renders its content");
    assert!(
        drawn.position[0] > 640.0 && drawn.position[1] > 360.0,
        "the layer renders at its bottom-right anchored placement, got {:?}",
        drawn.position,
    );
}

#[test]
fn unregistered_always_on_name_never_enters_the_composed_set() {
    // The removal-next-frame property in its assertable form: `always_on_layers()`
    // is a stateless read over the registry, so a name that is NOT registered (the
    // state after an entry is removed) never composes — the layer disappears the
    // moment its entry is gone.
    let stack = ModalStack::new();
    assert!(
        stack.always_on_layers().is_empty(),
        "an empty registry composes no always-on layers",
    );
    assert!(
        !stack
            .always_on_layers()
            .iter()
            .any(|e| e.name == "scanlines"),
        "an unregistered name is absent from the composed set",
    );
}

// --- Theme override from a mod drain reaching a rendered widget --------------

#[test]
fn mod_theme_token_overrides_engine_default_in_a_rendered_panel() {
    // The full theme-install shape `main.rs` runs: a mod's `theme` tokens
    // (`ModThemeTokens`) become a `ThemeDescriptor`, merge over `engine_default`,
    // and that merged theme is what widgets resolve against. A `panel` whose
    // `fill` is the `panel.default` token then draws the OVERRIDE color, not the
    // engine default — proving the override reaches a rendered widget. Panel
    // fills stay linear (no sRGB conversion), so the assertion is exact.
    let mod_tokens = ModThemeTokens {
        colors: HashMap::from([("panel.default".to_string(), [0.9, 0.1, 0.2, 1.0])]),
        ..Default::default()
    };
    // Mirror `install_mod_ui_theme_and_fonts`: ModThemeTokens -> ThemeDescriptor
    // -> merge over engine_default.
    let descriptor = ThemeDescriptor {
        colors: mod_tokens.colors,
        fonts: mod_tokens.fonts,
        spacing: mod_tokens.spacing,
    };
    let default = UiTheme::engine_default();
    let merged = default.with_override(&descriptor);

    let tree = AnchoredTree {
        anchor: Anchor::TopLeft,
        offset: [0.0, 0.0],
        root: Widget::Panel(PanelWidget {
            fill: ColorValue::Token("panel.default".into()),
            border: None,
            id: None,
            focus_neighbors: Default::default(),
            bind: None,
            style_ranges: None,
        }),
        capture_mode: CaptureMode::Passthrough,
        initial_focus: None,
        text_entry_target: None,
    };

    // Engine default renders the default panel surface...
    let default_data = render(&tree, &default, &no_slots(), &CellValues::new());
    let default_fill = default_data.quads.instances[0].color;
    assert!(
        (default_fill[0] - 0.9).abs() > 0.1,
        "sanity: the engine default is not the override color",
    );

    // ...the merged (override) theme renders the override color exactly.
    let data = render(&tree, &merged, &no_slots(), &CellValues::new());
    let fill = data.quads.instances[0].color;
    for (got, want) in fill.iter().zip([0.9, 0.1, 0.2, 1.0].iter()) {
        assert!(
            (got - want).abs() < 1e-6,
            "the rendered panel uses the mod theme override fill, got {fill:?}",
        );
    }
}

// --- Runtime font asset becomes usable by a text widget's `font` -------------

/// Path rule (ui.md §5): tests anchor to `CARGO_MANIFEST_DIR` + `../..` (the
/// workspace root) because `cargo test`'s cwd is the crate dir, while the
/// production loader resolves cwd-relative. A shipped TTF stands in for a
/// mod-supplied runtime font asset.
fn workspace_font(file_name: &str) -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join("content/base/fonts")
        .join(file_name)
}

#[test]
fn runtime_registered_font_is_usable_by_a_text_widget_font_token() {
    // The net-new RUNTIME font path, render side, end to end: read a TTF asset
    // from disk (`read_font_file`, the read the `main.rs` drain performs), register
    // it into a FRESH `FontSystem` that did not pre-load it (the `register_ui_font`
    // -> `FontSystem::load_font_data` seam), and prove a `text` node whose `font`
    // token names the registered family resolves to that family in the built draw
    // data. A fresh `FontSystem::new()` carries NO faces, so this exercises the
    // runtime load — not the compile-time `build_font_system` embed.
    let bytes = super::text::read_font_file(&workspace_font("JetBrainsMono-Regular.ttf"))
        .expect("the runtime font asset reads from the workspace-anchored path");

    let mut font_system = glyphon::FontSystem::new();
    font_system.db_mut().load_font_data(bytes);
    // The runtime register-and-check seam reports the family is queryable (this is
    // exactly what `UiTextRenderer::register_font` returns to its caller).
    assert!(
        super::text::font_family_is_registered(&font_system, "JetBrains Mono"),
        "the runtime-loaded face registers its family",
    );

    // A theme mapping a font TOKEN to that runtime family (the `fonts` table the
    // mod's `theme` drain merges in), and a `text` widget naming the token.
    let theme = UiTheme::engine_default().with_override(&ThemeDescriptor {
        fonts: HashMap::from([("modMono".to_string(), "JetBrains Mono".to_string())]),
        ..Default::default()
    });
    let tree = text_tree("123", ColorValue::Literal([1.0; 4]), Some("modMono".into()));

    let mut ui = UiTree::from_descriptor(&tree, &theme);
    let data = ui.build_draw_data_retained(
        [1280, 720],
        &mut font_system,
        &no_images(),
        &no_slots(),
        &CellValues::new(),
        0.0,
    );
    assert_eq!(
        data.texts[0].family, "JetBrains Mono",
        "the text widget's font token resolves to the runtime-registered family",
    );
}

// --- localState on a MIXED tree (store-bound + local-bound) ------------------

/// A vstack mixing a store-bound text (`{ slot }`) and a local-bound text
/// (`{ local }`) under one `localState` scope, plus a store-bound panel — the
/// production shape where authored cells coexist with authoritative slots.
fn mixed_tree(scope: &str) -> AnchoredTree {
    AnchoredTree {
        anchor: Anchor::Center,
        offset: [0.0, 0.0],
        root: Widget::VStack(ContainerWidget {
            gap: SpacingValue::Literal(0.0),
            padding: SpacingValue::Literal(0.0),
            align: Align::Start,
            fill: None,
            border: None,
            id: None,
            focus_neighbors: Default::default(),
            focus: None,
            restore_on_return: false,
            local_state: Some(LocalState {
                scope: scope.to_string(),
                cells: Default::default(),
            }),
            children: vec![
                // Store-bound: reads the authoritative slot table.
                Widget::Text(TextWidget {
                    content: "HP?".into(),
                    font_size: 18.0,
                    color: ColorValue::Literal([1.0; 4]),
                    font: None,
                    bind: Some(TextBind {
                        source: BindSource::Slot {
                            slot: "player.health".into(),
                        },
                        format: None,
                        tween: None,
                    }),
                    style_ranges: None,
                    id: None,
                    focus_neighbors: Default::default(),
                }),
                // Local-bound: reads the presentation cell.
                Widget::Text(TextWidget {
                    content: "C?".into(),
                    font_size: 18.0,
                    color: ColorValue::Literal([1.0; 4]),
                    font: None,
                    bind: Some(TextBind {
                        source: BindSource::Local {
                            local: "count".into(),
                        },
                        format: None,
                        tween: None,
                    }),
                    style_ranges: None,
                    id: None,
                    focus_neighbors: Default::default(),
                }),
                // Store-bound panel fill, so the tree mixes color + text binds.
                Widget::Panel(PanelWidget {
                    fill: ColorValue::Literal([0.0, 0.0, 0.0, 1.0]),
                    border: None,
                    id: None,
                    focus_neighbors: Default::default(),
                    bind: Some(PanelBind {
                        source: BindSource::Slot {
                            slot: "intro.flashColor".into(),
                        },
                        tween: None,
                    }),
                    style_ranges: None,
                }),
                Widget::Text(TextWidget {
                    content: "tail".into(),
                    font_size: 18.0,
                    color: ColorValue::Literal([1.0; 4]),
                    font: None,
                    bind: None,
                    style_ranges: None,
                    id: None,
                    focus_neighbors: Default::default(),
                }),
            ],
        }),
        capture_mode: CaptureMode::Passthrough,
        initial_focus: None,
        text_entry_target: None,
    }
}

fn one_cell(scope: &str, cell: &str, value: SlotValue) -> CellValues {
    let mut m = CellValues::new();
    m.insert((scope.to_string(), cell.to_string()), value);
    m
}

#[test]
fn mixed_tree_renders_both_store_and_local_binds() {
    // A modder-component-shaped subtree mixing a `{ slot }` bind and a `{ local }`
    // bind under one `localState` scope renders BOTH from one snapshot: the store
    // value from `slot_values`, the cell value from `cell_values`. Proves the two
    // bind sources resolve side by side on the production retained path.
    let tree = mixed_tree("hudScope");
    let slots = HashMap::from([("player.health".to_string(), SlotValue::Number(77.0))]);
    let cells = one_cell("hudScope", "count", SlotValue::Number(3.0));
    let data = render(&tree, &UiTheme::engine_default(), &slots, &cells);

    let rendered: Vec<&str> = data.texts.iter().map(|t| t.content.as_str()).collect();
    assert!(
        rendered.contains(&"77"),
        "the store-bound text renders the slot value, got {rendered:?}",
    );
    assert!(
        rendered.contains(&"3"),
        "the local-bound text renders the cell value, got {rendered:?}",
    );
}

#[test]
fn cell_write_on_mixed_tree_persists_without_a_settled_frame_recompute() {
    // The live cell value rides the snapshot, not the compared descriptor, so on
    // the production mixed tree: changing ONLY the cell value across frames
    // rebuilds the draw list (a content re-measure) but a follow-up frame with the
    // SAME snapshot recomputes nothing — the cell persists at a stable value
    // without forcing layout churn. Asserted via `recompute_count()`.
    let tree = mixed_tree("hudScope");
    let mut ui = UiTree::from_descriptor(&tree, &UiTheme::engine_default());
    let mut fs = fs();
    let slots = HashMap::from([("player.health".to_string(), SlotValue::Number(50.0))]);
    let cells = one_cell("hudScope", "count", SlotValue::Number(9.0));

    ui.build_draw_data_retained([1280, 720], &mut fs, &no_images(), &slots, &cells, 0.0);
    let after_first = ui.recompute_count();
    // Re-run the SAME snapshot: nothing changed, so nothing recomputes.
    ui.build_draw_data_retained([1280, 720], &mut fs, &no_images(), &slots, &cells, 0.0);
    assert_eq!(
        ui.recompute_count(),
        after_first,
        "a settled frame on the mixed tree recomputes nothing (cell rides the snapshot)",
    );
    // The cell value is still rendered (persists across the settled frame).
    let data = ui.build_draw_data_retained([1280, 720], &mut fs, &no_images(), &slots, &cells, 0.0);
    assert!(
        data.texts.iter().any(|t| t.content == "9"),
        "the cell value persists across frames",
    );
}
