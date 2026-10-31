//! The level-load state machine driven through the App: dispatch, the Loading
//! frame, the commit on the frame the bundle is ready, supersede, queued
//! unloads, suspend and retirement, one failure per load, the tiers read at
//! dispatch, the whole-load progress, and the grep gates on the commit path.
//! See: context/plans/in-progress/level-load-pipeline/index.md (Acceptance: Off the main thread 1, 3, 6, 7; Failure 1–3; Cancellation 1–8; Settings 1; Models 6; Progress 1) · research.md §9 (P1–P8, P10, P12, P18, P20–P22, P24)

use std::cell::Cell;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use postretro_level_loader::{HandOutStage, LevelLoadProgress};
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
    match Renderer::new_offscreen_holding_pipeline_build(64, 64) {
        Ok(held) => Some(held),
        Err(error) => {
            assert!(
                std::env::var_os("POSTRETRO_REQUIRE_GPU").is_none(),
                "{test}: POSTRETRO_REQUIRE_GPU is set but no adapter: {error:#}"
            );
            eprintln!("{test}: skipped, no adapter ({error:#})");
            None
        }
    }
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

// Off the main thread 1: with the upload stage held at its first creation
// (the upload thread's first job, the install-pipeline build, parked, so the
// load's parts queue behind it), Loading frames keep painting and the
// transport polls on each; across those frames the main thread creates no
// GPU object, and neither the parse nor a model parse runs on it. Released,
// the load reaches bundle-ready and commits.
#[test]
fn staged_load_upload_hold_keeps_loading_frames_painting() {
    let test = "staged_load_upload_hold_keeps_loading_frames_painting";
    let case = CommitCase::textured_with_model();
    let Some((renderer, release)) = held_renderer(test) else {
        return;
    };
    let mut app = app_with(renderer, &case.content_root);
    let main_thread = std::thread::current().id();
    let parse_thread = Arc::new(std::sync::Mutex::new(None));
    let model_thread = Arc::new(std::sync::Mutex::new(None));
    let (parse_seen, model_seen) = (Arc::clone(&parse_thread), Arc::clone(&model_thread));
    use_case(&mut app, &case, move |inputs| {
        let parse_seen = Arc::clone(&parse_seen);
        inputs.hooks.after_hand_out = Some(Arc::new(move |_, _| {
            *parse_seen.lock().unwrap() = Some(std::thread::current().id());
        }));
        let model_seen = Arc::clone(&model_seen);
        inputs.hooks.before_model_parse = Some(Arc::new(move |_| {
            *model_seen.lock().unwrap() = Some(std::thread::current().id());
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
    let mut font_system = postretro_ui::text::build_font_system();
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
    let model_ran_on = model_thread.lock().unwrap().expect("the model parse ran");
    assert_ne!(
        parse_ran_on, main_thread,
        "the parse ran off the main thread"
    );
    assert_ne!(
        model_ran_on, main_thread,
        "the model parse ran off the main thread"
    );

    release.release();
    run_to_commit(&mut app, &exit, "the released load");
    assert!(!exit.0.get());
    assert_eq!(installed_path(&app).as_deref(), Some(case.map.as_path()));
    retire_all(&mut app);
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
    let mut font_system = postretro_ui::text::build_font_system();
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
        app.follow_relevel_catalog("relevel".to_string());
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

// Failure 2, P18: late parse validation rejecting after hand-outs went out
// (the cross-section check): one load failure, every stage joins, nothing
// commits, and no cache entry from the load remains.
#[test]
fn late_parse_validation_fails_load_after_hand_outs() {
    let test = "late_parse_validation_fails_load_after_hand_outs";
    let level = crate::level_load::test_support::write_late_rejected_level();
    let case = CommitCase::textured_with_model();
    let Some(mut app) = gpu_app(test, &case) else {
        return;
    };
    // The model placement rides hand-out 4, so a model parse and its upload
    // run before the verdict.
    use_case(&mut app, &case, |inputs| inputs.hooks.edit_world = None);
    let exit = TestExit::default();
    let capture = LogCapture::start();
    request_path(&mut app, level.path());
    run_until(&mut app, &exit, "the late rejection", load_ended);
    let errors = loader_errors(&capture);
    assert_eq!(errors.len(), 1, "{errors:?}");
    assert!(errors[0].contains("level parse failed"), "{errors:?}");
    assert_eq!(app.boot_state, BootState::Frontend);
    assert!(app.level.is_none());
    retire_all(&mut app);
    let renderer = app.renderer.as_ref().unwrap();
    assert!(renderer.resident_level_sets().is_empty());
    assert!(
        renderer.model_cache_keys().is_empty(),
        "no cache entry remains"
    );
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

// Progress 1: the whole load's counter never decreases; it rises during
// the parse, prep, upload and Settling, stays below 1.0 until reveal and is
// exactly 1.0 at reveal, a timed-out reveal included. A level with no
// textures or models (an empty texture prep) reaches 1.0 too.
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
        let counter: Arc<std::sync::Mutex<Option<Arc<LevelLoadProgress>>>> = Arc::default();
        let (hand_out_samples, part_samples) = (Arc::clone(&samples), Arc::clone(&samples));
        let (hand_out_counter, part_counter) = (Arc::clone(&counter), Arc::clone(&counter));
        use_case(&mut app, case, move |inputs| {
            let samples = Arc::clone(&hand_out_samples);
            let counter = Arc::clone(&hand_out_counter);
            inputs.hooks.after_hand_out = Some(Arc::new(move |stage, _| {
                if let Some(counter) = counter.lock().unwrap().as_ref() {
                    let label = if stage == HandOutStage::Final {
                        "parse end"
                    } else {
                        "hand-out"
                    };
                    samples.lock().unwrap().push((label, counter.fraction()));
                }
            }));
            let samples = Arc::clone(&part_samples);
            let counter = Arc::clone(&part_counter);
            inputs.hooks.after_upload_part = Some(Arc::new(move |_| {
                if let Some(counter) = counter.lock().unwrap().as_ref() {
                    samples
                        .lock()
                        .unwrap()
                        .push(("upload part", counter.fraction()));
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

        let samples = samples.lock().unwrap().clone();
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
    for name in ["try_take", "cancel", "cancel_in_place"] {
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
    for krate in [
        "postretro",
        "renderer",
        "render-cpu",
        "sim",
        "physics",
        "visibility",
        "lighting",
    ] {
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

/// Types named `*Progress` that are not load-progress counters.
const NOT_LOAD_PROGRESS: &[&str] = &[
    // A weapon activation's per-tick state on a remote pawn.
    "RemoteActivationProgress",
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
            progress_counters_in(source)
                .into_iter()
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
