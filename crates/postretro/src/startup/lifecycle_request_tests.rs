//! Level request tests: load requests, deferral while loading, catalog and raw
//! path resolution, frontend loads, relevel and level verbs.
//! See: context/lib/boot_sequence.md §1 · §4

use super::lifecycle_test_fixtures::*;
use super::*;
use crate::startup::LevelRequest;
use std::collections::VecDeque;

use postretro_entities::{NamedReaction, PrimitiveDescriptor, ReactionDescriptor};

use postretro_scripting_core::data_descriptors::RegisteredUiTree;

use postretro_scripting_core::runtime::{Frontend, MenuCamera, StagedManifestCommitOutcome};

use postretro_scripting_core::staged_manifest::{
    StagedManifest, StagedManifestBuildResult, StagedManifestBuildStatus,
};

#[test]
fn loading_state_defers_and_coalesces_runtime_load_requests() {
    let mut app = test_app();
    let (_tx, rx) = std::sync::mpsc::channel();
    app.boot_state = BootState::Loading;
    app.level_rx = Some(rx);

    app.enqueue_level_request(LevelRequest::Load(LevelSource::Path(PathBuf::from(
        "content/dev/maps/first.prl",
    ))));
    app.enqueue_level_request(LevelRequest::Load(LevelSource::Path(PathBuf::from(
        "content/dev/maps/second.prl",
    ))));
    app.drain_level_requests();

    assert!(
        app.level_rx.is_some(),
        "the active worker receiver remains owned while Loading",
    );
    assert_eq!(
        app.level_requests.len(),
        1,
        "repeated load requests coalesce while a worker is in flight",
    );
    let Some(LevelRequest::Load(LevelSource::Path(path))) = app.level_requests.front() else {
        panic!("queued request should be the coalesced load");
    };
    assert_eq!(path, &PathBuf::from("content/dev/maps/second.prl"));
}

#[test]
fn boot_load_rejects_runtime_requests_while_loading() {
    let mut app = test_app();
    let (_tx, rx) = std::sync::mpsc::channel();
    app.boot_state = BootState::Loading;
    app.boot_load = true;
    app.level_rx = Some(rx);

    app.enqueue_level_request(LevelRequest::Unload);
    app.enqueue_level_request(LevelRequest::Load(LevelSource::Path(PathBuf::from(
        "content/dev/maps/runtime.prl",
    ))));
    app.drain_level_requests();

    assert!(app.level_rx.is_some());
    assert!(
        app.level_requests.is_empty(),
        "runtime requests cannot cancel or replace the active boot load",
    );
    assert!(
        app.boot_load,
        "boot fatality marker stays with the active load"
    );
}

#[test]
fn catalog_level_load_resolves_path_and_stores_in_flight_entry() {
    let mut app = test_app();
    app.boot_state = BootState::Frontend;
    app.content_root = PathBuf::from("content/mod");
    script_ctx(&app)
        .data_registry
        .borrow_mut()
        .replace_maps(vec![catalog_map(
            "e1m1",
            "maps/e1m1.prl",
            "Entryway",
            &["campaign", "intro"],
        )]);

    app.enqueue_level_request(LevelRequest::Load(LevelSource::Catalog("e1m1".to_string())));
    app.drain_level_requests();

    let load = app
        .level_load
        .as_ref()
        .expect("catalog load should start and store in-flight metadata");
    assert_eq!(load.map_path, PathBuf::from("content/mod/maps/e1m1.prl"));
    assert_eq!(load.content_root, PathBuf::from("content/mod"));
    assert_eq!(load.entry.catalog_id.as_deref(), Some("e1m1"));
    assert_eq!(load.entry.path, "maps/e1m1.prl");
    assert_eq!(load.entry.name, "Entryway");
    assert_eq!(load.entry.tags, ["campaign", "intro"]);
    assert!(matches!(app.boot_state, BootState::Loading));
    assert!(app.level_load_in_flight());

    drop_in_flight_worker(&mut app);
}

#[test]
fn missing_catalog_level_load_is_rejected_without_unloading_running_level() {
    let mut app = test_app();
    app.boot_state = BootState::Running;
    app.level = Some(level_world(FIXTURE_MAP_A, 1));
    script_ctx(&app)
        .data_registry
        .borrow_mut()
        .replace_maps(vec![catalog_map(
            "known",
            "maps/known.prl",
            "Known Map",
            &["campaign"],
        )]);

    app.enqueue_level_request(LevelRequest::Load(LevelSource::Catalog(
        "missing".to_string(),
    )));
    app.drain_level_requests();

    assert!(
        app.level.is_some(),
        "missing id must not unload active level"
    );
    assert!(app.level_load.is_none());
    assert!(app.level_rx.is_none());
    assert!(app.level_worker.is_none());
    assert!(app.level_requests.is_empty());
    assert!(matches!(app.boot_state, BootState::Running));
}

#[test]
fn raw_path_level_load_synthesizes_non_catalog_metadata() {
    let mut app = test_app();
    app.boot_state = BootState::Frontend;
    app.content_root = PathBuf::from("content/dev");
    let raw_path = PathBuf::from("content/dev/maps/raw-dev-map.prl");

    app.enqueue_level_request(LevelRequest::Load(LevelSource::Path(raw_path.clone())));
    app.drain_level_requests();

    let load = app
        .level_load
        .as_ref()
        .expect("raw path load should start with synthesized metadata");
    assert_eq!(load.map_path, raw_path);
    assert_eq!(load.content_root, PathBuf::from("content/dev"));
    assert_eq!(load.entry.catalog_id, None);
    assert_eq!(load.entry.path, "content/dev/maps/raw-dev-map.prl");
    assert_eq!(load.entry.name, "raw-dev-map");
    assert!(load.entry.tags.is_empty());
    assert!(matches!(app.boot_state, BootState::Loading));

    drop_in_flight_worker(&mut app);
}

#[test]
fn frontend_population_pushes_menu_and_enqueues_one_background_catalog_load() {
    let mut app = test_app();
    app.boot_state = BootState::Frontend;
    app.session
        .as_mut()
        .unwrap()
        .modal_stack
        .registry_mut()
        .register(
            "mainMenu",
            postretro_ui::demo::build_frontend_menu_descriptor(),
            postretro_ui::modal_stack::ScopeTier::Mod,
            false,
        );
    app.session.as_mut().unwrap().frontend = Some(Frontend {
        menu_tree: "mainMenu".to_string(),
        background_level: Some("menu_backdrop".to_string()),
        camera: MenuCamera {
            position: [4.0, 2.0, 8.0],
            yaw: -0.6,
            pitch: -0.1,
        },
    });

    app.populate_frontend();
    app.populate_frontend();

    assert_eq!(
        app.session.as_mut().unwrap().modal_stack.active_name(),
        Some("mainMenu")
    );
    assert_eq!(
        app.session.as_mut().unwrap().modal_stack.top_capture_mode(),
        postretro_ui::descriptor::CaptureMode::Capture,
        "frontend menu must suppress gameplay through the capture-mode path",
    );
    assert_eq!(
        app.level_requests.len(),
        1,
        "frontend population enqueues the declared backdrop exactly once",
    );
    let Some(LevelRequest::Load(LevelSource::Catalog(id))) = app.level_requests.front() else {
        panic!("frontend backdrop request should be a catalog load");
    };
    assert_eq!(id, "menu_backdrop");
}

#[test]
fn frontend_population_falls_back_before_loading_backdrop_when_menu_is_unknown() {
    let mut app = test_app();
    app.boot_state = BootState::Frontend;
    app.session
        .as_mut()
        .unwrap()
        .modal_stack
        .registry_mut()
        .register(
            postretro_ui::demo::FRONTEND_MENU_NAME,
            postretro_ui::demo::build_frontend_menu_descriptor(),
            postretro_ui::modal_stack::ScopeTier::Engine,
            false,
        );
    app.session.as_mut().unwrap().frontend = Some(Frontend {
        menu_tree: "missingMenu".to_string(),
        background_level: Some("menu_backdrop".to_string()),
        camera: MenuCamera {
            position: [4.0, 2.0, 8.0],
            yaw: -0.6,
            pitch: -0.1,
        },
    });

    app.populate_frontend();

    assert_eq!(
        app.session.as_mut().unwrap().modal_stack.active_name(),
        Some(postretro_ui::demo::FRONTEND_MENU_NAME),
        "unknown mod frontend menus must reveal the engine fallback",
    );
    assert_eq!(
        app.session.as_mut().unwrap().modal_stack.top_capture_mode(),
        postretro_ui::descriptor::CaptureMode::Capture
    );
    assert_eq!(
        app.level_requests.pop_front(),
        Some(LevelRequest::Load(LevelSource::Catalog(
            "menu_backdrop".to_string()
        ))),
        "backdrops load only after a capturing frontend modal is present",
    );
}

#[test]
fn staged_frontend_commit_replaces_active_frontend_modal() {
    let mut app = test_app();
    app.boot_state = BootState::Frontend;
    app.session
        .as_mut()
        .unwrap()
        .modal_stack
        .registry_mut()
        .register(
            postretro_ui::demo::FRONTEND_MENU_NAME,
            postretro_ui::demo::build_frontend_menu_descriptor(),
            postretro_ui::modal_stack::ScopeTier::Engine,
            false,
        );
    app.session
        .as_mut()
        .unwrap()
        .modal_stack
        .registry_mut()
        .register(
            "oldMenu",
            postretro_ui::demo::build_frontend_menu_descriptor(),
            postretro_ui::modal_stack::ScopeTier::Mod,
            false,
        );
    app.session.as_mut().unwrap().frontend = Some(Frontend {
        menu_tree: "oldMenu".to_string(),
        background_level: None,
        camera: MenuCamera {
            position: [0.0, 0.0, 0.0],
            yaw: 0.0,
            pitch: 0.0,
        },
    });
    app.present_frontend_menu();
    assert_eq!(
        app.session.as_mut().unwrap().modal_stack.active_name(),
        Some("oldMenu")
    );

    let staged = StagedManifestBuildResult {
        generation: 4,
        mod_root: PathBuf::from("content/dev"),
        status: StagedManifestBuildStatus::Built(Box::new(StagedManifest {
            name: "Replacement".to_string(),
            id: "replacement".to_string(),
            version: "1".to_string(),
            input: None,
            render: Default::default(),
            movers: Default::default(),
            audio: Default::default(),
            switching: Default::default(),
            default_weapon_placement: None,
            entities: Vec::new(),
            factions: Default::default(),
            sentiment: Vec::new(),
            faction_sentiment_decay: 0.0,
            entity_faction_names: Vec::new(),
            fluids: Default::default(),
            ghost_styles: Default::default(),
            maps: Vec::new(),
            reactions: Vec::new(),
            crossings: Vec::new(),
            events: Vec::new(),
            trigger_events: Vec::new(),
            trigger_pools: Vec::new(),
            ui_trees: vec![RegisteredUiTree {
                name: "newMenu".to_string(),
                tree: postretro_ui::demo::build_frontend_menu_descriptor(),
                always_on: false,
                hide_below: false,
            }],
            presentation_templates: Vec::new(),
            presentation_overlays: Vec::new(),
            theme: Default::default(),
            frontend: Some(Frontend {
                menu_tree: "newMenu".to_string(),
                background_level: None,
                camera: MenuCamera {
                    position: [1.0, 2.0, 3.0],
                    yaw: 0.25,
                    pitch: -0.5,
                },
            }),
            ui_images: Default::default(),
            loading: Default::default(),
            store_declarations: Default::default(),
            dependency_paths: Vec::new(),
            player_events: Vec::new(),
        })),
        diagnostics: Vec::new(),
    };
    let committed = StagedManifestCommitOutcome::Committed {
        generation: 4,
        descriptor_count: 0,
        applied_actions: 0,
        dropped_missing_targets: 0,
        changed_movement_entities: Vec::new(),
    };

    app.commit_staged_ui_manifest(&staged, &committed);
    assert_eq!(
        app.session.as_mut().unwrap().modal_stack.active_name(),
        Some("newMenu"),
        "staged replacement updates the active frontend modal clone",
    );

    let omitted = StagedManifestBuildResult {
        generation: 5,
        mod_root: PathBuf::from("content/dev"),
        status: StagedManifestBuildStatus::NoStartScript,
        diagnostics: Vec::new(),
    };
    let omitted_committed = StagedManifestCommitOutcome::Committed {
        generation: 5,
        descriptor_count: 0,
        applied_actions: 0,
        dropped_missing_targets: 0,
        changed_movement_entities: Vec::new(),
    };

    app.commit_staged_ui_manifest(&omitted, &omitted_committed);
    assert_eq!(
        app.session.as_mut().unwrap().modal_stack.active_name(),
        Some(postretro_ui::demo::FRONTEND_MENU_NAME),
        "staged omission replaces the active frontend modal with the engine fallback",
    );
    assert_eq!(
        app.session.as_mut().unwrap().modal_stack.top_capture_mode(),
        postretro_ui::descriptor::CaptureMode::Capture
    );
}

#[test]
fn no_backdrop_frontend_button_activation_dispatches_load_command() {
    use postretro_ui::tree::{FocusNeighbors, FocusRect, FocusRectList, NodeInteraction};

    let mut app = test_app();
    app.boot_state = BootState::Frontend;
    crate::scripting::reactions::system_commands::register_system_reaction_primitives(
        &mut app.session.as_mut().unwrap().scripting.system_registry,
    );
    app.session
        .as_mut()
        .unwrap()
        .modal_stack
        .registry_mut()
        .register(
            postretro_ui::demo::FRONTEND_MENU_NAME,
            postretro_ui::demo::build_frontend_menu_descriptor(),
            postretro_ui::modal_stack::ScopeTier::Engine,
            false,
        );
    app.present_frontend_menu();
    app.session.as_mut().unwrap().ui_focus_rects = Some(FocusRectList {
        rects: vec![FocusRect {
            id: "play".to_string(),
            rect: [0.0, 0.0, 100.0, 32.0],
            z: 0,
            group: None,
            neighbors: FocusNeighbors::default(),
            interaction: Some(NodeInteraction::Button {
                on_press: "startCampaign".to_string(),
                repeat_on_hold: None,
            }),
            selected: None,
            checked: None,
            disabled: false,
            tablist: None,
            clip: None,
        }],
        groups: Vec::new(),
        initial_focus: Some("play".to_string()),
        restore_on_return: false,
        owner: None,
    });
    script_ctx(&app)
        .data_registry
        .borrow_mut()
        .reactions
        .push(NamedReaction {
            name: "startCampaign".to_string(),
            descriptor: ReactionDescriptor::Primitive(PrimitiveDescriptor {
                primitive: "loadLevel".to_string(),
                target: None,
                kind: None,
                tag: None,
                args: serde_json::json!({ "map": "e1m1" }),
                on_complete: None,
            }),
        });

    app.fire_focused_button_activation(Some("play"));
    app.dispatch_system_commands();

    assert_eq!(
        app.level_requests.pop_front(),
        Some(LevelRequest::Load(LevelSource::Catalog("e1m1".to_string()))),
    );
    assert!(
        app.session.as_mut().unwrap().modal_stack.is_empty(),
        "frontend activation clears the menu before gameplay load starts",
    );
}

#[test]
fn catalog_tags_are_available_on_in_flight_load_before_data_script_runs() {
    let mut app = test_app();
    app.boot_state = BootState::Frontend;
    app.content_root = PathBuf::from("content/mod");
    script_ctx(&app)
        .data_registry
        .borrow_mut()
        .replace_maps(vec![catalog_map(
            "arena",
            "maps/arena.prl",
            "Arena",
            &["deathmatch", "night"],
        )]);

    app.enqueue_level_request(LevelRequest::Load(LevelSource::Catalog(
        "arena".to_string(),
    )));
    app.drain_level_requests();

    assert_eq!(
        app.level_load
            .as_ref()
            .expect("catalog load should be in flight before install")
            .entry
            .tags,
        ["deathmatch", "night"],
        "catalog tags must be present before worker delivery and data-script install",
    );
    assert!(
        script_ctx(&app).data_registry.borrow().reactions.is_empty(),
        "data script has not run while load metadata is already available",
    );

    drop_in_flight_worker(&mut app);
}

#[test]
fn level_identity_distinguishes_catalog_and_normalizes_raw_paths() {
    let root = PathBuf::from("/mods/demo");
    assert_eq!(
        level_identity(&LevelSource::Catalog("e1m1".to_string()), &root),
        "e1m1"
    );
    assert_eq!(
        level_identity(
            &LevelSource::Path(PathBuf::from("/mods/demo/maps/./episode/../e1m1.prl")),
            &root,
        ),
        "path:maps/e1m1.prl"
    );
}

/// A `/`-rooted path is *not* absolute on Windows — it carries no drive
/// prefix, so `Path::is_absolute` is false and `level_identity` takes its
/// `cwd.join` branch. These fixtures carry a prefix there so the test below
/// exercises the absolute branch it is named for on both platforms.
#[cfg(windows)]
const OUTSIDE_CONTENT_ROOT_LEVEL: &str = "C:/tmp/test.prl";

#[cfg(not(windows))]
const OUTSIDE_CONTENT_ROOT_LEVEL: &str = "/tmp/test.prl";

#[cfg(windows)]
const ABSOLUTE_CONTENT_ROOT: &str = "C:/mods/demo";

#[cfg(not(windows))]
const ABSOLUTE_CONTENT_ROOT: &str = "/mods/demo";

#[test]
fn level_identity_keeps_outside_content_root_absolute() {
    let root = PathBuf::from(ABSOLUTE_CONTENT_ROOT);
    let outside = PathBuf::from(OUTSIDE_CONTENT_ROOT_LEVEL);
    // Guard the fixture, not just the result: if either path stops being
    // absolute, `level_identity` silently resolves it against the working
    // directory and this test no longer covers the case it is named for.
    assert!(root.is_absolute(), "content-root fixture must be absolute");
    assert!(outside.is_absolute(), "level fixture must be absolute");
    // Outside the content root, so `strip_prefix` must not fire and the
    // identity keeps the full absolute path.
    assert!(!outside.starts_with(&root));

    assert_eq!(
        level_identity(&LevelSource::Path(outside), &root),
        format!("path:{OUTSIDE_CONTENT_ROOT_LEVEL}"),
    );
}

#[test]
fn queued_load_requests_coalesce_before_lifecycle_drain() {
    let mut app = test_app();

    app.enqueue_level_request(LevelRequest::Load(LevelSource::Catalog(
        "intermediate".to_string(),
    )));
    app.enqueue_level_request(LevelRequest::Load(LevelSource::Catalog(
        "final".to_string(),
    )));

    assert_eq!(app.level_requests.len(), 1);
    assert_eq!(
        app.level_requests.front(),
        Some(&LevelRequest::Load(LevelSource::Catalog(
            "final".to_string()
        ))),
        "rapid frontend activations should not install intermediate maps",
    );
}

#[test]
fn relevel_does_not_restart_an_active_or_already_selected_catalog_load() {
    let mut app = test_app();
    script_ctx(&app)
        .data_registry
        .borrow_mut()
        .replace_maps(vec![catalog_map("e1m1", "maps/e1m1.prl", "Entryway", &[])]);

    app.active_level_source = Some(LevelSource::Catalog("e1m1".to_string()));
    app.follow_relevel_catalog("e1m1".to_string());
    assert!(
        app.level_requests.is_empty(),
        "a relevel naming the active catalog map must not restart it"
    );

    app.active_level_source = None;
    app.level_load = Some(InFlightLevelLoad {
        map_path: PathBuf::from("content/dev/maps/e1m1.prl"),
        content_root: PathBuf::from("content/dev"),
        entry: LevelLoadEntry {
            catalog_id: Some("e1m1".to_string()),
            path: "maps/e1m1.prl".to_string(),
            name: "Entryway".to_string(),
            tags: Vec::new(),
            loading_tree: Vec::new(),
        },
    });
    app.follow_relevel_catalog("e1m1".to_string());
    assert!(
        app.level_requests.is_empty(),
        "a relevel naming the in-flight catalog map must not restart it"
    );

    app.level_load = None;
    app.follow_relevel_catalog("e1m1".to_string());
    app.follow_relevel_catalog("e1m1".to_string());
    assert_eq!(
        app.level_requests,
        VecDeque::from([LevelRequest::Load(LevelSource::Catalog("e1m1".to_string()))]),
        "duplicate relevels queued before the lifecycle drain coalesce too"
    );
}

#[test]
fn unknown_relevel_catalog_warns_and_does_not_queue_a_load() {
    let mut app = test_app();
    let logs = crate::scripting::reactions::log_capture::capture(|| {
        app.follow_relevel_catalog("missing-map".to_string());
    });

    assert!(
        logs.iter().any(|(level, message)| {
            *level == log::Level::Warn
                && message.contains("[Net] relevel names unknown catalog id")
                && message.contains("missing-map")
        }),
        "unknown catalog id must produce the pinned recoverable relevel warning: {logs:?}"
    );
    assert!(
        app.level_requests.is_empty(),
        "an unknown relevel catalog must not queue a load or otherwise alter the client"
    );
}

#[test]
fn closing_control_surfaces_a_client_side_incompatible_host_diagnostic() {
    let mut app = test_app();
    let expected = postretro_net::wire::ProtocolVersion {
        app_protocol_id: 7,
        wire_version: 3,
    };
    let received = postretro_net::wire::ProtocolVersion {
        app_protocol_id: 8,
        wire_version: 3,
    };
    let logs = crate::scripting::reactions::log_capture::capture(|| {
        crate::client_drain_control(
            &mut app,
            vec![postretro_net::wire::ServerControlMessage::Divergence(
                postretro_net::wire::DivergenceReason::Closing(
                    postretro_net::wire::ClosingCause::Protocol { expected, received },
                ),
            )],
        );
    });

    assert!(
        logs.iter().any(|(level, message)| {
            *level == log::Level::Error
                && message.contains("[Net] incompatible host")
                && message.contains("protocol mismatch")
        }),
        "a typed admission refusal must reach the client-side incompatible-host diagnostic: {logs:?}"
    );
    assert!(
        !logs.iter().any(|(_, message)| message.contains("--mod")),
        "a protocol refusal has no local mod remedy, so it must not carry a --mod hint: {logs:?}"
    );
}

#[test]
fn mod_id_closing_control_tells_the_client_how_to_relaunch_on_the_host_mod() {
    let mut app = test_app();
    let capture = postretro_test_log_capture::LogCapture::start();
    crate::client_drain_control(
        &mut app,
        vec![postretro_net::wire::ServerControlMessage::Divergence(
            postretro_net::wire::DivergenceReason::Closing(
                postretro_net::wire::ClosingCause::ModId {
                    expected: "postretro.dev".to_string(),
                    received: "acme.other".to_string(),
                    expected_version: "1.2.0".to_string(),
                    received_version: "0.4.0".to_string(),
                },
            ),
        )],
    );

    capture.assert_logged_once(
            log::Level::Error,
            "[Net] incompatible host: host runs mod postretro.dev (1.2.0), this client runs acme.other (0.4.0); relaunch with --mod <folder>, where content/<folder> declares mod id postretro.dev",
        );
}

#[test]
fn load_level_system_command_queues_catalog_load_request() {
    let mut app = test_app();
    app.session
        .as_mut()
        .unwrap()
        .modal_stack
        .registry_mut()
        .register(
            "deathScreen",
            postretro_ui::demo::build_frontend_menu_descriptor(),
            postretro_ui::modal_stack::ScopeTier::Mod,
            false,
        );
    app.session
        .as_mut()
        .unwrap()
        .modal_stack
        .push_named("deathScreen", None);

    script_ctx(&app)
        .system_commands
        .push(postretro_entities::SystemReactionCommand::LoadLevel {
            map: "e1m1".to_string(),
            dispatch_source: "named:levelLoad".to_string(),
        });
    app.dispatch_system_commands();

    assert_eq!(
        app.level_requests.pop_front(),
        Some(LevelRequest::Load(LevelSource::Catalog("e1m1".to_string())))
    );
    assert!(app.level_requests.is_empty());
    assert!(
        app.session.as_mut().unwrap().modal_stack.is_empty(),
        "starting gameplay clears the initiating modal before controls return",
    );
}

#[test]
fn restart_level_system_command_requeues_retained_active_source() {
    let mut app = test_app();
    app.active_level_source = Some(LevelSource::Path(PathBuf::from(
        "content/dev/maps/raw-dev-map.prl",
    )));

    script_ctx(&app).system_commands.push(
        postretro_entities::SystemReactionCommand::RestartLevel {
            dispatch_source: "named:restart".to_string(),
        },
    );
    app.dispatch_system_commands();

    assert_eq!(
        app.level_requests.pop_front(),
        Some(LevelRequest::Load(LevelSource::Path(PathBuf::from(
            "content/dev/maps/raw-dev-map.prl"
        ))))
    );
    assert!(app.level_requests.is_empty());
}

/// Map authority is the host's (`networking.md` §Slot lifecycle): a
/// connected client drops `loadLevel` and `restartLevel` from any local
/// source, leaves its modal stack alone, and warns once per source and
/// verb however often the source fires.
#[test]
fn connected_client_drops_level_verbs_and_warns_once_per_source() {
    use postretro_test_log_capture::LogCapture;
    let mut app = test_app();
    connect_test_app_as_client(&mut app);
    app.active_level_source = Some(LevelSource::Catalog("e1m1".to_string()));
    app.session
        .as_mut()
        .unwrap()
        .modal_stack
        .registry_mut()
        .register(
            "deathScreen",
            postretro_ui::demo::build_frontend_menu_descriptor(),
            postretro_ui::modal_stack::ScopeTier::Mod,
            false,
        );
    app.session
        .as_mut()
        .unwrap()
        .modal_stack
        .push_named("deathScreen", None);

    let capture = LogCapture::start();
    for _ in 0..2 {
        for source in ["crossing:7", "named:levelLoad"] {
            script_ctx(&app).system_commands.push(
                postretro_entities::SystemReactionCommand::LoadLevel {
                    map: "e1m2".to_string(),
                    dispatch_source: source.to_string(),
                },
            );
        }
        script_ctx(&app).system_commands.push(
            postretro_entities::SystemReactionCommand::RestartLevel {
                dispatch_source: "crossing:7".to_string(),
            },
        );
        app.dispatch_system_commands();
    }

    assert!(
        app.level_requests.is_empty(),
        "a client never queues its own level change: {:?}",
        app.level_requests,
    );
    assert_eq!(
        app.session.as_mut().unwrap().modal_stack.active_name(),
        Some("deathScreen"),
        "a dropped level verb leaves the initiating modal in place",
    );
    capture.assert_logged_once(
        log::Level::Warn,
        "loadLevel from `crossing:7` ignored on a connected client",
    );
    capture.assert_logged_once(
        log::Level::Warn,
        "loadLevel from `named:levelLoad` ignored on a connected client",
    );
    capture.assert_logged_once(
        log::Level::Warn,
        "restartLevel from `crossing:7` ignored on a connected client",
    );
}

/// The other side of the client gate: single-player (no endpoint) and a
/// listen host both run the level verbs, and neither warns.
#[test]
fn host_and_single_player_run_level_verbs_without_warning() {
    use postretro_test_log_capture::LogCapture;
    for host in [false, true] {
        let mut app = test_app();
        if host {
            app.session.as_mut().unwrap().net_endpoint = Some(
                crate::netcode::NetEndpoint::from_role(
                    &crate::netcode::NetRole::Host { port: 0 },
                    None,
                )
                .expect("host endpoint constructs")
                .expect("host role yields an endpoint"),
            );
        }
        let capture = LogCapture::start();
        script_ctx(&app).system_commands.push(
            postretro_entities::SystemReactionCommand::LoadLevel {
                map: "e1m2".to_string(),
                dispatch_source: "crossing:7".to_string(),
            },
        );
        app.dispatch_system_commands();

        assert_eq!(
            app.level_requests.pop_front(),
            Some(LevelRequest::Load(LevelSource::Catalog("e1m2".to_string()))),
            "host={host}",
        );

        app.active_level_source = Some(LevelSource::Catalog("e1m1".to_string()));
        script_ctx(&app).system_commands.push(
            postretro_entities::SystemReactionCommand::RestartLevel {
                dispatch_source: "crossing:7".into(),
            },
        );
        app.dispatch_system_commands();

        assert_eq!(
            app.level_requests.pop_front(),
            Some(LevelRequest::Load(LevelSource::Catalog("e1m1".to_string()))),
            "host={host}",
        );
        capture.assert_not_logged(log::Level::Warn, "ignored on a connected client");
    }
}

#[test]
fn return_to_frontend_system_command_queues_unload_then_backdrop_load() {
    let mut app = test_app();
    app.session
        .as_mut()
        .unwrap()
        .modal_stack
        .registry_mut()
        .register(
            "mainMenu",
            postretro_ui::demo::build_frontend_menu_descriptor(),
            postretro_ui::modal_stack::ScopeTier::Mod,
            false,
        );
    app.session.as_mut().unwrap().frontend = Some(Frontend {
        menu_tree: "mainMenu".to_string(),
        background_level: Some("menuBackdrop".to_string()),
        camera: MenuCamera {
            position: [0.0, 0.0, 0.0],
            yaw: 0.0,
            pitch: 0.0,
        },
    });

    script_ctx(&app)
        .system_commands
        .push(postretro_entities::SystemReactionCommand::ReturnToFrontend);
    app.dispatch_system_commands();

    assert_eq!(
        app.session.as_mut().unwrap().modal_stack.active_name(),
        Some("mainMenu"),
        "returning to frontend presents the menu before backdrop reload",
    );
    assert_eq!(app.level_requests.pop_front(), Some(LevelRequest::Unload));
    assert_eq!(
        app.level_requests.pop_front(),
        Some(LevelRequest::Load(LevelSource::Catalog(
            "menuBackdrop".to_string()
        )))
    );
    assert!(app.level_requests.is_empty());
}

#[cfg(feature = "dev-tools")]
#[test]
fn dev_level_cycle_ignores_missing_target_without_unloading() {
    let mut app = test_app();
    app.level = Some(level_world(FIXTURE_MAP_A, 1));
    app.boot_state = BootState::Running;

    let mut missing_target =
        std::env::temp_dir().join("postretro-missing-dev-level-cycle-target.prl");
    let mut salt = 0;
    while missing_target.exists() {
        salt += 1;
        missing_target = std::env::temp_dir().join(format!(
            "postretro-missing-dev-level-cycle-target-{salt}.prl"
        ));
    }

    app.enqueue_dev_level_cycle_target(missing_target);

    assert!(
        app.level.is_some(),
        "missing generated dev PRL must not unload the active level",
    );
    assert!(app.level_requests.is_empty());
    assert!(matches!(app.boot_state, BootState::Running));
}

#[cfg(feature = "dev-tools")]
#[test]
fn dev_level_cycle_ignores_runtime_load_in_flight_without_queueing_duplicate() {
    let mut app = test_app();
    let (_tx, rx) = std::sync::mpsc::channel();
    app.boot_state = BootState::Loading;
    app.boot_load = false;
    app.level_rx = Some(rx);

    let target = std::env::temp_dir().join(format!(
        "postretro-existing-dev-level-cycle-target-{}.prl",
        std::process::id()
    ));
    std::fs::write(&target, b"test target exists").expect("create dev cycle test target");

    app.enqueue_dev_level_cycle_target(target.clone());

    let _ = std::fs::remove_file(&target);
    assert!(
        app.level_requests.is_empty(),
        "duplicate dev lifecycle cycle must not queue behind an active runtime load",
    );
    assert!(matches!(app.boot_state, BootState::Loading));
    assert!(app.level_rx.is_some());
}
