//! Mover probe channel through the binary: the bounce half's per-pose memo. A `light_poses`
//! change keeps both termini and every mid pose it leaves in place, baking only the new poses;
//! moving a three-waypoint mover's middle waypoint changes its near-field domain, keeps its
//! termini and re-bakes its mid poses; built `high`, a `light_bounces` change re-bakes every pose
//! of that mover, while a count id 34 clamps to its own hits; an unedited warm build hits every
//! pose. Every warm PRL equals the edited map's `--no-cache` PRL of the same fidelity, byte for
//! byte.
//! See: context/lib/build_pipeline.md §Build Cache, §Compiler pipeline step 12 (Mover probe channel)

mod mover_cache_support;
mod prl_build_support;

use mover_cache_support::*;

/// The plate's six automatic poses, every one baked: the base build fills
/// an empty cache, so no pose memo is read.
fn assert_base_bakes_every_plate_pose() {
    assert_eq!(
        mover_memo(&base().1.logs, "plate"),
        "miss: termini miss; re-bakes pose(s) 0, 1, 2, 3, 4, 5 of 6",
        "{}",
        base().1.logs
    );
}

// Two cold builds are byte-identical
// (`the_section_is_identical_across_cold_builds_and_worker_counts`), and an
// unedited warm build reads every pose of every mover from the memo, re-bakes
// none and equals the `--no-cache` build byte for byte.
#[test]
fn an_unedited_warm_build_hits_every_pose_and_equals_cold() {
    assert_base_bakes_every_plate_pose();
    let warm = warm_equals_cold(&Scene::default());
    assert_eq!(bounce_share(&warm), (0, MOVERS), "{warm}");
    assert_eq!(pose_share(&warm), (0, 6 + 8), "{warm}");
    assert_eq!(mover_memo(&warm, "plate"), "hit (6 pose(s))", "{warm}");
    assert_eq!(mover_memo(&warm, "shutter"), "hit (8 pose(s))", "{warm}");
}

// The plate's `light_poses` goes from its automatic six (t 0,
// 0.2 … 1) to eleven (t 0, 0.1 … 1). Its termini hit, as do its old mid
// poses at t 0.2, 0.4, 0.6 and 0.8, which the new count keeps in place; only
// the five new mid poses bake. Skipping and the depth floor run after every
// read, so the hits are stored or skipped against their new neighbours, and
// the warm PRL equals the edited map's cold PRL byte for byte.
#[test]
fn a_light_poses_change_keeps_both_termini_and_bakes_only_the_new_poses() {
    assert_base_bakes_every_plate_pose();
    let warm = warm_equals_cold(&Scene {
        plate_poses: Some("11"),
        ..Scene::default()
    });
    assert_logged(&warm, "[cache] sh_base_volume hit");
    assert_eq!(
        mover_memo(&warm, "plate"),
        "miss: termini hit; re-bakes pose(s) 1, 3, 5, 7, 9 of 11",
        "{warm}"
    );
    assert_eq!(mover_memo(&warm, "shutter"), "hit (8 pose(s))", "{warm}");
    assert_eq!(pose_share(&warm), (5, 11 + 8), "{warm}");
    assert_eq!(bounce_share(&warm), (1, MOVERS), "{warm}");
    // At the interactive default, `low`: one posed bounce.
    assert_logged(&warm, "re-baked 1 posed bounce(s) at 5 of 11 pose(s)");
}

// The shutter's path gains a middle waypoint on its line, then
// that waypoint moves off the line with both ends where they were. The
// near-field domain, the swept brushes along the whole path, grows, so every
// mid pose re-bakes over the new domain; neither terminus gathers over the
// domain, so both hit. No state gathered over the old domain survives, and
// the warm PRL equals the edited map's cold PRL byte for byte.
#[test]
fn moving_a_middle_waypoint_changes_the_domain_keeps_the_termini_and_equals_cold() {
    let straight = Scene {
        shutter_middle: Some([136, 0, 124]),
        ..Scene::default()
    };
    let cache = tempfile::tempdir().expect("create middle-waypoint cache");
    let filled = build(&straight, CacheMode::Fill(cache.path()), JOBS);
    let bent = Scene {
        shutter_middle: Some([136, 64, 124]),
        ..Scene::default()
    };
    let warm = warm_over(&bent, cache.path());
    assert_logged(&warm, "[cache] sh_base_volume hit");
    let (before, after) = (
        near_field_probes(&filled.logs, "shutter"),
        near_field_probes(&warm, "shutter"),
    );
    assert!(
        after > before,
        "the bent path's domain holds more probes: {before} -> {after}"
    );
    let shutter = mover_memo(&warm, "shutter");
    assert!(
        shutter.starts_with("miss: termini hit; re-bakes pose(s) 1, "),
        "{shutter}\n{warm}"
    );
    assert!(bounce_memo_line(&warm, "plate", "hit"), "{warm}");
    assert_eq!(bounce_share(&warm), (1, MOVERS), "{warm}");
}

// Cache half: built `high`, setting the plate's `light_bounces` to 2
// re-bakes every one of its poses, termini included, and no other mover's;
// warm equals cold at `high`. A count past id 34's bakes id 34's, so it keys
// alike and every pose hits: the key folds the effective count.
#[test]
fn a_light_bounces_change_rebakes_every_pose_of_that_mover_and_warm_equals_cold() {
    let warm_high = |scene: &Scene| {
        let cache = tempfile::tempdir().expect("create warm cache");
        copy_dir(&high_base().0, cache.path());
        warm_over_with(scene, cache.path(), HIGH)
    };
    assert_eq!(
        mover_memo(&high_base().1.logs, "plate"),
        "miss: termini miss; re-bakes pose(s) 0, 1, 2, 3, 4, 5 of 6",
        "{}",
        high_base().1.logs
    );
    let warm = warm_high(&Scene {
        plate_bounces: Some("2"),
        ..Scene::default()
    });
    assert_eq!(
        mover_memo(&warm, "plate"),
        "miss: termini miss; re-bakes pose(s) 0, 1, 2, 3, 4, 5 of 6",
        "{warm}"
    );
    assert_eq!(mover_memo(&warm, "shutter"), "hit (8 pose(s))", "{warm}");
    assert_eq!(pose_share(&warm), (6, 6 + 8), "{warm}");

    let clamped = warm_high(&Scene {
        plate_bounces: Some("99"),
        ..Scene::default()
    });
    assert_eq!(bounce_share(&clamped), (0, MOVERS), "{clamped}");
}
