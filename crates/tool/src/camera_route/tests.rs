//! `generate-camera-route`: deterministic files, header-only differences
//! across paths and copies, and every refusal before a write.
//!
//! See: context/lib/testing_guide.md

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use glam::Vec3;
use postretro_camera_route::{
    DEFAULT_ASPECT, GeneratedKind, Smoothing, WALK_SEED, WalkKind, bake_token, exact_route,
    parse_pose_file, pose_fingerprint,
};
use postretro_level_loader::{CellData, CellLocatorChild, CellLocatorNodeData, LevelWorld};

use super::args::{GenerateArgs, parse_args};
use super::{generate, run};

/// The checked-in 20 KB fixture: portals and an id-51 set. Read from the
/// variable cargo sets at run time, never compiled in (`main.rs` tests).
fn fixture() -> PathBuf {
    PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").expect("cargo sets CARGO_MANIFEST_DIR"))
        .join("../../content/dev/maps/test-fixtures/sh-streaming-hinted-door.prl")
}

fn os(args: &[&str]) -> Vec<OsString> {
    args.iter().map(OsString::from).collect()
}

fn generate_to(level: &Path, out: &Path, extra: &[&str]) -> Result<String, String> {
    let mut args = vec![
        level.to_str().expect("utf-8 path").to_string(),
        "-o".to_string(),
        out.to_str().expect("utf-8 path").to_string(),
    ];
    args.extend(extra.iter().map(|arg| arg.to_string()));
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    assert_eq!(run(os(&args))?, 0);
    Ok(std::fs::read_to_string(out).expect("the verb wrote its file"))
}

/// Split a file into its bake line and every other line.
fn split_bake(text: &str) -> (String, Vec<String>) {
    let bake = text
        .lines()
        .find(|line| line.starts_with("bake "))
        .expect("bake line")
        .to_string();
    let rest = text
        .lines()
        .filter(|line| !line.starts_with("bake "))
        .map(str::to_string)
        .collect();
    (bake, rest)
}

#[test]
fn generate_camera_route_writes_identical_bytes_on_two_runs() {
    let dir = tempfile::tempdir().expect("tempdir");
    for kind in ["far-point", "random"] {
        let extra = [
            "--kind", kind, "--steps", "120", "--seed", "0x2a", "--pitch", "-7.5",
        ];
        let a = generate_to(&fixture(), &dir.path().join("a.route"), &extra).expect("first run");
        let b = generate_to(&fixture(), &dir.path().join("b.route"), &extra).expect("second run");
        assert_eq!(a, b, "{kind}");
        let parsed = parse_pose_file(&a).expect("a valid pose file");
        assert_eq!(parsed.header.param("seed"), Some("0x2a"));
        assert_eq!(parsed.header.param("steps"), Some("120"));
        assert_eq!(parsed.header.param("pitch"), Some("-7.5"));
        assert_eq!(parsed.header.param("hfov"), Some("100"));
        assert!(a.starts_with(&format!(
            "postretro-camera-route 1\norigin generated {kind}\n"
        )));
    }
}

#[test]
fn generate_camera_route_from_another_path_or_a_copy_differs_only_in_the_bake_line() {
    let dir = tempfile::tempdir().expect("tempdir");

    // The same file spelled another way.
    let respelled = fixture()
        .parent()
        .expect("fixture directory")
        .join("../test-fixtures/sh-streaming-hinted-door.prl");

    // A copy with another modification time.
    let copy = dir.path().join("copy.prl");
    std::fs::copy(fixture(), &copy).expect("copy the fixture");
    std::fs::File::options()
        .write(true)
        .open(&copy)
        .and_then(|file| {
            file.set_modified(std::time::UNIX_EPOCH + std::time::Duration::from_secs(1_000_000))
        })
        .expect("set the copy's mtime");

    for extra in [
        &["--kind", "far-point", "--steps", "80"][..],
        &["--kind", "smoothed", "--steps", "40"][..],
    ] {
        let original = generate_to(&fixture(), &dir.path().join("a.route"), extra).expect("run");
        let other_path = generate_to(&respelled, &dir.path().join("b.route"), extra).expect("run");
        let copied = generate_to(&copy, &dir.path().join("c.route"), extra).expect("run");

        let (bake, rest) = split_bake(&original);
        for (label, text, differs) in [
            ("another path", &other_path, &["prl"][..]),
            ("a copy", &copied, &["mtime", "prl"][..]),
        ] {
            let (other_bake, other_rest) = split_bake(text);
            assert_eq!(
                rest, other_rest,
                "{extra:?}, {label}: every line but the bake line is identical"
            );
            for key in ["id51", "walk_epoch", "size", "mtime", "prl"] {
                let same = bake_token(&bake, key) == bake_token(&other_bake, key);
                assert_eq!(
                    !same,
                    differs.contains(&key),
                    "{extra:?}, {label}: bake token {key}"
                );
            }
        }
    }
}

/// Every consecutive pair across no cut moves at most `speed` / 60 m and
/// turns at most `turn_cap` / 60 degrees, the short way.
fn assert_within_limits(file: &postretro_camera_route::PoseFile, speed: f32, turn_cap: f32) {
    let stride = speed / 60.0;
    let turn = turn_cap.to_radians() / 60.0;
    let wrap = |angle: f32| {
        use std::f32::consts::{PI, TAU};
        angle - TAU * ((angle + PI) / TAU).floor()
    };
    let (mut moved_max, mut turned_max) = (0.0f32, 0.0f32);
    for pair in file.poses.windows(2) {
        if pair[1].cut {
            continue;
        }
        moved_max = moved_max.max(pair[0].eye.distance(pair[1].eye));
        turned_max = turned_max.max(wrap(pair[1].yaw - pair[0].yaw).abs());
    }
    assert!(moved_max <= stride + 1e-4, "{moved_max} m over {stride}");
    assert!(turned_max <= turn + 1e-5, "{turned_max} rad over {turn}");
    assert!(moved_max > stride - 1e-4, "the route reaches its speed");
}

#[test]
fn generate_camera_route_smoothed_is_deterministic_keeps_its_limits_and_names_them() {
    let dir = tempfile::tempdir().expect("tempdir");
    for (extra, walk, speed, turn_cap) in [
        (
            &["--kind", "smoothed", "--steps", "60"][..],
            "far-point",
            8.0,
            270.0,
        ),
        (
            &[
                "--kind",
                "smoothed",
                "--steps",
                "60",
                "--walk",
                "random",
                "--speed",
                "5.5",
                "--turn-cap",
                "120",
                "--pitch",
                "-10",
            ][..],
            "random",
            5.5,
            120.0,
        ),
    ] {
        let a = generate_to(&fixture(), &dir.path().join("a.route"), extra).expect("first run");
        let b = generate_to(&fixture(), &dir.path().join("b.route"), extra).expect("second run");
        assert_eq!(a, b, "{extra:?}");
        assert!(a.starts_with("postretro-camera-route 1\norigin generated smoothed\n"));
        assert!(!a.contains("\nspawn "), "a smoothed file has no walk spawn");
        let file = parse_pose_file(&a).expect("a valid pose file");
        let keys: Vec<&str> = file.header.params.iter().map(|(k, _)| k.as_str()).collect();
        assert_eq!(
            keys,
            [
                "walk", "seed", "steps", "pitch", "hfov", "speed", "turn_cap"
            ]
        );
        assert_eq!(file.header.param("walk"), Some(walk));
        assert_eq!(file.header.param("speed"), Some(speed.to_string().as_str()));
        assert_eq!(
            file.header.param("turn_cap"),
            Some(turn_cap.to_string().as_str())
        );
        assert!(file.poses.iter().all(|pose| pose.walk.is_some()));
        assert_within_limits(&file, speed, turn_cap);
    }
}

#[test]
fn generate_camera_route_defaults_reproduce_the_walk_frames_and_projection() {
    let parsed =
        parse_args(os(&["level.prl", "--kind", "random", "-o", "out.route"])).expect("args");
    assert_eq!(
        parsed,
        GenerateArgs {
            level: "level.prl".into(),
            kind: GeneratedKind::Random,
            walk: WalkKind::Random,
            smoothing: None,
            seed: WALK_SEED,
            steps: 2_000,
            pitch_degrees: 0.0,
            hfov_degrees: 100.0,
            out: PathBuf::from("out.route"),
        }
    );
    // A smoothed route follows the far-point walk at 8 m/s and 270°/s
    // unless told otherwise.
    let smoothed = parse_args(os(&["level.prl", "--kind", "smoothed", "-o", "o"])).expect("args");
    assert_eq!(
        (smoothed.kind, smoothed.walk, smoothed.smoothing),
        (
            GeneratedKind::Smoothed,
            WalkKind::FarPointTour,
            Some(Smoothing {
                speed: 8.0,
                turn_cap_degrees: 270.0
            })
        )
    );

    let level = fixture().to_str().expect("utf-8").to_string();
    let world = postretro_level_loader::load_prl(&level).expect("load the fixture");
    let args = GenerateArgs {
        level,
        steps: 150,
        ..parsed
    };
    let file = generate(&world, &args).expect("generate");
    assert_eq!(file.header.param("seed"), Some("0x5eedb10c"));
    assert_eq!(file.header.param("pitch"), Some("0"));
    assert_eq!(
        file.header.projection.aspect.to_bits(),
        DEFAULT_ASPECT.to_bits()
    );
    assert_eq!(
        file.header.projection.aspect.to_bits(),
        (16.0f32 / 9.0f32).to_bits()
    );
    let walk = exact_route(&world, WalkKind::Random, 150, 0.0).expect("walk");
    let print = |pose: &postretro_camera_route::RoutePose, projection| {
        let (step, cell) = pose.walk.expect("walk-made pose");
        pose_fingerprint(
            step as usize,
            cell,
            pose.eye,
            pose.yaw,
            pose.pitch,
            projection,
        )
    };
    let projection = (
        file.header.projection.hfov_degrees,
        file.header.projection.aspect,
    );
    assert_eq!(file.poses.len(), walk.poses.len());
    assert!(
        file.poses
            .iter()
            .zip(&walk.poses)
            .all(|(a, b)| print(a, projection) == print(b, (100.0, 16.0 / 9.0)))
    );
}

#[test]
fn generate_camera_route_refuses_bad_parameters_naming_them() {
    let smoothed = |flag: &'static str, value: &'static str| {
        ["l.prl", "--kind", "smoothed", flag, value, "-o", "o"]
    };
    let mut cases: Vec<(Vec<&str>, &str)> = Vec::new();
    for value in ["0", "-1", "-0", "NaN", "inf", "fast"] {
        cases.push((smoothed("--speed", value).to_vec(), "--speed"));
        cases.push((smoothed("--turn-cap", value).to_vec(), "--turn-cap"));
    }
    cases.push((smoothed("--walk", "spiral").to_vec(), "--walk"));
    for (flag, value) in [("--walk", "random"), ("--speed", "8"), ("--turn-cap", "90")] {
        for kind in ["far-point", "random"] {
            let args = vec!["l.prl", "--kind", kind, flag, value, "-o", "o"];
            cases.push((args, flag));
        }
    }
    for (args, named) in &cases {
        let error = parse_args(os(args)).expect_err("refused");
        assert!(error.contains(named), "{args:?}: {error}");
    }

    for (args, named) in [
        (
            &["l.prl", "--kind", "spiral", "-o", "o"][..],
            "far-point, random, smoothed",
        ),
        (&["l.prl", "--kind", "spiral", "-o", "o"][..], "--kind"),
        (&["l.prl", "-o", "o"][..], "--kind"),
        (&["l.prl", "--kind", "random"][..], "-o"),
        (&["--kind", "random", "-o", "o"][..], "level path"),
        (
            &["l.prl", "--kind", "random", "--steps", "0", "-o", "o"][..],
            "--steps",
        ),
        (
            &["l.prl", "--kind", "random", "--seed", "0xZZ", "-o", "o"][..],
            "--seed",
        ),
        (
            &["l.prl", "--kind", "random", "--hfov", "59.9", "-o", "o"][..],
            "--hfov",
        ),
        (
            &["l.prl", "--kind", "random", "--hfov", "131", "-o", "o"][..],
            "--hfov",
        ),
        (
            &["l.prl", "--kind", "random", "--hfov", "NaN", "-o", "o"][..],
            "--hfov",
        ),
        (
            &["l.prl", "--kind", "random", "--pitch", "90", "-o", "o"][..],
            "--pitch",
        ),
        (
            &["l.prl", "--kind", "random", "--speed", "8", "-o", "o"][..],
            "--speed",
        ),
    ] {
        let error = parse_args(os(args)).expect_err("refused");
        assert!(error.contains(named), "{args:?}: {error}");
    }
    assert_eq!(
        parse_args(os(&[
            "l.prl",
            "--kind=far-point",
            "--seed=12",
            "--hfov=60",
            "--out=o"
        ]))
        .map(|a| (a.kind, a.walk, a.seed, a.hfov_degrees)),
        Ok((GeneratedKind::FarPoint, WalkKind::FarPointTour, 12, 60.0))
    );
}

#[test]
fn generate_camera_route_refuses_a_non_positive_speed_or_turn_cap_before_reading_or_writing() {
    let dir = tempfile::tempdir().expect("tempdir");
    let out = dir.path().join("out.route");
    for (flag, value) in [("--speed", "0"), ("--turn-cap", "-90"), ("--speed", "NaN")] {
        let extra = ["--kind", "smoothed", flag, value];
        // A level that cannot be read: the parameter is refused before it.
        let missing = dir.path().join("missing.prl");
        for level in [fixture(), missing] {
            let error = generate_to(&level, &out, &extra).expect_err("refused");
            assert!(error.contains(flag), "{error}");
            assert!(!out.exists(), "nothing written");
        }
    }
}

#[test]
fn generate_camera_route_refuses_an_unreadable_level_before_writing() {
    let dir = tempfile::tempdir().expect("tempdir");
    let garbage = dir.path().join("garbage.prl");
    std::fs::write(&garbage, b"not a level").expect("write garbage");
    for level in [dir.path().join("missing.prl"), garbage] {
        let out = dir.path().join("out.route");
        let error = generate_to(&level, &out, &["--kind", "random"]).expect_err("refused");
        assert!(error.contains(level.to_str().unwrap()), "{error}");
        assert!(!out.exists(), "nothing written for {}", level.display());
    }
}

/// Two open cells; `portal` joins them or not.
fn two_cell_world(solid: bool) -> LevelWorld {
    let cell = |x: f32| CellData {
        bounds_min: Vec3::new(x, 0.0, 0.0),
        bounds_max: Vec3::new(x + 4.0, 4.0, 4.0),
        face_start: 0,
        face_count: 0,
        portal_ref_start: 0,
        portal_ref_count: 0,
        is_solid: solid,
        is_exterior: false,
        is_drawable: !solid,
    };
    let node = CellLocatorNodeData {
        plane_normal: Vec3::X,
        plane_distance: 4.0,
        front: CellLocatorChild::Cell(1),
        back: CellLocatorChild::Cell(0),
    };
    LevelWorld::new_visibility_only(
        vec![cell(0.0), cell(4.0)],
        Vec::new(),
        CellLocatorChild::Node(0),
        vec![node],
        Vec::new(),
        false,
    )
    .expect("valid world")
}

#[test]
fn generate_camera_route_refuses_a_level_without_camera_cells_or_portals_naming_it() {
    let args = parse_args(os(&["maps/empty.prl", "--kind", "far-point", "-o", "o"])).expect("args");
    for (world, why) in [
        (two_cell_world(true), "no camera cells"),
        (two_cell_world(false), "no portal between camera cells"),
    ] {
        let error = generate(&world, &args).expect_err("refused");
        assert!(
            error.contains("maps/empty.prl") && error.contains(why),
            "{error}"
        );
    }
}

#[test]
fn generate_camera_route_pitch_bound_is_inclusive_like_the_engine_camera() {
    let pitch = |value: &str| {
        parse_args(os(&[
            "l.prl", "--kind", "random", "--pitch", value, "-o", "o",
        ]))
        .map(|args| args.pitch_degrees)
    };
    assert_eq!(pitch("89"), Ok(89.0));
    assert_eq!(pitch("-89"), Ok(-89.0));
    for value in ["89.001", "-89.5", "90", "NaN"] {
        let error = pitch(value).expect_err("refused");
        assert!(error.contains("--pitch"), "{value}: {error}");
    }
}

#[test]
fn generate_camera_route_usage_states_ranges_defaults_and_the_smoothed_only_flags() {
    let usage = parse_args(os(&["--bogus"])).expect_err("refused");
    for needed in [
        "0x",
        "-89 to 89",
        "60 to 130",
        "default 2000",
        "default 0",
        "default 100",
        "default 8",
        "default 270",
        "--out",
        "apply only to --kind smoothed",
    ] {
        assert!(usage.contains(needed), "usage lacks {needed:?}: {usage}");
    }
}

#[test]
fn generate_camera_route_names_the_right_cause_for_each_route_error() {
    let world = two_cell_world(false);
    let named = |extra: &[&str]| {
        let mut args = vec!["maps/x.prl", "-o", "o"];
        args.extend_from_slice(extra);
        generate(&world, &parse_args(os(&args)).expect("args")).expect_err("refused")
    };
    // A level problem keeps the level hint; the smoothed-only limits do not.
    let level = named(&["--kind", "far-point"]);
    assert!(level.contains("camera cells with a portal"), "{level}");

    let args = GenerateArgs {
        smoothing: Some(Smoothing {
            speed: 0.0,
            turn_cap_degrees: 270.0,
        }),
        ..parse_args(os(&["maps/x.prl", "--kind", "smoothed", "-o", "o"])).expect("args")
    };
    let error = generate(&two_cell_world(false), &args).expect_err("refused");
    assert!(error.contains("speed"), "{error}");
    assert!(!error.contains("camera cells with a portal"), "{error}");
}

/// Two camera cells with a portal between them, whose locator answers each
/// cell's own box with the other cell: no eye can stand in any cell.
fn unlocatable_world() -> LevelWorld {
    use postretro_level_loader::PortalData;
    let cell = |x: f32, refs_at: u32| CellData {
        bounds_min: Vec3::new(x, 0.0, 0.0),
        bounds_max: Vec3::new(x + 4.0, 4.0, 4.0),
        face_start: 0,
        face_count: 0,
        portal_ref_start: refs_at,
        portal_ref_count: 1,
        is_solid: false,
        is_exterior: false,
        is_drawable: true,
    };
    let node = CellLocatorNodeData {
        plane_normal: Vec3::X,
        plane_distance: 4.0,
        front: CellLocatorChild::Cell(0),
        back: CellLocatorChild::Cell(1),
    };
    let portal = PortalData {
        polygon: vec![
            Vec3::new(4.0, 0.0, 0.0),
            Vec3::new(4.0, 4.0, 0.0),
            Vec3::new(4.0, 4.0, 4.0),
        ],
        front_cell: 0,
        back_cell: 1,
    };
    LevelWorld::new_visibility_only(
        vec![cell(0.0, 0), cell(4.0, 1)],
        vec![0, 0],
        CellLocatorChild::Node(0),
        vec![node],
        vec![portal],
        true,
    )
    .expect("valid world")
}

#[test]
fn generate_camera_route_refuses_a_smoothed_route_with_no_poses_naming_level_and_parameters() {
    let args = parse_args(os(&[
        "maps/unlocatable.prl",
        "--kind",
        "smoothed",
        "--steps",
        "30",
        "--speed",
        "6",
        "--turn-cap",
        "90",
        "-o",
        "o",
    ]))
    .expect("args");
    let error = generate(&unlocatable_world(), &args).expect_err("refused");
    for needed in [
        "maps/unlocatable.prl",
        "no poses",
        "30 steps",
        "speed 6",
        "turn cap 90",
        "0x5eedb10c",
    ] {
        assert!(error.contains(needed), "{error} lacks {needed:?}");
    }
}
