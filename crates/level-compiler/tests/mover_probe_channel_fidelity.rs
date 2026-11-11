//! Mover probe channel through the binary: bake fidelity. `low` caps every mover's
//! `light_bounces` at 1, `high` bakes each mover's own, and `--release` is `high` on an empty
//! cache. The mode comes from the flags, never the cache: `--no-cache` and a cache directory that
//! cannot be created both bake `low`, and `--release` beside `--bake-fidelity` fails before any
//! stage. Per fidelity, with no higher entries cached, warm equals cold. A `low` build serves
//! cached `high` poses before baking its own, and the Build Summary says which movers show less
//! than `high`.
//! See: context/lib/build_pipeline.md §Build Cache (Bake fidelity)

mod mover_cache_support;
mod prl_build_support;

use std::sync::OnceLock;

use mover_cache_support::*;
use postretro_level_format::SectionId;
use postretro_level_format::mover_probe_channel::MoverProbeChannelSection;

/// The default map built cold at `low`, the interactive default.
fn low_cold() -> &'static Built {
    static BUILT: OnceLock<Built> = OnceLock::new();
    BUILT.get_or_init(|| build(&Scene::default(), CacheMode::Cold, JOBS))
}

/// The default map built `--release`.
fn release() -> &'static Built {
    static BUILT: OnceLock<Built> = OnceLock::new();
    BUILT.get_or_init(|| build_with(&Scene::default(), CacheMode::Cold, JOBS, &["--release"]))
}

fn channel(built: &Built) -> MoverProbeChannelSection {
    let bytes = section(&built.prl, SectionId::MoverProbeChannel).expect("id 57 present");
    MoverProbeChannelSection::from_bytes(&bytes).expect("id 57 decodes")
}

/// The mover id the Build Summary's fidelity line names for `mover`.
fn mover_id(logs: &str, mover: &str) -> u32 {
    let prefix = format!("  kinematic_mover '{mover}' (#");
    let line = logs
        .lines()
        .find(|line| line.starts_with(&prefix))
        .unwrap_or_else(|| panic!("a fidelity summary line for {mover}\n{logs}"));
    let id = &line[prefix.len()..];
    id[..id.find(')').expect("the id's end")]
        .parse()
        .expect("a mover id")
}

/// The Build Summary's fidelity line for `mover`, after its id.
fn summary(logs: &str, mover: &str) -> String {
    let line = logs
        .lines()
        .find(|line| line.starts_with(&format!("  kinematic_mover '{mover}' (#")))
        .unwrap_or_else(|| panic!("a fidelity summary line for {mover}\n{logs}"));
    line[line.find("): ").expect("the id's end") + 3..].to_string()
}

// AC 20: `low` bakes every mover as its `light_bounces` capped at 1 (the
// default map at `low` packs exactly as the map with both movers at
// `light_bounces 1` at `high`); `high` bakes each mover's own (the default
// map's id 57 differs between them); `--release` equals an interactive
// `high` build on an empty cache, byte for byte.
#[test]
fn low_caps_every_mover_at_one_bounce_high_bakes_its_own_and_release_is_high_on_an_empty_cache() {
    let capped = Scene {
        plate_bounces: Some("1"),
        shutter_bounces: Some("1"),
        ..Scene::default()
    };
    let capped_high = build_with(&capped, CacheMode::Cold, JOBS, HIGH);
    assert_same_prl(
        low_cold(),
        &capped_high,
        "low equals every mover's light_bounces capped at 1",
    );

    let empty = tempfile::tempdir().expect("create empty cache");
    let high_on_empty = build_with(&Scene::default(), CacheMode::Fill(empty.path()), JOBS, HIGH);
    assert_same_prl(
        release(),
        &high_on_empty,
        "--release equals high on an empty cache",
    );
    assert_ne!(
        section(&low_cold().prl, SectionId::MoverProbeChannel),
        section(&release().prl, SectionId::MoverProbeChannel),
        "high bakes each mover's own light_bounces, past 1"
    );

    let low = &low_cold().logs;
    assert_logged(
        low,
        "Mover bounce fidelity: low build; 2 of 2 mover(s) approximate",
    );
    assert!(
        summary(low, "plate").starts_with("6 pose(s), low, approximate: 6 of 6 pose(s) at 1 of 5"),
        "{low}"
    );
    let released = &release().logs;
    assert_logged(
        released,
        "Mover bounce fidelity: high (--release) build; 0 of 2 mover(s) approximate",
    );
    assert!(
        summary(released, "plate").starts_with("6 pose(s), high;"),
        "{released}"
    );
    // The release build baked id 34 itself, so it projects from its own time.
    assert!(
        summary(released, "plate").contains("--release bounce bake est. "),
        "{released}"
    );
    assert!(!summary(released, "plate").contains('—'), "{released}");
    // AC 29: the projection adds each mid pose's share of the static patches,
    // its domain's, counted alike with no memo (`--release`) and with one.
    let share = |logs: &str| {
        let line = summary(logs, "plate");
        let from = line.find("(2 termini + ").expect("a mid-pose term");
        let to = line.find(" of static patches)").expect("the share's end");
        line[from..to].to_string()
    };
    assert!(
        share(released).starts_with("(2 termini + 4 mid pose(s) × "),
        "{released}"
    );
    assert_eq!(share(released), share(&high_on_empty.logs));
    // A `light_bounces 1` mover shows what `high` shows, at either fidelity.
    assert!(summary(&capped_high.logs, "plate").starts_with("6 pose(s), high;"));
}

// AC 20: `light_bounces 0` stores no bounce entry at either fidelity.
#[test]
fn light_bounces_zero_stores_no_bounce_at_either_fidelity() {
    let dark = Scene {
        plate_bounces: Some("0"),
        ..Scene::default()
    };
    for flags in [&[][..], HIGH] {
        let built = build_with(&dark, CacheMode::Cold, JOBS, flags);
        let plate = mover_id(&built.logs, "plate");
        let channel = channel(&built);
        assert!(
            channel
                .bounce_entries
                .iter()
                .all(|entry| entry.mover_id != plate),
            "{flags:?}: the plate stores no bounce"
        );
        assert!(
            channel
                .bounce_entries
                .iter()
                .any(|entry| entry.mover_id != plate),
            "{flags:?}: the shutter still does"
        );
        assert!(
            summary(&built.logs, "plate").contains("none (light_bounces 0)"),
            "{}",
            built.logs
        );
    }
}

// AC 21: `--release` beside `--bake-fidelity` fails before any stage,
// naming both flags.
#[test]
fn release_beside_bake_fidelity_fails_before_any_stage_naming_both_flags() {
    let dir = tempfile::tempdir().expect("create build directory");
    let input = dir.path().join("fidelity.map");
    std::fs::write(&input, map(&Scene::default())).expect("write fixture map");
    for value in ["low", "high"] {
        let output = prl_build_support::prl_build()
            .env("RUST_LOG", "info")
            .arg(&input)
            .args(["--no-tui", "--release", "--bake-fidelity", value])
            .output()
            .expect("spawn prl-build");
        let (stdout, stderr) = (
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr),
        );
        assert!(
            !output.status.success(),
            "{value}: refused\n{stdout}{stderr}"
        );
        assert!(
            stderr.contains("--release") && stderr.contains("--bake-fidelity"),
            "{value}: {stderr}"
        );
        assert!(
            !stdout.contains("Parsing") && !stderr.contains("Parsing"),
            "{value}: no stage ran\n{stdout}{stderr}"
        );
        assert!(!dir.path().join("fidelity.prl").exists());
    }
}

// AC 21: a cache directory that cannot be created leaves the build with no
// cache, as `--release` and `--no-cache` do, yet it bakes `low`: the mode
// comes from the flags. It packs exactly as the `--no-cache` build.
#[test]
fn a_cache_directory_that_cannot_be_created_bakes_low() {
    let dir = tempfile::tempdir().expect("create blocker directory");
    let blocker = dir.path().join("a-file");
    std::fs::write(&blocker, b"not a directory").expect("write blocker");
    let built = build(
        &Scene::default(),
        CacheMode::Fill(&blocker.join("prl-cache")),
        JOBS,
    );
    assert_logged(&built.logs, "cache disabled: failed to create");
    assert_logged(
        &built.logs,
        "Mover bounce fidelity: low build; 2 of 2 mover(s) approximate",
    );
    assert_same_prl(
        low_cold(),
        &built,
        "an uncreatable cache bakes as --no-cache",
    );
}

// AC 23: per fidelity, with no higher entries cached, a warm build equals a
// `--no-cache` build byte for byte; `low` fills and reads its own entries,
// `high` its own. Two `low` builds with the same inputs and the same cache,
// that cache holding `high` entries, are byte-identical.
#[test]
fn per_fidelity_warm_equals_no_cache_and_two_low_builds_over_one_cache_are_alike() {
    for flags in [&[][..], HIGH] {
        let cache = tempfile::tempdir().expect("create cache");
        build_with(
            &Scene::default(),
            CacheMode::Fill(cache.path()),
            JOBS,
            flags,
        );
        let warm = warm_over_with(&Scene::default(), cache.path(), flags);
        assert_eq!(pose_share(&warm), (0, 6 + 3), "{flags:?}\n{warm}");
    }

    let edited = Scene {
        plate_poses: Some("11"),
        ..Scene::default()
    };
    let builds: Vec<Built> = (0..2)
        .map(|_| {
            let cache = tempfile::tempdir().expect("create cache copy");
            copy_dir(high_base().0.path(), cache.path());
            build(&edited, CacheMode::Warm(cache.path()), JOBS)
        })
        .collect();
    assert_same_prl(&builds[0], &builds[1], "two low builds over one cache");
}

// AC 24 and AC 27's low half. After a `high` build, a `low` build serves
// `high` states for every pose it can: the unchanged shutter wholly, and for
// the plate after a `light_poses` edit its termini and the mid poses the edit
// keeps, baking only the new ones `low`; a mover edited all over bakes `low`.
// A `high` build after a `low` one re-bakes every pose at `high` and equals
// `--release` byte for byte.
#[test]
fn low_after_high_serves_high_and_high_after_low_rebakes_every_pose_and_equals_release() {
    let warm_low_over_high = |scene: &Scene| {
        let cache = tempfile::tempdir().expect("create cache copy");
        copy_dir(high_base().0.path(), cache.path());
        build(scene, CacheMode::Warm(cache.path()), JOBS).logs
    };

    let unchanged = warm_low_over_high(&Scene::default());
    assert_eq!(pose_share(&unchanged), (0, 6 + 3), "{unchanged}");
    assert_eq!(
        mover_memo(&unchanged, "shutter"),
        "hit (3 pose(s), 3 served from high)",
        "{unchanged}"
    );
    assert_logged(
        &unchanged,
        "Mover bounce fidelity: low build; 0 of 2 mover(s) approximate",
    );
    assert!(
        summary(&unchanged, "plate").starts_with("6 pose(s), high;"),
        "{unchanged}"
    );

    let more_poses = warm_low_over_high(&Scene {
        plate_poses: Some("11"),
        ..Scene::default()
    });
    assert_eq!(
        mover_memo(&more_poses, "plate"),
        "miss: termini hit; re-bakes pose(s) 1, 3, 5, 7, 9 of 11, 6 served from high",
        "{more_poses}"
    );
    assert_eq!(
        mover_memo(&more_poses, "shutter"),
        "hit (3 pose(s), 3 served from high)",
        "{more_poses}"
    );
    assert_logged(&more_poses, "re-baked 1 posed bounce(s) at 5 of 11 pose(s)");
    assert!(
        summary(&more_poses, "plate")
            .starts_with("11 pose(s), low, approximate: 5 of 11 pose(s) at 1 of 5"),
        "{more_poses}"
    );
    assert!(
        summary(&more_poses, "shutter").starts_with("3 pose(s), high;"),
        "{more_poses}"
    );

    // Moving the plate's last waypoint keeps its first terminus, which the
    // `high` entry serves; every other pose bakes `low`.
    let moved = warm_low_over_high(&Scene {
        plate_last: [0, 160, 104],
        ..Scene::default()
    });
    assert_eq!(
        mover_memo(&moved, "plate"),
        "miss: first terminus hit, last miss; re-bakes pose(s) 1, 2, 3, 4, 5 of 6, 1 served \
         from high",
        "{moved}"
    );
    assert!(
        summary(&moved, "plate")
            .starts_with("6 pose(s), low, approximate: 5 of 6 pose(s) at 1 of 5"),
        "{moved}"
    );

    let cache = tempfile::tempdir().expect("create low cache");
    copy_dir(base().0.path(), cache.path());
    let high_after_low = build_with(&Scene::default(), CacheMode::Warm(cache.path()), JOBS, HIGH);
    assert_eq!(
        pose_share(&high_after_low.logs),
        (6 + 3, 6 + 3),
        "{}",
        high_after_low.logs
    );
    assert_same_prl(
        release(),
        &high_after_low,
        "high after low equals --release",
    );
}
