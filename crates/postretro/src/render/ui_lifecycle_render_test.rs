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
use postretro_ui::descriptor::{
    Align, AnchoredTree, BindSource, CaptureMode, ColorValue, ContainerWidget, LocalState,
    PanelBind, PanelWidget, SpacingValue, TextBind, TextWidget, Widget,
};
use postretro_ui::layout::Anchor;
use postretro_ui::modal_stack::{ModalStack, ScopeTier};
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
            entities: Vec::new(),
            maps: Vec::new(),
            reactions: Vec::new(),
            crossings: Vec::new(),
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
