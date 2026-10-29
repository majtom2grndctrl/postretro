use glam::Vec3;

use super::bake_gap::{self, BakeGapRecord, FramePose, FrameResult, Sequence};
use super::poses::{self, PoseSet, WorldPose};
use super::ray_sweep::{self, MissKind, PoseRow, RayMiss, SweepRecord};
use super::structure;

fn pose(label: &str, x: f32) -> WorldPose {
    WorldPose {
        label: label.to_string(),
        eye: Vec3::new(x, 1.5, -0.1 * x + 1.0e-7),
        yaw_degrees: 90.0,
        pitch_degrees: -15.0,
    }
}

#[test]
fn pose_file_round_trips_every_float_bit_for_bit() {
    let set = PoseSet {
        notes: vec!["grid spacing=2 m".to_string()],
        poses: vec![pose("g0.0.0@0", 1.0 / 3.0), pose("fc:P1", -17.78)],
    };
    let back = poses::parse_poses(&poses::format_poses(&set)).unwrap();
    assert_eq!(back, set);
    assert!(back.poses[0].same_pose(&set.poses[0]));
}

#[test]
fn frame_cost_import_lifts_the_capsule_centre_to_the_eye() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("poses.txt");
    std::fs::write(
        &file,
        "P1 -17.78,1.0,-13.8176,90,0\n\nC1 -3.25,1.0,-3.25,0,15\n",
    )
    .unwrap();
    let imported = poses::import_frame_cost(file.to_str().unwrap()).unwrap();
    assert_eq!(imported.len(), 2);
    assert_eq!(imported[0].label, "fc:P1");
    assert_eq!(imported[0].eye, Vec3::new(-17.78, 1.5, -13.8176));
    assert_eq!(imported[1].pitch_degrees, 15.0);
}

#[test]
fn grid_spacing_never_drops_below_the_floor_and_rounds_up() {
    assert_eq!(poses::grid_spacing(10.0, 600), 1.0);
    // 10_000 m² over 100 columns wants 10 m.
    assert_eq!(poses::grid_spacing(10_000.0, 100), 10.0);
    // 10_000 m² over 600 columns wants 4.08 m, rounded up to 4.25 m.
    assert_eq!(poses::grid_spacing(10_000.0, 600), 4.25);
}

fn row(pose: WorldPose, path: &str, misses: &[u32]) -> PoseRow {
    PoseRow {
        pose,
        fields: vec![
            ("path".to_string(), path.to_string()),
            ("misses".to_string(), misses.len().to_string()),
        ],
        misses: misses
            .iter()
            .map(|&ray| RayMiss {
                ray,
                point: Vec3::new(1.0, 2.0, 3.0),
                cell: 7,
                kind: MissKind::Drawn,
            })
            .collect(),
    }
}

fn sweep(rows: Vec<PoseRow>) -> SweepRecord {
    SweepRecord {
        bake: "id51=none".to_string(),
        camera: "rays 4x2".to_string(),
        rows,
    }
}

#[test]
fn sweep_record_round_trips() {
    let record = sweep(vec![
        row(pose("a", 1.0), "portal", &[3, 5]),
        row(pose("b", 2.0), "portal", &[]),
    ]);
    let back = ray_sweep::parse_record(&ray_sweep::format_record(&record)).unwrap();
    assert_eq!(back, record);
}

#[test]
fn unpaired_sweep_fails_on_any_miss_or_trip() {
    let clean = sweep(vec![row(pose("a", 1.0), "portal", &[])]);
    assert!(ray_sweep::verdict(None, &clean).passed());
    let missed = sweep(vec![row(pose("a", 1.0), "portal", &[4])]);
    assert!(!ray_sweep::verdict(None, &missed).passed());
    let tripped = sweep(vec![row(pose("a", 1.0), "step_limit", &[])]);
    assert!(!ray_sweep::verdict(None, &tripped).passed());
}

#[test]
fn paired_sweep_reports_a_carried_miss_and_fails_a_new_one() {
    let before = sweep(vec![row(pose("a", 1.0), "portal", &[4])]);
    let carried = sweep(vec![row(pose("a", 1.0), "portal", &[4])]);
    let verdict = ray_sweep::verdict(Some(&before), &carried);
    assert!(verdict.passed());
    assert_eq!(verdict.by_kind[0], [("a".to_string(), 0, 1)]);
    let new = sweep(vec![row(pose("a", 1.0), "portal", &[4, 9])]);
    let verdict = ray_sweep::verdict(Some(&before), &new);
    assert!(!verdict.passed());
    assert_eq!(verdict.by_kind[0][0].1, 1, "only ray 9 is new");
}

#[test]
fn culled_and_faceless_misses_are_reported_never_failed() {
    let mut row = row(pose("a", 1.0), "portal", &[]);
    for (ray, kind) in [(2, MissKind::Culled), (3, MissKind::Faceless)] {
        row.fields
            .push((format!("{}_misses", kind.label()), "1".to_string()));
        row.misses.push(RayMiss {
            ray,
            point: Vec3::ZERO,
            cell: 1,
            kind,
        });
    }
    let record = sweep(vec![row]);
    let back = ray_sweep::parse_record(&ray_sweep::format_record(&record)).unwrap();
    assert_eq!(back, record);
    let verdict = ray_sweep::verdict(None, &record);
    assert!(verdict.passed());
    assert_eq!(verdict.by_kind[1], [("a".to_string(), 1, 1)]);
    assert_eq!(verdict.by_kind[2], [("a".to_string(), 1, 1)]);
}

#[test]
fn paired_sweep_is_invalid_when_world_poses_differ() {
    let before = sweep(vec![row(pose("a", 1.0), "portal", &[])]);
    let moved = sweep(vec![row(pose("a", 1.0 + 1.0e-6), "portal", &[])]);
    let verdict = ray_sweep::verdict(Some(&before), &moved);
    assert!(verdict.invalid.is_some());
    assert!(!verdict.passed());
}

fn gap(deferred_texels: &[u64]) -> BakeGapRecord {
    let frames = deferred_texels
        .iter()
        .enumerate()
        .map(|(i, &texels)| {
            (
                FramePose {
                    eye: Vec3::new(i as f32 * 0.1, 1.5, 0.3),
                    yaw: 1.0 / 3.0,
                    pitch: 0.0,
                },
                FrameResult {
                    deferred: u32::from(texels > 0),
                    deferred_texels: texels,
                    ..FrameResult::default()
                },
            )
        })
        .collect();
    BakeGapRecord {
        bake: "id51=abc".to_string(),
        modes: "completed reads".to_string(),
        pool_policy: "demand-sized growth headroom 4/3".to_string(),
        sequences: vec![Sequence {
            label: "random walk".to_string(),
            spawn: Vec3::new(0.25, 1.5, 0.75),
            frames,
        }],
    }
}

#[test]
fn bake_gap_record_round_trips() {
    let record = gap(&[0, 64, 0]);
    let back = bake_gap::parse_record(&bake_gap::format_record(&record)).unwrap();
    assert_eq!(back, record);
}

#[test]
fn bake_gap_compares_texel_area_summed_per_sequence() {
    let before = gap(&[0, 256, 0]);
    // Fewer, larger deferrals elsewhere in the walk, same total: passes.
    let moved = gap(&[128, 0, 128]);
    assert!(bake_gap::verdict(Some(&before), &moved).passed());
    let worse = gap(&[0, 256, 1]);
    assert!(!bake_gap::verdict(Some(&before), &worse).passed());
}

#[test]
fn bake_gap_zero_before_must_stay_zero() {
    let before = gap(&[0, 0, 0]);
    assert!(bake_gap::verdict(Some(&before), &gap(&[0, 0, 0])).passed());
    assert!(!bake_gap::verdict(Some(&before), &gap(&[0, 1, 0])).passed());
}

#[test]
fn portal_shape_measures_a_unit_square() {
    let square = [
        Vec3::new(0.0, 0.0, 0.0),
        Vec3::new(1.0, 0.0, 0.0),
        Vec3::new(1.0, 1.0, 0.0),
        Vec3::new(0.0, 1.0, 0.0),
    ];
    let (area, normal, aspect) = structure::portal_shape(&square);
    assert!((area - 1.0).abs() < 1.0e-6);
    assert!((normal.z.abs() - 1.0).abs() < 1.0e-6);
    assert!((aspect - std::f32::consts::SQRT_2).abs() < 1.0e-5);
}

#[test]
fn bake_gap_pair_is_invalid_when_pool_policy_differs() {
    let before = gap(&[0, 64, 0]);
    let mut this = gap(&[0, 64, 0]);
    this.pool_policy = "per-drain sizing".to_string();
    let verdict = bake_gap::verdict(Some(&before), &this);
    assert!(verdict.invalid.is_some());
    assert!(!verdict.passed());
}

#[test]
fn bake_gap_rejects_record_one() {
    let text = "postretro-bake-gap-world-record 1\nbake x\nmodes y\n";
    assert!(bake_gap::parse_record(text).is_err());
}
