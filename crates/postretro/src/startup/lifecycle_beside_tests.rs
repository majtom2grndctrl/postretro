//! Loads beside a running level, driven through the App on Running frames:
//! the load runs to a held bundle while the running level draws, streams and
//! simulates unchanged; release-then-commit; cancel and failure leaving the
//! running level Running with its cache entries; tier, extent and descriptor
//! changes on Running frames; and a loading-screen request superseding it.
//! See: context/plans/in-progress/level-load-pipeline/index.md (Acceptance: Beside a running level 1–6, Per-load resources, Settings and extent 2, Models) · research.md §9 (P16, P26–P28, P30, P31)

use std::cell::Cell;

use postretro_entities::MeshDescriptor;
use postretro_entities::components::mesh::MeshComponent;
use postretro_visibility::{CameraCullVisibility, VisibilityPath, VisibleCells};

use super::*;
use crate::level_load::{BundleSet, LevelCommitReport, ModelCacheUse};
use crate::render::{ClearColor, LevelSetId, ModelContentKey, ShSampleRegionSets};
use crate::startup::lifecycle::lifecycle_beside::BesideLoadStep;

/// A small skinned model with hit zones, all buffers in `data:` URIs.
const ZONES: &str = super::super::lifecycle_commit_tests::MODEL;
/// A textured rifle: an external buffer and base-color PNGs.
const RIFLE: &str = "models/ar_4/model.gltf";
/// The classname a hot reload re-points at another model.
const RELOADED: &str = "beside_reload_crate";

/// What one Running frame did.
pub(super) struct RunningFrame {
    step: BesideLoadStep,
    /// The running level's frame, drawn through cell 0 as the fixture
    /// frames are hashed.
    hash: u64,
    /// SH clusters the frame's in-play drain installed.
    sh_installed: usize,
    /// Its streamed SH compose submitted.
    composed: bool,
}

/// One Running frame's load-relevant part, in `redraw`'s order: the
/// retirement poll (every boot state), the request drain, the beside poll
/// the Running path runs before its input, then the frame itself: its
/// clocks advance (the simulation's time), its extent commits, its level
/// streaming step runs in play (not settling), and it draws through the
/// offscreen capture with the drain batch that step prepared, whose outcome
/// the session applies as the play frame does. `None` once the frame is not
/// a Running frame. (The windowed `redraw` itself needs an event loop and a
/// surface; this is its offscreen stand-in.)
pub(super) fn running_frame(app: &mut App, exit: &TestExit) -> Option<RunningFrame> {
    app.poll_stage_retirements();
    if super::super::lifecycle_boot_state::boot_state_drains_level_requests(&app.boot_state) {
        app.drain_level_requests();
    }
    if app.boot_state != BootState::Running {
        return None;
    }
    let step = app.poll_beside_level_load(exit);
    assert_eq!(app.boot_state, BootState::Running, "the poll keeps Running");
    app.script_time += f64::from(DT);
    if let Some(session) = app.session.as_ref() {
        session.scripting.scheduler.begin_frame();
    }
    app.commit_render_extents();
    let cells = VisibleCells::Culled(vec![0]);
    let cpu = postretro_stage_timing::StageFrame::default();
    let monotonic_seconds = app.script_time;
    let (Some(world), Some(session), Some(renderer)) = (
        app.level.as_ref(),
        app.session.as_mut(),
        app.renderer.as_mut(),
    ) else {
        panic!("a Running frame has a level, a session and a renderer");
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
                monotonic_seconds,
                settling: false,
                cpu: &cpu,
            },
        )
        .expect("the in-play streaming step runs");
    let result = renderer
        .capture_frame_indirect(
            CameraCullVisibility {
                cells: &cells,
                path: VisibilityPath::PrlPortal { walk_reach: 1 },
            },
            &[],
            &[],
            &[],
            ShSampleRegionSets {
                visible_cells: &cells,
                fog_cells: &[],
                movers: &[],
            },
            Some(0),
            glam::Mat4::IDENTITY,
            glam::Vec3::new(0.0, 0.0, 1.0),
            &[],
            &[],
            1.0,
            ClearColor {
                r: 0.05,
                g: 0.1,
                b: 0.2,
                a: 1.0,
            },
            true,
            batch,
        )
        .expect("the frame drains");
    let sh_installed = result.outcome.accepted.len();
    let composed = result.compose_submitted;
    session
        .apply_sh_streaming_outcome(result.outcome, renderer)
        .expect("the outcome applies");
    session.mark_sh_streaming_compose_submitted(composed);
    let pixels = result.frame.expect("the frame records");
    let hash = pixels.iter().fold(0xcbf2_9ce4_8422_2325u64, |hash, byte| {
        (hash ^ u64::from(*byte)).wrapping_mul(0x0100_0000_01b3)
    });
    Some(RunningFrame {
        step,
        hash,
        sh_installed,
        composed,
    })
}

/// Running frames until `done`, each checked by `each`, failing past
/// [`APP_LOAD_TIMEOUT`]. Frames keep running while it waits, so nothing a
/// later frame decides is asserted early.
fn run_running_until(
    app: &mut App,
    exit: &TestExit,
    what: &str,
    done: impl Fn(&App) -> bool,
    mut each: impl FnMut(&App, &RunningFrame),
) {
    let deadline = Instant::now() + APP_LOAD_TIMEOUT;
    while !done(app) {
        assert!(
            Instant::now() < deadline,
            "{what}: never happened; state {:?}, load {:?}",
            app.boot_state,
            app.level_loads
        );
        let frame = running_frame(app, exit)
            .unwrap_or_else(|| panic!("{what}: left Running ({:?})", app.boot_state));
        each(app, &frame);
        std::thread::sleep(Duration::from_millis(1));
    }
}

/// Running frames until the retirement list is empty: retired stages join
/// through the poll at the top of each Running frame, never elsewhere.
fn retire_on_running_frames(app: &mut App, exit: &TestExit, each: impl FnMut(&App, &RunningFrame)) {
    run_running_until(
        app,
        exit,
        "retirement on Running frames",
        |app| app.stage_retirements.is_empty(),
        each,
    );
}

/// Reveal the committed, Settling level as a settled release does.
fn reveal(app: &mut App) {
    app.advance_settle(
        SettleReport {
            sh: ResourceSettle::Settled,
            lightmap: ResourceSettle::Settled,
        },
        Instant::now(),
    );
    assert!(app.reveal_if_released(), "the release reveals");
    assert_eq!(app.boot_state, BootState::Running);
}

/// An App running `case`: loaded in the loading-screen order, revealed,
/// its load's stages joined, and one Running frame drawn, so the frame's
/// extents are committed before a test reads the running level.
fn running_level(test: &str, case: &CommitCase) -> Option<(App, TestExit)> {
    let mut app = gpu_app(test, case)?;
    use_case(&mut app, case, |_| {});
    let exit = TestExit::default();
    request_path(&mut app, &case.map);
    run_to_commit(&mut app, &exit, "the running level");
    reveal(&mut app);
    retire_all(&mut app);
    running_frame(&mut app, &exit).expect("Running");
    Some((app, exit))
}

/// Running frames until the running level's frame stops changing (its
/// in-play streaming has installed what the pose draws): its hash.
fn settled_hash(app: &mut App, exit: &TestExit) -> u64 {
    let deadline = Instant::now() + APP_LOAD_TIMEOUT;
    let mut last = None;
    let mut stable = 0;
    loop {
        assert!(Instant::now() < deadline, "the running frame never settled");
        let frame = running_frame(app, exit).expect("Running");
        let settled = app.session.as_ref().unwrap().settle_report();
        let answered = |answer: ResourceSettle| {
            matches!(
                answer,
                ResourceSettle::Settled | ResourceSettle::NotStreamed
            )
        };
        if last == Some(frame.hash) && answered(settled.sh) && answered(settled.lightmap) {
            stable += 1;
        } else {
            stable = 0;
        }
        last = Some(frame.hash);
        if stable >= 4 {
            return frame.hash;
        }
        std::thread::sleep(Duration::from_millis(1));
    }
}

/// Start a load of `case` beside the running level, with `case`'s prep edits
/// and `extra`'s from here on.
fn begin_beside(
    app: &mut App,
    case: &CommitCase,
    extra: impl Fn(&mut LevelPrepInputs) + 'static,
) -> u64 {
    use_case(app, case, extra);
    assert!(
        app.begin_level_load_beside(case.map.clone(), case.content_root.clone()),
        "a Running level accepts a load beside it"
    );
    active_generation(app).expect("the load is active")
}

fn held(app: &App) -> bool {
    app.level_loads.held_bundle().is_some()
}

/// The held bundle's resident set.
fn held_set(app: &App) -> LevelSetId {
    match app.level_loads.held_bundle().expect("a held bundle").set {
        BundleSet::Resident(id) => id,
        BundleSet::PrepOnly(_) => panic!("a dispatched load builds a resident set"),
    }
}

fn active_set(app: &App) -> Option<LevelSetId> {
    app.renderer.as_ref().unwrap().active_level_set()
}

fn resident_sets(app: &App) -> Vec<LevelSetId> {
    app.renderer.as_ref().unwrap().resident_level_sets()
}

fn cache_keys(app: &App) -> Vec<ModelContentKey> {
    app.renderer.as_ref().unwrap().model_cache_keys()
}

fn set_summary(app: &App, id: LevelSetId) -> String {
    app.renderer
        .as_ref()
        .unwrap()
        .level_set_resource_summary(id)
        .expect("the set is known")
}

fn dev_prm_root() -> PathBuf {
    crate::startup::worker::derive_prm_root_dev_layout(&dev_content_root())
}

/// A streamed-lightmap level placing `records`, its models resolved in the
/// dev mod's content root.
fn entities_case(
    label: &'static str,
    records: Vec<postretro_level_format::map_entity::MapEntityRecord>,
) -> CommitCase {
    let level = crate::level_load::model_tests::write_level_with_entities(records);
    CommitCase {
        label,
        map: level.path().to_path_buf(),
        prm_root: dev_prm_root(),
        content_root: dev_content_root(),
        edit_world: None,
        edit_hand_out: None,
        models: &[],
        frame: true,
        _keep: Box::new(level),
    }
}

/// [`entities_case`] with one `prop_mesh` per handle.
fn props_case(label: &'static str, handles: &[&str]) -> CommitCase {
    entities_case(
        label,
        handles
            .iter()
            .enumerate()
            .map(|(index, handle)| {
                crate::level_load::model_tests::record("prop_mesh", &[("model", handle)], index)
            })
            .collect(),
    )
}

/// Every per-frame invariant of a running level beside a load: it stays
/// Running on its own set, which keeps drawing the same frame; no loading
/// screen starts and `loading.progress` holds its value.
struct RunningWatch {
    set: Option<LevelSetId>,
    hash: Option<u64>,
    progress: f32,
    frames: usize,
}

impl RunningWatch {
    fn new(app: &App, hash: Option<u64>) -> Self {
        Self {
            set: active_set(app),
            hash,
            progress: progress_slot(app),
            frames: 0,
        }
    }

    fn check(&mut self, app: &App, frame: &RunningFrame, what: &str) {
        self.frames += 1;
        assert_eq!(app.boot_state, BootState::Running, "{what}");
        assert_eq!(active_set(app), self.set, "{what}: the running set draws");
        if let Some(hash) = self.hash {
            assert_eq!(frame.hash, hash, "{what}: the running frame is unchanged");
        }
        assert!(
            app.loading_screen_progress().is_none(),
            "{what}: no loading screen"
        );
        assert_eq!(
            progress_slot(app),
            self.progress,
            "{what}: loading.progress does not move"
        );
    }
}

// Beside 1 (and progress isolation): a load runs to a held bundle beside a
// running level, on both streamed-SH fixtures (the running level and the
// load), while the running level's in-play SH drains install and compose on
// its Running frames; and beside a textured level whose frame settles first,
// whose every frame then hashes the same. On every Running frame the
// running level keeps its own set active, no loading screen starts and
// `loading.progress` stays put, though the load's own counter rises; the
// load's set stays resident and unpublished until the commit.
#[test]
fn beside_load_runs_to_bundle_ready_while_level_runs() {
    let test = "beside_load_runs_to_bundle_ready_while_level_runs";

    // Both streamed-SH: the load's upload parks until the running level's
    // drains have installed and composed, so they run meanwhile.
    let running = CommitCase::streamed_sh();
    let beside = CommitCase::streamed_sh();
    let Some((mut app, exit)) = running_level(test, &running) else {
        return;
    };
    let running_set = active_set(&app);
    let running_summary = app
        .renderer
        .as_ref()
        .unwrap()
        .active_level_resource_summary();
    let mut watch = RunningWatch::new(&app, None);
    let upload = Hold::default();
    let generation = begin_beside(&mut app, &beside, hold_upload(&upload));
    assert_eq!(app.boot_state, BootState::Running, "no unload, no Loading");
    assert!(app.loading_screen_progress().is_none(), "no loading screen");
    assert!(app.level_loads.is_staged(), "dispatched at once");
    assert!(
        !app.level_loads.active().unwrap().error_capture_armed,
        "beside a running level the uncaptured-error capture stays disarmed"
    );
    let (installed, composed) = (Cell::new(0), Cell::new(false));
    run_running_until(
        &mut app,
        &exit,
        "the running level's in-play drains",
        |_| upload.reached() && installed.get() > 0 && composed.get(),
        |app, frame| {
            watch.check(app, frame, "streamed: staged");
            installed.set(installed.get() + frame.sh_installed);
            composed.set(composed.get() || frame.composed);
            assert_eq!(frame.step, BesideLoadStep::Running);
        },
    );
    assert_eq!(active_generation(&app), Some(generation));
    upload.release();
    run_running_until(&mut app, &exit, "the held bundle", held, |app, frame| {
        watch.check(app, frame, "streamed: to bundle-ready");
    });
    let beside_set = held_set(&app);
    assert_ne!(Some(beside_set), running_set);
    assert_eq!(resident_sets(&app), [beside_set], "resident, unpublished");
    for _ in 0..3 {
        let frame = running_frame(&mut app, &exit).unwrap();
        watch.check(&app, &frame, "streamed: held");
        assert_eq!(frame.step, BesideLoadStep::Held, "held, not committed");
    }
    assert_eq!(
        app.renderer
            .as_ref()
            .unwrap()
            .active_level_resource_summary(),
        running_summary,
        "the running level's set is untouched"
    );
    let own = app.level_loads.active().unwrap().progress.fraction();
    assert!(own > 0.0, "the load credits its own counter: {own}");
    // It commits through the ordinary commit: Settling, its streaming
    // sessions built against its set.
    let report = app
        .commit_ready_beside_bundle(&exit)
        .expect("the held bundle commits");
    assert_eq!(report.set, Some(beside_set));
    assert_eq!(app.boot_state, BootState::Settling);
    assert_eq!(installed_path(&app).as_deref(), Some(beside.map.as_path()));
    retire_all(&mut app);
    // One renderer at a time: the suite shares one adapter's memory.
    drop(app);

    // A textured running level whose frame settles first, beside a load
    // with a model: every frame hashes the same.
    let running = CommitCase::textured();
    let beside = CommitCase::textured_with_model();
    let Some((mut app, exit)) = running_level(test, &running) else {
        return;
    };
    let hash = settled_hash(&mut app, &exit);
    let mut watch = RunningWatch::new(&app, Some(hash));
    begin_beside(&mut app, &beside, |_| {});
    run_running_until(&mut app, &exit, "the held bundle", held, |app, frame| {
        watch.check(app, frame, "textured: to bundle-ready");
    });
    for _ in 0..3 {
        let frame = running_frame(&mut app, &exit).unwrap();
        watch.check(&app, &frame, "textured: held");
    }
    assert!(watch.frames > 3, "frames ran while the load did");
    assert_eq!(resident_sets(&app), [held_set(&app)]);
    retire_on_running_frames(&mut app, &exit, |app, frame| {
        watch.check(app, frame, "textured: retiring");
    });
    assert!(app.cancel_beside_level_load());
    assert!(
        resident_sets(&app).is_empty(),
        "the cancel released its set"
    );
}

/// A set summary without its sprite collections, which the commit's sprite
/// registration (a named exception) adds.
fn without_smoke(summary: &str) -> String {
    summary
        .split(" smoke=")
        .next()
        .unwrap_or(summary)
        .to_string()
}

/// Commit the held bundle, the report and the set it published.
fn commit_held(app: &mut App, exit: &TestExit) -> (LevelCommitReport, LevelSetId) {
    let id = held_set(app);
    let report = app
        .commit_ready_beside_bundle(exit)
        .expect("the held bundle commits");
    assert_eq!(report.set, Some(id));
    (report, id)
}

// Beside 2 (P30): with a bundle held beside a running level, the release of
// the running level frees nothing the bundle holds and cancels nothing; the
// commit then needs no extent rebuild (its fog target and extent-bound
// groups intact), misses no model and re-uploads none, publishes exactly
// the set that was built, and frees only the entries the released level
// alone referenced. Holds when both levels are the same map.
#[test]
fn release_then_commit_frees_nothing_the_bundle_holds() {
    let test = "release_then_commit_frees_nothing_the_bundle_holds";
    let running = props_case("running", &[ZONES, RIFLE]);
    for (label, beside, expected) in [
        (
            "another map",
            props_case("beside", &[RIFLE]),
            crate::render::ModelCacheCommit {
                inserted: 0,
                freed: 1,
                retained: 1,
            },
        ),
        (
            "the same map",
            props_case("same", &[ZONES, RIFLE]),
            crate::render::ModelCacheCommit {
                inserted: 0,
                freed: 0,
                retained: 2,
            },
        ),
    ] {
        let Some((mut app, exit)) = running_level(test, &running) else {
            return;
        };
        let running_set = active_set(&app).unwrap();
        let keys_before = cache_keys(&app);
        assert_eq!(keys_before.len(), 2, "{label}");
        begin_beside(&mut app, &beside, |_| {});
        run_running_until(&mut app, &exit, label, held, |_, _| {});
        retire_on_running_frames(&mut app, &exit, |_, _| {});
        let (_, models) = app.level_loads.test.last_upload_creations.unwrap();
        assert_eq!(models.total(), 0, "{label}: every model hit: {models:?}");
        let cache = app
            .level_loads
            .held_bundle()
            .unwrap()
            .presentation
            .models
            .iter()
            .map(|model| model.cache)
            .collect::<Vec<_>>();
        assert!(
            cache.iter().all(|use_| *use_ == ModelCacheUse::Hit),
            "{label}: {cache:?}"
        );
        let built = without_smoke(&set_summary(&app, held_set(&app)));
        let before = gpu_creation_counts();
        let (report, id) = commit_held(&mut app, &exit);
        assert_eq!(
            gpu_creation_counts().since(before),
            report.created,
            "{label}: the release created nothing; every creation is the commit's"
        );
        assert_eq!(app.boot_state, BootState::Settling, "{label}");
        assert!(
            app.stage_retirements.is_empty(),
            "{label}: the release cancelled nothing"
        );
        assert_eq!(active_set(&app), Some(id), "{label}");
        assert!(
            !resident_sets(&app).contains(&running_set) && resident_sets(&app).is_empty(),
            "{label}: the running level's set went"
        );
        assert_eq!(report.extent_rebuilds, 0, "{label}: {report:?}");
        assert_eq!(report.publish_created.total(), 0, "{label}: {report:?}");
        assert_eq!(report.model_misses, 0, "{label}: {report:?}");
        assert_eq!(report.model_miss_created.total(), 0, "{label}: {report:?}");
        assert_eq!(
            report.uncounted_creations().total(),
            0,
            "{label}: {report:?}"
        );
        assert_eq!(report.model_cache, Some(expected), "{label}: {report:?}");
        let published = app
            .renderer
            .as_ref()
            .unwrap()
            .active_level_resource_summary()
            .unwrap();
        assert_eq!(
            without_smoke(&published),
            built,
            "{label}: the published set is the one built, fog target and extent groups intact"
        );
        assert_eq!(installed_path(&app).as_deref(), Some(beside.map.as_path()));
        retire_all(&mut app);
    }
}

/// Park the upload thread once both a cache-hit and a cache-miss model part
/// are done, until `hold` opens: the load has hit the running level's entry
/// and uploaded a new one.
fn hold_after_hit_and_miss(hold: &Hold) -> impl Fn(&mut LevelPrepInputs) + 'static {
    let hold = hold.clone();
    move |inputs: &mut LevelPrepInputs| {
        let parked = hold.clone();
        let seen = Arc::new(std::sync::Mutex::new((false, false)));
        inputs.hooks.after_upload_part = Some(Arc::new(move |part| {
            let both = {
                let mut seen = seen.lock().unwrap();
                match part {
                    LevelBuildPart::CachedModel => seen.0 = true,
                    LevelBuildPart::Model => seen.1 = true,
                    _ => {}
                }
                seen.0 && seen.1
            };
            if both && matches!(part, LevelBuildPart::CachedModel | LevelBuildPart::Model) {
                parked.park();
            }
        }));
    }
}

// Beside 3 (P28): a load beside a running level hits the running level's
// cached model and uploads a new one, and is then cancelled: explicitly
// while staged, explicitly while its bundle is held, by a superseding load
// beside the level, and by suspend. Every cancel reports nothing; the
// stages join through retirement polled on Running frames (the running
// level drawing the same frame on each); the cache keeps every entry the
// running level references, the one the load hit included, and none of the
// load's own; nothing from the load stays resident.
#[test]
fn cancelled_beside_load_keeps_running_level_cache() {
    let test = "cancelled_beside_load_keeps_running_level_cache";
    let running = props_case("running", &[ZONES]);
    let beside = props_case("beside", &[ZONES, RIFLE]);
    let Some((mut app, exit)) = running_level(test, &running) else {
        return;
    };
    let keys = cache_keys(&app);
    assert_eq!(keys.len(), 1, "the running level's model");
    let hash = settled_hash(&mut app, &exit);
    let mut watch = RunningWatch::new(&app, Some(hash));
    let capture = LogCapture::start();

    // Explicitly, while staged: parked after its hit and its upload.
    let upload = Hold::default();
    let first = begin_beside(&mut app, &beside, hold_after_hit_and_miss(&upload));
    run_running_until(
        &mut app,
        &exit,
        "the hit and the upload",
        |_| upload.reached(),
        |app, frame| watch.check(app, frame, "staged"),
    );
    // The hook parks only once both a cache-hit and a cache-miss model part
    // finished; the probe records the first of them (the second's record
    // follows its hook, which is parked).
    let parts = app
        .level_loads
        .staged()
        .unwrap()
        .probe()
        .upload_parts_done();
    assert!(
        parts.contains(&LevelBuildPart::CachedModel) || parts.contains(&LevelBuildPart::Model),
        "{parts:?}"
    );
    assert!(app.cancel_beside_level_load());
    assert_eq!(app.stage_retirements.retiring_loads(), [first]);
    assert!(!app.level_load_in_flight());
    upload.release();
    retire_on_running_frames(&mut app, &exit, |app, frame| {
        watch.check(app, frame, "retiring the staged cancel");
    });
    assert_eq!(cache_keys(&app), keys, "staged cancel: the cache as found");
    assert!(resident_sets(&app).is_empty(), "staged cancel");

    // Explicitly, with its bundle held: a hit and a miss, then released.
    begin_beside(&mut app, &beside, |_| {});
    run_running_until(&mut app, &exit, "the held bundle", held, |app, frame| {
        watch.check(app, frame, "to the held bundle");
    });
    let uses: Vec<_> = app
        .level_loads
        .held_bundle()
        .unwrap()
        .presentation
        .models
        .iter()
        .map(|model| (model.handle.clone(), model.cache))
        .collect();
    assert_eq!(
        uses,
        [
            (ZONES.to_string(), ModelCacheUse::Hit),
            (RIFLE.to_string(), ModelCacheUse::Miss)
        ]
    );
    assert!(app.cancel_beside_level_load());
    retire_on_running_frames(&mut app, &exit, |app, frame| {
        watch.check(app, frame, "retiring the held cancel");
    });
    assert_eq!(cache_keys(&app), keys, "held cancel: the cache as found");
    assert!(
        resident_sets(&app).is_empty(),
        "held cancel released its set"
    );

    // Superseded by another load beside the level, then cancelled.
    let upload = Hold::default();
    let superseded = begin_beside(&mut app, &beside, hold_after_hit_and_miss(&upload));
    run_running_until(
        &mut app,
        &exit,
        "the superseded load's upload",
        |_| upload.reached(),
        |app, frame| watch.check(app, frame, "before the supersede"),
    );
    let superseding = begin_beside(&mut app, &beside, |_| {});
    assert_eq!(app.stage_retirements.retiring_loads(), [superseded]);
    assert_eq!(active_generation(&app), Some(superseding));
    upload.release();
    retire_on_running_frames(&mut app, &exit, |app, frame| {
        watch.check(app, frame, "retiring the superseded load");
    });
    assert!(app.cancel_beside_level_load());
    retire_on_running_frames(&mut app, &exit, |app, frame| {
        watch.check(app, frame, "retiring the superseding load");
    });
    assert_eq!(cache_keys(&app), keys, "supersede: the cache as found");
    assert!(resident_sets(&app).is_empty(), "supersede");
    assert!(watch.frames > 0);
    assert!(
        loader_errors(&capture).is_empty(),
        "a cancelled load reports nothing"
    );

    // Suspend: the load cancels on the old renderer and its stages retire on
    // later frames.
    let upload = Hold::default();
    begin_beside(&mut app, &beside, hold_after_hit_and_miss(&upload));
    run_running_until(
        &mut app,
        &exit,
        "the upload",
        |_| upload.reached(),
        |_, _| {},
    );
    app.suspend_platform();
    assert!(!app.level_load_in_flight(), "suspend cancelled it");
    upload.release();
    retire_all(&mut app);
    assert!(
        loader_errors(&capture).is_empty(),
        "suspend reports nothing"
    );
}

// Beside 4 (P31): a load beside a running level failing (an injected parse
// panic, prep error and prep panic, an upload error and panic, and a real
// wgpu validation error the upload thread catches) is reported once; every
// stage joins through retirement polled on Running frames; nothing commits;
// the running level stays Running, drawing the same frame, its registry and
// cache entries intact, and no set from the load stays resident.
#[test]
fn beside_load_failure_keeps_level_running() {
    let test = "beside_load_failure_keeps_level_running";
    let running = props_case("running", &[ZONES]);
    let beside = CommitCase::textured_with_model();
    let Some((mut app, exit)) = running_level(test, &running) else {
        return;
    };
    let keys = cache_keys(&app);
    assert_eq!(keys.len(), 1);
    let entities = registry_fingerprint(&app);
    let level = installed_path(&app);
    let hash = settled_hash(&mut app, &exit);
    let mut watch = RunningWatch::new(&app, Some(hash));
    type Edit = Box<dyn Fn(&mut LevelPrepInputs)>;
    let rows: Vec<(&str, Edit)> = vec![
        (
            "parse panic",
            Box::new(|inputs| {
                inputs.hooks.after_hand_out = Some(Arc::new(|stage, _| {
                    if stage == HandOutStage::Spatial {
                        panic!("injected parse panic");
                    }
                }));
            }),
        ),
        (
            "prep error",
            Box::new(|inputs| {
                inputs.hooks.fail = vec![(PrepKind::Collision, "injected prep error")];
            }),
        ),
        (
            "prep panic",
            Box::new(|inputs| inputs.hooks.panic = Some(PrepKind::Fluid)),
        ),
        (
            "upload error",
            Box::new(|inputs| inputs.hooks.upload_textures_twice = true),
        ),
        (
            "upload panic",
            Box::new(|inputs| inputs.hooks.upload_fault = Some(UploadFault::Panic)),
        ),
        (
            "wgpu validation on the upload thread",
            Box::new(|inputs| inputs.hooks.upload_fault = Some(UploadFault::WritePastBufferEnd)),
        ),
    ];
    let capture = LogCapture::start();
    for (label, edit) in rows {
        capture.clear();
        begin_beside(&mut app, &beside, move |inputs| edit(inputs));
        let mut ended = false;
        run_running_until(
            &mut app,
            &exit,
            label,
            |app| !app.level_load_in_flight(),
            |app, frame| {
                watch.check(app, frame, label);
                ended |= frame.step == BesideLoadStep::Ended;
            },
        );
        assert!(ended, "{label}: the poll saw the failure");
        retire_on_running_frames(&mut app, &exit, |app, frame| {
            watch.check(app, frame, label);
        });
        for _ in 0..3 {
            let frame = running_frame(&mut app, &exit).unwrap();
            watch.check(&app, &frame, label);
        }
        let errors = loader_errors(&capture);
        assert_eq!(errors.len(), 1, "{label}: one report: {errors:?}");
        assert!(
            errors[0].contains("keeps running"),
            "{label}: the beside half: {errors:?}"
        );
        assert!(!exit.0.get(), "{label}: no exit");
        assert!(app.exit_result.is_ok(), "{label}");
        assert_eq!(installed_path(&app), level, "{label}: nothing committed");
        assert_eq!(registry_fingerprint(&app), entities, "{label}");
        assert_eq!(cache_keys(&app), keys, "{label}: cache entries intact");
        assert!(resident_sets(&app).is_empty(), "{label}: no set stays");
    }
}

// Beside 5 (P26): a mod hot reload committed on a Running frame while a
// load runs beside the running level, through the real staged-manifest
// route (the reload built on the runtime's staged lane and committed by the
// redraw's staged-manifest poll). The load dispatched first, so its
// prediction read the old descriptors: the reloaded classname's model, ZONES.
// The reload re-points that classname at the rifle; the runtime defers the
// mesh change for the running level, which keeps drawing ZONES on every
// Running frame until its release (e). At the commit:
// (a) the placement spawns from the reloaded descriptors, with the rifle;
// (b) the rifle, a model only the reload added, takes the counted fallback:
//     one miss, logged once, loaded into the published set;
// (c) ZONES, a model only the reload removed, is released at commit: out of
//     the published set, its cache entry freed (the released running level
//     was its only other holder), counted and logged once, creating nothing;
// (d) no entry keyed on a pre-reload install-time parameter is hit: the
//     load's one cache hit (ZONES) and every model the published set draws
//     are keyed by the files read after the reload at the commit's
//     parameters (install parameters come only from the dispatch tiers,
//     which no reload changes).
#[test]
fn hot_reload_beside_load_installs_reloaded_models() {
    let test = "hot_reload_beside_load_installs_reloaded_models";
    let running = props_case("running", &[ZONES]);
    let beside = entities_case(
        "beside",
        vec![crate::level_load::model_tests::record(RELOADED, &[], 0)],
    );
    let Some((mut app, exit)) = running_level(test, &running) else {
        return;
    };
    let mut crate_type = super::super::lifecycle_test_fixtures::descriptor(RELOADED);
    crate_type.mesh = Some(MeshDescriptor {
        model: ZONES.to_string(),
        shadow_only: false,
        attachments: Default::default(),
        shadow_bias_scale: 1.0,
        animations: Default::default(),
        default_state: None,
        locomotion: None,
    });
    // The mod's descriptors before the reload.
    script_ctx(&app)
        .data_registry
        .borrow_mut()
        .replace_entity_types(vec![crate_type]);
    let running_set = active_set(&app).unwrap();
    let identity = |app: &App, id: LevelSetId, handle: &str| {
        app.renderer
            .as_ref()
            .unwrap()
            .level_set_model_identity(id, handle)
    };
    let running_entry = identity(&app, running_set, ZONES);
    assert!(running_entry.is_some());

    let upload = Hold::default();
    begin_beside(&mut app, &beside, hold_upload(&upload));
    assert!(
        app.level_loads.is_staged(),
        "dispatched: the prediction read the old descriptors"
    );
    // The reload commits on Running frames while the load runs.
    let mod_root = super::reload_mod_root(RELOADED, RIFLE);
    let capture = LogCapture::start();
    super::commit_staged_reload(&mut app, mod_root.path(), |app| {
        running_frame(app, &exit).expect("Running");
        assert_eq!(identity(app, running_set, ZONES), running_entry, "(e)");
    });
    assert_eq!(app.boot_state, BootState::Running);
    capture.assert_logged_once(
        log::Level::Warn,
        "staged descriptor refresh deferred components.mesh for `beside_reload_crate`",
    );
    assert!(
        app.session
            .as_ref()
            .unwrap()
            .scripting
            .script_runtime
            .deferred_mesh_descriptors()
            .is_some(),
        "the mesh change waits for the next level install"
    );
    upload.release();
    run_running_until(&mut app, &exit, "the held bundle", held, |app, _| {
        assert_eq!(
            identity(app, running_set, ZONES),
            running_entry,
            "(e) the running level keeps its model"
        );
    });
    let bundle = app.level_loads.held_bundle().unwrap();
    let predicted: Vec<String> = bundle
        .presentation
        .models
        .handles()
        .map(str::to_string)
        .collect();
    assert_eq!(
        predicted,
        [ZONES],
        "the prediction read the old descriptors"
    );
    assert_eq!(
        bundle.presentation.models.get(ZONES).unwrap().cache,
        ModelCacheUse::Hit,
        "the load hit the running level's entry"
    );
    let params = crate::level_load::ModelInstallParams::at_tier(
        crate::startup::render_profile::renderer_surface_depth_quality(
            bundle.tiers.surface_depth_quality,
        ),
    );
    let prm_root = dev_prm_root();
    let zones_key = super::model_key_now(&dev_content_root(), &prm_root, ZONES, params);
    assert_eq!(
        app.renderer
            .as_ref()
            .unwrap()
            .cached_model(&zones_key)
            .map(|cached| cached.identity()),
        running_entry,
        "(d) the hit entry is the one the post-reload files key at the commit's parameters"
    );

    capture.clear();
    let (report, id) = commit_held(&mut app, &exit);
    // (b)
    assert_eq!(report.model_misses, 1, "{report:?}");
    assert!(report.model_miss_created.total() > 0, "{report:?}");
    capture.assert_logged_once(
        log::Level::Warn,
        "model prediction missed 'models/ar_4/model.gltf'",
    );
    // (c): ZONES' entry is freed by the commit's one cache sweep, after the
    // model install, the released running level and the released
    // over-prediction having been its only other holders.
    assert_eq!(report.model_over_predictions, 1, "{report:?}");
    assert_eq!(
        report.model_cache,
        Some(crate::render::ModelCacheCommit {
            inserted: 0,
            freed: 1,
            retained: 1,
        }),
        "{report:?}"
    );
    capture.assert_logged_once(
        log::Level::Warn,
        "model prediction over-predicted 'models/pose-modifier-fixture/joint_zones.gltf'",
    );
    assert_eq!(report.uncounted_creations().total(), 0, "{report:?}");
    let renderer = app.renderer.as_ref().unwrap();
    assert!(
        renderer.level_set_model_identity(id, ZONES).is_none(),
        "(c) the removed model left the published set"
    );
    assert!(
        renderer.cached_model(&zones_key).is_none(),
        "(c) and its cache entry was freed"
    );
    // (b), (d)
    let rifle_key = super::model_key_now(&dev_content_root(), &prm_root, RIFLE, params);
    let rifle_entry = renderer.level_set_model_identity(id, RIFLE);
    assert!(
        rifle_entry.is_some(),
        "(b) the reloaded descriptor's model loads into the published set"
    );
    assert_eq!(
        renderer
            .cached_model(&rifle_key)
            .map(|cached| cached.identity()),
        rifle_entry,
        "(d) the set's model is the entry the post-reload files key"
    );
    assert_eq!(
        renderer.model_cache_keys(),
        vec![rifle_key],
        "the cache holds only what the published set draws"
    );
    // (a)
    let ctx = script_ctx(&app);
    let registry = ctx.registry.borrow();
    let meshes: Vec<String> = registry
        .iter_with_kind(postretro_entities::ComponentKind::Mesh)
        .filter_map(|(entity, _)| {
            registry
                .get_component::<MeshComponent>(entity)
                .ok()
                .map(|mesh| mesh.model.clone())
        })
        .collect();
    assert!(
        meshes.iter().any(|model| model == RIFLE),
        "(a) the placement spawned with the reloaded model: {meshes:?}"
    );
    assert!(
        !meshes.iter().any(|model| model == ZONES),
        "(a) nothing spawned with the removed model: {meshes:?}"
    );
    drop(registry);
    retire_all(&mut app);
}

// The commit sweeps the model cache once, after its model install: a model
// the prediction missed whose content only the released running level held
// is a cache hit on the commit frame, drawn with the running level's entry,
// not parsed and uploaded again. A hot reload committed beside the load
// re-points the placement's classname from ZONES, which the load predicted
// and uploaded, to the rifle, which only the running level draws.
#[test]
fn commit_miss_reuses_released_level_model() {
    let test = "commit_miss_reuses_released_level_model";
    let running = props_case("running", &[RIFLE]);
    let beside = entities_case(
        "beside",
        vec![crate::level_load::model_tests::record(RELOADED, &[], 0)],
    );
    let Some((mut app, exit)) = running_level(test, &running) else {
        return;
    };
    let mut crate_type = super::super::lifecycle_test_fixtures::descriptor(RELOADED);
    crate_type.mesh = Some(MeshDescriptor {
        model: ZONES.to_string(),
        shadow_only: false,
        attachments: Default::default(),
        shadow_bias_scale: 1.0,
        animations: Default::default(),
        default_state: None,
        locomotion: None,
    });
    // The mod's descriptors before the reload.
    script_ctx(&app)
        .data_registry
        .borrow_mut()
        .replace_entity_types(vec![crate_type]);
    let running_set = active_set(&app).unwrap();
    let running_rifle = app
        .renderer
        .as_ref()
        .unwrap()
        .level_set_model_identity(running_set, RIFLE);
    assert!(running_rifle.is_some(), "the running level draws the rifle");

    let upload = Hold::default();
    begin_beside(&mut app, &beside, hold_upload(&upload));
    assert!(
        app.level_loads.is_staged(),
        "dispatched: the prediction read the old descriptors"
    );
    let mod_root = super::reload_mod_root(RELOADED, RIFLE);
    super::commit_staged_reload(&mut app, mod_root.path(), |app| {
        running_frame(app, &exit).expect("Running");
    });
    upload.release();
    run_running_until(&mut app, &exit, "the held bundle", held, |_, _| {});
    let models = &app.level_loads.held_bundle().unwrap().presentation.models;
    assert_eq!(models.handles().collect::<Vec<_>>(), [ZONES]);
    assert_eq!(
        models.get(ZONES).unwrap().cache,
        ModelCacheUse::Miss,
        "the running level does not draw ZONES: the load uploaded it"
    );

    let capture = LogCapture::start();
    let (report, id) = commit_held(&mut app, &exit);
    assert_eq!(report.model_misses, 1, "{report:?}");
    capture.assert_logged_once(
        log::Level::Warn,
        "model prediction missed 'models/ar_4/model.gltf'",
    );
    assert_eq!(
        report.model_miss_created.total(),
        0,
        "the miss hit the released level's entry, uploading nothing: {report:?}"
    );
    assert_eq!(report.uncounted_creations().total(), 0, "{report:?}");
    assert_eq!(
        app.renderer
            .as_ref()
            .unwrap()
            .level_set_model_identity(id, RIFLE),
        running_rifle,
        "the published set draws the running level's rifle entry"
    );
    // ZONES' offer joined at the publish; the one sweep after the install
    // freed it (released as an over-prediction) and kept the rifle.
    assert_eq!(report.model_over_predictions, 1, "{report:?}");
    assert_eq!(
        report.model_cache,
        Some(crate::render::ModelCacheCommit {
            inserted: 1,
            freed: 1,
            retained: 1,
        }),
        "{report:?}"
    );
    retire_all(&mut app);
}

// The beside order runs the commit's checks before it releases the running
// level: a held bundle whose check fails (the SH controller rejecting its
// level's topology, the commit checks' own seam) takes the failure route's
// beside half. The running level keeps Running on its own set with its
// registry and cache entries, the bundle's set is released, nothing
// commits, and the load reports once.
#[test]
fn beside_commit_check_failure_keeps_level_running() {
    let test = "beside_commit_check_failure_keeps_level_running";
    let running = CommitCase::textured();
    let beside = CommitCase::streamed_sh();
    let Some((mut app, exit)) = running_level(test, &running) else {
        return;
    };
    let running_set = active_set(&app);
    let level = installed_path(&app);
    let entities = registry_fingerprint(&app);
    let keys = cache_keys(&app);
    begin_beside(&mut app, &beside, |_| {});
    run_running_until(&mut app, &exit, "the held bundle", held, |_, _| {});
    let bundle_set = held_set(&app);
    match &mut app
        .level_loads
        .active_mut()
        .expect("the held load is active")
        .phase
    {
        crate::startup::level_loads::ActivePhase::Ready(bundle) => bundle
            .streaming
            .sh
            .as_mut()
            .expect("the fixture streams SH")
            .topology
            .break_for_tests(),
        _ => panic!("a held load's bundle is ready"),
    }
    let capture = LogCapture::start();
    assert!(
        app.commit_ready_beside_bundle(&exit).is_none(),
        "the check refuses the bundle"
    );
    assert_eq!(
        app.boot_state,
        BootState::Running,
        "the level keeps Running"
    );
    assert_eq!(active_set(&app), running_set, "its set stays active");
    assert_eq!(installed_path(&app), level, "nothing committed");
    assert_eq!(registry_fingerprint(&app), entities);
    assert_eq!(cache_keys(&app), keys, "cache entries intact");
    assert!(
        !resident_sets(&app).contains(&bundle_set),
        "the bundle's set was released"
    );
    assert!(!app.level_load_in_flight());
    let errors = loader_errors(&capture);
    assert_eq!(errors.len(), 1, "one report: {errors:?}");
    assert!(
        errors[0].contains("level commit checks failed") && errors[0].contains("keeps running"),
        "the commit check's failure, on the beside route: {errors:?}"
    );
    assert!(!exit.0.get(), "no exit");
    assert!(app.exit_result.is_ok());
    // The running level draws on.
    for _ in 0..3 {
        running_frame(&mut app, &exit).expect("still Running");
    }
    assert_eq!(loader_errors(&capture).len(), 1, "still one report");
    retire_all(&mut app);
}

// Beside 6 (P27): a Surface Depth change and a shadow-tier change on a
// Running frame while a load runs beside the level do not reach the load:
// its bundle carries the dispatch tiers and its set matches one built
// beside the same level with no change; the live Surface Depth rewrite
// touches only the running level's materials.
#[test]
fn running_frame_tier_change_skips_beside_load() {
    use crate::options::{ShadowQuality, SurfaceDepthQuality};
    let test = "running_frame_tier_change_skips_beside_load";
    let running = CommitCase::textured();
    let beside = CommitCase::textured_with_model();
    let Some((mut app, exit)) = running_level(test, &running) else {
        return;
    };

    // The reference: the same load beside the level, no change.
    begin_beside(&mut app, &beside, |_| {});
    run_running_until(&mut app, &exit, "the reference bundle", held, |_, _| {});
    let reference_tiers = app.level_loads.held_bundle().unwrap().tiers;
    let reference = set_summary(&app, held_set(&app));
    assert!(app.cancel_beside_level_load());
    retire_on_running_frames(&mut app, &exit, |_, _| {});

    let upload = Hold::default();
    begin_beside(&mut app, &beside, hold_upload(&upload));
    assert!(app.level_loads.is_staged(), "dispatched at the old tiers");
    let options = app.session.as_ref().unwrap().player_options.clone();
    let shadow = if options.shadow_quality == ShadowQuality::Low {
        ShadowQuality::High
    } else {
        ShadowQuality::Low
    };
    let depth = if options.surface_depth_quality == SurfaceDepthQuality::Off {
        SurfaceDepthQuality::On
    } else {
        SurfaceDepthQuality::Off
    };
    let running_materials: usize = {
        let summary = app
            .renderer
            .as_ref()
            .unwrap()
            .active_level_resource_summary()
            .unwrap();
        summary
            .split("gpu_textures=")
            .nth(1)
            .and_then(|rest| rest.split(' ').next())
            .and_then(|count| count.parse().ok())
            .expect("the summary counts materials")
    };
    // A Running frame's options step applies both changes.
    running_frame(&mut app, &exit).unwrap();
    let capture = LogCapture::start();
    {
        let session = app.session.as_mut().unwrap();
        session.player_options.shadow_quality = shadow;
        session.player_options.surface_depth_quality = depth;
    }
    app.configure_player_shadow_quality(shadow);
    app.apply_player_surface_depth_quality(depth);
    capture.assert_logged_once(
        log::Level::Info,
        &format!("({running_materials} materials)"),
    );
    upload.release();
    run_running_until(&mut app, &exit, "the bundle", held, |_, _| {});
    assert_eq!(
        app.level_loads.held_bundle().unwrap().tiers,
        reference_tiers,
        "the bundle carries the dispatch tiers"
    );
    assert_eq!(
        set_summary(&app, held_set(&app)),
        reference,
        "built as one dispatched at the old tiers"
    );
    assert!(app.cancel_beside_level_load());
    retire_on_running_frames(&mut app, &exit, |_, _| {});
}

// Settings and extent 2, P16's Running half: two scene-extent changes on
// different Running frames between a beside load's dispatch and its commit
// (the build parked at its first part, so it built at the dispatch extent).
// The running level's resizes rebuild only its own groups (the load's set
// keeps its built extent and no publish rebuild is counted); the commit
// rebuilds the bundle's extent-bound groups and fog target once, counted,
// and the published set binds the new extent.
#[test]
fn running_frame_extent_change_rebuilds_beside_bundle_once() {
    let test = "running_frame_extent_change_rebuilds_beside_bundle_once";
    let running = CommitCase::textured();
    let beside = CommitCase::textured();
    let Some((mut app, exit)) = running_level(test, &running) else {
        return;
    };
    let built_at = app.renderer.as_ref().unwrap().scene_extent();
    let upload = Hold::default();
    begin_beside(&mut app, &beside, hold_upload(&upload));
    let rebuilds = app.renderer.as_ref().unwrap().extent_rebuilds();
    for (width, height) in [(96u32, 80u32), (128, 96)] {
        app.renderer
            .as_mut()
            .unwrap()
            .record_surface_size(width, height);
        running_frame(&mut app, &exit).unwrap();
        let renderer = app.renderer.as_ref().unwrap();
        let scene = renderer.scene_extent();
        assert_eq!((scene.width, scene.height), (width, height));
        assert!(
            renderer
                .active_level_resource_summary()
                .unwrap()
                .contains(&format!("extent=({scene:?}")),
            "the running level's resize rebuilt its own groups"
        );
    }
    upload.release();
    run_running_until(&mut app, &exit, "the bundle", held, |_, _| {});
    let renderer = app.renderer.as_ref().unwrap();
    assert_eq!(
        renderer.extent_rebuilds(),
        rebuilds,
        "the running level's resizes count no publish rebuild"
    );
    let scene = renderer.scene_extent();
    let held_summary = set_summary(&app, held_set(&app));
    assert!(
        held_summary.contains(&format!("extent=({built_at:?}")),
        "the load's set keeps the extent it was built at: {held_summary}"
    );
    let (report, _) = commit_held(&mut app, &exit);
    assert_eq!(report.extent_rebuilds, 1, "{report:?}");
    assert!(report.publish_created.total() > 0, "{report:?}");
    assert_eq!(report.uncounted_creations().total(), 0, "{report:?}");
    let summary = app
        .renderer
        .as_ref()
        .unwrap()
        .active_level_resource_summary()
        .unwrap();
    assert!(
        summary.contains(&format!("extent=({scene:?}")),
        "the first frame binds the new extent's views: {summary}"
    );
    retire_all(&mut app);
}

// A loading-screen request supersedes a load beside the running level: the
// drain cancels it first (its stages retire, it reports nothing, its set is
// released), then the request takes the ordinary path, unload then load
// behind the loading screen. An unload request cancels one the same way and
// the level unloads to Frontend.
#[test]
fn loading_screen_request_supersedes_beside_load() {
    let test = "loading_screen_request_supersedes_beside_load";
    let running = CommitCase::textured();
    let beside = CommitCase::textured_with_model();
    let next = CommitCase::streamed_sh();
    let Some((mut app, exit)) = running_level(test, &running) else {
        return;
    };
    let capture = LogCapture::start();

    let upload = Hold::default();
    let cancelled = begin_beside(&mut app, &beside, hold_upload(&upload));
    run_running_until(
        &mut app,
        &exit,
        "the upload",
        |_| upload.reached(),
        |_, _| {},
    );
    use_case(&mut app, &next, |_| {});
    request_path(&mut app, &next.map);
    assert!(
        running_frame(&mut app, &exit).is_none(),
        "the request leaves Running"
    );
    assert_eq!(app.boot_state, BootState::Loading);
    assert!(app.stage_retirements.retiring_loads().contains(&cancelled));
    assert_ne!(active_generation(&app), Some(cancelled));
    assert!(
        !app.level_loads.beside_active(),
        "a loading-screen load now"
    );
    assert!(
        app.loading_screen_progress().is_some(),
        "its loading screen"
    );
    upload.release();
    run_to_commit(&mut app, &exit, "the superseding load");
    assert_eq!(installed_path(&app).as_deref(), Some(next.map.as_path()));
    assert!(
        resident_sets(&app).is_empty(),
        "the beside set was released"
    );
    retire_all(&mut app);

    // An unload request cancels a held one, and the level unloads.
    reveal(&mut app);
    begin_beside(&mut app, &beside, |_| {});
    run_running_until(&mut app, &exit, "the held bundle", held, |_, _| {});
    app.enqueue_level_request(LevelRequest::Unload);
    assert!(running_frame(&mut app, &exit).is_none());
    assert_eq!(app.boot_state, BootState::Frontend);
    assert!(!app.level_load_in_flight());
    assert!(app.level.is_none());
    assert!(resident_sets(&app).is_empty(), "the held set was released");
    retire_all(&mut app);
    assert!(
        loader_errors(&capture).is_empty(),
        "a cancel reports nothing"
    );
}

// Marks, beside a running level: the load logs its own line C, after its
// reveal like any load: dispatch, `bundle_ready` closed on the Running frame
// that took the bundle (its stages inside), `bundle_held` until its caller
// commits, the commit's checks (run before the release), the running
// level's release, the commit frame, the settle hold and the reveal. No
// loading-screen mark: it has none. It adds up from the beside entry to the
// reveal. The running level's own line C, still waiting for a present when
// the load begins beside it, is logged at the beside entry with its own
// marks, not the load's.
#[test]
fn beside_load_logs_its_own_line_c() {
    let test = "beside_load_logs_its_own_line_c";
    let running = CommitCase::textured();
    let beside = CommitCase::textured_with_model();
    let Some((mut app, exit)) = running_level(test, &running) else {
        return;
    };
    assert!(
        app.pending_level_log,
        "no frame presented since the running level's reveal: its line C waits"
    );
    let capture = LogCapture::start();
    let wall_start = Instant::now();
    begin_beside(&mut app, &beside, |_| {});
    assert!(!app.pending_level_log, "the beside entry closed it");
    let running_line = logged_line_c(&capture);
    let running_marks: Vec<String> = parse_line_c(&running_line)
        .into_iter()
        .map(|mark| mark.name)
        .collect();
    assert_eq!(
        running_marks.last().map(String::as_str),
        Some("first_level_frame"),
        "{running_line}"
    );
    for beside_mark in ["bundle_held", "commit_checks", "running_release"] {
        assert!(
            !running_marks.iter().any(|mark| mark == beside_mark),
            "the running level's line holds none of the load's marks: {running_line}"
        );
    }
    capture.clear();
    run_running_until(&mut app, &exit, "the held bundle", held, |_, _| {});
    for _ in 0..2 {
        running_frame(&mut app, &exit).expect("Running while held");
    }
    commit_held(&mut app, &exit);
    settle_and_reveal(&mut app);
    let wall = wall_start.elapsed();
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
            "bundle_held",
            "commit_checks",
            "running_release",
            "commit_frame",
            "settle_hold",
            "reveal",
            "first_level_frame"
        ],
        "{line}"
    );
    assert_adds_up_to(&marks, wall);
    let bundle_ready = &marks[1];
    assert_eq!(bundle_ready.inside[0].0, "prl_parse", "{line}");
    assert_eq!(bundle_ready.inside[1].0, "upload", "{line}");
    assert!(marks[2].ms > 0.0, "held across Running frames: {line}");
    let commit_frame = &marks[5];
    assert!(
        !commit_frame
            .inside
            .iter()
            .any(|(name, _)| name == "commit_checks"),
        "the checks ran before the release, not again in the commit: {line}"
    );
    assert_group_sums(commit_frame);
    retire_all(&mut app);
}

// The beside poll and commit, the gate: the Running frame polls a load
// beside the level after its world-less early return and before its input;
// the beside entry, poll, hold, commit (the release spliced in), cancel and
// failure half contain no blocking receive, thread join, device wait or
// sleep.
#[test]
fn beside_path_has_no_blocking_wait() {
    let frame_loop = include_str!("../frame_loop/mod.rs");
    let redraw = function_source(frame_loop, "redraw");
    let frontend = redraw
        .find("app.render_frontend_frame(event_loop, now);")
        .expect("the world-less early return");
    let poll = redraw
        .find("app.poll_beside_level_load(event_loop);")
        .expect("every Running frame polls the beside load");
    let input = redraw
        .find("cpu_stages.scope(cpu_timing::FrameStage::Input)")
        .expect("the Input stage");
    assert!(
        frontend < poll && poll < input,
        "after Frontend, before Input"
    );

    let beside = include_str!("lifecycle_beside.rs");
    let net = include_str!("lifecycle_net.rs");
    let publish = include_str!("../../../renderer/src/render/renderer_resources/publish.rs");
    let resources = include_str!("../../../renderer/src/render/renderer_resources.rs");
    let mut checked: Vec<(String, String)> = Vec::new();
    for name in [
        "begin_level_load_beside",
        "poll_beside_level_load",
        "hold_beside_bundle",
        "commit_ready_beside_bundle",
        "cancel_beside_level_load",
        "finish_beside_level_failure",
    ] {
        checked.push((name.to_string(), function_source(beside, name).to_string()));
    }
    checked.push((
        "unload_level (the release)".to_string(),
        function_source(net, "unload_level").to_string(),
    ));
    checked.push((
        "Renderer::release_level_resources".to_string(),
        function_source(resources, "release_level_resources").to_string(),
    ));
    checked.push((
        "Renderer::release_level_set".to_string(),
        function_source(publish, "release_level_set").to_string(),
    ));
    for (name, source) in &checked {
        let waits = blocking_waits_in(source);
        assert!(waits.is_empty(), "`{name}` blocks: {waits:?}");
    }
    // The commit it calls is the Loading frame's, under the existing gate,
    // with the bundle its checks passed; those checks (the commit's own,
    // under the same gate) run before the release, and a failing one takes
    // the beside half of the failure route.
    let commit = function_source(beside, "commit_ready_beside_bundle");
    let checks = commit
        .find("self.check_level_bundle(*bundle)")
        .expect("the beside commit checks the bundle");
    let failure = commit
        .find("self.finish_beside_level_failure(")
        .expect("a failed check keeps the running level running");
    let release = commit
        .find("self.unload_level();")
        .expect("the running level's release");
    let committed = commit
        .find("self.commit_ready_level_bundle(checked, event_loop)")
        .expect("the checked bundle commits");
    assert!(
        checks < failure && failure < release && release < committed,
        "check, then release, then commit"
    );
}
