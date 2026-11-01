//! Prep's ordering against the staged parse: texture prep before the last
//! section read, planner topology after the parse installs it, cancel, and
//! progress.
//! See: context/plans/in-progress/level-load-pipeline/index.md (Off the main thread, Progress)

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use postretro_level_loader::{HandOutStage, LevelLoadProgress, LoadPhase};

use super::test_support::{
    HOLD_TIMEOUT, PrepProbe, inputs_for, no_prm_root, poll_to_outcome, run_prep, test_pool,
    write_textured_level,
};
use super::*;

// Off-main 5: with the parse held right after hand-out 2, texture prep reads
// its sidecar and finishes before the parse reads another section.
#[test]
fn texture_prep_starts_before_the_last_section_read() {
    let (level, prm_root) = write_textured_level();
    let pool = test_pool();
    let mut inputs = inputs_for(level.path(), prm_root.path());
    let finished_during_hold = Arc::new(AtomicBool::new(false));
    let hold_reached = Arc::new(AtomicBool::new(false));
    {
        let finished_during_hold = Arc::clone(&finished_during_hold);
        let hold_reached = Arc::clone(&hold_reached);
        inputs.hooks.after_hand_out = Some(Arc::new(move |stage, probe: &PrepProbe| {
            if stage != HandOutStage::Geometry {
                return;
            }
            hold_reached.store(true, Ordering::Release);
            // The parse waits here: nothing past hand-out 2 is read until
            // texture prep has finished, or the hold times out.
            let finished = probe.wait_until(|state| state.finished.contains(&PrepKind::Textures));
            let state = probe.state();
            assert_eq!(state.last_hand_out, Some(HandOutStage::Geometry));
            finished_during_hold.store(finished, Ordering::Release);
        }));
    }
    let (_prep, outcome, _progress) = run_prep(&pool, inputs);
    let prepared = outcome.expect("the textured level prepares");
    let gpu = prepared
        .gpu
        .as_ref()
        .expect("a load that builds no set keeps its GPU products");
    assert!(
        hold_reached.load(Ordering::Acquire),
        "the parse reached hand-out 2"
    );
    assert!(
        finished_during_hold.load(Ordering::Acquire),
        "texture prep finished while the parse was held after hand-out 2 (a timeout fails)"
    );
    assert!(
        matches!(
            gpu.textures.entries.as_slice(),
            [postretro_render_cpu::texture_prep::PreparedTexture::Bundle { .. }]
        ),
        "texture prep read the wall's sidecar"
    );
    assert_eq!(gpu.diffuse_dims, [(32, 16)]);
}

// P19: the planner topology reads the manifest's cluster adjacency and seam
// portals, which the parse installs last. Held at every hand-out up to the
// entities, with every task those hand-outs started finished, the streaming
// level data has not started; after the parse accepts, it decodes a
// topology (it would panic on an uninstalled manifest).
#[test]
fn planner_topology_waits_for_parse_install() {
    let (_dir, path) = crate::sh_streaming::sync_manifest_test_fixture::write_one_cluster_prl();
    let pool = test_pool();
    let mut inputs = inputs_for(&path, &no_prm_root());
    let early_starts = Arc::new(Mutex::new(Vec::new()));
    let holds_timed_out = Arc::new(AtomicBool::new(false));
    {
        let early_starts = Arc::clone(&early_starts);
        let holds_timed_out = Arc::clone(&holds_timed_out);
        inputs.hooks.after_hand_out = Some(Arc::new(move |stage, probe: &PrepProbe| {
            let expected: &[PrepKind] = match stage {
                HandOutStage::ShManifest => &[],
                HandOutStage::Geometry => &[PrepKind::Textures],
                HandOutStage::Spatial | HandOutStage::Entities | HandOutStage::Final => &[
                    PrepKind::Textures,
                    PrepKind::RangeCheck,
                    PrepKind::ShResidency,
                ],
            };
            if !probe.wait_until(|state| expected.iter().all(|kind| state.finished.contains(kind)))
            {
                holds_timed_out.store(true, Ordering::Release);
            }
            if probe.state().started.contains(&PrepKind::StreamingData) {
                early_starts.lock().unwrap().push(stage);
            }
        }));
    }
    let (_prep, outcome, _progress) = run_prep(&pool, inputs);
    let prepared = outcome.expect("the streamed level prepares");
    let gpu = prepared
        .gpu
        .as_ref()
        .expect("a load that builds no set keeps its GPU products");
    assert!(
        !holds_timed_out.load(Ordering::Acquire),
        "a hold timed out waiting for its stage's tasks"
    );
    assert!(
        early_starts.lock().unwrap().is_empty(),
        "streaming level data started before the parse returned: {:?}",
        early_starts.lock().unwrap()
    );
    let sh = prepared
        .streaming
        .sh
        .as_ref()
        .expect("the fixture streams SH, so prep decoded its topology");
    assert!(gpu.sh_residency.is_some());
    assert_eq!(sh.manifest.cluster_count(), 1);
}

// Cancel during the parse: nothing is reported, no task the hold's hand-out
// did not already queue starts, the parse stops at its next boundary, and
// the group retires by polling.
#[test]
fn cancelled_prep_reports_nothing_and_spawns_no_more_tasks() {
    let (level, prm_root) = write_textured_level();
    let pool = test_pool();
    let mut inputs = inputs_for(level.path(), prm_root.path());
    let (held_tx, held_rx) = mpsc::channel::<()>();
    let (release_tx, release_rx) = mpsc::channel::<()>();
    let release_rx = Mutex::new(release_rx);
    let held_tx = Mutex::new(held_tx);
    let release_timed_out = Arc::new(AtomicBool::new(false));
    {
        let release_timed_out = Arc::clone(&release_timed_out);
        inputs.hooks.after_hand_out = Some(Arc::new(move |stage, _probe: &PrepProbe| {
            if stage != HandOutStage::Geometry {
                return;
            }
            let _ = held_tx.lock().unwrap().send(());
            if release_rx
                .lock()
                .unwrap()
                .recv_timeout(HOLD_TIMEOUT)
                .is_err()
            {
                release_timed_out.store(true, Ordering::Release);
            }
        }));
    }
    let progress = Arc::new(LevelLoadProgress::new());
    let prep = start_level_prep(pool.group(), inputs, Arc::clone(&progress));
    let probe = prep.probe();
    held_rx
        .recv_timeout(HOLD_TIMEOUT)
        .expect("the parse reached hand-out 2");
    // Queued, not started: the cancel drops every still-queued task of the
    // group under the queue lock, so nothing is taken off the queue after
    // it. The `tasks_started` counter can lag, though: a worker bumps it
    // after releasing the lock, so a task taken before the cancel may be
    // counted after it. Nothing new is queued.
    let queued_at_cancel = pool.tasks_queued();
    let mut retirement = prep.cancel();
    release_tx.send(()).unwrap();

    let deadline = Instant::now() + HOLD_TIMEOUT;
    while !retirement.try_finish() {
        assert!(
            Instant::now() < deadline,
            "the cancelled group never retired"
        );
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
    assert!(!release_timed_out.load(Ordering::Acquire));
    assert_eq!(
        pool.tasks_queued(),
        queued_at_cancel,
        "no task queued after the cancel"
    );
    assert!(
        pool.tasks_started() <= queued_at_cancel,
        "every started task was queued before the cancel"
    );
    let state = probe.state();
    assert_eq!(
        state.last_hand_out,
        Some(HandOutStage::Geometry),
        "the parse stopped at its next stage boundary"
    );
    assert!(!state.started.contains(&PrepKind::RangeCheck));
    assert!(!state.started.contains(&PrepKind::Assemble));
    assert!(state.settled, "a cancelled prep keeps no outcome to report");
    assert!(
        !progress.phase(LoadPhase::Prep).is_finished(),
        "a cancelled prep credits no finish"
    );
}

// Prep credits its share in tasks finished: a level with no SH manifest
// credits that kind on hand-out 1, texture prep's credit lands while the
// parse is held, and the finished prep reaches exactly its share. An empty
// prep set reaches its whole share on entry (P15).
#[test]
fn prep_progress_credits_tasks_and_empty_prep_reaches_its_share() {
    let (level, prm_root) = write_textured_level();
    let pool = test_pool();
    let mut inputs = inputs_for(level.path(), prm_root.path());
    let at_hold = Arc::new(Mutex::new(None));
    let progress = Arc::new(LevelLoadProgress::new());
    {
        let at_hold = Arc::clone(&at_hold);
        let progress = Arc::clone(&progress);
        inputs.hooks.after_hand_out = Some(Arc::new(move |stage, probe: &PrepProbe| {
            if stage == HandOutStage::Geometry
                && probe.wait_until(|state| state.finished.contains(&PrepKind::Textures))
            {
                *at_hold.lock().unwrap() = Some((
                    progress.phase(LoadPhase::Prep).counts(),
                    progress.fraction(),
                ));
            }
        }));
    }
    let total = PrepKind::ALL.len() as u64;
    let mut prep = start_level_prep(pool.group(), inputs, Arc::clone(&progress));
    assert_eq!(
        progress.phase(LoadPhase::Prep).counts().1,
        total,
        "prep plans its fixed set of kinds on entry"
    );
    poll_to_outcome(&mut prep).expect("the textured level prepares");
    let ((done, planned), held_fraction) = at_hold
        .lock()
        .unwrap()
        .expect("the hold after hand-out 2 sampled progress");
    assert_eq!(planned, total);
    // The empty SH plan (hand-out 1) and texture prep, nothing past them.
    assert_eq!(done, 2, "credited kinds while the parse was held");
    let prep_share = postretro_level_loader::LOAD_PHASE_WEIGHT_PREP as f32;
    let parse_share = postretro_level_loader::LOAD_PHASE_WEIGHT_PARSE as f32;
    assert!(held_fraction > 0.0 && held_fraction < parse_share + prep_share);

    let prep_progress = progress.phase(LoadPhase::Prep);
    assert!(prep_progress.is_finished());
    assert_eq!(prep_progress.counts(), (total, total));
    assert!(
        (progress.fraction() - (parse_share + prep_share)).abs() < 1e-5,
        "parse and prep finished, upload and settling not: {}",
        progress.fraction()
    );
    assert!(progress.fraction() < 1.0);

    let empty = LevelLoadProgress::new();
    enter_prep_progress(&empty, 0);
    assert!(empty.phase(LoadPhase::Prep).is_finished());
    assert!(
        (empty.fraction() - prep_share).abs() < 1e-6,
        "an empty prep set reaches its whole share on entry: {}",
        empty.fraction()
    );
}

// The prepared level carries what it was dispatched with (limits, tiers, the
// `.prm` root) and each task's time; with a renderer, the inputs come from
// the App's dispatch capture.
#[test]
fn prepared_level_carries_its_dispatch_snapshot_and_timings() {
    let (level, prm_root) = write_textured_level();
    let pool = test_pool();
    let mut app = crate::startup::lifecycle::tests::test_app();
    app.renderer = super::test_support::offscreen_renderer_or_skip(
        "prepared_level_carries_its_dispatch_snapshot_and_timings",
    );
    let content_root = level.path().parent().unwrap().to_path_buf();
    let inputs =
        match app.capture_level_prep_inputs(level.path().to_path_buf(), content_root.clone()) {
            Some(mut inputs) => {
                assert_eq!(inputs.content_root, content_root);
                assert_eq!(
                    inputs.device_limits,
                    app.renderer.as_ref().unwrap().install_device_limits()
                );
                inputs.prm_cache_root = prm_root.path().to_path_buf();
                inputs
            }
            None => {
                assert!(
                    app.renderer.is_none(),
                    "a renderer and a session capture inputs"
                );
                inputs_for(level.path(), prm_root.path())
            }
        };
    let (limits, tiers, audio_enabled) = (inputs.device_limits, inputs.tiers, inputs.audio_enabled);
    let (_prep, outcome, _progress) = run_prep(&pool, inputs);
    let prepared = outcome.expect("the textured level prepares");
    let gpu = prepared
        .gpu
        .as_ref()
        .expect("a load that builds no set keeps its GPU products");
    assert_eq!(gpu.device_limits, limits);
    assert_eq!(gpu.tiers, tiers);
    assert_eq!(gpu.prm_cache_root, prm_root.path());
    let PreparedPresentation { sounds, models } = prepared.presentation;
    // The test App has no audio device; the fallback inputs have audio on,
    // and the fixture's content root has no sounds.
    assert_eq!(
        sounds.map(|sounds| sounds.file_count()),
        audio_enabled.then_some(0),
        "sounds decode exactly when the dispatch had audio"
    );
    assert!(
        models.is_empty(),
        "no descriptor or placement names a model"
    );

    assert!(prepared.timings.parse > std::time::Duration::ZERO);
    let timed: std::collections::BTreeSet<PrepKind> = prepared
        .timings
        .tasks
        .iter()
        .map(|(kind, _)| *kind)
        .collect();
    // Every kind ran as a task but the empty SH plan, credited on hand-out 1,
    // and the empty model parse, credited when the prediction came back empty.
    let ran: std::collections::BTreeSet<PrepKind> = PrepKind::ALL
        .into_iter()
        .filter(|kind| !matches!(kind, PrepKind::ShResidency | PrepKind::ModelParse))
        .filter(|kind| audio_enabled || *kind != PrepKind::Sounds)
        .collect();
    assert_eq!(timed, ran);
}

// A dispatch with no audio credits sound decode itself. When the parse and
// every task it spawns finish before the dispatching thread gets to that
// credit (a busy machine descheduling it right after it queued the parse),
// the credit must still complete the prep: it once landed after the last
// task's completion check and the prep never yielded.
#[test]
fn prep_without_audio_completes_when_the_parse_outruns_dispatch() {
    let (level, prm_root) = write_textured_level();
    let pool = test_pool();
    let mut inputs = inputs_for(level.path(), prm_root.path());
    inputs.audio_enabled = false;
    let ran_ahead = Arc::new(AtomicBool::new(false));
    {
        let ran_ahead = Arc::clone(&ran_ahead);
        inputs.hooks.after_parse_queued = Some(Arc::new(move |probe: &PrepProbe| {
            // Hold dispatch until the parse accepted and every task but the
            // dispatch-credited sound decode finished.
            let others_done = probe.wait_until(|state| {
                state.parse_accepted
                    && PrepKind::ALL
                        .iter()
                        .filter(|kind| **kind != PrepKind::Sounds)
                        .all(|kind| state.finished.contains(kind))
            });
            ran_ahead.store(others_done, Ordering::Release);
        }));
    }
    let (_prep, outcome, _progress) = run_prep(&pool, inputs);
    assert!(
        ran_ahead.load(Ordering::Acquire),
        "the parse and its tasks finished while dispatch was held"
    );
    let prepared = outcome.expect("the prep completes");
    assert!(
        prepared.presentation.sounds.is_none(),
        "no audio, no sounds"
    );
}
