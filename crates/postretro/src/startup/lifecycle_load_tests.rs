//! The level-load state machine driven through the App: dispatch, the Loading
//! frame, the commit on the frame the bundle is ready, supersede, queued
//! unloads, suspend and retirement, one failure per load, the tiers read at
//! dispatch, the whole-load progress, and the grep gates on the commit path.
//! See: context/plans/done/level-load-pipeline/index.md (Acceptance: Off the main thread 1, 3, 6, 7; Failure 1–3; Cancellation 1–8; Settings 1; Models 6; Progress 1) · research.md §9 (P1–P8, P10, P12, P18, P20–P22, P24)

use std::cell::Cell;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use postretro_level_loader::{HandOutStage, LevelLoadProgress, LoadPhase};
use postretro_renderer::{PipelineBuildRelease, gpu_creation_counts};
use postretro_test_log_capture::LogCapture;

use super::lifecycle_commit_tests::{CommitCase, dev_content_root};
use super::lifecycle_load::LoadingFrame;
use super::lifecycle_test_fixtures::*;
use super::*;
use crate::level_load::test_support::{APP_LOAD_TIMEOUT, HOLD_TIMEOUT, UploadFault};
use crate::level_load::{LevelPrepInputs, PrepKind};
use crate::render::{LevelBuildPart, Renderer};
use crate::session::level_streaming::settle::{ResourceSettle, SettleReport};
use crate::startup::{BootDestination, LevelRequest};

const DT: f32 = 1.0 / 60.0;

/// The test's stand-in for the event loop: records an exit request.
#[derive(Default)]
struct TestExit(Cell<bool>);

impl LoopExit for TestExit {
    fn exit(&self) {
        self.0.set(true);
    }
}

/// A point a stage thread parks at until the test releases it (or
/// [`HOLD_TIMEOUT`] passes, so a failed test never strands a thread). Once
/// released it stays open: later loads pass straight through.
#[derive(Clone, Default)]
struct Hold(Arc<HoldState>);

#[derive(Default)]
struct HoldState {
    reached: AtomicBool,
    open: AtomicBool,
}

impl Hold {
    fn park(&self) {
        self.0.reached.store(true, Ordering::Release);
        let deadline = Instant::now() + HOLD_TIMEOUT;
        while !self.0.open.load(Ordering::Acquire) && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(1));
        }
    }

    fn reached(&self) -> bool {
        self.0.reached.load(Ordering::Acquire)
    }

    fn release(&self) {
        self.0.open.store(true, Ordering::Release);
    }

    fn wait_reached(&self, what: &str) {
        let deadline = Instant::now() + APP_LOAD_TIMEOUT;
        while !self.reached() {
            assert!(Instant::now() < deadline, "never reached the hold: {what}");
            std::thread::sleep(Duration::from_millis(1));
        }
    }
}

/// A test App over `renderer`, set up as full init sets one up (the
/// player's tiers, built-in classnames, a pinned trigger-pool seed).
fn app_with(renderer: Renderer, content_root: &Path) -> App {
    let mut app = test_app();
    app.renderer = Some(renderer);
    app.content_root = content_root.to_path_buf();
    crate::scripting::builtins::register_builtins(
        &mut app.session.as_mut().unwrap().classname_dispatch,
    );
    let options = app.session.as_ref().unwrap().player_options.clone();
    app.configure_player_shadow_quality(options.shadow_quality);
    app.apply_player_surface_depth_quality(options.surface_depth_quality);
    app.apply_player_reflection_quality(options.reflection_quality);
    app.session_boot_config = crate::startup::session::SessionBootConfig::with_pool_seed(1);
    app.boot_state = BootState::Frontend;
    app
}

fn gpu_app(test: &str, case: &CommitCase) -> Option<App> {
    crate::level_load::test_support::offscreen_renderer_or_skip(test)
        .map(|renderer| app_with(renderer, &case.content_root))
}

/// An offscreen renderer whose install-pipeline build (its upload thread's
/// first job) waits for the returned handle; `None` without an adapter.
fn held_renderer(test: &str) -> Option<(Renderer, PipelineBuildRelease)> {
    crate::level_load::test_support::offscreen_renderer_holding_pipeline_build_or_skip(test)
}

/// Every dispatch from here on reads `case`'s `.prm` root and edits, then
/// `extra`'s.
fn use_case(app: &mut App, case: &CommitCase, extra: impl Fn(&mut LevelPrepInputs) + 'static) {
    let prm_root = case.prm_root.clone();
    let (edit_world, edit_hand_out) = (case.edit_world, case.edit_hand_out);
    app.level_loads.test.edit_inputs = Some(Box::new(move |inputs| {
        inputs.prm_cache_root = prm_root.clone();
        inputs.hooks.edit_world = edit_world;
        inputs.hooks.edit_hand_out = edit_hand_out;
        extra(inputs);
    }));
}

fn request_path(app: &mut App, map: &Path) {
    app.enqueue_level_request(LevelRequest::Load(LevelSource::Path(map.to_path_buf())));
}

/// The paint's CPU half: the bar, the UI clock, the image poll, the tree.
fn paint_cpu(app: &mut App) {
    app.advance_loading_screen(DT);
    app.poll_loading_images();
    let _ = app.loading_screen_snapshot();
}

/// One redraw's load-relevant part, in `redraw`'s order: the retirement poll
/// (every boot state), the request drain (where the state drains), then the
/// Loading step and, when it paints, the paint's CPU half. `None` on a frame
/// that is not a Loading frame.
fn frame(app: &mut App, exit: &TestExit) -> Option<LoadingFrame> {
    app.poll_stage_retirements();
    if super::lifecycle_boot_state::boot_state_drains_level_requests(&app.boot_state) {
        app.drain_level_requests();
    }
    if app.boot_state != BootState::Loading {
        return None;
    }
    let step = app.step_loading_frame(exit, DT);
    if step == LoadingFrame::Paint {
        paint_cpu(app);
    }
    Some(step)
}

/// Run frames until `done`, failing past [`APP_LOAD_TIMEOUT`].
fn run_until(app: &mut App, exit: &TestExit, what: &str, done: impl Fn(&App) -> bool) {
    let deadline = Instant::now() + APP_LOAD_TIMEOUT;
    while !done(app) {
        assert!(
            Instant::now() < deadline,
            "{what}: never happened; state {:?}, load {:?}",
            app.boot_state,
            app.level_loads
        );
        frame(app, exit);
        std::thread::sleep(Duration::from_millis(1));
    }
}

/// No load in flight and none queued: the last request's load ended.
fn load_ended(app: &App) -> bool {
    !app.level_load_in_flight() && app.level_requests.is_empty()
}

/// Frames until the load requested commits (Settling) or ends otherwise.
fn run_to_commit(app: &mut App, exit: &TestExit, what: &str) {
    run_until(app, exit, what, load_ended);
    assert_eq!(app.boot_state, BootState::Settling, "{what}: committed");
}

/// Poll the retirement list empty, failing past [`APP_LOAD_TIMEOUT`].
fn retire_all(app: &mut App) {
    let deadline = Instant::now() + APP_LOAD_TIMEOUT;
    while !app.stage_retirements.is_empty() {
        assert!(Instant::now() < deadline, "the stages never retired");
        app.poll_stage_retirements();
        std::thread::sleep(Duration::from_millis(1));
    }
}

/// The staged load's prep probe.
fn probe(app: &App) -> crate::level_load::test_support::PrepProbe {
    app.level_loads
        .staged()
        .expect("the load was dispatched")
        .probe()
}

/// Its stages are done: prep settled and the set delivered (not yet taken).
fn stages_ready(app: &App) -> bool {
    app.level_loads.staged().is_some_and(|stages| {
        let probe = stages.probe();
        probe.state().settled && probe.upload_parts_done().contains(&LevelBuildPart::Finish)
    })
}

fn active_generation(app: &App) -> Option<u64> {
    app.level_loads.active().map(|active| active.generation)
}

fn dispatched(app: &App) -> Vec<u64> {
    app.level_loads
        .test
        .dispatched
        .iter()
        .map(|(generation, _)| *generation)
        .collect()
}

/// The installed level's source path.
fn installed_path(app: &App) -> Option<PathBuf> {
    match app.active_level_source.as_ref()? {
        LevelSource::Path(path) => Some(path.clone()),
        LevelSource::Catalog(id) => Some(PathBuf::from(id)),
    }
}

fn loader_errors(capture: &LogCapture) -> Vec<String> {
    capture
        .records()
        .into_iter()
        .filter(|record| {
            record.level == log::Level::Error && record.message.starts_with("[Loader]")
        })
        .map(|record| record.message)
        .collect()
}

fn progress_slot(app: &App) -> f32 {
    match postretro_scripting_core::store_bridge::read_store_slot(
        &app.session.as_ref().unwrap().scripting.script_ctx,
        "loading.progress",
    )
    .unwrap()
    {
        postretro_entities::slot_table::SlotValue::Number(value) => value,
        other => panic!("loading.progress is a number: {other:?}"),
    }
}

/// What a failed load must leave untouched in the registry: the entities
/// a level install creates (lights, fog volumes, meshes) and the local pawn.
fn registry_fingerprint(app: &App) -> (usize, usize, usize, bool) {
    use postretro_entities::ComponentKind;
    let ctx = script_ctx(app);
    let registry = ctx.registry.borrow();
    (
        registry.iter_with_kind(ComponentKind::Light).count(),
        registry.iter_with_kind(ComponentKind::FogVolume).count(),
        registry.iter_with_kind(ComponentKind::Mesh).count(),
        registry.local_player_pawn().is_some(),
    )
}

/// Hold every dispatch behind a stand-in retiring stage until the returned
/// hold opens: the next load starts queued, its counter and loading screen
/// up, no stage work.
fn block_dispatch(app: &mut App) -> Hold {
    let gate = Hold::default();
    let open = gate.clone();
    app.stage_retirements
        .push_upload(postretro_renderer::UploadRetirement::from_thread(
            std::thread::spawn(move || {
                let deadline = Instant::now() + HOLD_TIMEOUT;
                while !open.0.open.load(Ordering::Acquire) && Instant::now() < deadline {
                    std::thread::sleep(Duration::from_millis(1));
                }
            }),
        ));
    gate
}

fn register_loading_tree(app: &mut App) {
    app.session
        .as_mut()
        .unwrap()
        .modal_stack
        .registry_mut()
        .register(
            super::super::loading_screen::LOADING_SCREEN_NAME,
            postretro_ui::tree_asset::load_named_tree(
                &postretro_ui::CoreRoot::at(
                    Path::new(env!("CARGO_MANIFEST_DIR"))
                        .join("../..")
                        .join("core"),
                )
                .ui_asset_path("loadingScreen.json"),
            )
            .expect("the engine loading tree loads"),
            postretro_ui::modal_stack::ScopeTier::Engine,
            false,
        );
}

/// Each prep kind's task, recorded with the thread that ran it.
type TaskThreads = Arc<std::sync::Mutex<Vec<(PrepKind, std::thread::ThreadId)>>>;

/// Off the main thread 1, on `case`: the upload stage held at its first
/// creation (the upload thread's first job, the install-pipeline build,
/// parked, so the load's parts queue behind it) while 12 Loading frames run
/// and paint on this, the main thread. Prep runs with audio on, its sounds
/// decoded from `sound_root`. Every prep task records the thread it ran on,
/// the parse and each model parse too; none ran here, and each of `expect`
/// ran. Across the held frames the main thread creates no GPU object and the
/// transport polls on each. Released, the load reaches bundle-ready and
/// commits.
fn assert_upload_hold_keeps_main_thread_clear(
    test: &str,
    case: &CommitCase,
    sound_root: &Path,
    expect: &[PrepKind],
) {
    let Some((renderer, release)) = held_renderer(test) else {
        return;
    };
    let mut app = app_with(renderer, &case.content_root);
    let main_thread = std::thread::current().id();
    let parse_thread = Arc::new(std::sync::Mutex::new(None));
    let model_thread = Arc::new(std::sync::Mutex::new(None));
    let task_threads: TaskThreads = Arc::default();
    let (parse_seen, model_seen, tasks_seen) = (
        Arc::clone(&parse_thread),
        Arc::clone(&model_thread),
        Arc::clone(&task_threads),
    );
    let sound_root = sound_root.to_path_buf();
    use_case(&mut app, case, move |inputs| {
        inputs.audio_enabled = true;
        inputs.content_root = sound_root.clone();
        let parse_seen = Arc::clone(&parse_seen);
        inputs.hooks.after_hand_out = Some(Arc::new(move |_, _| {
            *parse_seen.lock().unwrap() = Some(std::thread::current().id());
        }));
        let model_seen = Arc::clone(&model_seen);
        inputs.hooks.before_model_parse = Some(Arc::new(move |_| {
            *model_seen.lock().unwrap() = Some(std::thread::current().id());
        }));
        let tasks_seen = Arc::clone(&tasks_seen);
        inputs.hooks.on_task_run = Some(Arc::new(move |kind| {
            tasks_seen
                .lock()
                .unwrap()
                .push((kind, std::thread::current().id()));
        }));
    });
    let exit = TestExit::default();
    request_path(&mut app, &case.map);
    frame(&mut app, &exit);
    assert!(app.level_loads.is_staged(), "dispatched on the drain");
    // Prep runs to its end on the pool while the upload thread is held.
    assert!(
        probe(&app).wait_until(|state| state.settled),
        "prep finished behind the held upload"
    );
    assert!(
        probe(&app).upload_parts_done().is_empty(),
        "no set-build part ran: the upload is held at its first creation"
    );

    let polls = app.level_loads.test.transport_polls;
    let mut font_system = postretro_ui::text::build_embedded_font_system();
    for held_frame in 0..12 {
        let before = gpu_creation_counts();
        let step = frame(&mut app, &exit);
        assert_eq!(
            gpu_creation_counts().since(before).total(),
            0,
            "frame {held_frame}: the Loading step created a GPU object"
        );
        assert_eq!(step, Some(LoadingFrame::Paint), "frame {held_frame}");
        // A real paint of the loading frame, on the main thread.
        let renderer = app.renderer.as_mut().unwrap();
        renderer.set_ui_snapshot(postretro_ui::UiReadSnapshot::default());
        renderer
            .capture_world_less_frame(&mut font_system, crate::render::SPLASH_CLEAR_COLOR, 0.0)
            .expect("the held Loading frame paints");
    }
    assert_eq!(
        app.level_loads.test.transport_polls - polls,
        12,
        "the transport polled on every Loading frame"
    );
    assert!(app.level.is_none(), "nothing committed while held");
    let parse_ran_on = parse_thread.lock().unwrap().expect("the parse ran");
    assert_ne!(
        parse_ran_on, main_thread,
        "the parse ran off the main thread"
    );
    if !case.models.is_empty() {
        let model_ran_on = model_thread.lock().unwrap().expect("the model parse ran");
        assert_ne!(
            model_ran_on, main_thread,
            "the model parse ran off the main thread"
        );
    }
    let ran = task_threads.lock().unwrap().clone();
    for kind in expect {
        assert!(
            ran.iter().any(|(ran_kind, _)| ran_kind == kind),
            "{}: {kind:?} ran as a prep task: {ran:?}",
            case.label
        );
    }
    let on_main: Vec<PrepKind> = ran
        .iter()
        .filter(|(_, thread)| *thread == main_thread)
        .map(|(kind, _)| *kind)
        .collect();
    assert!(
        on_main.is_empty(),
        "{}: prep work ran on the main thread: {on_main:?}",
        case.label
    );

    release.release();
    run_to_commit(&mut app, &exit, "the released load");
    assert!(!exit.0.get());
    assert_eq!(installed_path(&app).as_deref(), Some(case.map.as_path()));
    retire_all(&mut app);
}

// Off the main thread 1, on the textured level with a model: with the upload
// stage held at its first creation, Loading frames keep painting and the
// transport polls on each; across them the main thread creates no GPU object,
// and the parse, each `.prm` read and parse (texture prep), the glTF parse
// and the sound decode each ran on a pool thread, never on the main thread.
// Released, the load reaches bundle-ready and commits.
#[test]
fn staged_load_upload_hold_keeps_loading_frames_painting() {
    let case = CommitCase::textured_with_model();
    assert_upload_hold_keeps_main_thread_clear(
        "staged_load_upload_hold_keeps_loading_frames_painting",
        &case,
        &dev_content_root(),
        &[
            PrepKind::Textures,
            PrepKind::Sounds,
            PrepKind::ModelPrediction,
            PrepKind::ModelParse,
        ],
    );
}

// Off the main thread 1, the SH half, on the streamed-SH level: the SH
// residency plan and the topology derivation (the streaming level data: the
// id-49 hints and the planner topology) and the sound decode each ran on a
// pool thread while the held Loading frames painted on the main thread.
#[test]
fn staged_load_upload_hold_derives_sh_off_the_main_thread() {
    let case = CommitCase::streamed_sh();
    assert_upload_hold_keeps_main_thread_clear(
        "staged_load_upload_hold_derives_sh_off_the_main_thread",
        &case,
        &dev_content_root(),
        &[
            PrepKind::ShResidency,
            PrepKind::StreamingData,
            PrepKind::Sounds,
        ],
    );
}

// Off the main thread 3, the frame half: a Loading step whose bundle is not
// ready paints, at once, while a stage thread is parked; nothing waits.
#[test]
fn loading_step_paints_when_bundle_not_ready() {
    let test = "loading_step_paints_when_bundle_not_ready";
    let case = CommitCase::textured();
    let Some(mut app) = gpu_app(test, &case) else {
        return;
    };
    let hold = Hold::default();
    let parked = hold.clone();
    use_case(&mut app, &case, move |inputs| {
        let parked = parked.clone();
        inputs.hooks.after_hand_out = Some(Arc::new(move |stage, _| {
            if stage == HandOutStage::Geometry {
                parked.park();
            }
        }));
    });
    let exit = TestExit::default();
    request_path(&mut app, &case.map);
    frame(&mut app, &exit);
    hold.wait_reached("the parse's geometry hand-out");
    for _ in 0..8 {
        let started = Instant::now();
        assert_eq!(frame(&mut app, &exit), Some(LoadingFrame::Paint));
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "a Loading step waited on a parked stage: {:?}",
            started.elapsed()
        );
        assert!(app.level_load_in_flight());
    }
    hold.release();
    run_to_commit(&mut app, &exit, "the released load");
    retire_all(&mut app);
}

/// One Settling redraw's offscreen stand-in, in `run_settling_frame`'s
/// order: a release decided on the previous frame reveals now (`None`);
/// otherwise the held level's streaming step from the presented pose (the
/// real `prepare_held_level_frame`), the settle check against the session's
/// streaming state and its decision (`advance_settle`), the loading screen's
/// paint, and the held frame drawn with that step's drain batch, whose
/// outcome the session applies. The frame's report. (The windowed redraw
/// also polls the transport and staged reloads, which no test here feeds; it
/// presents through a surface, which an offscreen renderer lacks, so the
/// held frame draws through `capture_held_level_frame`.)
fn settling_frame(app: &mut App) -> Option<SettleReport> {
    app.poll_stage_retirements();
    if app.reveal_if_released() {
        return None;
    }
    assert_eq!(app.boot_state, BootState::Settling, "a Settling frame");
    let frame_start = Instant::now();
    app.commit_render_extents();
    let view = app.presented_pose().held_view(app.camera.aspect());
    let held = app
        .prepare_held_level_frame(view)
        .expect("the held level streams")
        .expect("a held level frame: a level, a session and a full renderer");
    let report = app.session.as_ref().unwrap().settle_report();
    app.advance_settle(report, frame_start);
    paint_cpu(app);
    capture_held_level_frame(app, held);
    Some(report)
}

/// Draw `held` through the offscreen capture where a Settling frame presents
/// it on the surface: the same sample regions (main and mirror), the same
/// drain batch, its outcome applied and its compose marked as the windowed
/// frame does. An offscreen renderer has no surface, and a capture has no
/// compose-only scene, so the frame also records the world draw a held
/// frame skips; nothing it draws reaches the session's settle state.
fn capture_held_level_frame(app: &mut App, held: crate::app::world_less_frame::HeldLevelFrame) {
    let (view, visible_render, batch) = held.into_parts();
    let App {
        session,
        renderer,
        planar_render,
        script_time,
        ..
    } = app;
    let (Some(session), Some(renderer)) = (session.as_mut(), renderer.as_mut()) else {
        panic!("a held frame needs a session and a renderer");
    };
    renderer.set_planar_reflection_view(planar_render.camera(), planar_render.visible());
    renderer.set_planar_mirror_light_reach(planar_render.mirror_light_cell_aabbs());
    let result = renderer
        .capture_frame_indirect(
            visible_render.camera_cull(),
            &visible_render.light_reachable_cell_mask,
            &visible_render.reachable_cell_aabbs,
            &visible_render.fog_reachable,
            crate::render::ShSampleRegionSets {
                visible_cells: planar_render.drawn_cells(&visible_render.visible_cells),
                fog_cells: planar_render.sample_fog_cells(&visible_render.fog_reachable),
                movers: &[],
            },
            Some(visible_render.stats.camera_cell),
            view.camera.view_projection,
            view.camera.eye_position,
            &[],
            &[],
            *script_time as f32,
            crate::render::SPLASH_CLEAR_COLOR,
            true,
            batch,
        )
        .expect("the held frame drains");
    let submitted = result.compose_submitted;
    session
        .apply_sh_streaming_outcome(result.outcome, renderer)
        .expect("the outcome applies");
    result.frame.expect("the held frame draws");
    session.mark_sh_streaming_compose_submitted(submitted);
}

/// Settling frames until one reveals, failing past [`APP_LOAD_TIMEOUT`]:
/// each frame's report, in order.
fn settle_to_reveal(app: &mut App, what: &str) -> Vec<SettleReport> {
    let deadline = Instant::now() + APP_LOAD_TIMEOUT;
    let mut reports = Vec::new();
    while let Some(report) = settling_frame(app) {
        reports.push(report);
        assert!(
            Instant::now() < deadline,
            "{what}: never revealed; last report {report:?}"
        );
        std::thread::sleep(Duration::from_millis(1));
    }
    assert_eq!(app.boot_state, BootState::Running, "{what}: revealed");
    reports
}

/// Off the main thread 5, the reveal half: `case` loads through Loading
/// frames to its commit, then real Settling frames reach the release and the
/// reveal, settled (the session's settle check answered, never the
/// timeout). `check` reads the committed level first.
fn assert_reaches_reveal(test: &str, case: &CommitCase, check: impl Fn(&App)) {
    let Some(mut app) = gpu_app(test, case) else {
        return;
    };
    register_loading_tree(&mut app);
    use_case(&mut app, case, |_| {});
    let exit = TestExit::default();
    let capture = LogCapture::start();
    request_path(&mut app, &case.map);
    run_to_commit(&mut app, &exit, case.label);
    check(&app);
    let counter = app.loading_screen_progress().expect("the load's counter");
    let entered = Instant::now();
    let reports = settle_to_reveal(&mut app, case.label);
    let last = *reports.last().expect("at least one Settling frame decided");
    assert!(
        last.settled(),
        "{}: released settled, not timed out: {last:?}",
        case.label
    );
    assert!(
        entered.elapsed() < super::super::settling::SETTLE_TIMEOUT,
        "{}: before the timeout",
        case.label
    );
    capture.assert_not_logged(log::Level::Warn, "settle timeout");
    assert_eq!(counter.fraction(), 1.0, "{}: 1.0 at reveal", case.label);
    assert!(
        app.loading_screen_progress().is_none(),
        "{}: the loading screen ended",
        case.label
    );
    assert!(!exit.0.get(), "{}: no exit", case.label);
    retire_all(&mut app);
}

// Off the main thread 5: a map with no textures reaches reveal. The
// streamed-SH level names no texture, so its texture prep is an empty kind;
// its one SH cluster streams in through real Settling frames (the session's
// SH answer moves from asked to settled), and the reveal follows.
#[test]
fn no_texture_map_reaches_reveal() {
    let case = CommitCase::streamed_sh();
    assert_reaches_reveal("no_texture_map_reaches_reveal", &case, |app| {
        let level = app.level.as_ref().expect("committed");
        assert!(level.texture_names.is_empty(), "the level has no texture");
        assert!(level.sh_stream_manifest().is_some(), "its SH streams");
        let sh = app.session.as_ref().unwrap().settle_report().sh;
        assert_ne!(sh, ResourceSettle::NotStreamed, "SH is a settle resource");
    });
}

// Off the main thread 5: a map with no SH manifest reaches reveal. The
// textured level's SH is a resident volume with no stream manifest, so no
// residency plan or SH session exists (SH answers "not streamed"); its
// baked texture uploads and its lightmap streams in through real Settling
// frames until the settle check releases it, and the reveal follows.
#[test]
fn no_sh_manifest_map_reaches_reveal() {
    let case = CommitCase::textured();
    assert_reaches_reveal("no_sh_manifest_map_reaches_reveal", &case, |app| {
        let level = app.level.as_ref().expect("committed");
        assert!(level.sh_stream_manifest().is_none(), "no SH manifest");
        assert!(!level.texture_names.is_empty(), "a baked texture");
        assert_eq!(
            app.session.as_ref().unwrap().settle_report().sh,
            ResourceSettle::NotStreamed,
            "no SH session"
        );
    });
}

/// The first level frame's windowed entry with its surface acquire forced
/// to skip: the in-play streaming step, then `render_frame_indirect`, which
/// takes the skipped-acquire path (its drain admitted, no compose recorded,
/// the frame's queued writes flushed) and presents nothing.
fn skipped_level_frame(app: &mut App) {
    use postretro_visibility::{CameraCullVisibility, VisibilityPath, VisibleCells};
    app.poll_stage_retirements();
    assert_eq!(app.boot_state, BootState::Running, "a level frame");
    app.script_time += f64::from(DT);
    app.commit_render_extents();
    let cells = VisibleCells::Culled(vec![0]);
    let cpu = postretro_stage_timing::StageFrame::default();
    let now = app.script_time;
    let (Some(world), Some(session), Some(renderer)) = (
        app.level.as_ref(),
        app.session.as_mut(),
        app.renderer.as_mut(),
    ) else {
        panic!("a level frame has a level, a session and a renderer");
    };
    let batch = session
        .run_level_streaming_step(
            world.sh_stream_manifest(),
            Some(world),
            renderer,
            crate::session::level_streaming::StreamingFrame {
                visible_cells: &cells,
                sh_visible_cells: &cells,
                camera_cell: Some(0),
                path: VisibilityPath::PrlPortal { walk_reach: 1 },
                monotonic_seconds: now,
                settling: false,
                cpu: &cpu,
            },
        )
        .expect("the in-play streaming step runs");
    renderer.skip_next_surface_acquire();
    let result = renderer
        .render_frame_indirect(
            &mut session.font_system,
            CameraCullVisibility {
                cells: &cells,
                path: VisibilityPath::PrlPortal { walk_reach: 1 },
            },
            &[],
            &[],
            &[],
            crate::render::ShSampleRegionSets {
                visible_cells: &cells,
                fog_cells: &[],
                movers: &[],
            },
            Some(0),
            glam::Mat4::IDENTITY,
            &[],
            now,
            crate::render::SPLASH_CLEAR_COLOR,
            crate::render::FrameScene::World,
            batch,
        )
        .expect("the frame drains");
    session
        .apply_sh_streaming_outcome(result.outcome, renderer)
        .expect("the outcome applies");
    session.mark_sh_streaming_compose_submitted(result.compose_submitted);
    assert!(
        result.frame.expect("the frame runs").is_none(),
        "the acquire skipped: nothing to present"
    );
}

// Off the main thread 4 (P13): the upload's last write is submitted before
// the first frame that draws it presents, also when that frame's surface
// acquire is skipped. A full App load of the textured level with a model
// (a baked texture and a model, both written on the upload thread) runs
// through Loading frames to its commit and through real Settling frames to
// the reveal; the first two level frames run the windowed frame entry with
// the surface acquire forced to skip (`skip_next_surface_acquire`: the
// offscreen renderer has no surface, so this is the seam, and the frame
// takes the occluded/timed-out path), and the third presents through the
// offscreen capture, the present's stand-in. The upload thread's write
// ledger reads every staged byte submitted, by upload-thread submits, at
// the commit, across both skipped frames and just before the present; no
// upload write lands after it.
#[test]
fn upload_last_write_submitted_before_first_level_present() {
    let test = "upload_last_write_submitted_before_first_level_present";
    let case = CommitCase::textured_with_model();
    let Some(mut app) = gpu_app(test, &case) else {
        return;
    };
    use_case(&mut app, &case, |_| {});
    let exit = TestExit::default();
    let writes = |app: &App| {
        app.renderer
            .as_ref()
            .unwrap()
            .upload_writes()
            .expect("a full renderer's upload thread")
    };
    let before = writes(&app);
    request_path(&mut app, &case.map);
    run_to_commit(&mut app, &exit, "the load");
    let at_commit = writes(&app);
    assert!(
        at_commit.staged_bytes > before.staged_bytes,
        "the load's set build wrote on the upload thread: {before:?} -> {at_commit:?}"
    );
    assert!(
        at_commit.submits > before.submits,
        "and submitted from it: {at_commit:?}"
    );
    assert_eq!(
        at_commit.unsubmitted_bytes(),
        0,
        "every upload write was submitted before the commit: {at_commit:?}"
    );
    settle_to_reveal(&mut app, "the load");
    assert_eq!(
        writes(&app),
        at_commit,
        "Settling wrote nothing on the upload thread"
    );
    for skipped in 0..2 {
        skipped_level_frame(&mut app);
        assert_eq!(
            writes(&app),
            at_commit,
            "skipped level frame {skipped}: every upload write stays submitted"
        );
    }
    let before_present = writes(&app);
    assert_eq!(
        before_present.unsubmitted_bytes(),
        0,
        "the last upload write was submitted before the first present: {before_present:?}"
    );
    assert_eq!(before_present.submitted_bytes, at_commit.submitted_bytes);
    beside::running_frame(&mut app, &exit).expect("the first presenting level frame");
    assert_eq!(
        writes(&app),
        before_present,
        "no upload write after the present"
    );
    retire_all(&mut app);
}

// Off the main thread 6: a load dispatched before the install pipelines are
// built waits for them on the upload thread, never on a Loading frame; once
// they land, its set build creates no pipeline of its own.
#[test]
fn load_waits_for_pipeline_build_off_main() {
    let test = "load_waits_for_pipeline_build_off_main";
    let case = CommitCase::textured();
    let Some((renderer, release)) = held_renderer(test) else {
        return;
    };
    let mut app = app_with(renderer, &case.content_root);
    use_case(&mut app, &case, |_| {});
    let exit = TestExit::default();
    request_path(&mut app, &case.map);
    frame(&mut app, &exit);
    assert!(probe(&app).wait_until(|state| state.settled));
    for _ in 0..6 {
        let started = Instant::now();
        assert_eq!(frame(&mut app, &exit), Some(LoadingFrame::Paint));
        assert!(started.elapsed() < Duration::from_secs(2));
    }
    assert!(probe(&app).upload_parts_done().is_empty());
    release.release();
    run_to_commit(&mut app, &exit, "the load after the build");
    let (created, _) = app
        .level_loads
        .test
        .last_upload_creations
        .expect("the set arrived");
    assert_eq!(created.pipelines, 0, "the set build reused the pipelines");
    assert!(created.total() > 0, "the build did create its resources");
    retire_all(&mut app);
}

// Off the main thread 6: a frontend-only boot builds the install pipelines
// without blocking a frame: with the build parked, Frontend frames run and a
// world-less frame paints.
#[test]
fn frontend_boot_builds_pipelines_without_blocking() {
    let test = "frontend_boot_builds_pipelines_without_blocking";
    let Some((renderer, release)) = held_renderer(test) else {
        return;
    };
    let mut app = app_with(renderer, &dev_content_root());
    let exit = TestExit::default();
    let mut font_system = postretro_ui::text::build_embedded_font_system();
    for _ in 0..4 {
        let started = Instant::now();
        assert_eq!(frame(&mut app, &exit), None, "a Frontend frame");
        let renderer = app.renderer.as_mut().unwrap();
        renderer.set_ui_snapshot(postretro_ui::UiReadSnapshot::default());
        renderer
            .capture_world_less_frame(&mut font_system, crate::render::SPLASH_CLEAR_COLOR, 0.0)
            .expect("the frontend frame paints while the build is parked");
        assert!(started.elapsed() < Duration::from_secs(5));
    }
    assert_eq!(app.boot_state, BootState::Frontend);
    release.release();
}

// Off the main thread 6, P5: cancelling a load while the pipeline build runs
// neither cancels nor restarts the build; the next load installs on it and
// its set build creates no pipeline.
#[test]
fn cancel_mid_pipeline_build_keeps_build() {
    let test = "cancel_mid_pipeline_build_keeps_build";
    let case = CommitCase::textured();
    let Some((renderer, release)) = held_renderer(test) else {
        return;
    };
    let mut app = app_with(renderer, &case.content_root);
    use_case(&mut app, &case, |_| {});
    let exit = TestExit::default();
    request_path(&mut app, &case.map);
    frame(&mut app, &exit);
    let first = active_generation(&app).unwrap();
    assert!(probe(&app).wait_until(|state| state.settled));
    // An unload request cancels the load mid-build.
    app.enqueue_level_request(LevelRequest::Unload);
    frame(&mut app, &exit);
    assert_eq!(app.boot_state, BootState::Frontend);
    assert!(!app.level_load_in_flight());
    assert_eq!(app.stage_retirements.retiring_loads(), [first]);
    release.release();
    request_path(&mut app, &case.map);
    run_to_commit(&mut app, &exit, "the next load");
    let (created, _) = app.level_loads.test.last_upload_creations.unwrap();
    assert_eq!(created.pipelines, 0, "the kept build serves the next load");
    assert_eq!(dispatched(&app).len(), 2);
    retire_all(&mut app);
}

/// How a suspend test parks the load.
#[derive(Debug, Clone, Copy)]
enum SuspendPoint {
    /// The parse, at its geometry hand-out: prep in flight.
    Prep,
    /// The upload thread, after the build's first part: later parts queued.
    Upload,
}

// Cancellation 1, P6: suspend during prep and during upload. The cancelled
// load's stages join through the retirement list, polled on frames before
// the resumed renderer exists and never waiting there; nothing commits; a
// load requested on the resumed renderer stays queued until they join, then
// installs on the new device.
#[test]
fn suspend_mid_load_retires_stages() {
    let test = "suspend_mid_load_retires_stages";
    for point in [SuspendPoint::Prep, SuspendPoint::Upload] {
        let case = CommitCase::textured();
        let Some(mut app) = gpu_app(test, &case) else {
            return;
        };
        let hold = Hold::default();
        let parked = hold.clone();
        use_case(&mut app, &case, move |inputs| match point {
            SuspendPoint::Prep => {
                let parked = parked.clone();
                inputs.hooks.after_hand_out = Some(Arc::new(move |stage, _| {
                    if stage == HandOutStage::Geometry {
                        parked.park();
                    }
                }));
            }
            SuspendPoint::Upload => {
                let parked = parked.clone();
                inputs.hooks.after_upload_part = Some(Arc::new(move |_| parked.park()));
            }
        });
        let exit = TestExit::default();
        request_path(&mut app, &case.map);
        frame(&mut app, &exit);
        hold.wait_reached(&format!("{point:?}"));

        app.suspend_platform();
        assert!(app.renderer.is_none(), "{point:?}");
        assert_eq!(app.boot_state, BootState::Booting, "{point:?}");
        assert!(!app.level_load_in_flight(), "{point:?}");
        assert_eq!(app.stage_retirements.retiring_loads().len(), 1, "{point:?}");
        // Frames before the resumed renderer exists poll without waiting.
        for _ in 0..4 {
            let started = Instant::now();
            assert_eq!(frame(&mut app, &exit), None);
            assert!(started.elapsed() < Duration::from_millis(500), "{point:?}");
        }
        assert!(
            !app.stage_retirements.is_empty(),
            "{point:?}: the parked stage has not joined"
        );

        // Resume, and request a load while the old stages still run.
        let renderer = crate::level_load::test_support::offscreen_renderer_or_skip(test).unwrap();
        app.renderer = Some(renderer);
        app.boot_state = BootState::Frontend;
        request_path(&mut app, &case.map);
        for _ in 0..4 {
            frame(&mut app, &exit);
            assert!(app.level_load_in_flight());
            assert!(
                !app.level_loads.is_staged(),
                "{point:?}: the resumed load waits for the old generation"
            );
        }
        assert!(
            app.level.is_none(),
            "{point:?}: the suspended load never committed"
        );
        hold.release();
        run_to_commit(&mut app, &exit, &format!("{point:?}: the resumed load"));
        assert_eq!(dispatched(&app).len(), 2, "{point:?}");
        assert!(!exit.0.get(), "{point:?}");
        retire_all(&mut app);
    }
}

// Cancellation 1, P22: suspend while the install-pipeline build runs. The
// old upload thread retires with its build; the resumed renderer builds its
// own; the load requested after resume waits for the old thread to join,
// then installs, its set build creating no pipeline: none from the old
// device reaches it.
#[test]
fn suspend_mid_pipeline_build_rebuilds_on_new_device() {
    let test = "suspend_mid_pipeline_build_rebuilds_on_new_device";
    let case = CommitCase::textured();
    let Some((renderer, release)) = held_renderer(test) else {
        return;
    };
    let mut app = app_with(renderer, &case.content_root);
    use_case(&mut app, &case, |_| {});
    let exit = TestExit::default();
    request_path(&mut app, &case.map);
    frame(&mut app, &exit);
    assert!(probe(&app).wait_until(|state| state.settled));

    app.suspend_platform();
    assert_eq!(app.stage_retirements.retiring_uploads(), 1);
    assert_eq!(app.stage_retirements.retiring_loads().len(), 1);
    let resumed = crate::level_load::test_support::offscreen_renderer_or_skip(test).unwrap();
    app.renderer = Some(resumed);
    app.boot_state = BootState::Frontend;
    request_path(&mut app, &case.map);
    for _ in 0..4 {
        frame(&mut app, &exit);
        assert!(!app.level_loads.is_staged(), "the old device's stages run");
    }
    release.release();
    run_to_commit(&mut app, &exit, "the resumed load");
    let (created, _) = app.level_loads.test.last_upload_creations.unwrap();
    assert_eq!(
        created.pipelines, 0,
        "the resumed renderer's own build served it"
    );
    let renderer = app.renderer.as_ref().unwrap();
    assert!(
        renderer.active_level_set().is_some(),
        "the resumed load's set"
    );
    assert!(
        renderer.resident_level_sets().is_empty(),
        "no other set reached the resumed renderer"
    );
    retire_all(&mut app);
}

// Cancellation 2, P20: a cancelled load reports nothing. Its stage fails
// after the cancel (held until then); the failure reaches no failure path,
// and the load that superseded it installs.
#[test]
fn cancelled_load_errors_reach_no_failure_path() {
    let test = "cancelled_load_errors_reach_no_failure_path";
    let case = CommitCase::textured();
    let Some(mut app) = gpu_app(test, &case) else {
        return;
    };
    let hold = Hold::default();
    let parked = hold.clone();
    let first_load = Arc::new(AtomicBool::new(true));
    use_case(&mut app, &case, move |inputs| {
        if first_load.swap(false, Ordering::AcqRel) {
            inputs.hooks.fail = vec![(PrepKind::Collision, "fails after its cancel")];
            let parked = parked.clone();
            inputs.hooks.before_injected_failure = Some(Arc::new(move |_| parked.park()));
        }
    });
    let capture = LogCapture::start();
    let exit = TestExit::default();
    request_path(&mut app, &case.map);
    frame(&mut app, &exit);
    hold.wait_reached("the injected failure");
    // Superseded while its failure is parked.
    request_path(&mut app, &case.map);
    frame(&mut app, &exit);
    hold.release();
    run_to_commit(&mut app, &exit, "the superseding load");
    assert!(
        loader_errors(&capture).is_empty(),
        "{:?}",
        loader_errors(&capture)
    );
    assert!(!exit.0.get());
    assert!(app.exit_result.is_ok());
    retire_all(&mut app);

    // Forced interleaving: an uncaptured wgpu error held for a load (its
    // upload parked, so it cannot commit first), which a request on the same
    // frame supersedes before any Loading step routes it. The cancel drops
    // it with the load; the next load never sees it.
    let upload = Hold::default();
    use_case(&mut app, &case, hold_upload(&upload));
    request_path(&mut app, &case.map);
    frame(&mut app, &exit);
    assert!(app.level_loads.is_staged());
    app.renderer
        .as_ref()
        .unwrap()
        .provoke_uncaptured_validation_error();
    request_path(&mut app, &case.map);
    frame(&mut app, &exit);
    upload.release();
    run_to_commit(&mut app, &exit, "the load after the held error");
    assert!(
        loader_errors(&capture).is_empty(),
        "{:?}",
        loader_errors(&capture)
    );
    assert!(app.exit_result.is_ok());
    retire_all(&mut app);
}

// Cancellation 2, P20: suspend during a CLI boot load; the resumed boot load
// installs and the process does not exit.
#[test]
fn suspend_during_boot_load_resumes_and_installs() {
    let test = "suspend_during_boot_load_resumes_and_installs";
    let case = CommitCase::textured();
    let Some(mut app) = gpu_app(test, &case) else {
        return;
    };
    let hold = Hold::default();
    let parked = hold.clone();
    use_case(&mut app, &case, move |inputs| {
        let parked = parked.clone();
        inputs.hooks.after_hand_out = Some(Arc::new(move |stage, _| {
            if stage == HandOutStage::Spatial {
                parked.park();
            }
        }));
    });
    let exit = TestExit::default();
    app.start_boot_destination(BootDestination::BootMap(case.map.clone()));
    assert!(app.boot_load);
    hold.wait_reached("the boot load's spatial hand-out");
    app.suspend_platform();
    assert!(!app.boot_load, "suspend resets the boot load");
    assert!(!app.level_load_in_flight());
    hold.release();
    // The splash replays on resume and starts the boot map again.
    let renderer = crate::level_load::test_support::offscreen_renderer_or_skip(test).unwrap();
    app.renderer = Some(renderer);
    app.start_boot_destination(BootDestination::BootMap(case.map.clone()));
    run_to_commit(&mut app, &exit, "the resumed boot load");
    assert!(!exit.0.get(), "the process does not exit");
    assert!(app.exit_result.is_ok());
    retire_all(&mut app);
}

/// Park the upload thread after each build part, through one hold.
fn hold_upload(hold: &Hold) -> impl Fn(&mut LevelPrepInputs) + 'static {
    let hold = hold.clone();
    move |inputs: &mut LevelPrepInputs| {
        let parked = hold.clone();
        inputs.hooks.after_upload_part = Some(Arc::new(move |_| parked.park()));
    }
}

// Cancellation 3: a superseding request mid-pipeline (the upload parked)
// cancels the in-flight load, which never commits; the new load installs.
#[test]
fn superseding_request_cancels_in_flight_load() {
    let test = "superseding_request_cancels_in_flight_load";
    let case = CommitCase::textured();
    let other = CommitCase::streamed_sh();
    let Some(mut app) = gpu_app(test, &case) else {
        return;
    };
    let hold = Hold::default();
    use_case(&mut app, &case, hold_upload(&hold));
    let exit = TestExit::default();
    request_path(&mut app, &case.map);
    frame(&mut app, &exit);
    hold.wait_reached("the upload's first part");
    let first = active_generation(&app).unwrap();
    use_case(&mut app, &other, |_| {});
    request_path(&mut app, &other.map);
    frame(&mut app, &exit);
    assert_eq!(app.stage_retirements.retiring_loads(), [first]);
    assert_ne!(active_generation(&app), Some(first));
    hold.release();
    run_to_commit(&mut app, &exit, "the superseding load");
    assert_eq!(installed_path(&app).as_deref(), Some(other.map.as_path()));
    retire_all(&mut app);
}

// Cancellation 3, P4: three loads requested on successive frames, each
// before the previous cancelled one joined: only the last installs, the
// middle never dispatches, no two loads' stages overlap, and the pool (built
// once) starts no thread for the later loads.
#[test]
fn three_successive_requests_install_only_last() {
    let test = "three_successive_requests_install_only_last";
    let case = CommitCase::textured();
    let Some(mut app) = gpu_app(test, &case) else {
        return;
    };
    let hold = Hold::default();
    use_case(&mut app, &case, hold_upload(&hold));
    let exit = TestExit::default();
    request_path(&mut app, &case.map);
    frame(&mut app, &exit);
    hold.wait_reached("load A's upload");
    let a = active_generation(&app).unwrap();
    let threads = app.load_pool.built().unwrap().threads_started();

    request_path(&mut app, Path::new("content/dev/maps/b-never-loads.prl"));
    frame(&mut app, &exit);
    let b = active_generation(&app).unwrap();
    assert_ne!(a, b);
    assert!(
        !app.level_loads.is_staged(),
        "B waits behind A's retirement"
    );
    request_path(&mut app, &case.map);
    frame(&mut app, &exit);
    let c = active_generation(&app).unwrap();
    assert_ne!(b, c);
    assert_eq!(
        app.stage_retirements.retiring_loads(),
        [a],
        "B left nothing to retire"
    );
    assert!(
        !app.level_loads.is_staged(),
        "C waits behind A's retirement"
    );

    hold.release();
    run_to_commit(&mut app, &exit, "load C");
    assert_eq!(dispatched(&app), [a, c], "B ran no stage");
    assert_eq!(
        app.load_pool.built().unwrap().threads_started(),
        threads,
        "the pool is built once; later loads start no thread"
    );
    retire_all(&mut app);
}

// Cancellation 4, P1: a request already queued when its frame finds the
// bundle ready cancels the load (the drain runs before the Loading step).
#[test]
fn queued_request_cancels_ready_bundle() {
    let test = "queued_request_cancels_ready_bundle";
    let case = CommitCase::textured();
    let other = CommitCase::streamed_sh();
    let Some(mut app) = gpu_app(test, &case) else {
        return;
    };
    use_case(&mut app, &case, |_| {});
    let exit = TestExit::default();
    request_path(&mut app, &case.map);
    frame(&mut app, &exit);
    let first = active_generation(&app).unwrap();
    let deadline = Instant::now() + APP_LOAD_TIMEOUT;
    while !stages_ready(&app) {
        assert!(Instant::now() < deadline, "the first load never got ready");
        std::thread::sleep(Duration::from_millis(1));
    }
    use_case(&mut app, &other, |_| {});
    request_path(&mut app, &other.map);
    frame(&mut app, &exit);
    assert!(app.level.is_none(), "the ready bundle never committed");
    assert_eq!(app.stage_retirements.retiring_loads(), [first]);
    run_to_commit(&mut app, &exit, "the queued load");
    assert_eq!(installed_path(&app).as_deref(), Some(other.map.as_path()));
    retire_all(&mut app);
}

// Cancellation 4, P2: a relevel the commit frame's own transport poll
// receives is handled on the next frame, after the commit, through the
// ordinary unload.
#[test]
fn same_frame_transport_relevel_handled_after_commit() {
    let test = "same_frame_transport_relevel_handled_after_commit";
    let case = CommitCase::textured();
    let other = CommitCase::streamed_sh();
    let Some(mut app) = gpu_app(test, &case) else {
        return;
    };
    script_ctx(&app)
        .data_registry
        .borrow_mut()
        .replace_maps(vec![
            catalog_map("first", case.map.to_str().unwrap(), "First", &[]),
            catalog_map("relevel", other.map.to_str().unwrap(), "Relevel", &[]),
        ]);
    use_case(&mut app, &case, |_| {});
    let exit = TestExit::default();
    app.enqueue_level_request(LevelRequest::Load(LevelSource::Catalog("first".into())));
    frame(&mut app, &exit);
    let deadline = Instant::now() + APP_LOAD_TIMEOUT;
    while !stages_ready(&app) {
        assert!(Instant::now() < deadline, "the load never got ready");
        std::thread::sleep(Duration::from_millis(1));
    }
    app.level_loads.test.on_transport_poll = Some(Box::new(|app| {
        app.follow_relevel_catalog("relevel".to_string(), 1);
    }));
    assert_eq!(frame(&mut app, &exit), Some(LoadingFrame::Ended));
    assert_eq!(app.boot_state, BootState::Settling, "the load committed");
    assert_eq!(
        app.active_level_source,
        Some(LevelSource::Catalog("first".into()))
    );
    assert_eq!(
        app.level_requests.front(),
        Some(&LevelRequest::Load(LevelSource::Catalog("relevel".into()))),
        "the relevel waits for the next frame's drain"
    );
    // Next frame: the ordinary unload, then the relevel's load, dispatched
    // with no level installed. (A fixture this small may commit on that
    // same frame, so the level itself is not read here.)
    use_case(&mut app, &other, |_| {});
    frame(&mut app, &exit);
    assert_eq!(
        app.level_loads.test.dispatched.as_slice(),
        [(1, false), (2, false)],
        "the drain unloaded the committed level before the relevel dispatched"
    );
    run_to_commit(&mut app, &exit, "the relevel");
    assert_eq!(
        app.active_level_source,
        Some(LevelSource::Catalog("relevel".into()))
    );
    retire_all(&mut app);
}

// Cancellation 5, P3: an unload request queued behind an in-flight load
// cancels it; the game enters Frontend with no backdrop (none was queued),
// the loading screen ends, and nothing commits.
#[test]
fn unload_queued_behind_load_cancels_it() {
    let test = "unload_queued_behind_load_cancels_it";
    let case = CommitCase::textured();
    let Some(mut app) = gpu_app(test, &case) else {
        return;
    };
    register_loading_tree(&mut app);
    let hold = Hold::default();
    use_case(&mut app, &case, hold_upload(&hold));
    let exit = TestExit::default();
    request_path(&mut app, &case.map);
    app.enqueue_level_request(LevelRequest::Unload);
    // The load dispatches first; the unload behind it drains next frame.
    frame(&mut app, &exit);
    assert!(app.level_loads.is_staged());
    hold.wait_reached("the upload");
    frame(&mut app, &exit);
    assert_eq!(app.boot_state, BootState::Frontend);
    assert!(!app.level_load_in_flight());
    assert!(app.level_load.is_none());
    assert!(app.level_requests.is_empty(), "no backdrop was queued");
    assert!(
        app.loading_screen_progress().is_none(),
        "the loading screen ended"
    );
    hold.release();
    retire_all(&mut app);
    assert!(app.level.is_none(), "nothing committed");
    assert!(
        app.renderer
            .as_ref()
            .unwrap()
            .resident_level_sets()
            .is_empty(),
        "the cancelled load's set never stayed resident"
    );
}

// Cancellation 5, P24: a load and a return to frontend on one tick, with a
// backdrop declared: the load never starts, and the backdrop installs.
#[test]
fn load_then_return_to_frontend_installs_backdrop() {
    let test = "load_then_return_to_frontend_installs_backdrop";
    let case = CommitCase::textured();
    let Some(mut app) = gpu_app(test, &case) else {
        return;
    };
    script_ctx(&app)
        .data_registry
        .borrow_mut()
        .replace_maps(vec![
            catalog_map("x", "maps/x-never-loads.prl", "X", &[]),
            catalog_map("backdrop", case.map.to_str().unwrap(), "Backdrop", &[]),
        ]);
    {
        let session = app.session.as_mut().unwrap();
        session.modal_stack.registry_mut().register(
            postretro_ui::demo::FRONTEND_MENU_NAME,
            postretro_ui::demo::build_frontend_menu_descriptor(),
            postretro_ui::modal_stack::ScopeTier::Engine,
            false,
        );
        session.frontend = Some(postretro_scripting_core::runtime::Frontend {
            menu_tree: postretro_ui::demo::FRONTEND_MENU_NAME.to_string(),
            background_level: Some("backdrop".to_string()),
            camera: postretro_scripting_core::runtime::MenuCamera {
                position: [0.0, 0.0, 0.0],
                yaw: 0.0,
                pitch: 0.0,
            },
        });
    }
    use_case(&mut app, &case, |_| {});
    let exit = TestExit::default();
    app.enqueue_level_request(LevelRequest::Load(LevelSource::Catalog("x".into())));
    app.return_to_frontend();
    run_to_commit(&mut app, &exit, "the backdrop");
    assert_eq!(dispatched(&app).len(), 1, "X never dispatched");
    assert_eq!(
        app.active_level_source,
        Some(LevelSource::Catalog("backdrop".into()))
    );
    retire_all(&mut app);
}

const BOOT_REFUSAL: &str = "ignoring runtime lifecycle request while boot map load is in flight";

// Cancellation 6, P8: a runtime request during a CLI boot load is refused
// with the warning and cancels nothing, while the load is queued, parsing,
// preparing and uploading; the boot load then installs.
#[test]
fn runtime_request_during_boot_load_refused_per_stage() {
    let test = "runtime_request_during_boot_load_refused_per_stage";
    let case = CommitCase::textured_with_model();
    let Some((renderer, release)) = held_renderer(test) else {
        return;
    };
    let mut app = app_with(renderer, &case.content_root);
    let (parse, prep, upload) = (Hold::default(), Hold::default(), Hold::default());
    let (parse_hook, prep_hook, upload_hook) = (parse.clone(), prep.clone(), upload.clone());
    use_case(&mut app, &case, move |inputs| {
        let parse_hook = parse_hook.clone();
        inputs.hooks.after_hand_out = Some(Arc::new(move |stage, _| {
            if stage == HandOutStage::ShManifest {
                parse_hook.park();
            }
        }));
        let prep_hook = prep_hook.clone();
        inputs.hooks.before_model_parse = Some(Arc::new(move |_| prep_hook.park()));
        let upload_hook = upload_hook.clone();
        inputs.hooks.after_upload_part = Some(Arc::new(move |_| upload_hook.park()));
    });
    let exit = TestExit::default();
    let capture = LogCapture::start();
    let refuse_in = |app: &mut App, exit: &TestExit, stage: &str| {
        let generation = active_generation(app);
        app.enqueue_level_request(LevelRequest::Unload);
        request_path(app, Path::new("content/dev/maps/runtime.prl"));
        frame(app, exit);
        assert!(
            app.level_requests.is_empty(),
            "{stage}: refused, not queued"
        );
        assert_eq!(active_generation(app), generation, "{stage}: not cancelled");
        assert_eq!(app.boot_state, BootState::Loading, "{stage}");
        assert!(app.boot_load, "{stage}");
    };

    // Queued: a stage of an older generation still retiring holds it.
    let queued = block_dispatch(&mut app);
    app.start_boot_destination(BootDestination::BootMap(case.map.clone()));
    assert!(app.level_load_in_flight() && !app.level_loads.is_staged());
    refuse_in(&mut app, &exit, "queued");
    queued.release();
    let wait = |app: &mut App, exit: &TestExit, hold: &Hold, what: &str| {
        let deadline = Instant::now() + APP_LOAD_TIMEOUT;
        while !hold.reached() {
            assert!(Instant::now() < deadline, "never reached: {what}");
            frame(app, exit);
            std::thread::sleep(Duration::from_millis(1));
        }
    };
    wait(&mut app, &exit, &parse, "the boot parse");
    refuse_in(&mut app, &exit, "parse");
    parse.release();
    wait(&mut app, &exit, &prep, "the boot model parse");
    refuse_in(&mut app, &exit, "prep");
    prep.release();
    release.release();
    wait(&mut app, &exit, &upload, "the boot upload");
    refuse_in(&mut app, &exit, "upload");
    upload.release();
    run_to_commit(&mut app, &exit, "the boot load");
    assert_eq!(
        capture
            .records()
            .iter()
            .filter(|record| record.message.contains(BOOT_REFUSAL))
            .count(),
        8,
        "each refused request warns"
    );
    assert!(!exit.0.get());
    retire_all(&mut app);
}

// Cancellation 6, P8: a runtime request during a runtime load no longer
// waits for it to install: the drain on the next frame cancels it while its
// stage is still parked, and the new load installs.
#[test]
fn runtime_request_supersedes_runtime_load() {
    let test = "runtime_request_supersedes_runtime_load";
    let case = CommitCase::textured();
    let Some(mut app) = gpu_app(test, &case) else {
        return;
    };
    let hold = Hold::default();
    let parked = hold.clone();
    use_case(&mut app, &case, move |inputs| {
        let parked = parked.clone();
        inputs.hooks.after_hand_out = Some(Arc::new(move |stage, _| {
            if stage == HandOutStage::Spatial {
                parked.park();
            }
        }));
    });
    let exit = TestExit::default();
    request_path(&mut app, &case.map);
    frame(&mut app, &exit);
    hold.wait_reached("the parse");
    let first = active_generation(&app).unwrap();
    request_path(&mut app, &case.map);
    frame(&mut app, &exit);
    assert!(hold.reached() && !app.stage_retirements.is_empty());
    assert_ne!(active_generation(&app), Some(first), "superseded at once");
    assert_eq!(app.stage_retirements.retiring_loads(), [first]);
    hold.release();
    run_to_commit(&mut app, &exit, "the superseding load");
    retire_all(&mut app);
}

// Cancellation 7, P12: no stage work while a request only waits in the
// queue: the first-launch hold and the splash's OS-preference wait drain
// nothing (no load starts, the pool is never built), and in the
// loading-screen order the old level unloads before the new load
// dispatches.
#[test]
fn queued_request_starts_no_stage_work() {
    use super::lifecycle_boot_state::boot_state_drains_level_requests;
    assert!(!boot_state_drains_level_requests(&BootState::Splash));
    assert!(!boot_state_drains_level_requests(
        &BootState::FirstLaunchHold
    ));
    assert!(!boot_state_drains_level_requests(&BootState::Booting));
    for state in [BootState::Splash, BootState::FirstLaunchHold] {
        let mut app = test_app();
        app.boot_state = state;
        request_path(&mut app, Path::new("content/dev/maps/queued.prl"));
        let exit = TestExit::default();
        for _ in 0..3 {
            assert_eq!(frame(&mut app, &exit), None);
        }
        assert_eq!(app.level_requests.len(), 1, "still queued");
        assert!(!app.level_load_in_flight());
        assert!(app.load_pool.built().is_none(), "no stage work ran");
    }

    let test = "queued_request_starts_no_stage_work";
    let case = CommitCase::textured();
    let Some(mut app) = gpu_app(test, &case) else {
        return;
    };
    use_case(&mut app, &case, |_| {});
    let exit = TestExit::default();
    request_path(&mut app, &case.map);
    run_to_commit(&mut app, &exit, "the first level");
    assert!(app.level.is_some());
    // A level change from Settling (installed): unload, then dispatch.
    request_path(&mut app, &case.map);
    frame(&mut app, &exit);
    assert_eq!(
        app.level_loads.test.dispatched,
        [(1, false), (2, false)],
        "each load dispatched with no level installed"
    );
    run_to_commit(&mut app, &exit, "the level change");
    retire_all(&mut app);
}

// Cancellation 8: a superseded load keeps its loading tree and UI clock,
// shows the new level's name and restarts the bar at 0; the old load's
// stages credit nothing to the new bar.
#[test]
fn superseded_load_keeps_loading_tree_and_restarts_bar() {
    let test = "superseded_load_keeps_loading_tree_and_restarts_bar";
    let case = CommitCase::textured();
    let Some(mut app) = gpu_app(test, &case) else {
        return;
    };
    register_loading_tree(&mut app);
    let hold = Hold::default();
    let parked = hold.clone();
    let first_load = Arc::new(AtomicBool::new(true));
    use_case(&mut app, &case, move |inputs| {
        if first_load.swap(false, Ordering::AcqRel) {
            let parked = parked.clone();
            inputs.hooks.after_hand_out = Some(Arc::new(move |stage, _| {
                if stage == HandOutStage::Spatial {
                    parked.park();
                }
            }));
        }
    });
    let exit = TestExit::default();
    request_path(&mut app, &case.map);
    frame(&mut app, &exit);
    hold.wait_reached("the first load's parse");
    frame(&mut app, &exit);
    let old_counter = app.loading_screen_progress().unwrap();
    let old_bar = progress_slot(&app);
    assert!(old_bar > 0.0, "the first load's parse moved the bar");
    let before = app.loading_screen_snapshot().unwrap();

    request_path(&mut app, &case.map);
    frame(&mut app, &exit);
    let new_counter = app.loading_screen_progress().unwrap();
    assert!(!Arc::ptr_eq(&old_counter, &new_counter), "a fresh counter");
    let name = postretro_scripting_core::store_bridge::read_store_slot(
        &app.session.as_ref().unwrap().scripting.script_ctx,
        "loading.levelName",
    )
    .unwrap();
    assert_eq!(
        name,
        postretro_entities::slot_table::SlotValue::String(
            case.map.file_stem().unwrap().to_string_lossy().into_owned()
        )
    );
    // The old load still credits its own counter as it winds down.
    hold.release();
    let deadline = Instant::now() + APP_LOAD_TIMEOUT;
    while !app.stage_retirements.is_empty() {
        assert!(Instant::now() < deadline);
        app.poll_stage_retirements();
        paint_cpu(&mut app);
        assert!(
            new_counter.fraction() == 0.0 || app.level_loads.is_staged(),
            "nothing reaches the new counter before it dispatches"
        );
        std::thread::sleep(Duration::from_millis(1));
    }
    let after = app.loading_screen_snapshot().unwrap();
    assert_eq!(after.trees[0].name, before.trees[0].name, "the same tree");
    assert!(after.time_seconds > before.time_seconds, "the clock ran on");
    assert!(
        progress_slot(&app) < old_bar,
        "the bar restarted below the superseded load's"
    );
    run_to_commit(&mut app, &exit, "the superseding load");
    retire_all(&mut app);
}

/// One row of the failure matrix.
struct FailureRow {
    label: &'static str,
    map: PathBuf,
    edit: Box<dyn Fn(&mut LevelPrepInputs)>,
    /// Provoke an uncaptured main-thread validation error on a Loading
    /// frame once the load dispatched.
    main_submit_error: bool,
}

// Failure 1: an injected error and an injected panic in each of parse, prep
// and upload, a real wgpu validation error on the upload thread, and one
// that first surfaces on the main thread (uncaptured, routed by the next
// frame) each end the load as one failure: one report, no registry
// mutation, no parity published, Frontend at runtime and a non-zero exit at
// boot, the process never aborting.
#[test]
fn injected_stage_failures_end_load_once() {
    let test = "injected_stage_failures_end_load_once";
    let case = CommitCase::textured();
    let Some(mut app) = gpu_app(test, &case) else {
        return;
    };
    let garbage = tempfile::NamedTempFile::new().unwrap();
    std::fs::write(garbage.path(), b"not a prl file at all").unwrap();
    let rows = vec![
        FailureRow {
            label: "parse error",
            map: garbage.path().to_path_buf(),
            edit: Box::new(|_| {}),
            main_submit_error: false,
        },
        FailureRow {
            label: "parse panic",
            map: case.map.clone(),
            edit: Box::new(|inputs| {
                inputs.hooks.after_hand_out = Some(Arc::new(|stage, _| {
                    if stage == HandOutStage::Spatial {
                        panic!("injected parse panic");
                    }
                }));
            }),
            main_submit_error: false,
        },
        FailureRow {
            label: "prep error",
            map: case.map.clone(),
            edit: Box::new(|inputs| {
                inputs.hooks.fail = vec![(PrepKind::Collision, "injected prep error")];
            }),
            main_submit_error: false,
        },
        FailureRow {
            label: "prep panic",
            map: case.map.clone(),
            edit: Box::new(|inputs| inputs.hooks.panic = Some(PrepKind::Fluid)),
            main_submit_error: false,
        },
        FailureRow {
            label: "upload error",
            map: case.map.clone(),
            edit: Box::new(|inputs| inputs.hooks.upload_textures_twice = true),
            main_submit_error: false,
        },
        FailureRow {
            label: "upload panic",
            map: case.map.clone(),
            edit: Box::new(|inputs| inputs.hooks.upload_fault = Some(UploadFault::Panic)),
            main_submit_error: false,
        },
        FailureRow {
            label: "wgpu validation on the upload thread",
            map: case.map.clone(),
            edit: Box::new(|inputs| {
                inputs.hooks.upload_fault = Some(UploadFault::WritePastBufferEnd);
            }),
            main_submit_error: false,
        },
        FailureRow {
            label: "wgpu validation at a main-thread submit",
            map: case.map.clone(),
            edit: Box::new(|_| {}),
            main_submit_error: true,
        },
    ];
    let exit = TestExit::default();
    let capture = LogCapture::start();
    for (index, row) in rows.into_iter().enumerate() {
        let boot = index == 0 || row.main_submit_error;
        capture.clear();
        let prm_root = case.prm_root.clone();
        let edit = row.edit;
        app.level_loads.test.edit_inputs = Some(Box::new(move |inputs| {
            inputs.prm_cache_root = prm_root.clone();
            edit(inputs);
        }));
        let entities = registry_fingerprint(&app);
        app.exit_result = Ok(());
        exit.0.set(false);
        if boot {
            app.start_boot_destination(BootDestination::BootMap(row.map.clone()));
        } else {
            request_path(&mut app, &row.map);
            frame(&mut app, &exit);
        }
        if row.main_submit_error {
            assert!(app.level_loads.is_staged(), "{}", row.label);
            app.renderer
                .as_ref()
                .unwrap()
                .provoke_uncaptured_validation_error();
        }
        run_until(&mut app, &exit, row.label, load_ended);
        let errors = loader_errors(&capture);
        assert_eq!(errors.len(), 1, "{}: one report: {errors:?}", row.label);
        assert!(
            errors[0].matches("level upload failed").count() <= 1,
            "{}: the stage is named once: {errors:?}",
            row.label
        );
        if row.main_submit_error {
            assert!(
                errors[0].contains("level load failed: uncaptured wgpu error"),
                "{}: an uncaptured error claims no stage: {errors:?}",
                row.label
            );
        }
        assert!(app.level.is_none(), "{}: nothing committed", row.label);
        assert!(
            app.published_level_identity.is_none(),
            "{}: no parity",
            row.label
        );
        assert_eq!(
            registry_fingerprint(&app),
            entities,
            "{}: no registry mutation",
            row.label
        );
        if boot {
            assert!(exit.0.get(), "{}: a boot load exits", row.label);
            assert!(app.exit_result.is_err(), "{}: non-zero", row.label);
            assert!(!app.boot_load);
            app.boot_state = BootState::Frontend;
        } else {
            assert!(!exit.0.get(), "{}: no exit", row.label);
            assert!(app.exit_result.is_ok(), "{}", row.label);
            assert_eq!(app.boot_state, BootState::Frontend, "{}", row.label);
        }
        // The load's stages retire; a later frame reports nothing more.
        retire_all(&mut app);
        for _ in 0..3 {
            frame(&mut app, &exit);
        }
        assert_eq!(loader_errors(&capture).len(), 1, "{}: still one", row.label);
        assert!(
            app.renderer
                .as_ref()
                .unwrap()
                .resident_level_sets()
                .is_empty(),
            "{}: no set stays resident",
            row.label
        );
    }
    // Outside a load, wgpu's default holds: an uncaptured error panics.
    let renderer = app.renderer.as_ref().unwrap();
    let panic = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        renderer.provoke_uncaptured_validation_error();
    }));
    assert!(
        panic.is_err(),
        "outside a load an uncaptured error is fatal"
    );
}

// Failure 2, P18: late parse validation rejecting after hand-outs went out,
// on a cross-section check (a legacy BSP-node section beside the cells) and
// on a cluster directory naming a cell past the cell table. Each through an
// App load at runtime: hand-outs 1–4 went out (a model parse and its upload
// ran on them), then the parse's verdict is the one load failure; every
// stage joins, no stage panicked on the unvalidated hand-outs, nothing
// commits, no registry mutation, no parity, the game is in Frontend, and no
// cache entry or set from the load remains.
#[test]
fn late_parse_validation_fails_load_after_hand_outs() {
    let test = "late_parse_validation_fails_load_after_hand_outs";
    let cross_section = crate::level_load::test_support::write_late_rejected_level();
    let cluster_directory = crate::level_load::test_support::write_late_cluster_directory_level();
    let case = CommitCase::textured_with_model();
    let Some(mut app) = gpu_app(test, &case) else {
        return;
    };
    let exit = TestExit::default();
    let capture = LogCapture::start();
    for (label, map, names) in [
        ("cross-section", cross_section.path(), "BspNodes"),
        (
            "cluster directory past the cell table",
            cluster_directory.path(),
            "runtime_cell_count",
        ),
    ] {
        capture.clear();
        let stages: Arc<std::sync::Mutex<Vec<HandOutStage>>> = Arc::default();
        let seen = Arc::clone(&stages);
        // The model placement rides hand-out 4, so a model parse and its
        // upload run before the verdict.
        use_case(&mut app, &case, move |inputs| {
            inputs.hooks.edit_world = None;
            let seen = Arc::clone(&seen);
            inputs.hooks.after_hand_out = Some(Arc::new(move |stage, _| {
                seen.lock().unwrap().push(stage);
            }));
        });
        let entities = registry_fingerprint(&app);
        request_path(&mut app, map);
        run_until(&mut app, &exit, label, load_ended);
        let errors = loader_errors(&capture);
        assert_eq!(errors.len(), 1, "{label}: one report: {errors:?}");
        assert!(
            errors[0].contains("level parse failed") && errors[0].contains(names),
            "{label}: the parse's verdict, naming {names}: {errors:?}"
        );
        assert_eq!(
            *stages.lock().unwrap(),
            [
                HandOutStage::ShManifest,
                HandOutStage::Geometry,
                HandOutStage::Spatial,
                HandOutStage::Entities
            ],
            "{label}: hand-outs 1–4 went out before the verdict"
        );
        assert_eq!(app.boot_state, BootState::Frontend, "{label}");
        assert!(app.level.is_none(), "{label}: nothing committed");
        assert!(app.published_level_identity.is_none(), "{label}: no parity");
        assert_eq!(
            registry_fingerprint(&app),
            entities,
            "{label}: no registry mutation"
        );
        assert!(!exit.0.get() && app.exit_result.is_ok(), "{label}: no exit");
        retire_all(&mut app);
        capture.assert_not_logged(log::Level::Error, "panicked");
        let renderer = app.renderer.as_ref().unwrap();
        assert!(renderer.resident_level_sets().is_empty(), "{label}");
        assert!(
            renderer.model_cache_keys().is_empty(),
            "{label}: no cache entry remains"
        );
    }
}

// Failure 4, through an App load: a streamed-SH init error (the residency
// plan refusing the manifest; no fixture's manifest makes it refuse, so the
// injected refusal stands in, as at prep) and an id-49 hint section that
// fails to decode (a hint naming a cluster the directory lacks, a real level
// edit) each fail the load before commit, through the one failure route the
// snapshot and controller rejections take: one `LevelLoadFailure` report,
// nothing committed, no registry mutation, no parity, Frontend at runtime,
// no panic and no exit.
#[test]
fn streamed_sh_level_data_failures_end_load_before_commit() {
    use postretro_level_format::cluster_directory::ClusterHintRecord;
    fn hint_names_missing_cluster(world: &mut postretro_level_loader::LevelWorld) {
        let directory = world.cluster_directory.as_mut().expect("streamed id 49");
        directory.cluster_hints.push(ClusterHintRecord {
            cluster_id: 99,
            flags: 0,
            priority: 0,
        });
    }
    let test = "streamed_sh_level_data_failures_end_load_before_commit";
    let case = CommitCase::streamed_sh();
    let Some(mut app) = gpu_app(test, &case) else {
        return;
    };
    let exit = TestExit::default();
    let capture = LogCapture::start();
    type Edit = Box<dyn Fn(&mut LevelPrepInputs)>;
    let rows: [(&str, &str, Edit); 2] = [
        (
            "streamed-SH init",
            "streamed-SH residency plan",
            Box::new(|inputs| {
                inputs.hooks.fail = vec![(PrepKind::ShResidency, "injected plan refusal")];
            }),
        ),
        (
            "id-49 hint decode",
            "cluster hint names an out-of-range cluster",
            Box::new(|inputs| inputs.hooks.edit_world = Some(hint_names_missing_cluster)),
        ),
    ];
    for (label, names, edit) in rows {
        capture.clear();
        use_case(&mut app, &case, move |inputs| edit(inputs));
        let entities = registry_fingerprint(&app);
        request_path(&mut app, &case.map);
        run_until(&mut app, &exit, label, load_ended);
        let errors = loader_errors(&capture);
        assert_eq!(errors.len(), 1, "{label}: one report: {errors:?}");
        assert!(
            errors[0].contains(names),
            "{label}: names {names}: {errors:?}"
        );
        assert!(
            !errors[0].contains("panicked"),
            "{label}: a check failed by panicking: {errors:?}"
        );
        assert_eq!(app.boot_state, BootState::Frontend, "{label}");
        assert!(app.level.is_none(), "{label}: nothing committed");
        assert!(app.published_level_identity.is_none(), "{label}: no parity");
        assert!(
            app.session.as_ref().unwrap().sh_streaming.is_none(),
            "{label}: no streaming session"
        );
        assert_eq!(
            registry_fingerprint(&app),
            entities,
            "{label}: no registry mutation"
        );
        assert!(!exit.0.get() && app.exit_result.is_ok(), "{label}: no exit");
        retire_all(&mut app);
        assert!(
            app.renderer
                .as_ref()
                .unwrap()
                .resident_level_sets()
                .is_empty(),
            "{label}: the load's set was released"
        );
    }
    // The same level unedited loads and commits.
    use_case(&mut app, &case, |_| {});
    request_path(&mut app, &case.map);
    run_to_commit(&mut app, &exit, "the unedited level");
    retire_all(&mut app);
}

// Failure 3, P10: two stages failing together, and the parse failing while
// prep runs, each report exactly one failure; every stage joins and nothing
// commits.
#[test]
fn concurrent_stage_failures_report_once() {
    let test = "concurrent_stage_failures_report_once";
    let case = CommitCase::textured();
    let late = crate::level_load::test_support::write_late_rejected_level();
    let Some(mut app) = gpu_app(test, &case) else {
        return;
    };
    let exit = TestExit::default();
    let capture = LogCapture::start();
    for (label, map, together) in [
        ("two prep tasks together", case.map.clone(), true),
        (
            "the parse failing while prep runs",
            late.path().to_path_buf(),
            false,
        ),
    ] {
        capture.clear();
        use_case(&mut app, &case, move |inputs| {
            inputs.hooks.edit_world = None;
            inputs.hooks.edit_hand_out = None;
            if together {
                inputs.hooks.fail = vec![
                    (PrepKind::Collision, "collision refused"),
                    (PrepKind::StreamingData, "streaming refused"),
                ];
                inputs.hooks.fail_together = Some(Arc::new(std::sync::atomic::AtomicUsize::new(0)));
            } else {
                inputs.hooks.fail = vec![(PrepKind::RangeCheck, "range refused")];
                inputs.hooks.upload_textures_twice = true;
            }
        });
        request_path(&mut app, &map);
        run_until(&mut app, &exit, label, load_ended);
        retire_all(&mut app);
        for _ in 0..3 {
            frame(&mut app, &exit);
        }
        let errors = loader_errors(&capture);
        assert_eq!(errors.len(), 1, "{label}: {errors:?}");
        assert!(app.level.is_none(), "{label}");
        assert_eq!(app.boot_state, BootState::Frontend, "{label}");
    }
}

// Settings 1, P21: tiers are read at dispatch, not when the request is
// queued: a tier changed on the frame a request is queued reaches its load.
#[test]
fn tiers_snapshot_at_dispatch_not_enqueue() {
    use crate::options::ShadowQuality;
    let test = "tiers_snapshot_at_dispatch_not_enqueue";
    let case = CommitCase::textured();
    let Some(mut app) = gpu_app(test, &case) else {
        return;
    };
    // The upload parks, so the load is still staged when its tiers are read.
    let upload = Hold::default();
    use_case(&mut app, &case, hold_upload(&upload));
    let exit = TestExit::default();
    let initial = app.session.as_ref().unwrap().player_options.shadow_quality;
    let changed = if initial == ShadowQuality::Low {
        ShadowQuality::High
    } else {
        ShadowQuality::Low
    };
    // A Frontend frame queues the request, then its options step applies a
    // tier change.
    request_path(&mut app, &case.map);
    app.session.as_mut().unwrap().player_options.shadow_quality = changed;
    // The next frame's drain dispatches.
    frame(&mut app, &exit);
    let tiers = app.dispatched_level_tiers().expect("dispatched");
    assert_eq!(tiers.shadow_quality, changed, "the change reached the load");
    upload.release();
    run_to_commit(&mut app, &exit, "the load");
    retire_all(&mut app);
}

/// A boundary's label and each phase's own fraction there.
type PhaseSample = (String, [f32; 4]);

/// Each phase's own fraction, in [`LoadPhase::ALL`] order.
fn phase_fractions(counter: &LevelLoadProgress) -> [f32; 4] {
    LoadPhase::ALL.map(|phase| counter.phase(phase).fraction())
}

/// Index of `phase` in [`phase_fractions`].
fn phase_index(phase: LoadPhase) -> usize {
    LoadPhase::ALL
        .iter()
        .position(|candidate| *candidate == phase)
        .unwrap()
}

// Progress 1: the whole load's counter never decreases; it rises during
// the parse, prep, upload and Settling, stays below 1.0 until reveal and is
// exactly 1.0 at reveal, a timed-out reveal included. A level with no
// textures or models (an empty texture prep) reaches 1.0 too.
//
// Sampled at each stage boundary (each hand-out, a prep task finished while
// prep still runs, each upload part, the commit, Settling, the release and
// the reveal), every phase credits part of its own share before it ends: the
// bar rises inside each phase, not only at its end. On the streamed-manifest
// map the bar moved at hand-out 1, which the parse sends before its first
// section read (the level loader's
// `staged_parse_progress_moves_before_the_first_section_read`). The
// streamed-SH case is the near-empty level: one cluster, no textures, models
// or sounds, so its texture, sound and model prep kinds are empty; it reaches
// 1.0 at a timed-out reveal (P15). The level loader's
// `near_empty_level_and_empty_phases_reach_one` covers a prep set with no
// kinds at all, which no App load has.
#[test]
fn load_progress_spans_every_phase() {
    let test = "load_progress_spans_every_phase";
    let textured = CommitCase::textured();
    let bare = CommitCase::streamed_sh();
    let Some(mut app) = gpu_app(test, &textured) else {
        return;
    };
    register_loading_tree(&mut app);
    for (case, timeout) in [(&textured, false), (&bare, true)] {
        let samples: Arc<std::sync::Mutex<Vec<(&'static str, f32)>>> = Arc::default();
        // Each phase's fraction at each boundary, beside the whole bar.
        let phases: Arc<std::sync::Mutex<Vec<PhaseSample>>> = Arc::default();
        let first_hand_out: Arc<std::sync::Mutex<Option<HandOutStage>>> = Arc::default();
        let counter: Arc<std::sync::Mutex<Option<Arc<LevelLoadProgress>>>> = Arc::default();
        let (hand_out_samples, part_samples) = (Arc::clone(&samples), Arc::clone(&samples));
        let (hand_out_phases, part_phases) = (Arc::clone(&phases), Arc::clone(&phases));
        let hand_out_first = Arc::clone(&first_hand_out);
        let (hand_out_counter, part_counter) = (Arc::clone(&counter), Arc::clone(&counter));
        use_case(&mut app, case, move |inputs| {
            let samples = Arc::clone(&hand_out_samples);
            let phases = Arc::clone(&hand_out_phases);
            let first = Arc::clone(&hand_out_first);
            let counter = Arc::clone(&hand_out_counter);
            inputs.hooks.after_hand_out = Some(Arc::new(move |stage, probe| {
                if let Some(counter) = counter.lock().unwrap().as_ref() {
                    let label = if stage == HandOutStage::Final {
                        "parse end"
                    } else {
                        "hand-out"
                    };
                    samples.lock().unwrap().push((label, counter.fraction()));
                    first.lock().unwrap().get_or_insert(stage);
                    phases
                        .lock()
                        .unwrap()
                        .push((format!("{stage:?}"), phase_fractions(counter)));
                    if stage == HandOutStage::Final {
                        // Prep cannot finish while this hook holds the
                        // parse (assembly waits for the verdict), so once
                        // one of its tasks finished, prep is mid-way.
                        assert!(
                            probe.wait_until(|state| !state.finished.is_empty()),
                            "a prep task finished before the parse returned"
                        );
                        samples
                            .lock()
                            .unwrap()
                            .push(("prep task", counter.fraction()));
                        phases
                            .lock()
                            .unwrap()
                            .push(("prep task".to_string(), phase_fractions(counter)));
                    }
                }
            }));
            let samples = Arc::clone(&part_samples);
            let phases = Arc::clone(&part_phases);
            let counter = Arc::clone(&part_counter);
            inputs.hooks.after_upload_part = Some(Arc::new(move |part| {
                if let Some(counter) = counter.lock().unwrap().as_ref() {
                    samples
                        .lock()
                        .unwrap()
                        .push(("upload part", counter.fraction()));
                    phases
                        .lock()
                        .unwrap()
                        .push((format!("upload {part:?}"), phase_fractions(counter)));
                }
            }));
        });
        let exit = TestExit::default();
        // Queue the load first, so the hooks hold its counter before the
        // parse's first hand-out.
        let queued = block_dispatch(&mut app);
        request_path(&mut app, &case.map);
        frame(&mut app, &exit);
        assert!(!app.level_loads.is_staged());
        let load_counter = app.loading_screen_progress().unwrap();
        assert_eq!(load_counter.fraction(), 0.0);
        *counter.lock().unwrap() = Some(Arc::clone(&load_counter));
        queued.release();
        run_to_commit(&mut app, &exit, case.label);
        let at_commit = load_counter.fraction();
        samples.lock().unwrap().push(("commit", at_commit));
        let settling_floor = (1.0 - postretro_level_loader::LOAD_PHASE_WEIGHT_SETTLING) as f32;
        assert!(
            (at_commit - settling_floor).abs() < 1e-4,
            "{}: parse, prep and upload are whole at commit: {at_commit}",
            case.label
        );
        // Settling credits its shortfall, then the release fills the bar.
        let started = Instant::now();
        let unsettled = |count| SettleReport {
            sh: ResourceSettle::Unsettled(count),
            lightmap: ResourceSettle::NotStreamed,
        };
        app.advance_settle(unsettled(4), started);
        app.advance_settle(unsettled(1), started);
        samples
            .lock()
            .unwrap()
            .push(("settling", load_counter.fraction()));
        phases
            .lock()
            .unwrap()
            .push(("settling".to_string(), phase_fractions(&load_counter)));
        assert!(
            load_counter.fraction() < 1.0,
            "{}: below 1.0 until reveal",
            case.label
        );
        if timeout {
            app.advance_settle(
                unsettled(1),
                started + super::super::settling::SETTLE_TIMEOUT,
            );
        } else {
            app.advance_settle(
                SettleReport {
                    sh: ResourceSettle::Settled,
                    lightmap: ResourceSettle::Settled,
                },
                started,
            );
        }
        assert_eq!(
            load_counter.fraction(),
            1.0,
            "{}: 1.0 at reveal",
            case.label
        );
        assert_eq!(
            progress_slot(&app),
            1.0,
            "{}: the bar paints 1.0",
            case.label
        );
        // The reveal itself, on the next redraw: the load's counter reads
        // exactly 1.0 there, every phase finished, timed out or not.
        assert!(app.reveal_if_released(), "{}: revealed", case.label);
        assert_eq!(app.boot_state, BootState::Running, "{}", case.label);
        assert_eq!(
            load_counter.fraction(),
            1.0,
            "{}: exactly 1.0 at reveal",
            case.label
        );
        assert!(load_counter.is_finished(), "{}", case.label);
        if timeout {
            assert!(
                app.level.as_ref().unwrap().texture_names.is_empty(),
                "the near-empty level has no textures"
            );
        }

        let samples = samples.lock().unwrap().clone();
        assert!(
            samples.iter().all(|(_, value)| *value < 1.0),
            "{}: below 1.0 at every boundary before the release: {samples:?}",
            case.label
        );
        let phases = phases.lock().unwrap().clone();
        let mid_phase = |phase: LoadPhase, at: &str| {
            let fraction = phases
                .iter()
                .find(|(label, _)| label.as_str() == at)
                .unwrap_or_else(|| panic!("{}: no {at} sample: {phases:?}", case.label))
                .1[phase_index(phase)];
            assert!(
                fraction > 0.0 && fraction < 1.0,
                "{}: {phase:?} credits part of its share inside the phase, at {at}: {fraction} ({phases:?})",
                case.label
            );
        };
        // Hand-out 1, before the first section read: the parse has
        // credited its table read and not finished.
        let first = first_hand_out.lock().unwrap().expect("a hand-out arrived");
        assert_eq!(first, HandOutStage::ShManifest, "{}", case.label);
        mid_phase(LoadPhase::Parse, &format!("{first:?}"));
        mid_phase(LoadPhase::Prep, "prep task");
        mid_phase(LoadPhase::Upload, "upload Finish");
        mid_phase(LoadPhase::Settling, "settling");
        if timeout {
            // The streamed-manifest map: the bar moved at hand-out 1, which
            // carries the SH manifest and goes out before any section read.
            assert!(
                case.label == "streamed-sh" && samples[0].0 == "hand-out" && samples[0].1 > 0.0,
                "progress moved before the first section read: {samples:?}"
            );
        }
        for pair in samples.windows(2) {
            assert!(
                pair[1].1 >= pair[0].1,
                "{}: progress fell: {samples:?}",
                case.label
            );
        }
        let first = samples.first().unwrap().1;
        let parse_end = samples
            .iter()
            .find(|(label, _)| *label == "parse end")
            .expect("the parse ended")
            .1;
        assert!(
            first > 0.0,
            "{}: moved by the first hand-out: {samples:?}",
            case.label
        );
        assert!(parse_end > first, "{}: rose during the parse", case.label);
        assert!(
            at_commit > parse_end,
            "{}: rose through prep and upload",
            case.label
        );
        let parts: Vec<f32> = samples
            .iter()
            .filter(|(label, _)| *label == "upload part")
            .map(|(_, value)| *value)
            .collect();
        assert!(
            parts.windows(2).any(|pair| pair[1] > pair[0]) || parts.len() < 2,
            "{}: rose during the upload: {samples:?}",
            case.label
        );
        let settling = samples
            .iter()
            .find(|(label, _)| *label == "settling")
            .unwrap()
            .1;
        assert!(settling > at_commit, "{}: rose during Settling", case.label);
        // Reveal ends the load; the next level change starts a new one.
        app.unload_level();
        retire_all(&mut app);
    }
}

/// A mod root whose start script declares one entity type, `classname`,
/// whose mesh is `model`: the manifest a staged hot reload builds.
fn reload_mod_root(classname: &str, model: &str) -> tempfile::TempDir {
    let mod_root = tempfile::tempdir().expect("a temporary mod root");
    std::fs::write(
        mod_root.path().join("start-script.js"),
        format!(
            "globalThis.__postretroModManifest = {{ name: 'Reload', id: 'reload', version: '1', \
             entities: [{{ canonicalName: '{classname}', components: {{ mesh: {{ model: '{model}' }} }} }}] }};"
        ),
    )
    .expect("the start script is written");
    mod_root
}

/// The real staged-manifest hot reload of `mod_root`: queue the build on the
/// runtime's staged lane, then run `frame` and the redraw's staged-manifest
/// poll (`poll_staged_manifest_results`, the commit the file watcher's
/// reload reaches) until the build commits, failing past
/// [`APP_LOAD_TIMEOUT`].
fn commit_staged_reload(app: &mut App, mod_root: &Path, mut frame: impl FnMut(&mut App)) {
    let generation = app
        .session
        .as_mut()
        .unwrap()
        .scripting
        .script_runtime
        .enqueue_staged_manifest_build(mod_root)
        .expect("the staged build is queued")
        .expect("debug builds stage manifest reloads");
    let deadline = Instant::now() + APP_LOAD_TIMEOUT;
    loop {
        frame(app);
        app.poll_staged_manifest_results();
        let committed = app
            .session
            .as_ref()
            .unwrap()
            .scripting
            .script_runtime
            .committed_staged_manifest_generation();
        if committed == Some(generation) {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "the staged reload never committed"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
}

/// `handle`'s cache key under `content_root`, read from its files now, at
/// `params`.
fn model_key_now(
    content_root: &Path,
    prm_root: &Path,
    handle: &str,
    params: crate::level_load::ModelInstallParams,
) -> crate::render::ModelContentKey {
    let sources = postretro_model::gltf_loader::read_model_sources(&content_root.join(handle))
        .expect("the model's files read");
    let sidecars =
        postretro_render_cpu::texture_prep::ModelSidecars::read(sources.material_keys(), prm_root);
    crate::level_load::model_content_key(&sources, &sidecars, params)
}

// Models 2 (P9), through the real staged reload: a hot reload committed on a
// Running frame that changes a descriptor's mesh block (deferred for the
// running level, which keeps its model), then a load in the loading-screen
// order: the load predicts the reloaded model, so its commit misses nothing,
// and the placement spawns with it.
#[test]
fn hot_reload_then_load_predicts_the_reloaded_model() {
    let test = "hot_reload_then_load_predicts_the_reloaded_model";
    const CLASSNAME: &str = "p9_reload_crate";
    const OLD: &str = super::lifecycle_commit_tests::MODEL;
    const NEW: &str = "models/ar_4/model.gltf";
    let level = crate::level_load::model_tests::write_level_with_entities(vec![
        crate::level_load::model_tests::record(CLASSNAME, &[], 0),
    ]);
    let case = CommitCase {
        label: "p9",
        map: level.path().to_path_buf(),
        prm_root: crate::startup::worker::derive_prm_root_dev_layout(&dev_content_root()),
        content_root: dev_content_root(),
        edit_world: None,
        edit_hand_out: None,
        models: &[],
        frame: false,
        _keep: Box::new(level),
    };
    let Some(mut app) = gpu_app(test, &case) else {
        return;
    };
    let mut crate_type = descriptor(CLASSNAME);
    crate_type.mesh = Some(postretro_entities::MeshDescriptor {
        model: OLD.to_string(),
        shadow_only: false,
        attachments: Default::default(),
        shadow_bias_scale: 1.0,
        animations: Default::default(),
        default_state: None,
        locomotion: None,
    });
    script_ctx(&app)
        .data_registry
        .borrow_mut()
        .replace_entity_types(vec![crate_type]);
    use_case(&mut app, &case, |_| {});
    let exit = TestExit::default();
    request_path(&mut app, &case.map);
    run_to_commit(&mut app, &exit, "the old level");
    app.advance_settle(
        SettleReport {
            sh: ResourceSettle::Settled,
            lightmap: ResourceSettle::Settled,
        },
        Instant::now(),
    );
    assert!(app.reveal_if_released());
    retire_all(&mut app);
    let old_set = app.renderer.as_ref().unwrap().active_level_set().unwrap();
    let old_entry = app
        .renderer
        .as_ref()
        .unwrap()
        .level_set_model_identity(old_set, OLD);
    assert!(old_entry.is_some(), "the old level draws the old model");

    let mod_root = reload_mod_root(CLASSNAME, NEW);
    commit_staged_reload(&mut app, mod_root.path(), |_| {});
    assert_eq!(app.boot_state, BootState::Running);
    assert!(
        app.session
            .as_ref()
            .unwrap()
            .scripting
            .script_runtime
            .deferred_mesh_descriptors()
            .is_some(),
        "the mesh change is deferred for the running level"
    );
    assert_eq!(
        app.renderer
            .as_ref()
            .unwrap()
            .level_set_model_identity(old_set, OLD),
        old_entry,
        "the old level keeps the old model until its unload"
    );

    let capture = LogCapture::start();
    request_path(&mut app, &case.map);
    run_to_commit(&mut app, &exit, "the load after the reload");
    capture.assert_not_logged(log::Level::Warn, "model prediction missed");
    let new_set = app.renderer.as_ref().unwrap().active_level_set().unwrap();
    assert!(
        app.renderer
            .as_ref()
            .unwrap()
            .level_set_model_identity(new_set, NEW)
            .is_some(),
        "the load predicted, built and draws the reloaded model"
    );
    let ctx = script_ctx(&app);
    let registry = ctx.registry.borrow();
    assert!(
        registry
            .iter_with_kind(postretro_entities::ComponentKind::Mesh)
            .any(|(entity, _)| registry
                .get_component::<postretro_entities::components::mesh::MeshComponent>(entity)
                .is_ok_and(|mesh| mesh.model == NEW)),
        "the placement spawned with the reloaded model"
    );
    drop(registry);
    retire_all(&mut app);
}

// Models 6, P7, at the App: suspend with a ready, uncommitted bundle drops
// it with the renderer and its cache; nothing from it reaches the resumed
// renderer, whose first load re-uploads every model it uses and installs.
#[test]
fn suspend_clears_model_cache_and_drops_ready_bundle() {
    let test = "suspend_clears_model_cache_and_drops_ready_bundle";
    let case = CommitCase::textured_with_model();
    let capture = LogCapture::start();
    let Some(mut app) = gpu_app(test, &case) else {
        return;
    };
    use_case(&mut app, &case, |_| {});
    let exit = TestExit::default();
    request_path(&mut app, &case.map);
    run_to_commit(&mut app, &exit, "the warm load");
    assert!(!app.renderer.as_ref().unwrap().model_cache_keys().is_empty());
    // The committed load's stages join first: dispatch waits for an empty
    // retirement list, and the wait below polls no frames.
    retire_all(&mut app);
    // A second load of the same map, ready and uncommitted: a cache hit.
    // Drained (which dispatches it) but never stepped: an all-hit load on
    // this small fixture can be ready on its dispatch frame, and a Loading
    // step would then commit it before the suspend.
    request_path(&mut app, &case.map);
    app.drain_level_requests();
    assert!(
        app.level_loads.staged().is_some(),
        "the second load dispatched on its drain"
    );
    let deadline = Instant::now() + APP_LOAD_TIMEOUT;
    while !stages_ready(&app) {
        // This wait polls no frame (a frame would commit the bundle), so a
        // load that failed would never report: name its state instead.
        assert!(
            Instant::now() < deadline,
            "the second load never got ready: state {:?}, load {:?}, retiring {:?}, prep {:?}, upload parts {:?}, loader errors {:?}",
            app.boot_state,
            app.level_loads,
            app.stage_retirements,
            app.level_loads
                .staged()
                .map(|stages| stages.probe().state()),
            app.level_loads
                .staged()
                .map(|stages| stages.probe().upload_parts_done()),
            loader_errors(&capture)
        );
        std::thread::sleep(Duration::from_millis(1));
    }
    app.suspend_platform();
    assert!(!app.level_load_in_flight(), "the ready bundle dropped");
    retire_all(&mut app);

    let resumed = crate::level_load::test_support::offscreen_renderer_or_skip(test).unwrap();
    assert!(resumed.model_cache_keys().is_empty());
    app.renderer = Some(resumed);
    app.boot_state = BootState::Frontend;
    request_path(&mut app, &case.map);
    run_to_commit(&mut app, &exit, "the first load after resume");
    let (_, models) = app.level_loads.test.last_upload_creations.unwrap();
    assert!(models.total() > 0, "every model re-uploaded: {models:?}");
    let renderer = app.renderer.as_ref().unwrap();
    assert!(renderer.active_level_set().is_some(), "the new load's set");
    assert!(
        renderer.resident_level_sets().is_empty(),
        "the dropped bundle's set never reached the resumed renderer"
    );
    assert!(!renderer.model_cache_keys().is_empty());
    retire_all(&mut app);
}

/// One mark of log line C: its name, milliseconds, and its `incl.` marks.
#[derive(Debug)]
struct LineCMark {
    name: String,
    ms: f64,
    inside: Vec<(String, f64)>,
}

fn line_c_entry(entry: &str) -> (String, f64) {
    let (name, value) = entry
        .split_once('=')
        .unwrap_or_else(|| panic!("a `name=…ms` entry: {entry}"));
    let ms = value
        .strip_suffix("ms")
        .and_then(|ms| ms.parse().ok())
        .unwrap_or_else(|| panic!("milliseconds: {entry}"));
    (name.to_string(), ms)
}

/// Parse a logged line C: `[Startup] a=1.0ms, b=2.0ms (incl. c=0.5ms), …`.
fn parse_line_c(line: &str) -> Vec<LineCMark> {
    let body = line.strip_prefix("[Startup] ").expect("a [Startup] line");
    let mut parts = Vec::new();
    let (mut depth, mut start) = (0usize, 0usize);
    for (at, c) in body.char_indices() {
        match c {
            '(' => depth += 1,
            ')' => depth -= 1,
            ',' if depth == 0 => {
                parts.push(&body[start..at]);
                start = at + 1;
            }
            _ => {}
        }
    }
    parts.push(&body[start..]);
    parts
        .into_iter()
        .map(|part| {
            let part = part.trim();
            let (head, inside) = match part.find(" (incl. ") {
                Some(at) => (
                    &part[..at],
                    part[at + " (incl. ".len()..]
                        .strip_suffix(')')
                        .expect("a closed incl. group")
                        .split(", ")
                        .map(line_c_entry)
                        .collect(),
                ),
                None => (part, Vec::new()),
            };
            let (name, ms) = line_c_entry(head);
            LineCMark { name, ms, inside }
        })
        .collect()
}

/// The one line C `capture` holds.
fn logged_line_c(capture: &LogCapture) -> String {
    let lines: Vec<String> = capture
        .records()
        .into_iter()
        .filter(|record| {
            record.level == log::Level::Info
                && record.message.starts_with("[Startup] load_dispatch=")
        })
        .map(|record| record.message)
        .collect();
    assert_eq!(lines.len(), 1, "one line C: {lines:?}");
    lines.into_iter().next().unwrap()
}

/// Rounding per printed mark: each prints to 0.1 ms.
const LINE_C_ROUNDING_MS: f64 = 0.05;

/// A group's `incl.` marks are disjoint and consecutive: they sum to it.
fn assert_group_sums(mark: &LineCMark) {
    let sum: f64 = mark.inside.iter().map(|(_, ms)| ms).sum();
    let rounding = LINE_C_ROUNDING_MS * (mark.inside.len() + 1) as f64;
    assert!(
        (sum - mark.ms).abs() <= rounding,
        "{}'s marks sum to it: {sum} vs {} ({:?})",
        mark.name,
        mark.ms,
        mark.inside
    );
}

/// The edges a wall clock around a load sees outside line C: the request
/// drain before `begin_level_load` resets the timings, and the reveal's tail
/// after its mark (loading screen end, splash clear, reveal publish).
const LINE_C_EDGE_TOLERANCE_MS: f64 = 50.0;

/// Line C's top-level marks before `first_level_frame` add up to the wall
/// time from the load's start to its reveal, within each mark's rounding and
/// [`LINE_C_EDGE_TOLERANCE_MS`]; `incl.` groups stay out of the sum.
fn assert_adds_up_to(marks: &[LineCMark], wall: Duration) {
    let through_reveal: Vec<&LineCMark> = marks
        .iter()
        .take_while(|mark| mark.name != "first_level_frame")
        .collect();
    assert_eq!(
        through_reveal.last().map(|mark| mark.name.as_str()),
        Some("reveal"),
        "{marks:?}"
    );
    let sum: f64 = through_reveal.iter().map(|mark| mark.ms).sum();
    let wall_ms = wall.as_secs_f64() * 1000.0;
    let rounding = LINE_C_ROUNDING_MS * through_reveal.len() as f64;
    assert!(
        sum <= wall_ms + rounding,
        "the marks fit inside the wall clock: {sum} ms vs {wall_ms} ms ({marks:?})"
    );
    assert!(
        wall_ms - sum <= LINE_C_EDGE_TOLERANCE_MS + rounding,
        "the marks add up to the wall clock: {sum} ms vs {wall_ms} ms ({marks:?})"
    );
}

/// Hold the committed level in Settling for a while, decide the release,
/// wait a redraw's gap, and reveal: `settle_hold` and `reveal` each span a
/// measurable interval of their own.
fn settle_and_reveal(app: &mut App) {
    std::thread::sleep(Duration::from_millis(20));
    app.advance_settle(
        SettleReport {
            sh: ResourceSettle::Settled,
            lightmap: ResourceSettle::Settled,
        },
        Instant::now(),
    );
    std::thread::sleep(Duration::from_millis(10));
    assert!(app.reveal_if_released(), "the release reveals");
    assert!(app.pending_level_log, "line C closes on the reveal frame");
}

// Marks: line C names each staged-load stage (dispatch; the stages inside
// `bundle_ready`: the parse, the off-main set build as `upload`, each prep
// kind), the progress-membership capture and every other stage of the
// commit frame inside `commit_frame`, the settle hold and the reveal, then
// the reveal frame. Read from the real line an App load logs, its
// top-level marks add up to the wall time from the load's start to its
// reveal, and the commit frame's marks add up to it.
#[test]
fn line_c_names_staged_marks_and_adds_up() {
    let test = "line_c_names_staged_marks_and_adds_up";
    let case = CommitCase::textured_with_model();
    let Some(mut app) = gpu_app(test, &case) else {
        return;
    };
    use_case(&mut app, &case, |_| {});
    let exit = TestExit::default();
    let capture = LogCapture::start();
    request_path(&mut app, &case.map);
    // Line C starts in the drain that begins the load.
    let wall_start = Instant::now();
    app.drain_level_requests();
    assert_eq!(app.boot_state, BootState::Loading);
    run_to_commit(&mut app, &exit, "the load");
    settle_and_reveal(&mut app);
    let wall = wall_start.elapsed();
    // The reveal frame's tail, as the frame loop runs it after its present.
    app.level_timings.log_level_line();
    let line = logged_line_c(&capture);
    eprintln!("{test}: {line}");
    let marks = parse_line_c(&line);

    let names: Vec<&str> = marks.iter().map(|mark| mark.name.as_str()).collect();
    assert_eq!(
        names,
        [
            "load_dispatch",
            "bundle_ready",
            "commit_frame",
            "settle_hold",
            "reveal",
            "first_level_frame"
        ],
        "{line}"
    );
    assert_adds_up_to(&marks, wall);
    let mark = |name: &str| marks.iter().find(|mark| mark.name == name).unwrap();
    assert!(mark("settle_hold").ms >= 19.0, "{line}");
    assert!(mark("reveal").ms >= 9.0, "{line}");

    // Inside `bundle_ready`: the stages that ran in it, overlapping, each
    // named once.
    let bundle_ready = mark("bundle_ready");
    let inside: Vec<&str> = bundle_ready
        .inside
        .iter()
        .map(|(name, _)| name.as_str())
        .collect();
    assert_eq!(inside[..2], ["prl_parse", "upload"], "{line}");
    for kind in [
        "texture_prep",
        "range_check",
        "assemble",
        "collision",
        "streaming_data",
        "lightmap_family",
        "fluid",
        "model_prediction",
        "model_parse",
    ] {
        assert!(inside.contains(&kind), "{kind} in bundle_ready: {line}");
    }
    let mut unique = inside.clone();
    unique.sort_unstable();
    unique.dedup();
    assert_eq!(unique.len(), inside.len(), "no repeated label: {line}");
    for (name, ms) in &bundle_ready.inside[..2] {
        assert!(
            *ms > 0.0 && *ms <= bundle_ready.ms + LINE_C_ROUNDING_MS * 2.0,
            "{name} ran inside bundle_ready: {line}"
        );
    }

    // Inside `commit_frame`: the commit's stages from the bundle taken to
    // Settling entered, the progress-membership capture among them, adding
    // up to it.
    let commit = mark("commit_frame");
    let stages: Vec<&str> = commit
        .inside
        .iter()
        .map(|(name, _)| name.as_str())
        .collect();
    assert_eq!(stages.first(), Some(&"commit_checks"), "{line}");
    assert_eq!(stages.last(), Some(&"enter_settling"), "{line}");
    let at = |name: &str| {
        stages
            .iter()
            .position(|stage| *stage == name)
            .unwrap_or_else(|| panic!("{name} in commit_frame: {line}"))
    };
    assert!(at("set_publish") < at("model_bindings"));
    assert!(at("model_bindings") < at("progress_membership"));
    assert!(at("progress_membership") < at("level_load_event"));
    assert!(at("level_load_event") < at("streaming_sessions"));
    assert_group_sums(commit);
    retire_all(&mut app);
}

// The retired worker and the main-thread install still run, from tests
// only, as the oracle: the worker parses on its own thread and the oracle
// installs its payload into Settling.
#[test]
fn retired_worker_and_oracle_install_still_run_from_tests() {
    let (level, prm_root) = crate::level_load::test_support::write_textured_level();
    let (tx, rx) = std::sync::mpsc::channel();
    let handle = super::super::worker::spawn_level_worker(
        level.path().to_path_buf(),
        level.path().parent().unwrap().to_path_buf(),
        None,
        Arc::new(postretro_level_loader::LoadProgress::new()),
        tx,
    );
    let mut payload = rx
        .recv_timeout(APP_LOAD_TIMEOUT)
        .expect("the worker delivers")
        .expect("the worker parses the level");
    handle.join().expect("the worker thread ends cleanly");
    assert!(payload.level.is_some());
    assert!(
        payload
            .timings
            .iter()
            .any(|(stage, _)| *stage == "prl_parse")
    );
    payload.prm_cache_root = prm_root.path().to_path_buf();
    let mut app = test_app();
    app.boot_state = BootState::Loading;
    let exit = TestExit::default();
    app.finish_level_payload(payload, &exit);
    assert_eq!(app.boot_state, BootState::Settling);
    assert!(app.level.is_some());
    assert!(!exit.0.get());
}

/// `source` with every `//` line comment removed.
fn strip_line_comments(source: &str) -> String {
    source
        .lines()
        .map(|line| line.split("//").next().unwrap_or(""))
        .collect::<Vec<_>>()
        .join("\n")
}

/// What a commit path must never call: a blocking receive, a thread join, a
/// device or queue wait, the oracle's pipeline wait, or a sleep.
const BLOCKING_WAITS: &[&str] = &[
    ".recv()",
    ".recv_timeout(",
    ".join()",
    "block_on(",
    "device.poll(",
    ".poll(wgpu",
    "PollType::Wait",
    "wait_for_oracle",
    "wait_install_pipelines",
    "Condvar",
    ".wait(",
    "thread::sleep",
    "park(",
];

fn blocking_waits_in(source: &str) -> Vec<&'static str> {
    let code = strip_line_comments(source);
    BLOCKING_WAITS
        .iter()
        .copied()
        .filter(|wait| code.contains(wait))
        .collect()
}

/// One function's text in `source`, signature to its closing brace at the
/// function's own indent.
fn function_source<'a>(source: &'a str, name: &str) -> &'a str {
    let start = source
        .find(&format!("fn {name}("))
        .or_else(|| source.find(&format!("fn {name}<")))
        .unwrap_or_else(|| panic!("`{name}` exists"));
    let line_start = source[..start].rfind('\n').map_or(0, |at| at + 1);
    let indent = &source[line_start..start];
    let indent: String = indent.chars().take_while(|c| *c == ' ').collect();
    let close = format!("\n{indent}}}\n");
    let end = source[start..]
        .find(&close)
        .map_or(source.len(), |at| start + at + close.len());
    &source[start..end]
}

// Off the main thread 3, the gate: the commit path and the Loading step
// contain no blocking receive, thread join, device wait or oracle pipeline
// wait. Read: the commit with every install stage it calls spliced in, the
// Loading step and its dispatch, the stage handle's main-thread half, prep's
// main-thread half, and the retirement polls.
#[test]
fn commit_path_has_no_blocking_wait() {
    let load = include_str!("lifecycle_load.rs");
    let stages = include_str!("../level_load/stages.rs");
    let prep = include_str!("../level_load/mod.rs");
    let retirements = include_str!("../app/stage_retirements.rs");
    let mut checked: Vec<(String, String)> = vec![(
        "commit_level_bundle (spliced)".to_string(),
        super::lifecycle_test_fixtures::commit_level_bundle_source(),
    )];
    for name in [
        "begin_level_load",
        "try_dispatch_level_load",
        "run_loading_frame",
        "step_loading_frame",
        "next_loading_step",
        "commit_ready_level_bundle",
        "cancel_active_level_load",
        "abandon_level_load_for_frontend",
        "cancel_level_load_for_suspend",
    ] {
        checked.push((name.to_string(), function_source(load, name).to_string()));
    }
    for name in [
        "begin_level_load_stages",
        "try_take",
        "poll_prep",
        "poll_prep_with_verdict",
        "keep_prep_outcome",
        "poll_upload",
        "fail",
        "bundle",
        "cancel",
        "stop",
        "try_finish",
    ] {
        checked.push((
            format!("stages::{name}"),
            function_source(stages, name).to_string(),
        ));
    }
    for name in [
        "try_take",
        "try_take_with_verdict",
        "cancel",
        "cancel_in_place",
    ] {
        checked.push((
            format!("prep::{name}"),
            function_source(prep, name).to_string(),
        ));
    }
    for name in ["poll", "poll_stage_retirements", "try_finish"] {
        checked.push((
            format!("retirements::{name}"),
            function_source(retirements, name).to_string(),
        ));
    }
    // What the commit calls outside the spliced install files: the commit
    // checks' streaming preparation and its install, the model sweep with
    // its miss fallback, sprite registration, the world's CPU install, the
    // loading-tree promotion, Settling's entry, and the renderer's receipt,
    // publish and cache commit.
    let finish = include_str!("lifecycle_finish.rs");
    let settling = include_str!("settling.rs");
    let residency = include_str!("lifecycle_spawn_residency.rs");
    let hooks = include_str!("../session/level_streaming/hooks.rs");
    let images = include_str!("loading_screen_images.rs");
    let level_build = include_str!("../../../renderer/src/render/level_build.rs");
    let publish = include_str!("../../../renderer/src/render/renderer_resources/publish.rs");
    let cache = include_str!("../../../renderer/src/render/model_cache.rs");
    for (name, source, function) in [
        (
            "enter_installed_level_settling",
            finish,
            "enter_installed_level_settling",
        ),
        ("enter_settling", settling, "enter_settling"),
        (
            "install_prepared_level_streaming_sessions",
            residency,
            "install_prepared_level_streaming_sessions",
        ),
        ("prepare_level_streaming", hooks, "prepare_level_streaming"),
        (
            "install_prepared_level_streaming",
            hooks,
            "install_prepared_level_streaming",
        ),
        (
            "promote_level_tree_images",
            images,
            "promote_level_tree_images",
        ),
        ("PendingLevelSet::try_take", level_build, "try_take"),
        ("Renderer::publish_level_set", publish, "publish_level_set"),
        (
            "Renderer::enter_model_cache_offers",
            cache,
            "enter_model_cache_offers",
        ),
        ("Renderer::sweep_model_cache", cache, "sweep_model_cache"),
        (
            "Renderer::release_unused_active_set_models",
            cache,
            "release_unused_active_set_models",
        ),
        (
            "Renderer::upload_missed_model_into_active_set",
            cache,
            "upload_missed_model_into_active_set",
        ),
    ] {
        checked.push((
            name.to_string(),
            function_source(source, function).to_string(),
        ));
    }
    for (name, source) in [
        (
            "lifecycle_world_cpu.rs",
            include_str!("lifecycle_world_cpu.rs"),
        ),
        (
            "lifecycle_model_sweep.rs",
            include_str!("lifecycle_model_sweep.rs"),
        ),
        (
            "lifecycle_sprite_collections.rs",
            include_str!("lifecycle_sprite_collections.rs"),
        ),
        (
            "level_load/model_cache.rs",
            include_str!("../level_load/model_cache.rs"),
        ),
        (
            "level_load/commit.rs",
            include_str!("../level_load/commit.rs"),
        ),
    ] {
        let production = source.split("\n#[cfg(test)]").next().unwrap_or("");
        assert!(production.len() > 200, "{name}: the production half reads");
        checked.push((format!("{name} (production)"), production.to_string()));
    }
    for (name, source) in &checked {
        let waits = blocking_waits_in(source);
        assert!(waits.is_empty(), "`{name}` blocks: {waits:?}");
    }
    // The gate sees what it forbids.
    assert_eq!(
        blocking_waits_in("fn f() { let x = rx.recv(); handle.join(); }"),
        [".recv()", ".join()"]
    );
    assert_eq!(
        blocking_waits_in("fn f() { slot.wait_for_oracle(); // .recv() in a comment\n}"),
        ["wait_for_oracle"]
    );
    assert!(blocking_waits_in("fn f() { pending.try_take(); }").is_empty());
}

/// Non-test Rust sources under the crates above the level loader, with the
/// path each came from.
fn sources_above_level_loader() -> Vec<(PathBuf, String)> {
    let crates = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
    let mut out = Vec::new();
    for krate in CRATES_ABOVE_LEVEL_LOADER {
        let mut stack = vec![crates.join(krate).join("src")];
        while let Some(dir) = stack.pop() {
            let Ok(entries) = std::fs::read_dir(&dir) else {
                continue;
            };
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    stack.push(path);
                } else if path.extension().is_some_and(|ext| ext == "rs") {
                    let name = path.file_name().unwrap().to_string_lossy();
                    if name.ends_with("_tests.rs")
                        || name == "tests.rs"
                        || name.ends_with("_test.rs")
                    {
                        continue;
                    }
                    let source = std::fs::read_to_string(&path).unwrap();
                    let production = source
                        .split("#[cfg(test)]\nmod tests")
                        .next()
                        .unwrap_or("")
                        .to_string();
                    out.push((path, production));
                }
            }
        }
    }
    out
}

/// Every workspace crate that depends on the level loader, directly or
/// through another (normal dependencies, not dev-dependencies), by directory.
/// `crates_above_level_loader_cover_every_dependent` keeps it complete.
const CRATES_ABOVE_LEVEL_LOADER: &[&str] = &[
    "ai",
    "level-compiler",
    "lighting",
    "netcode",
    "physics",
    "postretro",
    "render-cpu",
    "renderer",
    "sim",
    "tool",
    "visibility",
];

/// Types named `*Progress` that are not load-progress counters.
const NOT_LOAD_PROGRESS: &[&str] = &[
    // A weapon activation's per-tick state on a remote pawn.
    "RemoteActivationProgress",
    // The level compiler's bake-stage reporter: compile progress, offline.
    "StageProgress",
    // The level compiler's TUI worker: a detached bake's stage counts.
    "DetachProgress",
    // The assist accumulator's per-helper revive/assist progress: gameplay
    // state in the sim crate, unrelated to level loading.
    "HelperProgress",
];

/// Files whose `fn fraction(` is not a progress counter.
const NOT_PROGRESS_FRACTION: &[&str] = &[
    // An emitter's texel coverage fraction per key, in the level compiler.
    "embedded_emitter.rs",
];

/// The progress-counter definitions in `source`: a type named `*Progress`
/// (other than [`NOT_LOAD_PROGRESS`]), a `fraction` function, or a
/// parse-share constant.
fn progress_counters_in(source: &str) -> Vec<String> {
    let code = strip_line_comments(source);
    let mut found = Vec::new();
    for line in code.lines() {
        let trimmed = line.trim_start();
        let item = trimmed
            .trim_start_matches("pub(crate) ")
            .trim_start_matches("pub(super) ")
            .trim_start_matches("pub ");
        for keyword in ["struct ", "enum ", "type "] {
            if let Some(rest) = item.strip_prefix(keyword) {
                let name: String = rest
                    .chars()
                    .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
                    .collect();
                if name.ends_with("Progress") && !NOT_LOAD_PROGRESS.contains(&name.as_str()) {
                    found.push(format!("{keyword}{name}"));
                }
            }
        }
        if item.starts_with("fn fraction(") {
            found.push("fn fraction".to_string());
        }
        if item.contains("LOAD_PARSE_SHARE") {
            found.push("LOAD_PARSE_SHARE".to_string());
        }
    }
    found
}

// Progress 1, the gate: no crate above the level loader defines its own
// progress counter; the loading screen reads the level loader's
// phase-weighted `LevelLoadProgress`.
#[test]
fn no_progress_counter_above_level_loader() {
    let sources = sources_above_level_loader();
    assert!(sources.len() > 100, "the scan reads the crates");
    let defined: Vec<String> = sources
        .iter()
        .flat_map(|(path, source)| {
            let fraction_exempt = NOT_PROGRESS_FRACTION
                .iter()
                .any(|file| path.ends_with(file));
            progress_counters_in(source)
                .into_iter()
                .filter(move |found| !(fraction_exempt && found == "fn fraction"))
                .map(move |found| format!("{}: {found}", path.display()))
        })
        .collect();
    assert!(
        defined.is_empty(),
        "progress counters above the level loader: {defined:?}"
    );
    let screen = include_str!("loading_screen.rs");
    assert!(screen.contains("progress: Arc<LevelLoadProgress>,"));
    // The gate sees what it forbids.
    assert_eq!(
        progress_counters_in(
            "pub(crate) struct ParseProgress { done: u64 }\nconst LOAD_PARSE_SHARE: f32 = 0.85;\n    pub fn fraction(&self) -> f32 { 0.0 }\n// struct CommentProgress"
        ),
        ["struct ParseProgress", "LOAD_PARSE_SHARE", "fn fraction"]
    );
    assert!(progress_counters_in("struct ProgressTracker;").is_empty());
}

/// The `[package]` name of the crate in `dir`, and the workspace crates its
/// normal dependency tables name.
fn crate_manifest(dir: &Path) -> Option<(String, Vec<String>)> {
    let manifest = std::fs::read_to_string(dir.join("Cargo.toml")).ok()?;
    let mut name = None;
    let mut deps = Vec::new();
    let mut section = String::new();
    for line in manifest.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            section = line.to_string();
            continue;
        }
        if section == "[package]" && line.starts_with("name") {
            name = line.split('"').nth(1).map(str::to_string);
        }
        let normal = section == "[dependencies]"
            || (section.starts_with("[target.") && section.ends_with(".dependencies]"));
        if normal && line.starts_with("postretro") {
            let dep: String = line
                .chars()
                .take_while(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
                .collect();
            deps.push(dep);
        }
    }
    Some((name?, deps))
}

// The progress gate's crate list follows the dependency graph: every crate
// that depends on the level loader, directly or through another, is scanned.
#[test]
fn crates_above_level_loader_cover_every_dependent() {
    let crates = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
    let manifests: Vec<(String, String, Vec<String>)> = std::fs::read_dir(&crates)
        .unwrap()
        .flatten()
        .filter_map(|entry| {
            let dir = entry.file_name().to_string_lossy().into_owned();
            crate_manifest(&entry.path()).map(|(name, deps)| (dir, name, deps))
        })
        .collect();
    assert!(manifests.len() > 20, "the scan reads the workspace crates");
    let mut above: Vec<String> = vec!["postretro-level-loader".to_string()];
    loop {
        let before = above.len();
        for (_, name, deps) in &manifests {
            if !above.contains(name) && deps.iter().any(|dep| above.contains(dep)) {
                above.push(name.clone());
            }
        }
        if above.len() == before {
            break;
        }
    }
    let mut dirs: Vec<&str> = manifests
        .iter()
        .filter(|(_, name, _)| name != "postretro-level-loader" && above.contains(name))
        .map(|(dir, _, _)| dir.as_str())
        .collect();
    dirs.sort_unstable();
    assert_eq!(dirs, CRATES_ABOVE_LEVEL_LOADER);
}

// Off the main thread 7, the retired worker: no production path spawns the
// level worker. Its spawn compiles only for tests, and no source outside
// its own file names it.
#[test]
fn level_worker_spawns_only_from_tests() {
    let worker = include_str!("worker.rs");
    assert!(
        worker.contains("#[cfg(test)]\npub(crate) fn spawn_level_worker("),
        "the worker's spawn compiles for tests only"
    );
    let callers: Vec<String> = sources_above_level_loader()
        .into_iter()
        .filter(|(path, source)| {
            !path.ends_with("worker.rs")
                && strip_line_comments(source).contains("spawn_level_worker(")
        })
        .map(|(path, _)| path.display().to_string())
        .collect();
    assert!(
        callers.is_empty(),
        "production callers of the worker: {callers:?}"
    );
}

#[path = "lifecycle_beside_tests.rs"]
mod beside;
