// Binary-side UI lifecycle coverage: manifest tree registration, theme/font
// install data, and mixed store/local binds through the CPU render path.
// See: context/lib/ui.md · context/lib/scripting.md §11

use std::collections::HashMap;
use std::path::PathBuf;

use postretro_entities::SlotValue;
use postretro_foundation::ModThemeTokens;
use postretro_scripting_core::data_descriptors::RegisteredUiTree;
use postretro_scripting_core::runtime::StagedManifestCommitOutcome;
use postretro_scripting_core::staged_manifest::{
    StagedManifest, StagedManifestBuildResult, StagedManifestBuildStatus,
};
use postretro_ui::UiTreeEntry;
use postretro_ui::descriptor::{
    Align, AnchoredTree, BarMax, BarMaxStateRef, BarWidget, BindSource, CaptureMode, ColorValue,
    ContainerWidget, Easing, LocalState, PanelBind, PanelWidget, SliderBind, SpacingValue,
    TextBind, TextTween, TextWidget, Widget,
};
use postretro_ui::layout::Anchor;
use postretro_ui::modal_stack::{ModalStack, ScopeTier};
use postretro_ui::style_ranges::{StyleEntry, StyleRanges};
use postretro_ui::theme::{ThemeDescriptor, UiTheme};
use postretro_ui::tree::{CellValues, ImageSizes, UiDrawData, UiTree};

fn font_system() -> postretro_ui::text::FontSystem {
    postretro_ui::text::build_font_system()
}

fn no_images() -> ImageSizes {
    ImageSizes::new()
}

fn no_slots() -> HashMap<String, SlotValue> {
    HashMap::new()
}

fn render_tree(
    tree: &AnchoredTree,
    theme: &UiTheme,
    slots: &HashMap<String, SlotValue>,
    cells: &CellValues,
) -> UiDrawData {
    let mut ui = UiTree::from_descriptor(tree, theme);
    let mut fs = font_system();
    ui.build_draw_data_retained([1280, 720], &mut fs, &no_images(), slots, cells, 0.0)
}

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
            visible_when: None,
            role: None,
        }),
        capture_mode: CaptureMode::Passthrough,
        initial_focus: None,
        text_entry_target: None,
        accessible_name: None,
        role: None,
    }
}

fn registered(name: &str, tree: AnchoredTree, always_on: bool) -> RegisteredUiTree {
    RegisteredUiTree {
        name: name.to_string(),
        tree,
        always_on,
    }
}

fn staged_manifest_result(
    generation: u64,
    ui_trees: Vec<RegisteredUiTree>,
    theme: ModThemeTokens,
) -> StagedManifestBuildResult {
    StagedManifestBuildResult {
        generation,
        mod_root: PathBuf::from("content/dev"),
        status: StagedManifestBuildStatus::Built(Box::new(StagedManifest {
            name: "UiLifecycle".to_string(),
            id: "ui-lifecycle".to_string(),
            version: "1".to_string(),
            render: Default::default(),
            movers: Default::default(),
            switching: Default::default(),
            entities: Vec::new(),
            maps: Vec::new(),
            reactions: Vec::new(),
            crossings: Vec::new(),
            events: Vec::new(),
            trigger_events: Vec::new(),
            trigger_pools: Vec::new(),
            ui_trees,
            theme,
            frontend: None,
            store_declarations: Default::default(),
            dependency_paths: Vec::new(),
        })),
        diagnostics: Vec::new(),
    }
}

fn failed_staged_manifest_result(generation: u64) -> StagedManifestBuildResult {
    StagedManifestBuildResult {
        generation,
        mod_root: PathBuf::from("content/dev"),
        status: StagedManifestBuildStatus::Failed,
        diagnostics: Vec::new(),
    }
}

fn committed(generation: u64) -> StagedManifestCommitOutcome {
    StagedManifestCommitOutcome::Committed {
        generation,
        descriptor_count: 0,
        applied_actions: 0,
        dropped_missing_targets: 0,
    }
}

fn apply_staged_ui_snapshot(
    stack: &mut ModalStack,
    theme: &mut UiTheme,
    result: &StagedManifestBuildResult,
    outcome: &StagedManifestCommitOutcome,
) {
    if !matches!(outcome, StagedManifestCommitOutcome::Committed { .. }) {
        return;
    }

    match &result.status {
        StagedManifestBuildStatus::Built(manifest) => {
            stack.replace_script_tree_tier(manifest.ui_trees.clone(), ScopeTier::Mod);
            *theme = merge_theme(manifest.theme.clone());
        }
        StagedManifestBuildStatus::NoStartScript => {
            stack.replace_script_tree_tier(Vec::<RegisteredUiTree>::new(), ScopeTier::Mod);
            *theme = UiTheme::engine_default();
        }
        StagedManifestBuildStatus::Failed => {}
    }
}

fn merge_theme(tokens: ModThemeTokens) -> UiTheme {
    UiTheme::engine_default().with_override(&ThemeDescriptor {
        colors: tokens.colors,
        fonts: tokens.fonts,
        spacing: tokens.spacing,
    })
}

fn approx_color(got: [f32; 4], want: [f32; 4]) -> bool {
    got.iter()
        .zip(want.iter())
        .all(|(got, want)| (*got - *want).abs() < 1.0e-6)
}

fn rendered_texts(tree: &AnchoredTree, theme: &UiTheme) -> Vec<String> {
    render_tree(tree, theme, &no_slots(), &CellValues::new())
        .texts
        .into_iter()
        .map(|text| text.content)
        .collect()
}

#[test]
fn mod_manifest_tree_resolves_by_name_and_renders_on_cold_launch() {
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
        .expect("mod manifest tree resolves by name after registration");
    let data = render_tree(
        resolved,
        &UiTheme::engine_default(),
        &no_slots(),
        &CellValues::new(),
    );
    assert!(
        data.texts.iter().any(|t| t.content == "OBJECTIVE"),
        "resolved manifest tree renders through the retained UI path",
    );
}

#[test]
fn mod_hud_shadow_renders_the_mod_tree_not_the_engine_hud() {
    let mut stack = ModalStack::new();
    stack.registry_mut().register(
        "hud",
        text_tree("ENGINE HUD", ColorValue::Literal([1.0; 4]), None),
        ScopeTier::Engine,
        true,
    );
    stack.register_script_trees(
        vec![registered(
            "hud",
            text_tree("MOD HUD", ColorValue::Literal([1.0; 4]), None),
            true,
        )],
        ScopeTier::Mod,
    );

    let resolved = stack.tree("hud").expect("hud resolves");
    let data = render_tree(
        resolved,
        &UiTheme::engine_default(),
        &no_slots(),
        &CellValues::new(),
    );
    assert!(data.texts.iter().any(|t| t.content == "MOD HUD"));
    assert!(!data.texts.iter().any(|t| t.content == "ENGINE HUD"));
}

#[test]
fn staged_manifest_hud_lifecycle_reveals_fallback_and_preserves_committed_snapshot() {
    let mut stack = ModalStack::new();
    stack.registry_mut().register(
        "hud",
        text_tree("ENGINE HUD", ColorValue::Token("critical".into()), None),
        ScopeTier::Engine,
        true,
    );

    let mut theme = merge_theme(ModThemeTokens {
        colors: HashMap::from([("critical".to_string(), [0.1, 0.2, 0.3, 1.0])]),
        ..Default::default()
    });
    stack.register_script_trees(
        vec![registered(
            "hud",
            text_tree("COLD MOD HUD", ColorValue::Token("critical".into()), None),
            true,
        )],
        ScopeTier::Mod,
    );

    let cold_hud = stack.tree("hud").expect("cold-launch hud resolves");
    let cold_texts = rendered_texts(cold_hud, &theme);
    assert_eq!(cold_texts, vec!["COLD MOD HUD"]);
    assert_eq!(theme.color("critical"), Some([0.1, 0.2, 0.3, 1.0]));

    let staged_hud = staged_manifest_result(
        1,
        vec![registered(
            "hud",
            text_tree("STAGED HUD", ColorValue::Token("critical".into()), None),
            true,
        )],
        ModThemeTokens {
            colors: HashMap::from([("critical".to_string(), [0.4, 0.5, 0.6, 1.0])]),
            ..Default::default()
        },
    );
    apply_staged_ui_snapshot(&mut stack, &mut theme, &staged_hud, &committed(1));

    let staged_hud = stack
        .tree("hud")
        .expect("successful staged hud still resolves by name");
    let staged_texts = rendered_texts(staged_hud, &theme);
    assert_eq!(staged_texts, vec!["STAGED HUD"]);
    assert_eq!(theme.color("critical"), Some([0.4, 0.5, 0.6, 1.0]));

    let omitted_hud = staged_manifest_result(
        2,
        Vec::new(),
        ModThemeTokens {
            colors: HashMap::from([("critical".to_string(), [0.7, 0.8, 0.9, 1.0])]),
            ..Default::default()
        },
    );
    apply_staged_ui_snapshot(&mut stack, &mut theme, &omitted_hud, &committed(2));

    let fallback_layers = stack.always_on_layers();
    assert_eq!(
        fallback_layers
            .iter()
            .map(|layer| layer.name.as_str())
            .collect::<Vec<_>>(),
        vec!["hud"],
        "omitting hud from the mod tier reveals the engine fallback layer",
    );
    let fallback_hud = stack
        .tree("hud")
        .expect("engine fallback hud resolves after staged omission");
    let fallback_texts = rendered_texts(fallback_hud, &theme);
    assert_eq!(fallback_texts, vec!["ENGINE HUD"]);
    assert_eq!(theme.color("critical"), Some([0.7, 0.8, 0.9, 1.0]));

    let failed_result = failed_staged_manifest_result(3);
    apply_staged_ui_snapshot(
        &mut stack,
        &mut theme,
        &failed_result,
        &StagedManifestCommitOutcome::FailedBuild { generation: 3 },
    );
    let stale_result = staged_manifest_result(
        1,
        vec![registered(
            "hud",
            text_tree("STALE HUD", ColorValue::Token("critical".into()), None),
            true,
        )],
        ModThemeTokens {
            colors: HashMap::from([("critical".to_string(), [1.0, 0.0, 1.0, 1.0])]),
            ..Default::default()
        },
    );
    apply_staged_ui_snapshot(
        &mut stack,
        &mut theme,
        &stale_result,
        &StagedManifestCommitOutcome::DiscardedStale {
            generation: 1,
            latest_requested: Some(3),
        },
    );

    let preserved_hud = stack
        .tree("hud")
        .expect("preserved fallback hud still resolves after failed/stale results");
    let preserved_texts = rendered_texts(preserved_hud, &theme);
    assert_eq!(preserved_texts, vec!["ENGINE HUD"]);
    assert_eq!(
        theme.color("critical"),
        Some([0.7, 0.8, 0.9, 1.0]),
        "failed and stale staged results preserve the last committed theme snapshot",
    );
}

#[test]
fn always_on_layer_composes_and_renders_at_its_anchored_placement() {
    let mut overlay_tree = text_tree("OVERLAY", ColorValue::Literal([1.0; 4]), None);
    overlay_tree.anchor = Anchor::BottomRight;
    overlay_tree.offset = [-8.0, -8.0];

    let mut stack = ModalStack::new();
    stack
        .registry_mut()
        .register("scanlines", overlay_tree, ScopeTier::Mod, true);

    let layers = stack.always_on_layers();
    let overlay = layers
        .iter()
        .find(|entry| entry.name == "scanlines")
        .expect("always-on overlay composes as a base layer");
    let data = render_tree(
        &overlay.descriptor,
        &UiTheme::engine_default(),
        &no_slots(),
        &CellValues::new(),
    );
    let drawn = data
        .texts
        .iter()
        .find(|text| text.content == "OVERLAY")
        .expect("always-on layer renders its content");
    assert!(
        drawn.position[0] > 640.0 && drawn.position[1] > 360.0,
        "bottom-right anchored overlay should render in the lower-right quadrant, got {:?}",
        drawn.position,
    );
}

#[test]
fn mod_theme_token_overrides_engine_default_in_a_rendered_panel() {
    let theme = merge_theme(ModThemeTokens {
        colors: HashMap::from([("panel.default".to_string(), [0.9, 0.1, 0.2, 1.0])]),
        ..Default::default()
    });
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
            visible_when: None,
            role: None,
        }),
        capture_mode: CaptureMode::Passthrough,
        initial_focus: None,
        text_entry_target: None,
        accessible_name: None,
        role: None,
    };

    let data = render_tree(&tree, &theme, &no_slots(), &CellValues::new());
    assert!(
        approx_color(data.quads.instances[0].color, [0.9, 0.1, 0.2, 1.0]),
        "rendered panel uses the mod theme override, got {:?}",
        data.quads.instances[0].color,
    );
}

fn workspace_font(file_name: &str) -> PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join("content/base/fonts")
        .join(file_name)
}

#[test]
fn runtime_registered_font_is_usable_by_a_text_widget_font_token() {
    let bytes = postretro_ui::text::read_font_file(&workspace_font("JetBrainsMono-Regular.ttf"))
        .expect("runtime font asset reads from the workspace");
    let mut font_system = postretro_ui::text::FontSystem::new();
    font_system.db_mut().load_font_data(bytes);
    assert!(
        postretro_ui::text::font_family_is_registered(&font_system, "JetBrains Mono"),
        "runtime-loaded face registers its family",
    );

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
    assert_eq!(data.texts[0].family, "JetBrains Mono");
}

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
            visible_when: None,
            role: None,
            children: vec![
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
                    visible_when: None,
                    role: None,
                }),
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
                    visible_when: None,
                    role: None,
                }),
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
                    visible_when: None,
                    role: None,
                }),
            ],
        }),
        capture_mode: CaptureMode::Passthrough,
        initial_focus: None,
        text_entry_target: None,
        accessible_name: None,
        role: None,
    }
}

#[test]
fn mixed_tree_renders_both_store_and_local_binds() {
    let slots = HashMap::from([("player.health".to_string(), SlotValue::Number(77.0))]);
    let mut cells = CellValues::new();
    cells.insert(
        ("hudScope".to_string(), "count".to_string()),
        SlotValue::Number(3.0),
    );

    let data = render_tree(
        &mixed_tree("hudScope"),
        &UiTheme::engine_default(),
        &slots,
        &cells,
    );
    let rendered: Vec<&str> = data
        .texts
        .iter()
        .map(|text| text.content.as_str())
        .collect();
    assert!(
        rendered.contains(&"77"),
        "store-bound text rendered: {rendered:?}"
    );
    assert!(
        rendered.contains(&"3"),
        "local-bound text rendered: {rendered:?}"
    );
}

// --- Always-on compose is a stateless read (removal-next-frame) ---------------

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
            .any(|entry| entry.name == "scanlines"),
        "an unregistered name is absent from the composed set",
    );
}

// --- localState cell persistence across settled frames -----------------------

fn one_cell(scope: &str, cell: &str, value: SlotValue) -> CellValues {
    let mut m = CellValues::new();
    m.insert((scope.to_string(), cell.to_string()), value);
    m
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
    let mut fs = font_system();
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

// --- Retained-draw-data reuse across frames, health-bar easeOut tween ---------
//
// Restores the property the dropped heavy test
// `development_hud_cold_launch_and_staged_snapshots_build_retained_draw_data`
// guarded through its `RetainedLayerHarness` path: a HUD health bar whose bind
// carries an easeOut tween, drawn through the retained path across frames, eases
// the DISPLAYED fill fraction between health changes — a mid-tween frame renders
// an in-flight fraction strictly between the two settled endpoints, and a later
// frame settles to the new value (and into the critical style band). The original
// built this HUD by shelling out to `scripts-build` (mod-init over
// `content/dev/start-script.ts`); this synthetic sibling builds the same descriptor
// in-memory — no `run_mod_init`, no `ScriptCtx` — mirroring the surviving tests.

const HUD_BAR_BACKGROUND: [f32; 4] = [0.035, 0.045, 0.060, 1.0];
const HUD_BAR_HEALTHY: [f32; 4] = [0.12, 0.72, 0.40, 1.0];
const HUD_BAR_CRITICAL: [f32; 4] = [0.86, 0.06, 0.12, 1.0];

/// Publish a `player.health` / `player.maxHealth` store snapshot as the engine's
/// per-frame slot map. The heavy original wrote these through `write_store_slot`
/// on a live `ScriptCtx` and read the slot table back; the surviving in-memory
/// tests construct the slot map directly (see `mixed_tree_renders_both_...`), so
/// this mirrors that.
fn publish_health_snapshot(health: f32, max_health: f32) -> HashMap<String, SlotValue> {
    HashMap::from([
        ("player.health".to_string(), SlotValue::Number(health)),
        (
            "player.maxHealth".to_string(),
            SlotValue::Number(max_health),
        ),
    ])
}

/// A HUD health bar bound to `player.health` over a `player.maxHealth` state max,
/// with an 180 ms easeOut tween on the bind and a critical style band at or below
/// a 0.25 displayed fraction. Faithful to the production HUD bar the original
/// exercised, built in-memory.
fn health_bar_tree() -> AnchoredTree {
    AnchoredTree {
        anchor: Anchor::BottomLeft,
        offset: [24.0, -24.0],
        root: Widget::Bar(BarWidget {
            bind: SliderBind {
                source: BindSource::Slot {
                    slot: "player.health".into(),
                },
                tween: Some(TextTween {
                    duration_ms: 180.0,
                    easing: Easing::EaseOut,
                    from: None,
                }),
            },
            max: BarMax::State(BarMaxStateRef {
                slot: "player.maxHealth".into(),
            }),
            fill: ColorValue::Literal(HUD_BAR_HEALTHY),
            background: ColorValue::Literal(HUD_BAR_BACKGROUND),
            width: None,
            height: None,
            id: None,
            style_ranges: Some(StyleRanges {
                max: 1.0,
                entries: vec![
                    StyleEntry {
                        up_to: Some(0.25),
                        color: Some(ColorValue::Literal(HUD_BAR_CRITICAL)),
                        pulse: None,
                        flash: None,
                    },
                    StyleEntry {
                        up_to: None,
                        color: Some(ColorValue::Literal(HUD_BAR_HEALTHY)),
                        pulse: None,
                        flash: None,
                    },
                ],
            }),
            visible_when: None,
            exit_fade: None,
            role: None,
        }),
        capture_mode: CaptureMode::Passthrough,
        initial_focus: None,
        text_entry_target: None,
        accessible_name: None,
        role: None,
    }
}

/// Faithful shrink of the original `RetainedLayerHarness`: it lazily builds one
/// `UiTree` per composed always-on layer and draws each through the RETAINED path,
/// so per-frame draw data is reused/updated across frames rather than rebuilt from
/// scratch. That reuse is what carries the tween's in-flight display value between
/// frames.
struct RetainedLayerHarness {
    trees: Vec<UiTree>,
    font_system: postretro_ui::text::FontSystem,
}

impl RetainedLayerHarness {
    fn new() -> Self {
        Self {
            trees: Vec::new(),
            font_system: font_system(),
        }
    }

    fn draw_layers(
        &mut self,
        layers: &[UiTreeEntry],
        theme: &UiTheme,
        slots: &HashMap<String, SlotValue>,
        time_seconds: f64,
    ) -> Vec<UiDrawData> {
        if self.trees.len() != layers.len() {
            self.trees = layers
                .iter()
                .map(|layer| UiTree::from_descriptor(&layer.descriptor, theme))
                .collect();
        }
        layers
            .iter()
            .enumerate()
            .map(|(index, _layer)| {
                self.trees[index].build_draw_data_retained(
                    [1280, 720],
                    &mut self.font_system,
                    &no_images(),
                    slots,
                    &CellValues::new(),
                    time_seconds,
                )
            })
            .collect()
    }
}

fn matches_bar_fill(color: [f32; 4]) -> bool {
    approx_color(color, HUD_BAR_HEALTHY) || approx_color(color, HUD_BAR_CRITICAL)
}

/// Displayed fill fraction = fill quad width / background quad width.
fn bar_fraction(data: &UiDrawData) -> f32 {
    let background = data
        .quads
        .instances
        .iter()
        .find(|quad| approx_color(quad.color, HUD_BAR_BACKGROUND))
        .expect("HUD health bar background quad renders");
    let fill = data
        .quads
        .instances
        .iter()
        .find(|quad| matches_bar_fill(quad.color))
        .expect("HUD health bar fill quad renders");
    fill.rect[2] / background.rect[2]
}

fn bar_fill_color(data: &UiDrawData) -> [f32; 4] {
    data.quads
        .instances
        .iter()
        .find(|quad| matches_bar_fill(quad.color))
        .expect("HUD health bar fill quad renders")
        .color
}

#[test]
fn retained_draw_data_reuses_across_frames_and_reflects_health_tween() {
    // Compose the health-bar HUD as a mod always-on layer (the register -> compose
    // path the production frame runs), then draw it through the retained harness.
    let mut stack = ModalStack::new();
    stack
        .registry_mut()
        .register("hud", health_bar_tree(), ScopeTier::Mod, true);
    let theme = UiTheme::engine_default();
    let layers = stack.always_on_layers();
    assert_eq!(
        layers.iter().map(|l| l.name.as_str()).collect::<Vec<_>>(),
        vec!["hud"],
        "the health-bar HUD composes as a single always-on layer",
    );

    let mut retained = RetainedLayerHarness::new();

    // Frame 0 — full health. No tween `from`, so the first resolution snaps to the
    // target: the bar fills completely and reads the healthy band.
    let full_slots = publish_health_snapshot(100.0, 100.0);
    let full_draws = retained.draw_layers(&layers, &theme, &full_slots, 0.0);
    let full_fraction = bar_fraction(&full_draws[0]);
    assert!(
        (full_fraction - 1.0).abs() < 0.01,
        "full health fills the bar, got {full_fraction}",
    );
    assert!(
        approx_color(bar_fill_color(&full_draws[0]), HUD_BAR_HEALTHY),
        "full health renders the healthy band",
    );

    // Frame 1 — retarget to low health at t=0: the tween segment starts easing from
    // the settled full value toward the new target (retained reuse carries the
    // in-flight display value forward from here).
    let low_slots = publish_health_snapshot(20.0, 100.0);
    retained.draw_layers(&layers, &theme, &low_slots, 0.0);

    // Frame 2 — 90 ms into the 180 ms easeOut tween: the displayed fraction is
    // in-flight, strictly between the two settled endpoints (full 1.0 and low 0.2).
    let mid_draws = retained.draw_layers(&layers, &theme, &low_slots, 0.09);
    let mid_fraction = bar_fraction(&mid_draws[0]);
    assert!(
        mid_fraction > 0.2 + 1e-3 && mid_fraction < full_fraction - 1e-3,
        "mid-tween fraction is in-flight strictly between low (0.2) and full ({full_fraction}), got {mid_fraction}",
    );
    assert!(
        !approx_color(bar_fill_color(&mid_draws[0]), HUD_BAR_CRITICAL),
        "the critical band must wait until the displayed fraction crosses its threshold, got {:?}",
        bar_fill_color(&mid_draws[0]),
    );

    // Frame 3 — past the 180 ms duration: the displayed value settles to the new
    // target, reaching 20 / 100 and crossing into the critical band.
    let settled_draws = retained.draw_layers(&layers, &theme, &low_slots, 0.181);
    let settled_fraction = bar_fraction(&settled_draws[0]);
    assert!(
        (settled_fraction - 0.2).abs() < 0.015,
        "settled bar reaches 20 / 100, got {settled_fraction}",
    );
    assert!(
        settled_fraction < mid_fraction - 1e-3,
        "the settled low fraction sits below the mid-tween fraction ({mid_fraction}), got {settled_fraction}",
    );
    assert!(
        approx_color(bar_fill_color(&settled_draws[0]), HUD_BAR_CRITICAL),
        "settled low health reaches the critical style band, got {:?}",
        bar_fill_color(&settled_draws[0]),
    );
}
