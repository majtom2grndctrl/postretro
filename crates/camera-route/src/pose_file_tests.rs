// The pose file round-trips bit-equal and its parser refuses anything off-contract.
// See: context/lib/testing_guide.md

use glam::Vec3;

use crate::test_fixtures::grid_world;
use crate::{
    DEFAULT_ASPECT, DEFAULT_HFOV_DEGREES, GeneratedKind, ParseReason, PoseFile, Projection,
    RouteHeader, RouteOrigin, RoutePose, WalkKind, exact_route, format_pose_file, parse_pose_file,
};

const BAKE: &str = "id51=4b7255ff5bdf714c walk_epoch=4 size=1234 mtime=99 prl=maps/a b.prl";

fn header() -> RouteHeader {
    RouteHeader {
        origin: RouteOrigin::Generated(GeneratedKind::FarPoint),
        params: vec![
            ("seed".into(), "0x5eedb10c".into()),
            ("steps".into(), "2000".into()),
        ],
        projection: Projection {
            hfov_degrees: DEFAULT_HFOV_DEGREES,
            aspect: DEFAULT_ASPECT,
        },
        bake: BAKE.to_string(),
        cadence_hz: 60,
        spawn: Some(Vec3::new(-0.0, 1.5e-7, 12345.678)),
    }
}

fn pose_bits(pose: &RoutePose) -> ([u32; 5], Option<(u32, u32)>, bool) {
    (
        [pose.eye.x, pose.eye.y, pose.eye.z, pose.yaw, pose.pitch].map(f32::to_bits),
        pose.walk,
        pose.cut,
    )
}

fn assert_bit_equal(a: &PoseFile, b: &PoseFile) {
    assert_eq!(a.header.origin, b.header.origin);
    assert_eq!(a.header.params, b.header.params);
    assert_eq!(
        a.header.projection.hfov_degrees.to_bits(),
        b.header.projection.hfov_degrees.to_bits()
    );
    assert_eq!(
        a.header.projection.aspect.to_bits(),
        b.header.projection.aspect.to_bits()
    );
    assert_eq!(a.header.bake, b.header.bake);
    assert_eq!(a.header.cadence_hz, b.header.cadence_hz);
    assert_eq!(
        a.header.spawn.map(|s| s.to_array().map(f32::to_bits)),
        b.header.spawn.map(|s| s.to_array().map(f32::to_bits))
    );
    assert_eq!(a.poses.len(), b.poses.len());
    for (i, (x, y)) in a.poses.iter().zip(&b.poses).enumerate() {
        assert_eq!(pose_bits(x), pose_bits(y), "pose {i}");
    }
}

#[test]
fn pose_file_round_trips_a_walk_route_and_edge_values_bit_equal() {
    let world = grid_world();
    let route = exact_route(&world, WalkKind::FarPointTour, 200, 0.3).expect("route");
    let mut poses = route.poses;
    poses.push(RoutePose {
        eye: Vec3::new(-0.0, f32::MIN_POSITIVE, -1.0e30),
        yaw: -0.0,
        pitch: std::f32::consts::FRAC_PI_2 * 0.99,
        walk: None,
        cut: true,
    });
    poses.push(RoutePose {
        eye: Vec3::new(0.1, 0.2, 0.3),
        yaw: 7.0 * std::f32::consts::FRAC_PI_2,
        pitch: -0.25,
        walk: Some((u32::MAX, 0)),
        cut: false,
    });
    let file = PoseFile {
        header: header(),
        poses,
    };
    let text = format_pose_file(&file);
    let parsed = parse_pose_file(&text).expect("parses");
    assert_bit_equal(&file, &parsed);
    assert_eq!(
        format_pose_file(&parsed),
        text,
        "a re-write is byte-identical"
    );
    assert!(text.contains("\npose -0 "), "negative zero keeps its sign");
}

#[test]
fn pose_file_header_exposes_the_same_bake_check_tokens() {
    let parsed = parse_pose_file(&format_pose_file(&PoseFile {
        header: RouteHeader {
            origin: RouteOrigin::Recorded,
            params: Vec::new(),
            spawn: None,
            ..header()
        },
        poses: vec![RoutePose {
            eye: Vec3::ZERO,
            yaw: 0.0,
            pitch: 0.0,
            walk: None,
            cut: false,
        }],
    }))
    .expect("parses");
    assert_eq!(parsed.header.origin, RouteOrigin::Recorded);
    assert!(parsed.header.params.is_empty());
    assert_eq!(parsed.header.spawn, None);
    assert_eq!(parsed.header.id51(), Some("4b7255ff5bdf714c"));
    assert_eq!(parsed.header.walk_epoch(), Some(4));
    assert!(
        parsed.header.bake.ends_with("prl=maps/a b.prl"),
        "a path keeps its spaces"
    );
}

const HEAD: &str = "postretro-camera-route 1\n\
origin generated random\n\
params seed=0x1 steps=2\n\
projection hfov_degrees=100 aspect=1.7777778\n\
bake id51=none walk_epoch=4 size=0 mtime=0 prl=x.prl\n\
cadence_hz 60\n";

fn error(text: &str) -> (usize, ParseReason) {
    let error = parse_pose_file(text).expect_err("refused");
    (error.line, error.reason)
}

#[test]
fn pose_file_parser_accepts_the_minimal_file() {
    let parsed = parse_pose_file(&format!("{HEAD}pose 1 2 3 0 0\n")).expect("parses");
    assert_eq!(parsed.poses.len(), 1);
    assert_eq!(parsed.header.param("steps"), Some("2"));
}

#[test]
fn pose_file_parser_refuses_an_empty_file_and_a_foreign_one() {
    assert_eq!(error(""), (1, ParseReason::Empty));
    assert!(matches!(
        error("postretro-world-poses 1\npose a 0 0 0 0 0\n"),
        (1, ParseReason::NotAPoseFile { .. })
    ));
}

#[test]
fn pose_file_parser_refuses_a_header_without_poses() {
    assert_eq!(error(HEAD), (7, ParseReason::NoPoses));
    assert_eq!(
        error(&format!("{HEAD}spawn 1 2 3\n")),
        (8, ParseReason::NoPoses)
    );
}

#[test]
fn pose_file_parser_refuses_a_truncated_header() {
    let text = HEAD.lines().take(4).collect::<Vec<_>>().join("\n");
    assert_eq!(error(&text), (5, ParseReason::Missing { expected: "bake" }));
}

#[test]
fn pose_file_parser_refuses_header_lines_out_of_order() {
    let mut lines: Vec<&str> = HEAD.lines().collect();
    lines.swap(1, 2);
    let text = format!("{}\npose 0 0 0 0 0\n", lines.join("\n"));
    assert_eq!(
        error(&text),
        (
            2,
            ParseReason::OutOfOrder {
                expected: "origin",
                found: "params".into()
            }
        )
    );
    assert_eq!(
        error(&format!("{HEAD}pose 0 0 0 0 0\nspawn 1 2 3\n")),
        (
            8,
            ParseReason::OutOfOrder {
                expected: "pose",
                found: "spawn".into()
            }
        )
    );
    assert!(matches!(
        error(&format!("{HEAD}pose 0 0 0 0 0\ncadence_hz 60\n")),
        (
            8,
            ParseReason::OutOfOrder {
                expected: "pose",
                ..
            }
        )
    ));
}

#[test]
fn pose_file_parser_refuses_unknown_and_blank_lines() {
    assert_eq!(
        error(&format!("{HEAD}pose 0 0 0 0 0\nfov 90\n")),
        (8, ParseReason::UnknownLine("fov 90".into()))
    );
    assert_eq!(
        error(&format!("{HEAD}\npose 0 0 0 0 0\n")),
        (7, ParseReason::BlankLine)
    );
}

#[test]
fn pose_file_parser_refuses_bad_and_non_finite_floats() {
    assert_eq!(
        error(&format!("{HEAD}pose 0 0 zero 0 0\n")),
        (
            7,
            ParseReason::BadFloat {
                field: "z",
                text: "zero".into()
            }
        )
    );
    assert!(matches!(
        error(&format!("{HEAD}pose 0 0 0 inf 0\n")),
        (7, ParseReason::BadFloat { field: "yaw", .. })
    ));
    assert!(matches!(
        error(&HEAD.replace("aspect=1.7777778", "aspect=NaN")),
        (
            4,
            ParseReason::BadFloat {
                field: "aspect",
                ..
            }
        )
    ));
}

#[test]
fn pose_file_parser_refuses_malformed_records() {
    for (text, line) in [
        (format!("{HEAD}pose 0 0 0 0\n"), 7),
        (format!("{HEAD}pose 0 0 0 0 0 walk 1\n"), 7),
        (format!("{HEAD}pose 0 0 0 0 0 cut walk 1 2\n"), 7),
        (format!("{HEAD}pose 0 0 0 0 0 extra\n"), 7),
        (
            HEAD.replace("origin generated random", "origin generated spiral") + "pose 0 0 0 0 0\n",
            2,
        ),
        (HEAD.replace("seed=0x1", "seed") + "pose 0 0 0 0 0\n", 3),
        (
            HEAD.replace("hfov_degrees=100", "fov=100") + "pose 0 0 0 0 0\n",
            4,
        ),
        (
            HEAD.replace("id51=none", "id51=xyz") + "pose 0 0 0 0 0\n",
            5,
        ),
        (HEAD.replace(" prl=x.prl", "") + "pose 0 0 0 0 0\n", 5),
        (
            HEAD.replace("cadence_hz 60", "cadence_hz 0") + "pose 0 0 0 0 0\n",
            6,
        ),
    ] {
        let (at, reason) = error(&text);
        assert_eq!(at, line, "{reason}");
        assert!(matches!(reason, ParseReason::Malformed { .. }), "{reason}");
    }
    assert!(matches!(
        error(&format!("{HEAD}pose 0 0 0 0 0 walk -1 2\n")),
        (7, ParseReason::BadInteger { field: "step", .. })
    ));
}
