//! `generate-camera-route`: deterministic files, header-only differences
//! across paths and copies, and every refusal before a write.
//!
//! See: context/lib/testing_guide.md

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use glam::Vec3;
use postretro_camera_route::{
    DEFAULT_ASPECT, WALK_SEED, WalkKind, bake_token, exact_route, parse_pose_file, pose_fingerprint,
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
    let extra = ["--kind", "far-point", "--steps", "80"];
    let original = generate_to(&fixture(), &dir.path().join("a.route"), &extra).expect("run");

    // The same file spelled another way.
    let respelled = fixture()
        .parent()
        .expect("fixture directory")
        .join("../test-fixtures/sh-streaming-hinted-door.prl");
    let other_path = generate_to(&respelled, &dir.path().join("b.route"), &extra).expect("run");

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
    let copied = generate_to(&copy, &dir.path().join("c.route"), &extra).expect("run");

    let (bake, rest) = split_bake(&original);
    for (label, text, differs) in [
        ("another path", &other_path, &["prl"][..]),
        ("a copy", &copied, &["mtime", "prl"][..]),
    ] {
        let (other_bake, other_rest) = split_bake(text);
        assert_eq!(
            rest, other_rest,
            "{label}: every line but the bake line is identical"
        );
        for key in ["id51", "walk_epoch", "size", "mtime", "prl"] {
            let same = bake_token(&bake, key) == bake_token(&other_bake, key);
            assert_eq!(!same, differs.contains(&key), "{label}: bake token {key}");
        }
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
            kind: WalkKind::Random,
            seed: WALK_SEED,
            steps: 2_000,
            pitch_degrees: 0.0,
            hfov_degrees: 100.0,
            out: PathBuf::from("out.route"),
        }
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
    for (args, named) in [
        (
            &["l.prl", "--kind", "smoothed", "-o", "o"][..],
            "far-point, random",
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
        .map(|a| (a.kind, a.seed, a.hfov_degrees)),
        Ok((WalkKind::FarPointTour, 12, 60.0))
    );
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
