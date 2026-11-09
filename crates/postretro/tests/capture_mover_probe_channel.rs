// GPU capture proofs for the mover probe channel: scenes force each mover's
// travel fraction, which poses its drawn geometry, shadow casters, occluder
// bounds and portal seal, and hands the renderer the light state the game
// would. The closet rows judge the closet with reflections off: the pool's
// reflection probe (id 54) is captured with movers absent
// (drafts/mover-aware-reflection-captures.md).
// See: context/lib/rendering_pipeline.md §4 (Mover state lighting) · §7.8
//
// Means are Rec. 709 luma of the post-tonemap sRGB capture (0–255). Captures
// compare on one adapter; no golden images.

use std::path::Path;

use image::RgbaImage;

mod capture_support;
// Shared by two targets, each using part of it.
#[allow(dead_code)]
mod mover_capture_support;

use capture_support::compile_dev_map;
use mover_capture_support::{
    Pose, Run, Scene, ScreenBox, frame_mean, luma, masked_mean, max_channel_delta, read_map,
    sibling_map, staticize, with_entity, without_channel,
};

const RESOLUTION: [u32; 2] = [960, 540];

/// A C10 case: a map, its label, the pose, the movers forced mid-travel, and
/// the light-term mask.
type MidTravel<'a> = (&'a Path, &'static str, Pose, Vec<(&'static str, f32)>, u32);
/// The brief's capture bar: no brighter than the reference plus 0.02 mean luma.
const BAR: f64 = 0.02;

const CLOSET_DOOR: &str = "closet_door";
/// The closet door's brush at its closed pose, mover-closet.README.md §Door.
const DOOR_MIN: [f32; 3] = [272.0, 488.0, 0.0];
const DOOR_MAX: [f32; 3] = [368.0, 504.0, 112.0];

/// Closet station 2: inside the closet, facing the door.
const STATION_2: Pose = Pose {
    map: [320.0, 720.0, 64.0],
    yaw: 270.0,
    pitch: -6.0,
    fov: 100.0,
};
/// Closet station 3: the closet floor at the door.
const STATION_3: Pose = Pose {
    map: [320.0, 610.0, 96.0],
    yaw: 270.0,
    pitch: -38.0,
    fov: 100.0,
};
/// Inside the closet near the door, facing away from it: the pool, floor and
/// back wall the door's probes light, and no view of the door's edges.
const CLOSET_INTERIOR: Pose = Pose {
    map: [320.0, 540.0, 72.0],
    yaw: 90.0,
    pitch: -20.0,
    fov: 100.0,
};
/// The door's closet face, as research.md §Captures framed it.
const DOOR_FACE: Pose = Pose {
    map: [320.0, 600.0, 56.0],
    yaw: 270.0,
    pitch: 0.0,
    fov: 60.0,
};

const SETPIECE_WALL: &str = "setpiece_wall";
const SETPIECE_ROOM: &str = "setpiece_room";
/// Set-piece station 3: behind the wall, the enemy spot in the foreground.
const SETPIECE_STATION_3: Pose = Pose {
    map: [368.0, 1536.0, 48.0],
    yaw: 270.0,
    pitch: -8.0,
    fov: 100.0,
};
/// Set-piece station 2: the rising room's doorway wall from outside.
const SETPIECE_STATION_2: Pose = Pose {
    map: [560.0, 320.0, 48.0],
    yaw: 0.0,
    pitch: 0.0,
    fov: 100.0,
};
/// A mesh receiver held on the enemy spot (the chaser patrols off it).
const ENEMY_SPOT_RECEIVER: &str = r#"{
"classname" "prop_mesh"
"origin" "368 1232 8"
"model" "models/decraniated_low_poly_retro_pixel/scene.gltf"
"angles" "0 270 0"
}"#;
/// Screen region the held receiver can cover: the plinth's footprint up to a
/// standing figure's height. The plinth is x 336–400, y 1200–1264, top z 8.
const RECEIVER_MIN: [f32; 3] = [320.0, 1184.0, 8.0];
const RECEIVER_MAX: [f32; 3] = [416.0, 1280.0, 120.0];
/// `LightTermMask::INDIRECT_STATIC`'s bit.
const INDIRECT_STATIC: u32 = 1 << 1;
/// `LightTermMask::BAKED_DIRECT_STATIC`'s bit.
const BAKED_DIRECT_STATIC: u32 = 1 << 3;

fn scene(force_movers: Vec<(&'static str, f32)>) -> Scene {
    Scene {
        resolution: RESOLUTION,
        force_movers,
        reflections_off: true,
        ..Scene::default()
    }
}

/// Pixels whose view ray meets the closet pool's inset surface (x 224–416,
/// y 544–688, z −8, trimmed 8 units from each edge), outside the door's box.
fn closet_pool_pixels(pose: Pose) -> impl Fn(u32, u32) -> bool {
    let door = ScreenBox::of(DOOR_MIN, DOOR_MAX, pose, RESOLUTION);
    move |x, y| {
        if door.contains(x, y) {
            return false;
        }
        let (sy, cy) = pose.yaw.to_radians().sin_cos();
        let (sp, cp) = pose.pitch.to_radians().sin_cos();
        let forward = [cy * cp, sy * cp, sp];
        let right = [sy, -cy, 0.0];
        let up = [-cy * sp, -sy * sp, cp];
        let [w, h] = RESOLUTION.map(|v| v as f32);
        let half_x = (pose.fov.to_radians() / 2.0).tan();
        let half_y = half_x * h / w;
        let nx = 2.0 * (x as f32 + 0.5) / w - 1.0;
        let ny = 1.0 - 2.0 * (y as f32 + 0.5) / h;
        let dir: [f32; 3] =
            std::array::from_fn(|i| forward[i] + right[i] * nx * half_x + up[i] * ny * half_y);
        if dir[2] >= 0.0 {
            return false;
        }
        let t = (-8.0 - pose.map[2]) / dir[2];
        let hit = [pose.map[0] + t * dir[0], pose.map[1] + t * dir[1]];
        (232.0..=408.0).contains(&hit[0]) && (552.0..=680.0).contains(&hit[1])
    }
}

// K1. The door forced closed, reflections off: stations 2 and 3, outside the
// door's projected box, read no brighter than the tiled static-door
// reference plus 0.02, and the closet pool stays as dark as the reference's.
// The door's closet face is recorded beside the reference's for the Manual
// closet row. Forced open at its last waypoint, the closet reads within 0.02
// of the same PRL with id 57 stripped. Every clause runs under streaming
// `sync-proof` and `off`: an unchanged terminus reads id 34 as each mode
// composes it (task 11c), so the open door renders today's frame on both.
// Replaces `mover_door_closet_does_not_darken`, which asserted the old glow.
#[test]
#[ignore = "requires a GPU adapter and a local prl-build bake; run with `cargo test -p postretro --features capture --test capture_mover_probe_channel -- --ignored`"]
fn k1_closet_closed_reads_as_dark_as_the_static_door_and_open_reads_as_stripped() {
    let run = Run::new();
    let closet = compile_dev_map(&run.workspace, "mover-closet");
    let reference = compile_dev_map(&run.workspace, "mover-closet-static-door");
    let stripped = without_channel(&run.workspace, &closet, "mover-closet-no-id57", false);
    let mut misses = Vec::new();

    for mode in ["sync-proof", "off"] {
        let in_mode = |scene: Scene| Scene {
            streaming: Some(mode),
            ..scene
        };
        let closed = in_mode(scene(vec![(CLOSET_DOOR, 0.0)]));
        let open = in_mode(scene(vec![(CLOSET_DOOR, 1.0)]));
        let plain = in_mode(scene(Vec::new()));
        for (pose, name) in [(STATION_2, "st2"), (STATION_3, "st3")] {
            let door = ScreenBox::of(DOOR_MIN, DOOR_MAX, pose, RESOLUTION);
            let outside = |x, y| !door.contains(x, y);
            let posed = run.capture(&closet, &format!("k1-{mode}-closed-{name}"), pose, &closed);
            let tiled = run.capture(&reference, &format!("k1-{mode}-ref-{name}"), pose, &plain);
            let posed_out = masked_mean(&posed, outside, luma).expect("pixels outside the door");
            let tiled_out = masked_mean(&tiled, outside, luma).expect("pixels outside the door");
            let pool = closet_pool_pixels(pose);
            let posed_pool = masked_mean(&posed, &pool, luma).expect("the pool is in view");
            let tiled_pool = masked_mean(&tiled, &pool, luma).expect("the pool is in view");
            eprintln!(
                "K1 {mode} {name} closed: frame {:.4} outside-door {posed_out:.4} pool \
                 {posed_pool:.4}; static-door reference: frame {:.4} outside-door \
                 {tiled_out:.4} pool {tiled_pool:.4}",
                frame_mean(&posed),
                frame_mean(&tiled),
            );
            if posed_out > tiled_out + BAR {
                misses.push(format!(
                    "{mode} {name}: closed closet {posed_out} against the reference's {tiled_out} + {BAR}"
                ));
            }
            if posed_pool > tiled_pool + BAR {
                misses.push(format!(
                    "{mode} {name}: closed closet pool {posed_pool} against the reference's {tiled_pool} + {BAR}"
                ));
            }

            let opened = run.capture(&closet, &format!("k1-{mode}-open-{name}"), pose, &open);
            let opened_stripped = run.capture(
                &stripped,
                &format!("k1-{mode}-open-stripped-{name}"),
                pose,
                &open,
            );
            let (with, without) = (frame_mean(&opened), frame_mean(&opened_stripped));
            let moved = opened
                .pixels()
                .zip(opened_stripped.pixels())
                .filter(|(a, b)| a != b)
                .count();
            eprintln!(
                "K1 {mode} {name} open: {with:.4}; id 57 stripped: {without:.4}; {moved} pixel(s) differ"
            );
            if (with - without).abs() > BAR {
                misses.push(format!(
                    "{mode} {name}: forced open {with} against stripped {without}"
                ));
            }
        }

        let door = ScreenBox::of(DOOR_MIN, DOOR_MAX, DOOR_FACE, RESOLUTION);
        let face = |image: &RgbaImage| {
            masked_mean(image, |x, y| door.contains(x, y), luma).expect("the door face is in view")
        };
        let posed = run.capture(
            &closet,
            &format!("k1-{mode}-closed-doorface"),
            DOOR_FACE,
            &closed,
        );
        let tiled = run.capture(
            &reference,
            &format!("k1-{mode}-ref-doorface"),
            DOOR_FACE,
            &plain,
        );
        eprintln!(
            "K1 {mode} door closet face (Manual closet row): closed {:.4}; static-door reference {:.4}",
            face(&posed),
            face(&tiled)
        );
    }
    assert!(
        misses.is_empty(),
        "K1 misses its bars:\n{}",
        misses.join("\n")
    );
}

/// The set-piece with a mesh receiver held on the enemy spot, its static-wall
/// reference, and the receiver-free backdrop that finds the receiver's pixels.
struct Setpiece {
    _sources: [tempfile::TempPath; 2],
    receiver: tempfile::TempPath,
    reference: tempfile::TempPath,
    backdrop: tempfile::TempPath,
}

impl Setpiece {
    fn compile(workspace: &Path) -> Self {
        let source = with_entity(&read_map(workspace, "mover-setpiece"), ENEMY_SPOT_RECEIVER);
        let receiver_map = sibling_map(workspace, "mover-setpiece-receiver", &source);
        let reference_map = sibling_map(
            workspace,
            "mover-setpiece-receiver-static-wall",
            &staticize(&source, SETPIECE_WALL, "setpiece_wall_closed"),
        );
        let stem = |path: &tempfile::TempPath| {
            path.file_stem()
                .and_then(|stem| stem.to_str())
                .expect("generated map stem")
                .to_owned()
        };
        Self {
            receiver: compile_dev_map(workspace, &stem(&receiver_map)),
            reference: compile_dev_map(workspace, &stem(&reference_map)),
            backdrop: compile_dev_map(workspace, "mover-setpiece"),
            _sources: [receiver_map, reference_map],
        }
    }
}

// K2. `setpiece_wall` forced closed: the mesh receiver held on the enemy spot
// reads, over its pixels, no brighter than in a build where the wall is a
// static brush at its closed pose plus 0.02, and darker than with the wall
// forced open. Forced open, it reads within 0.02 of the same PRL with id 57
// stripped. Its pixels are those in its screen region that differ from the
// receiver-free backdrop at both wall poses; the floor is not judged.
#[test]
#[ignore = "requires a GPU adapter and a local prl-build bake; run with `cargo test -p postretro --features capture --test capture_mover_probe_channel -- --ignored`"]
fn k2_setpiece_wall_closed_darkens_the_held_receiver_like_the_static_wall() {
    let run = Run::new();
    let maps = Setpiece::compile(&run.workspace);
    let stripped = without_channel(
        &run.workspace,
        &maps.receiver,
        "mover-setpiece-receiver-no-id57",
        false,
    );
    let pose = SETPIECE_STATION_3;
    let closed = Scene {
        reflections_off: false,
        ..scene(vec![(SETPIECE_WALL, 0.0)])
    };
    let open = Scene {
        reflections_off: false,
        ..scene(vec![(SETPIECE_WALL, 1.0)])
    };
    let plain = Scene {
        reflections_off: false,
        ..scene(Vec::new())
    };
    let posed_closed = run.capture(&maps.receiver, "t11-k2-closed", pose, &closed);
    let posed_open = run.capture(&maps.receiver, "t11-k2-open", pose, &open);
    let open_stripped = run.capture(&stripped, "t11-k2-open-stripped", pose, &open);
    let reference = run.capture(&maps.reference, "t11-k2-ref", pose, &plain);
    // The receiver's own pixels: with static indirect alone, the mesh changes
    // no world surface (it casts no shadow there), so every pixel that moves
    // against the receiver-free backdrop is the mesh's.
    let indirect_only = Scene {
        light_term_mask: Some(INDIRECT_STATIC),
        ..open.clone()
    };
    let mesh = run.capture(&maps.receiver, "t11-k2-mask-mesh", pose, &indirect_only);
    let backdrop = run.capture(&maps.backdrop, "t11-k2-mask-backdrop", pose, &indirect_only);
    let region = ScreenBox::of(RECEIVER_MIN, RECEIVER_MAX, pose, RESOLUTION);
    let receiver =
        |x: u32, y: u32| region.contains(x, y) && mesh.get_pixel(x, y) != backdrop.get_pixel(x, y);
    let pixels = posed_closed
        .enumerate_pixels()
        .filter(|(x, y, _)| receiver(*x, *y))
        .count();
    assert!(pixels >= 200, "the receiver must be in view: {pixels} px");
    let mean = |image: &RgbaImage| masked_mean(image, receiver, luma).expect("receiver pixels");
    let (closed_mean, open_mean, stripped_mean, reference_mean) = (
        mean(&posed_closed),
        mean(&posed_open),
        mean(&open_stripped),
        mean(&reference),
    );
    eprintln!(
        "K2 receiver ({pixels} px): wall closed {closed_mean:.3}, static-wall reference \
         {reference_mean:.3}, wall open {open_mean:.3}, open with id 57 stripped {stripped_mean:.3}"
    );
    assert!(
        closed_mean <= reference_mean + BAR,
        "closed {closed_mean} against the static-wall reference {reference_mean} + {BAR}"
    );
    assert!(
        closed_mean < open_mean,
        "closed {closed_mean} must read darker than open {open_mean}"
    );
    assert!(
        (open_mean - stripped_mean).abs() <= BAR,
        "open {open_mean} against stripped {stripped_mean}"
    );
}

// C10, with C7's GPU half, as far as frames can show it. At mid-travel,
// streaming `off` (the whole-load legacy compose) and `sync-proof` (the
// slot-pooled streamed compose, whose rows enter residency over preload
// frames with the state already handed over) are captured per pass: baked
// static direct alone (mask 8) and static indirect alone (mask 2). The two
// modes' frames already differ with id 57 stripped (their bases disagree by
// a few levels on part of each view), so byte identity is not a frame-level
// claim. The row holds the channel to two things: it is live in both modes,
// and it adds no disagreement between them past the stripped frames' own.
// The compose inputs themselves are proven equal per cell
// (`streamed_mover_channel`, task 9).
#[test]
#[ignore = "requires a GPU adapter and a local prl-build bake; run with `cargo test -p postretro --features capture --test capture_mover_probe_channel -- --ignored`"]
fn c10_streaming_off_and_sync_proof_compose_the_same_channel_output() {
    let run = Run::new();
    let closet = compile_dev_map(&run.workspace, "mover-closet");
    let setpiece = compile_dev_map(&run.workspace, "mover-setpiece");
    let both = vec![(SETPIECE_WALL, 0.5), (SETPIECE_ROOM, 0.5)];
    let cases: [MidTravel; 5] = [
        (
            &closet,
            "closet-st2",
            STATION_2,
            vec![(CLOSET_DOOR, 0.5)],
            INDIRECT_STATIC,
        ),
        (
            &setpiece,
            "setpiece-st2",
            SETPIECE_STATION_2,
            both.clone(),
            INDIRECT_STATIC,
        ),
        (
            &setpiece,
            "setpiece-st3",
            SETPIECE_STATION_3,
            both.clone(),
            INDIRECT_STATIC,
        ),
        (
            &setpiece,
            "setpiece-st2",
            SETPIECE_STATION_2,
            both.clone(),
            BAKED_DIRECT_STATIC,
        ),
        (
            &setpiece,
            "setpiece-st3",
            SETPIECE_STATION_3,
            both,
            BAKED_DIRECT_STATIC,
        ),
    ];
    let mut misses = Vec::new();
    for (map, name, pose, force, mask) in cases {
        let stripped = without_channel(&run.workspace, map, &format!("{name}-no-id57"), false);
        let sync = Scene {
            light_term_mask: Some(mask),
            ..scene(force)
        };
        let off = Scene {
            streaming: Some("off"),
            ..sync.clone()
        };
        let shot = |prl: &Path, mode: &Scene, label: &str| {
            run.capture(
                prl,
                &format!("t11-c10-{label}-{name}-mask{mask}"),
                pose,
                mode,
            )
        };
        let (off_on, sync_on) = (shot(map, &off, "off"), shot(map, &sync, "sync"));
        let (off_bare, sync_bare) = (
            shot(&stripped, &off, "off-stripped"),
            shot(&stripped, &sync, "sync-stripped"),
        );
        let pixels = off_on.width() * off_on.height();
        let agree = off_bare
            .pixels()
            .zip(sync_bare.pixels())
            .filter(|(a, b)| a == b)
            .count();
        let (baseline, with_channel) = (
            max_channel_delta(&off_bare, &sync_bare),
            max_channel_delta(&off_on, &sync_on),
        );
        let (live_off, live_sync) = (
            max_channel_delta(&off_on, &off_bare),
            max_channel_delta(&sync_on, &sync_bare),
        );
        eprintln!(
            "C10 {name} mask {mask}: off against sync-proof, max channel delta {with_channel} \
             with id 57 and {baseline} stripped (stripped frames agree at {agree} of {pixels} px); \
             the channel moves off by up to {live_off} and sync-proof by up to {live_sync}"
        );
        if with_channel > baseline.saturating_add(2) {
            misses.push(format!(
                "{name} mask {mask}: the modes differ by {with_channel} with id 57, past {baseline} stripped + 2"
            ));
        }
        if live_off <= 2 || live_sync <= 2 {
            misses.push(format!(
                "{name} mask {mask}: the channel must move both modes"
            ));
        }
    }
    assert!(misses.is_empty(), "C10 misses:\n{}", misses.join("\n"));
}

// D2, frame clauses. With id 57 stripped, handing the door's state over
// changes no byte: the frame at its forced rest pose (state handed over,
// portals sealed) equals the frame with no forced mover (no state). With id
// 57 and no state handed over, the frame moves no pixel by more than 2
// levels per channel against the same PRL with only its depth block stripped.
#[test]
#[ignore = "requires a GPU adapter and a local prl-build bake; run with `cargo test -p postretro --features capture --test capture_mover_probe_channel -- --ignored`"]
fn d2_no_section_is_byte_identical_and_no_state_is_within_two_levels_of_no_depth() {
    let run = Run::new();
    let closet = compile_dev_map(&run.workspace, "mover-closet");
    let stripped = without_channel(&run.workspace, &closet, "mover-closet-no-id57", false);
    let no_depth = without_channel(&run.workspace, &closet, "mover-closet-no-depth", true);
    let plain = scene(Vec::new());
    let rest = scene(vec![(CLOSET_DOOR, 0.0)]);
    // Facing away from the door: from stations 2 and 3 the rest pose's
    // portal seal (P11) culls the bright room seen through a sub-pixel gap at
    // the door's two top corners, which moves 2 px whatever the state.
    {
        let (pose, name) = (CLOSET_INTERIOR, "interior");
        let without_state =
            run.capture(&stripped, &format!("t11-d2-stripped-{name}"), pose, &plain);
        let with_state = run.capture(
            &stripped,
            &format!("t11-d2-stripped-state-{name}"),
            pose,
            &rest,
        );
        let delta = max_channel_delta(&without_state, &with_state);
        eprintln!("D2 {name}: id 57 stripped, state against none: max channel delta {delta}");
        assert!(
            without_state.as_raw() == with_state.as_raw(),
            "{name}: with no section, a handed-over state moved pixels by up to {delta}"
        );
        // The same state does move this view once id 57 is present.
        let gated = run.capture(&closet, &format!("t11-d2-state-{name}"), pose, &rest);
        let ungated = run.capture(&closet, &format!("t11-d2-nostate-{name}"), pose, &plain);
        let live = max_channel_delta(&gated, &ungated);
        eprintln!("D2 {name}: with id 57, state against none: max channel delta {live}");
        assert!(
            live > 2,
            "{name}: the door's state must move this view with id 57"
        );
    }
    for (pose, name) in [
        (STATION_2, "st2"),
        (STATION_3, "st3"),
        (CLOSET_INTERIOR, "interior"),
    ] {
        let channel = run.capture(&closet, &format!("t11-d2-nostate-{name}"), pose, &plain);
        let depthless = run.capture(&no_depth, &format!("t11-d2-nodepth-{name}"), pose, &plain);
        let delta = max_channel_delta(&channel, &depthless);
        eprintln!(
            "D2 {name}: id 57 without state {:.4}, depth block stripped {:.4}: max channel delta {delta}",
            frame_mean(&channel),
            frame_mean(&depthless)
        );
        assert!(
            delta <= 2,
            "{name}: no state moved a channel by {delta} levels against no depth block"
        );
    }
}
