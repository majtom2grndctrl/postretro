// D4 and D5 frame clauses on the compiled `mover-closet`, through the capture
// harness's offscreen frames: the frame a state is first handed over, and the
// frame a t′ steps from 1 to 0, each render within 2 levels per channel of
// the frame after, so no frame samples an unwritten live tile or pairs one
// terminus's bounce with the other's depth. Run once with
// `POSTRETRO_SH_STREAMING=off` (whole load) and once with `sync-proof`
// (streamed); the loader reads the mode from the environment.
// See: context/lib/rendering_pipeline.md §4 (Mover state lighting) · §7.8

use super::prepared::PreparedCapture;
use super::scene::parse_scene;
use crate::render::MoverLightState;

const UNIT: f32 = 0.0254;

/// Station 2 of the closet, facing the door; no forced mover, so the door
/// rests at its seeded closed pose and no state is handed over in preload.
fn prepare(map: &str) -> PreparedCapture {
    let [x, y, z] = [320.0f32, 720.0, 64.0];
    let scene = serde_json::json!({
        "map": map,
        "camera": {
            "position": [-y * UNIT, z * UNIT, -x * UNIT],
            "yaw_deg": 270.0,
            "pitch_deg": -6.0,
            "fov_deg": 100.0
        },
        "resolution": [480, 270],
        "output": "unused.png",
        "force_full_resident_sh_compose": true,
        "reflection_quality": "off"
    });
    let scene = parse_scene(&scene.to_string()).expect("closet frame scene");
    PreparedCapture::prepare(&scene).expect("closet capture prepares")
}

fn max_channel_delta(a: &[u8], b: &[u8]) -> u8 {
    assert_eq!(a.len(), b.len());
    a.as_chunks::<4>()
        .0
        .iter()
        .zip(b.as_chunks::<4>().0)
        .flat_map(|(p, q)| (0..3).map(move |c| p[c].abs_diff(q[c])))
        .max()
        .unwrap_or(0)
}

fn door_id(map: &str) -> u32 {
    let world = postretro_level_loader::load_prl(map).expect("the closet loads");
    assert_eq!(world.kinematic_geometry.movers.len(), 1, "one door");
    world.kinematic_geometry.movers[0].mover_id
}

#[test]
#[ignore = "requires a GPU adapter and MOVER_CLOSET_PRL (mover-closet compiled under content/dev/maps); run under POSTRETRO_SH_STREAMING=off and =sync-proof"]
fn d4_d5_first_hand_over_and_a_one_to_zero_step_render_within_two_levels_of_the_next_frame() {
    let map = std::env::var("MOVER_CLOSET_PRL").expect("set MOVER_CLOSET_PRL");
    let mode = std::env::var("POSTRETRO_SH_STREAMING").unwrap_or_default();
    let id = door_id(&map);
    let door = |t: f32| [MoverLightState { mover_id: id, t }];

    // D4 (P24): the door held at its seeded terminus, first handed over on a
    // captured frame.
    let mut capture = prepare(&map);
    let unstated = capture.capture_frame().expect("frame without state");
    capture.hand_over_mover_light_states(&door(0.0));
    let first = capture.capture_frame().expect("first hand-over frame");
    let after = capture.capture_frame().expect("the frame after");
    let first_delta = max_channel_delta(&first, &after);
    let state_delta = max_channel_delta(&unstated, &after);
    eprintln!(
        "D4 ({mode}): first hand-over against the frame after: {first_delta}; \
         the state's own effect: {state_delta}"
    );
    assert!(
        first_delta <= 2,
        "first hand-over frame moved {first_delta} levels"
    );
    assert!(
        state_delta > 2,
        "the closed door's state must move the frame"
    );

    // D5 (P25): t′ steps from 1 to 0 on one frame, then holds.
    let mut capture = prepare(&map);
    capture.hand_over_mover_light_states(&door(1.0));
    let open = capture.capture_frame().expect("t′ = 1");
    let _ = capture.capture_frame().expect("t′ = 1 held");
    capture.hand_over_mover_light_states(&door(0.0));
    let step = capture.capture_frame().expect("the 1 → 0 step frame");
    let held = capture.capture_frame().expect("t′ held at 0");
    let step_delta = max_channel_delta(&step, &held);
    let travel_delta = max_channel_delta(&open, &held);
    eprintln!(
        "D5 ({mode}): the 1 → 0 step frame against the next: {step_delta}; \
         t′ 1 against 0: {travel_delta}"
    );
    assert!(step_delta <= 2, "the step frame moved {step_delta} levels");
    assert!(travel_delta > 2, "t′ 1 and 0 must differ");
}
