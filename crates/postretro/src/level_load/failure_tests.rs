//! One failure per load: every level-data check, the late parse verdict,
//! concurrent failures and a panicking task each end prep with exactly one
//! report naming its stage.
//! See: context/plans/done/level-load-pipeline/index.md (Failure) · research.md §9 (P10, P18)

use std::sync::Arc;
use std::sync::atomic::AtomicUsize;
use std::time::{Duration, Instant};

use postretro_level_format::cluster_directory::ClusterHintRecord;
use postretro_level_loader::{HandOut, HandOutStage, LevelWorld};
use postretro_render_data::geometry::BvhLeaf;

use super::test_support::{
    PrepTestHooks, inputs_for, no_prm_root, run_prep, test_pool, write_late_rejected_level,
    write_textured_level,
};
use super::*;

/// Which fixture a row loads.
#[derive(Debug, Clone, Copy)]
enum Fixture {
    Textured,
    StreamedSh,
    StreamedLightmap,
    LateRejected,
}

/// Run prep on `fixture` with `hooks` and return its one failure, checking
/// nothing else is ever reported.
fn only_failure(fixture: Fixture, hooks: PrepTestHooks) -> LevelLoadFailure {
    let pool = test_pool();
    let (_keep, path, prm_root): (
        Box<dyn std::any::Any>,
        std::path::PathBuf,
        std::path::PathBuf,
    ) = match fixture {
        Fixture::Textured => {
            let (level, prm) = write_textured_level();
            let path = level.path().to_path_buf();
            let root = prm.path().to_path_buf();
            (Box::new((level, prm)), path, root)
        }
        Fixture::StreamedSh => {
            let (dir, path) =
                crate::sh_streaming::sync_manifest_test_fixture::write_one_cluster_prl();
            (Box::new(dir), path, no_prm_root())
        }
        Fixture::StreamedLightmap => {
            let level = crate::lightmap_streaming::prl_test_fixture::StreamedLightmapPrl::write();
            let path = level.path().to_path_buf();
            (Box::new(level), path, no_prm_root())
        }
        Fixture::LateRejected => {
            let level = write_late_rejected_level();
            let path = level.path().to_path_buf();
            (Box::new(level), path, no_prm_root())
        }
    };
    let mut inputs = inputs_for(&path, &prm_root);
    inputs.hooks = hooks;
    let (mut prep, outcome, _progress) = run_prep(&pool, inputs);
    let failure = match outcome {
        Ok(_) => panic!("{fixture:?}: prep accepted a level a check rejects"),
        Err(failure) => failure,
    };
    // Nothing else is ever reported, however long the stragglers run.
    let deadline = Instant::now() + Duration::from_millis(200);
    while Instant::now() < deadline {
        assert!(prep.try_take().is_none(), "{fixture:?}: a second report");
        std::thread::sleep(Duration::from_millis(5));
    }
    let mut retirement = prep.cancel();
    let deadline = Instant::now() + super::test_support::HOLD_TIMEOUT;
    while !retirement.try_finish() {
        assert!(
            Instant::now() < deadline,
            "{fixture:?}: the group never retired"
        );
        std::thread::sleep(Duration::from_millis(1));
    }
    failure
}

fn bad_leaf(hand_out: &mut HandOut) {
    if let HandOut::Spatial(spatial) = hand_out {
        // The parse validated its own copy; prep's sees a leaf past the
        // index buffer, which the range check refuses.
        Arc::make_mut(spatial).bvh.leaves.push(BvhLeaf {
            aabb_min: [0.0; 3],
            material_bucket_id: 0,
            aabb_max: [1.0; 3],
            index_offset: 6,
            index_count: 3,
            cell_id: 0,
            chunk_range_start: 0,
            chunk_range_count: 0,
        });
    }
}

fn hints_disagree_with_manifest(world: &mut LevelWorld) {
    // A second, memberless cluster: the hints decode, but they no longer
    // match the manifest's one-cluster directory.
    let directory = world.cluster_directory.as_mut().expect("streamed id 49");
    let mut extra = directory.clusters[0].clone();
    extra.member_start = 0;
    extra.member_count = 0;
    directory.clusters.push(extra);
}

fn hint_names_missing_cluster(world: &mut LevelWorld) {
    let directory = world.cluster_directory.as_mut().expect("streamed id 49");
    directory.cluster_hints.push(ClusterHintRecord {
        cluster_id: 99,
        flags: 0,
        priority: 0,
    });
}

fn drop_cluster_directory(world: &mut LevelWorld) {
    world.cluster_directory = None;
}

fn shrink_residency_set(world: &mut LevelWorld) {
    // Blocks keep naming cells 3 and 4 past a three-cell set.
    let set = world.cell_residency_set.as_mut().expect("fixture id 51");
    set.offsets.truncate(4);
}

#[test]
fn level_data_check_failures_end_prep_once() {
    struct Row {
        name: &'static str,
        fixture: Fixture,
        hooks: PrepTestHooks,
        stage: LoadStage,
        message: &'static str,
    }
    let edit_world = |edit: fn(&mut LevelWorld)| PrepTestHooks {
        edit_world: Some(edit),
        ..Default::default()
    };
    let fail = |kind: PrepKind, message: &'static str| PrepTestHooks {
        fail: vec![(kind, message)],
        ..Default::default()
    };
    let rows = [
        Row {
            name: "range",
            fixture: Fixture::Textured,
            hooks: PrepTestHooks {
                edit_hand_out: Some(bad_leaf),
                ..Default::default()
            },
            stage: LoadStage::Prep(PrepKind::RangeCheck),
            message: "BVH leaf 1 index range [6..9) exceeds Geometry index count 6",
        },
        // `CollisionWorld::from_level` cannot fail on any mesh today; the
        // seam stands in for a parry refusal.
        Row {
            name: "collision",
            fixture: Fixture::Textured,
            hooks: fail(PrepKind::Collision, "static collision: injected"),
            stage: LoadStage::Prep(PrepKind::Collision),
            message: "static collision: injected",
        },
        // No fixture's manifest makes the plan refuse; the seam stands in.
        Row {
            name: "streamed-SH plan",
            fixture: Fixture::StreamedSh,
            hooks: fail(PrepKind::ShResidency, "injected plan refusal"),
            stage: LoadStage::Prep(PrepKind::ShResidency),
            message: "injected plan refusal",
        },
        Row {
            name: "topology",
            fixture: Fixture::StreamedSh,
            hooks: edit_world(hints_disagree_with_manifest),
            stage: LoadStage::Prep(PrepKind::StreamingData),
            message: "id-49 hints disagree with the manifest's directory",
        },
        Row {
            name: "id-49 decode",
            fixture: Fixture::StreamedSh,
            hooks: edit_world(hint_names_missing_cluster),
            stage: LoadStage::Prep(PrepKind::StreamingData),
            message: "cluster hint names an out-of-range cluster",
        },
        Row {
            name: "missing id 49",
            fixture: Fixture::StreamedSh,
            hooks: edit_world(drop_cluster_directory),
            stage: LoadStage::Prep(PrepKind::StreamingData),
            message: "a streamed level carries no id 49",
        },
        Row {
            name: "block map",
            fixture: Fixture::StreamedLightmap,
            hooks: edit_world(shrink_residency_set),
            stage: LoadStage::Prep(PrepKind::StreamingData),
            message: "past the 3-cell residency set",
        },
        Row {
            name: "late parse verdict",
            fixture: Fixture::LateRejected,
            hooks: PrepTestHooks::default(),
            stage: LoadStage::Parse,
            message: "BspNodes(id 12)",
        },
    ];
    for row in rows {
        let failure = only_failure(row.fixture, row.hooks);
        assert_eq!(failure.stage, row.stage, "{}: {failure}", row.name);
        assert!(
            failure.message.contains(row.message),
            "{}: `{}` does not name `{}`",
            row.name,
            failure.message,
            row.message
        );
        assert!(
            !failure.message.starts_with("panicked"),
            "{}: a check failed by panicking: {failure}",
            row.name
        );
        assert_eq!(
            failure.parse_error.is_some(),
            row.stage == LoadStage::Parse,
            "{}",
            row.name
        );
    }
}

// P18: hand-outs 1–4 went out and pre-verdict tasks ran on them before the
// parse rejected; the rejection is the one report.
#[test]
fn late_parse_rejection_after_hand_outs_is_the_one_failure() {
    let pool = test_pool();
    let level = write_late_rejected_level();
    let inputs = inputs_for(level.path(), &no_prm_root());
    let (prep, outcome, _progress) = run_prep(&pool, inputs);
    let failure = outcome.expect_err("the parse rejects the level");
    assert_eq!(failure.stage, LoadStage::Parse);
    let state = prep.probe().state();
    assert_eq!(state.last_hand_out, Some(HandOutStage::Entities));
    assert!(!state.parse_accepted);
    assert!(
        !state.started.contains(&PrepKind::Assemble),
        "nothing past the verdict ran"
    );
    drop(prep.cancel());
}

// P10: two tasks fail together; exactly one failure is reported, and the
// other is dropped.
#[test]
fn concurrent_prep_failures_report_once() {
    let failure = only_failure(
        Fixture::Textured,
        PrepTestHooks {
            fail: vec![
                (PrepKind::Collision, "collision refused"),
                (PrepKind::StreamingData, "streaming refused"),
            ],
            fail_together: Some(Arc::new(AtomicUsize::new(0))),
            ..Default::default()
        },
    );
    assert!(
        matches!(
            failure.stage,
            LoadStage::Prep(PrepKind::Collision | PrepKind::StreamingData)
        ),
        "{failure}"
    );

    // A pre-verdict task's failure and the parse's rejection together: the
    // rejection wins, since the task's input was never promised valid.
    let failure = only_failure(
        Fixture::LateRejected,
        PrepTestHooks {
            fail: vec![(PrepKind::RangeCheck, "range refused")],
            ..Default::default()
        },
    );
    assert_eq!(failure.stage, LoadStage::Parse, "{failure}");

    // The model prediction reads hand-out 4, which the parse may still
    // reject: its panic waits for the verdict, and the rejection wins.
    let failure = only_failure(
        Fixture::LateRejected,
        PrepTestHooks {
            panic: Some(PrepKind::ModelPrediction),
            ..Default::default()
        },
    );
    assert_eq!(failure.stage, LoadStage::Parse, "{failure}");
}

// Sound decode starts on hand-out 4, while the parse can still reject the
// level: its panic, observed before the verdict, is held like any
// pre-verdict task's, so the later rejection is the one report. The parse
// waits at hand-out 4 until the decode has run and panicked and the main
// thread's polls have seen it.
#[test]
fn sound_panic_before_verdict_yields_to_parse_rejection() {
    let failure = only_failure(
        Fixture::LateRejected,
        PrepTestHooks {
            panic: Some(PrepKind::Sounds),
            after_hand_out: Some(Arc::new(
                |stage: HandOutStage, probe: &super::test_support::PrepProbe| {
                    if stage != HandOutStage::Entities {
                        return;
                    }
                    assert!(
                        probe.wait_until(|state| state.started.contains(&PrepKind::Sounds)),
                        "the sound decode never started"
                    );
                    // The decode panics as it starts; wait for the main
                    // thread to route that panic into the held failure
                    // before the parse may reject.
                    assert!(
                        probe.wait_until(|state| state.held_failure),
                        "the sound decode's panic was never held for the verdict"
                    );
                    assert!(!probe.state().settled, "the panic settled the load early");
                },
            )),
            ..Default::default()
        },
    );
    assert_eq!(failure.stage, LoadStage::Parse, "{failure}");
    assert!(failure.parse_error.is_some(), "{failure}");
}

#[test]
fn panicking_prep_task_fails_the_load() {
    for kind in [
        PrepKind::Fluid,
        PrepKind::Textures,
        PrepKind::Assemble,
        PrepKind::Sounds,
        PrepKind::ModelPrediction,
    ] {
        let failure = only_failure(
            Fixture::Textured,
            PrepTestHooks {
                panic: Some(kind),
                ..Default::default()
            },
        );
        assert_eq!(failure.stage, LoadStage::Prep(kind), "{failure}");
        assert!(
            failure.message.starts_with("panicked: injected panic"),
            "{failure}"
        );
    }
}
