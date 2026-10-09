// Loading-screen tree choice, slot lifecycle, and the deferred install frame.
// See: context/lib/boot_sequence.md §1 · context/lib/ui.md §1

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::mpsc;

use postretro_entities::slot_table::SlotValue;
use postretro_scripting_core::runtime::ModLoading;
use postretro_scripting_core::store_bridge::read_store_slot;
use postretro_ui::descriptor::{AnchoredTree, BarMax, Widget};
use postretro_ui::modal_stack::ScopeTier;

use super::*;
use crate::startup::lifecycle::tests::test_app;
use crate::startup::{BootState, LevelSource};

fn names(names: &[&str]) -> Vec<String> {
    names.iter().map(|name| name.to_string()).collect()
}

fn workspace_core_root() -> postretro_ui::CoreRoot {
    postretro_ui::CoreRoot::at(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .ancestors()
            .nth(2)
            .expect("crates/postretro has a workspace root two levels up")
            .join("core"),
    )
}

fn fallback_tree() -> AnchoredTree {
    let path = workspace_core_root().ui_asset_path("loadingScreen.json");
    postretro_ui::tree_asset::load_named_tree(&path)
        .expect("loadingScreen.json loads through the wire path")
}

fn register(app: &mut App, name: &str, tier: ScopeTier) {
    let session = app.session.as_mut().unwrap();
    session
        .modal_stack
        .registry_mut()
        .register(name, fallback_tree(), tier, false);
}

fn slot(app: &App, name: &str) -> SlotValue {
    read_store_slot(&app.session.as_ref().unwrap().scripting.script_ctx, name)
        .unwrap_or_else(|err| panic!("{name}: {err}"))
}

fn active_tree(app: &App) -> Option<String> {
    app.session
        .as_ref()
        .unwrap()
        .loading_screen
        .active
        .as_ref()
        .expect("a load is showing")
        .tree
        .clone()
}

fn entry(name: &str, loading_tree: &[&str]) -> LevelLoadEntry {
    LevelLoadEntry {
        catalog_id: Some("e1m1".to_string()),
        path: "maps/e1m1.prl".to_string(),
        name: name.to_string(),
        tags: Vec::new(),
        loading_tree: names(loading_tree),
    }
}

/// A delivered payload carrying a minimal one-cell level.
fn payload() -> LevelPayload {
    LevelPayload {
        level: Some(crate::runtime_movers::tests::single_cell_world(
            Default::default(),
        )),
        prm_cache_root: PathBuf::from("baked/materials"),
        timings: Vec::new(),
    }
}

/// What the worker delivers when the map file is missing: no level.
fn level_less_payload() -> LevelPayload {
    LevelPayload {
        level: None,
        ..payload()
    }
}

#[test]
fn loading_tree_resolution_prefers_catalog_then_mod_then_fallback() {
    let registered: HashSet<&str> = ["catA", "modA", "modB", LOADING_SCREEN_NAME].into();
    let is_registered = |name: &str| registered.contains(name);
    let mut warned = HashSet::new();

    let catalog = choose_loading_tree(
        &names(&["catA"]),
        &names(&["modA"]),
        is_registered,
        &mut warned,
        |_| 0,
    );
    assert_eq!(catalog.as_deref(), Some("catA"));

    let modwide = choose_loading_tree(
        &[],
        &names(&["modA", "modB"]),
        is_registered,
        &mut warned,
        |n| n - 1,
    );
    assert_eq!(
        modwide.as_deref(),
        Some("modB"),
        "the pick indexes the registered names"
    );

    let fallback = choose_loading_tree(&[], &[], is_registered, &mut warned, |_| 0);
    assert_eq!(fallback.as_deref(), Some(LOADING_SCREEN_NAME));

    let nothing = choose_loading_tree(&[], &[], |_| false, &mut warned, |_| 0);
    assert_eq!(
        nothing, None,
        "no registered tree leaves the splash to paint"
    );
}

#[test]
fn loading_tree_resolution_skips_unregistered_names_and_warns_once_each() {
    let registered: HashSet<&str> = ["modA", LOADING_SCREEN_NAME].into();
    let is_registered = |name: &str| registered.contains(name);
    let mut warned = HashSet::new();
    let capture = postretro_test_log_capture::LogCapture::start();

    // Every catalog name is unregistered, so the mod pool decides; within it
    // the unregistered name is never picked.
    for _ in 0..2 {
        let picked = choose_loading_tree(
            &names(&["ghostCatalog"]),
            &names(&["ghostMod", "modA"]),
            is_registered,
            &mut warned,
            |_| 0,
        );
        assert_eq!(picked.as_deref(), Some("modA"));
    }
    capture.assert_logged_once(log::Level::Warn, "loading tree `ghostCatalog`");
    capture.assert_logged_once(log::Level::Warn, "loading tree `ghostMod`");
}

#[test]
fn loading_tree_pick_covers_every_registered_candidate() {
    let pool = names(&["a", "b", "c"]);
    let mut seen = HashSet::new();
    for index in 0..3 {
        let picked = choose_loading_tree(
            &[],
            &pool,
            |_| true,
            &mut HashSet::new(),
            |n| {
                assert_eq!(n, 3);
                index
            },
        );
        seen.insert(picked.unwrap());
    }
    assert_eq!(seen.len(), 3);
    assert!(random_index(3) < 3);
}

#[test]
fn raw_path_load_shows_a_tree_from_the_mod_pool() {
    let mut app = test_app();
    register(&mut app, LOADING_SCREEN_NAME, ScopeTier::Engine);
    register(&mut app, "modLoading", ScopeTier::Mod);
    app.commit_loading_manifest(
        Default::default(),
        ModLoading {
            tree: names(&["modLoading"]),
        },
    );

    let load = app
        .resolve_level_source(LevelSource::Path(PathBuf::from("maps/does-not-exist.prl")))
        .expect("raw paths always resolve");
    assert!(
        load.entry.loading_tree.is_empty(),
        "a raw path has no catalog pool"
    );
    app.begin_level_load(load);

    assert_eq!(active_tree(&app).as_deref(), Some("modLoading"));
    assert_eq!(
        slot(&app, LEVEL_NAME_SLOT),
        SlotValue::String("does-not-exist".to_string())
    );
}

#[test]
fn a_mod_tree_named_loading_screen_shadows_the_engine_fallback() {
    let mut app = test_app();
    register(&mut app, LOADING_SCREEN_NAME, ScopeTier::Engine);
    register(&mut app, LOADING_SCREEN_NAME, ScopeTier::Mod);
    app.begin_loading_screen(&entry("Entryway", &[]));
    let snapshot = app.loading_screen_snapshot().expect("the tree resolves");
    assert_eq!(
        snapshot.trees.len(),
        1,
        "the loading tree is the only layer"
    );
    assert_eq!(snapshot.trees[0].tier, ScopeTier::Mod);
}

#[test]
fn loading_slots_are_set_at_begin_and_reset_at_end() {
    let mut app = test_app();
    register(&mut app, LOADING_SCREEN_NAME, ScopeTier::Engine);
    app.begin_loading_screen(&entry("Entryway", &[]));
    assert_eq!(
        slot(&app, LEVEL_NAME_SLOT),
        SlotValue::String("Entryway".to_string())
    );
    assert_eq!(slot(&app, PROGRESS_SLOT), SlotValue::Number(0.0));

    app.defer_level_payload(payload());
    assert_eq!(
        slot(&app, PROGRESS_SLOT),
        SlotValue::Number(LOAD_PARSE_SHARE)
    );

    app.end_loading_screen();
    assert_eq!(
        slot(&app, LEVEL_NAME_SLOT),
        SlotValue::String(String::new())
    );
    assert_eq!(slot(&app, PROGRESS_SLOT), SlotValue::Number(0.0));
    assert!(
        !app.has_deferred_level_payload(),
        "ending drops a held payload"
    );
}

/// Install hands the level to Settling with the loading screen still active;
/// the reveal and the failure route end it. These need an event loop to run,
/// so the routes are pinned in source. Every entry (boot map, catalog load,
/// restart, backdrop, relevel) installs through `finish_level_payload`, so
/// none reaches Running without Settling (L1, L4).
#[test]
fn loading_screen_ends_at_reveal_and_on_failure() {
    let source = include_str!("lifecycle.rs")
        .split(
            "#[cfg(test)]
pub(crate) mod tests",
        )
        .next()
        .unwrap();
    let success = source
        .split("fn finish_level_payload(")
        .nth(1)
        .unwrap()
        .split("fn finish_level_failure(")
        .next()
        .unwrap();
    let installed = success.find("self.install_level_payload(").unwrap();
    let settling = success.find("self.enter_settling(").unwrap();
    assert!(installed < settling);
    assert!(
        !success.contains("self.end_loading_screen();"),
        "the loading tree stays active through Settling"
    );
    assert!(
        !success.contains("BootState::Running"),
        "install never enters Running directly"
    );

    let settling_source = include_str!("settling.rs")
        .split("#[cfg(test)]")
        .next()
        .unwrap();
    let reveal = settling_source.split("fn reveal_level(").nth(1).unwrap();
    let ended = reveal.find("self.end_loading_screen();").unwrap();
    let running = reveal
        .find("self.boot_state = BootState::Running;")
        .unwrap();
    assert!(ended < running);

    let failure = source
        .split("fn finish_level_failure(")
        .nth(1)
        .unwrap()
        .split("fn install_level_payload(")
        .next()
        .unwrap();
    let ended = failure.find("self.end_loading_screen();").unwrap();
    let boot_exit = failure.find("if was_boot_load {").unwrap();
    assert!(
        ended < boot_exit,
        "a failed boot load resets before it exits"
    );
}

#[test]
fn loading_progress_follows_the_worker_scaled_to_the_parse_share_and_never_falls() {
    let mut app = test_app();
    register(&mut app, LOADING_SCREEN_NAME, ScopeTier::Engine);
    let progress = app.begin_loading_screen(&entry("Entryway", &[]));
    progress.begin(100);

    progress.advance(50);
    app.advance_loading_screen(0.016);
    assert_eq!(
        slot(&app, PROGRESS_SLOT),
        SlotValue::Number(LOAD_PARSE_SHARE * 0.5)
    );

    // A shown value is a floor even if the published counter reads lower.
    let session = app.session.as_mut().unwrap();
    let load = session.loading_screen.active.as_mut().unwrap();
    load.progress = std::sync::Arc::new(postretro_level_loader::LoadProgress::new());
    app.advance_loading_screen(0.016);
    assert_eq!(
        slot(&app, PROGRESS_SLOT),
        SlotValue::Number(LOAD_PARSE_SHARE * 0.5)
    );
}

#[test]
fn a_delivered_payload_installs_one_frame_after_delivery() {
    let mut app = test_app();
    register(&mut app, LOADING_SCREEN_NAME, ScopeTier::Engine);
    app.begin_loading_screen(&entry("Entryway", &[]));
    app.boot_state = BootState::Loading;
    let (tx, rx) = mpsc::channel();
    app.level_rx = Some(rx);
    tx.send(Ok(payload())).unwrap();

    // Delivery frame: the payload is held and the bar shows the parse share.
    assert!(matches!(app.next_loading_step(), LoadingStep::Paint));
    assert!(app.level_rx.is_none(), "the worker channel is drained");
    assert!(app.has_deferred_level_payload());
    assert!(
        app.level_load_in_flight(),
        "a held payload blocks other level requests"
    );
    assert_eq!(
        slot(&app, PROGRESS_SLOT),
        SlotValue::Number(LOAD_PARSE_SHARE)
    );
    let snapshot = app
        .loading_screen_snapshot()
        .expect("delivery frame paints");
    assert_eq!(snapshot.trees[0].name, LOADING_SCREEN_NAME);

    // Next frame: install.
    assert!(matches!(app.next_loading_step(), LoadingStep::Install(_)));
    assert!(!app.has_deferred_level_payload());
}

#[test]
fn a_level_less_payload_goes_to_the_failure_path_without_a_held_frame() {
    let mut app = test_app();
    register(&mut app, LOADING_SCREEN_NAME, ScopeTier::Engine);
    app.begin_loading_screen(&entry("Entryway", &[]));
    app.boot_state = BootState::Loading;
    let (tx, rx) = mpsc::channel();
    app.level_rx = Some(rx);
    tx.send(Ok(level_less_payload())).unwrap();

    assert!(matches!(app.next_loading_step(), LoadingStep::Install(_)));
    assert!(!app.has_deferred_level_payload());
    assert_eq!(
        slot(&app, PROGRESS_SLOT),
        SlotValue::Number(0.0),
        "the bar never shows the parse share for a load that failed"
    );
}

#[test]
fn level_tier_trees_are_never_loading_candidates() {
    let mut app = test_app();
    register(&mut app, LOADING_SCREEN_NAME, ScopeTier::Engine);
    register(&mut app, "levelLoading", ScopeTier::Level);
    app.commit_loading_manifest(
        Default::default(),
        ModLoading {
            tree: names(&["levelLoading"]),
        },
    );
    app.begin_loading_screen(&entry("Entryway", &["levelLoading"]));
    assert_eq!(active_tree(&app).as_deref(), Some(LOADING_SCREEN_NAME));
}

#[test]
fn staged_loading_fields_commit_only_with_a_committed_generation() {
    use postretro_scripting_core::runtime::StagedManifestCommitOutcome;
    use postretro_scripting_core::staged_manifest::{
        StagedManifestBuildResult, StagedManifestBuildStatus,
    };
    let result = |status| StagedManifestBuildResult {
        generation: 2,
        mod_root: PathBuf::from("content/dev"),
        status,
        diagnostics: Vec::new(),
    };
    let committed = StagedManifestCommitOutcome::Committed {
        generation: 2,
        descriptor_count: 0,
        applied_actions: 0,
        dropped_missing_targets: 0,
        changed_movement_entities: Vec::new(),
    };
    let mod_pool = |app: &App| {
        app.session
            .as_ref()
            .unwrap()
            .loading_screen
            .mod_pool
            .clone()
    };
    let mut app = test_app();
    register(&mut app, LOADING_SCREEN_NAME, ScopeTier::Engine);
    app.commit_loading_manifest(
        Default::default(),
        ModLoading {
            tree: names(&["committed"]),
        },
    );
    app.begin_loading_screen(&entry("Entryway", &[]));

    // A failed or stale generation leaves the committed pool alone.
    let manifest = dev_manifest();
    let built = result(StagedManifestBuildStatus::Built(Box::new(manifest.clone())));
    app.commit_staged_loading_manifest(
        &built,
        &StagedManifestCommitOutcome::FailedBuild { generation: 2 },
    );
    app.commit_staged_loading_manifest(
        &built,
        &StagedManifestCommitOutcome::DiscardedStale {
            generation: 2,
            latest_requested: Some(3),
        },
    );
    app.commit_staged_loading_manifest(&result(StagedManifestBuildStatus::Failed), &committed);
    assert_eq!(mod_pool(&app), names(&["committed"]));

    // A committed generation replaces it; the load already showing keeps its tree.
    app.commit_staged_loading_manifest(&built, &committed);
    assert_eq!(mod_pool(&app), manifest.loading.tree);
    assert_eq!(active_tree(&app).as_deref(), Some(LOADING_SCREEN_NAME));

    // A committed generation without a start script clears it.
    app.commit_staged_loading_manifest(
        &result(StagedManifestBuildStatus::NoStartScript),
        &committed,
    );
    assert!(mod_pool(&app).is_empty());
}

#[test]
fn without_a_loading_screen_a_delivered_payload_installs_at_once() {
    let mut app = test_app();
    app.boot_state = BootState::Loading;
    let (tx, rx) = mpsc::channel();
    app.level_rx = Some(rx);
    tx.send(Ok(payload())).unwrap();
    assert!(matches!(app.next_loading_step(), LoadingStep::Install(_)));
}

#[test]
fn engine_loading_screen_tree_centers_the_logo_and_binds_the_bar() {
    let tree = fallback_tree();
    let Widget::VStack(stack) = &tree.root else {
        panic!("the fallback root is a vstack");
    };
    let [Widget::Image(logo), Widget::Bar(bar)] = stack.children.as_slice() else {
        panic!("the fallback is the logo over a bar");
    };
    assert_eq!(logo.asset, crate::app::ui_images::SPLASH_LOGO_IMAGE);
    assert_eq!(
        logo.width,
        Some(489.0),
        "the splash's logo width on the reference canvas"
    );
    assert_eq!(logo.height, None, "the height follows the art's aspect");
    assert!(matches!(
        &bar.bind.source,
        postretro_ui::descriptor::BindSource::Slot { slot, .. } if slot == PROGRESS_SLOT
    ));
    assert_eq!(bar.max, BarMax::Literal(1.0));
    // The stack is centered and shifted down by half of what sits under the
    // logo, so the logo's center lands where the splash centers it.
    let gap = match stack.gap {
        postretro_ui::descriptor::SpacingValue::Literal(gap) => gap,
        ref other => panic!("literal gap expected, got {other:?}"),
    };
    let below_logo = gap + bar.height.expect("the bar authors its height");
    assert_eq!(tree.anchor, postretro_ui::layout::Anchor::Center);
    assert_eq!(tree.offset, [0.0, below_logo / 2.0]);
}

#[test]
fn engine_loading_screen_registers_with_the_built_in_trees() {
    let mut stack = postretro_ui::modal_stack::ModalStack::new();
    postretro_ui::tree_asset::register_tree_from_disk(
        stack.registry_mut(),
        &workspace_core_root(),
        LOADING_SCREEN_NAME,
        "loadingScreen.json",
        false,
    );
    let (tier, _) = stack
        .resolve_with_tier(LOADING_SCREEN_NAME)
        .expect("the fallback registers");
    assert_eq!(tier, ScopeTier::Engine);
    assert!(
        stack.always_on_layers().is_empty(),
        "the loading screen is never a HUD layer"
    );
}

fn workspace_root() -> PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("crates/postretro has a workspace root two levels up")
        .to_path_buf()
}

/// The dev mod's manifest, built from its TypeScript start script through the
/// same bundler and staged build the engine uses.
fn dev_manifest() -> postretro_scripting_core::staged_manifest::StagedManifest {
    use postretro_scripting_core::staged_manifest::{
        StagedManifestBuildConfig, StagedManifestBuildStatus, build_staged_manifest,
    };
    let entry = workspace_root().join("content/dev/start-script.ts");
    let bundled = postretro_script_compiler::bundle_entry(&entry).expect("the dev mod bundles");
    let root = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join("start-script.js"), bundled).unwrap();
    let result = build_staged_manifest(root.path(), 1, &StagedManifestBuildConfig::default());
    match result.status {
        StagedManifestBuildStatus::Built(manifest) => *manifest,
        other => panic!(
            "dev manifest did not build: {other:?}: {:?}",
            result.diagnostics
        ),
    }
}

#[test]
fn dev_mod_declares_loading_pool_override_and_images() {
    let manifest = dev_manifest();
    let pool = &manifest.loading.tree;
    assert!(
        pool.len() >= 2,
        "a mod-wide pool of at least two trees: {pool:?}"
    );
    let override_pool = manifest
        .maps
        .iter()
        .find(|map| !map.loading_tree.is_empty())
        .map(|map| map.loading_tree.clone())
        .expect("one catalog map overrides the pool");
    let tree_names: HashSet<&str> = manifest
        .ui_trees
        .iter()
        .map(|tree| tree.name.as_str())
        .collect();
    for name in pool.iter().chain(&override_pool) {
        assert!(
            tree_names.contains(name.as_str()),
            "`{name}` is a registered dev tree"
        );
    }
    let decoded = crate::app::ui_images::decode_mod_ui_images(
        &workspace_root().join("content/dev"),
        &manifest.ui_images,
    );
    assert_eq!(
        decoded.len(),
        manifest.ui_images.len(),
        "every dev uiImages entry decodes"
    );
    assert!(!manifest.ui_images.is_empty());
}

/// Renders a Loading frame offscreen through the real snapshot and the
/// windowed UI + resolve passes. Self-skips without a GPU adapter. With
/// `POSTRETRO_LOADING_CAPTURE_DIR` set, writes the frames there as PNGs.
#[test]
fn loading_frames_render_the_tree_over_the_splash_background() {
    const SIZE: [u32; 2] = [1280, 720];
    let mut renderer = match crate::render::Renderer::new_offscreen(SIZE[0], SIZE[1]) {
        Ok(renderer) => renderer,
        Err(err) => {
            eprintln!("loading frame capture skipped: {err:#}");
            return;
        }
    };
    let logo = crate::render::splash::load_splash(
        &crate::startup::SplashSource::Base,
        &workspace_core_root(),
    )
    .expect("the splash logo decodes");
    renderer
        .register_ui_image(
            crate::app::ui_images::SPLASH_LOGO_IMAGE,
            logo.data,
            logo.width,
            logo.height,
        )
        .expect("the splash logo fits a texture");
    // Art the device cannot hold is refused, never a validation failure.
    assert!(
        renderer
            .register_ui_image("zero", Vec::new(), 0, 4)
            .is_err()
    );
    assert!(
        renderer
            .register_ui_image("oversized", Vec::new(), 1, u32::MAX)
            .is_err()
    );
    let manifest = dev_manifest();
    for image in crate::app::ui_images::decode_mod_ui_images(
        &workspace_root().join("content/dev"),
        &manifest.ui_images,
    ) {
        renderer
            .register_ui_image(&image.key, image.rgba, image.width, image.height)
            .expect("every dev image fits a texture");
    }
    let dev_tree = manifest
        .maps
        .iter()
        .find_map(|map| map.loading_tree.first().cloned())
        .unwrap();
    // The dev theme, merged as mod init installs it, so tokens resolve as in play.
    renderer.set_ui_theme(
        postretro_ui::theme::UiTheme::engine_default().with_override(
            &postretro_ui::theme::ThemeDescriptor {
                colors: manifest.theme.colors.clone(),
                fonts: manifest.theme.fonts.clone(),
                spacing: manifest.theme.spacing.clone(),
            },
        ),
    );

    let mut app = test_app();
    register(&mut app, LOADING_SCREEN_NAME, ScopeTier::Engine);
    app.session
        .as_mut()
        .unwrap()
        .modal_stack
        .register_script_trees(manifest.ui_trees.clone(), ScopeTier::Mod);
    let mut font_system = postretro_ui::text::build_font_system();
    let out_dir = std::env::var_os("POSTRETRO_LOADING_CAPTURE_DIR").map(PathBuf::from);

    for (label, pool) in [
        ("engine-fallback", Vec::new()),
        ("dev-mod", vec![dev_tree.clone()]),
    ] {
        let progress = app.begin_loading_screen(&LevelLoadEntry {
            catalog_id: Some("combat-demo".to_string()),
            path: "maps/combat-demo.prl".to_string(),
            name: "Combat + Emissive Test".to_string(),
            tags: Vec::new(),
            loading_tree: pool,
        });
        progress.begin(100);
        progress.advance(60);
        // Two frames a second apart, so the bar's tween settles.
        let mut pixels = Vec::new();
        for dt in [0.016, 1.0] {
            app.advance_loading_screen(dt);
            let snapshot = app.loading_screen_snapshot().expect("a tree resolves");
            let time = snapshot.time_seconds;
            renderer.set_ui_snapshot(snapshot);
            pixels = renderer
                .capture_world_less_frame(&mut font_system, crate::render::SPLASH_CLEAR_COLOR, time)
                .expect("the world-less frame renders");
        }
        app.end_loading_screen();

        let image = image::RgbaImage::from_raw(SIZE[0], SIZE[1], pixels).unwrap();
        // The splash background, sRGB 8-bit (28, 33, 39), within rounding.
        let corner = image.get_pixel(4, 4).0;
        for (channel, expected) in corner.iter().zip([28u8, 33, 39]) {
            assert!(
                channel.abs_diff(expected) <= 1,
                "{label}: corner {corner:?} is the splash background"
            );
        }
        let drawn = image
            .pixels()
            .filter(|pixel| {
                pixel.0[..3]
                    .iter()
                    .zip([28u8, 33, 39])
                    .any(|(c, e)| c.abs_diff(e) > 8)
            })
            .count();
        assert!(drawn > 1000, "{label}: the loading tree draws ({drawn} px)");
        if let Some(dir) = &out_dir {
            std::fs::create_dir_all(dir).unwrap();
            image
                .save(dir.join(format!("loading-{label}.png")))
                .unwrap();
        }
    }
}
