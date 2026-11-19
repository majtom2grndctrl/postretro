//! A generated exact-frames pose file reproduces the walk's pose fingerprints.
//! See: context/lib/rendering_pipeline.md §12

// On demand, over a real PRL:
//
//   POSTRETRO_LIGHTMAP_WALK_PRL=<abs>/content/dev/maps/campaign-test.prl \
//     cargo test -p postretro --bin postretro \
//     camera_route_file_reproduces_the_walk_fingerprints_from_prl -- --ignored --nocapture
//
// The oracle is the harness `run` loop's eye stream as it stood before the
// route moved into `postretro-camera-route`; the file is what
// `postretro-tool generate-camera-route` writes at the walk's defaults.

use postretro_camera_route::{
    CADENCE_HZ, GeneratedKind, PoseFile, Projection, RouteHeader, RouteOrigin, SplitMix64,
    WALK_SEED, Walk, WalkKind, bake_identity, exact_route, eye_point, format_pose_file,
    parse_pose_file, pose_fingerprint,
};

use super::modes::WalkModes;
use super::{DEFAULT_STEPS, PRL_ENV};

#[test]
#[ignore = "measurement helper; set POSTRETRO_LIGHTMAP_WALK_PRL"]
fn camera_route_file_reproduces_the_walk_fingerprints_from_prl() {
    let path = std::env::var(PRL_ENV).unwrap_or_else(|_| panic!("{PRL_ENV} must name a PRL"));
    let world = postretro_level_loader::load_prl(&path).expect("load PRL");
    let bake = bake_identity(&path, &world);
    // The walk's defaults: 100° at 16:9, pitch 0.
    let modes = WalkModes::default();
    let projection = (modes.projection.hfov_degrees, modes.projection.aspect);
    println!("PRL: {path}\nbake read: {bake}");

    for kind in WalkKind::ALL {
        let walk = Walk::build(&world, kind, DEFAULT_STEPS);
        let mut rng = SplitMix64(WALK_SEED ^ 0xE7E5);
        let spawn = eye_point(&world, walk.cells[0], &mut rng);
        let mut expected = Vec::with_capacity(walk.frames() as usize);
        for (step, &cell) in walk.cells.iter().enumerate() {
            for frame in 0..walk.dwell[step] {
                let eye = eye_point(&world, cell, &mut rng);
                let yaw = walk.travel_yaw[step] + frame as f32 * std::f32::consts::FRAC_PI_2;
                expected.push(pose_fingerprint(
                    step,
                    cell,
                    eye,
                    yaw,
                    modes.pitch,
                    projection,
                ));
            }
        }

        let route = exact_route(&world, kind, DEFAULT_STEPS, modes.pitch).expect("exact route");
        let file = PoseFile {
            header: RouteHeader {
                origin: RouteOrigin::Generated(GeneratedKind::exact(kind)),
                params: vec![("steps".into(), DEFAULT_STEPS.to_string())],
                projection: Projection {
                    hfov_degrees: modes.projection.hfov_degrees,
                    aspect: modes.projection.aspect,
                },
                bake: bake.clone(),
                cadence_hz: CADENCE_HZ,
                spawn: Some(route.spawn),
            },
            poses: route.poses,
        };
        let out = std::env::temp_dir().join(format!(
            "postretro-camera-route-ac1-{}-{}.txt",
            kind.key(),
            std::process::id()
        ));
        std::fs::write(&out, format_pose_file(&file)).expect("write the pose file");
        let text = std::fs::read_to_string(&out).expect("read the pose file back");
        let _ = std::fs::remove_file(&out);
        let parsed =
            parse_pose_file(&text).unwrap_or_else(|why| panic!("{}: {why}", out.display()));

        let header = &parsed.header;
        assert_eq!(
            header.spawn.map(|s| s.to_array().map(f32::to_bits)),
            Some(spawn.to_array().map(f32::to_bits)),
            "{}: spawn eye",
            kind.label()
        );
        assert_eq!(
            header.id51(),
            postretro_camera_route::bake_token(&bake, "id51")
        );
        let got: Vec<u64> = parsed
            .poses
            .iter()
            .map(|pose| {
                let (step, cell) = pose.walk.expect("exact poses carry their walk step");
                pose_fingerprint(
                    step as usize,
                    cell,
                    pose.eye,
                    pose.yaw,
                    pose.pitch,
                    (header.projection.hfov_degrees, header.projection.aspect),
                )
            })
            .collect();
        assert_eq!(got.len(), expected.len(), "{}: frame count", kind.label());
        if let Some(i) = (0..got.len()).find(|&i| got[i] != expected[i]) {
            panic!("{}: fingerprints differ from frame {i}", kind.label());
        }
        println!(
            "{}: {} frames, {} bytes; every frame's fingerprint matches the walk",
            kind.label(),
            got.len(),
            text.len()
        );
    }
}
