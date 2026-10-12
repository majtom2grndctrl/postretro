// Co-op swim parity through the loopback harness: swim entry and exit, the fluid jump and its latch, and fluid crossings into low gravity, all predicted with no correction.
// See: context/lib/networking.md · context/plans/in-progress/E24--water-and-environment-volumes/index.md (Co-op parity)

#![cfg(test)]

use glam::Vec3;
use postretro_collision::{BodyProbe, EnvironmentVolume, resolve_environment};
use postretro_foundation::{
    FluidDescriptor, FluidRegistry, MovementStateKind, PlayerMovementComponent, SwimParams,
};
use postretro_net::harness::LinkConfig;

use super::predict_reconcile_harness_test_fixtures::{
    LoopbackHarness, NamedEnvironmentVolume, forward_command, gravity_box_volume, idle_command,
};
use crate::emission;
use crate::sim::SimCommand;

/// "No corrections" (brief Co-op Decision): every recorded correction is at most
/// this many metres under `light_link`.
const NO_CORRECTION_M: f32 = 1.0e-4;
const LOW_GRAVITY: f32 = -3.0;
const HOST_WATER_VISCOSITY: f32 = 2.0;

/// The course runs along -Z from the pawn's start: a deep pool, then a shallow
/// stretch whose immersion sits between the swim thresholds (a swimmer stays in
/// swim there, a walker never enters), its far face abutting a low-gravity room.
const POOL_MIN: Vec3 = Vec3::new(-50.0, -1.0, -14.0);
const POOL_MAX: Vec3 = Vec3::new(50.0, 1.9, -6.0);
const SHALLOWS_MIN: Vec3 = Vec3::new(-50.0, -1.0, -20.0);
const SHALLOWS_MAX: Vec3 = Vec3::new(50.0, 1.0, -14.0);
const LOW_GRAVITY_MIN: Vec3 = Vec3::new(-50.0, -1.0, -60.0);
const LOW_GRAVITY_MAX: Vec3 = Vec3::new(50.0, 20.0, -20.0);

fn light_link() -> LinkConfig {
    LinkConfig {
        delay: 32,
        jitter: 0,
        loss_probability: 0.0,
        seed: 0x6010,
    }
}

fn swim() -> SwimParams {
    SwimParams {
        speed: 4.5,
        accel: 6.0,
        sink_speed: 0.8,
        enter_depth: 0.5,
        exit_depth: 0.35,
        fluid_jump_velocity: 6.5,
    }
}

fn fluid(name: &str, viscosity: f32) -> FluidDescriptor {
    FluidDescriptor {
        name: name.to_string(),
        viscosity,
        view_tint: [0.1, 0.3, 0.4, 0.35],
        muffle: 0.8,
        surface_opacity: 0.6,
        enter_sound: None,
        exit_sound: None,
    }
}

fn manifest(fluids: Vec<FluidDescriptor>) -> FluidRegistry {
    FluidRegistry::from_declarations("harness", fluids.into_iter().map(Ok))
}

fn host_fluids() -> FluidRegistry {
    manifest(vec![fluid("water", HOST_WATER_VISCOSITY)])
}

/// The level both peers load: the pool and shallows name `water`; the room
/// sets gravity.
fn course() -> Vec<NamedEnvironmentVolume> {
    let water = |min: Vec3, max: Vec3| NamedEnvironmentVolume {
        volume: EnvironmentVolume {
            gravity: None,
            ..gravity_box_volume(min, max, 0.0)
        },
        fluid: Some("water".to_string()),
    };
    vec![
        water(POOL_MIN, POOL_MAX),
        water(SHALLOWS_MIN, SHALLOWS_MAX),
        NamedEnvironmentVolume {
            volume: gravity_box_volume(LOW_GRAVITY_MIN, LOW_GRAVITY_MAX, LOW_GRAVITY),
            fluid: None,
        },
    ]
}

fn in_box(position: Vec3, min: Vec3, max: Vec3) -> bool {
    position.cmpge(min).all() && position.cmple(max).all()
}

/// One client tick of a run, as the client predicted (and reconciled) it.
#[derive(Debug, Clone, Copy)]
struct ClientTick {
    tick: u32,
    state: MovementStateKind,
    latched: bool,
    /// The tick-END position's resolve, i.e. the next tick's tick-start view.
    immersion: f32,
    in_low_gravity: bool,
}

struct SwimRun {
    corrections: Vec<f32>,
    windows: Vec<(Option<u32>, Option<u32>)>,
    ticks: Vec<ClientTick>,
    /// Every swim-edge emission the client's frame drain received, by client
    /// tick: `(tick, address, fluid)`.
    swim_emissions: Vec<(u32, &'static str, Option<u16>)>,
    host_swam: bool,
    host_reached_low_gravity: bool,
}

/// Arm, then drive `steps` commands chosen from the client's predicted
/// position, recording every correction, reconcile window and client tick.
fn run_course(
    client_fluids: FluidRegistry,
    steps: u32,
    command_at: impl Fn(Vec3) -> SimCommand,
) -> SwimRun {
    let mut h =
        LoopbackHarness::with_fluid_tables(light_link(), course(), host_fluids(), client_fluids)
            .with_player_swim(swim());
    h.step_until_armed(&idle_command());
    let mut run = SwimRun {
        corrections: Vec::new(),
        windows: Vec::new(),
        ticks: Vec::new(),
        swim_emissions: Vec::new(),
        host_swam: false,
        host_reached_low_gravity: false,
    };
    for _ in 0..steps {
        let position = h.client_position().expect("the client is armed");
        h.step(&command_at(position));
        run.corrections
            .extend(h.latest_local_corrections.iter().copied());
        run.windows
            .extend(h.latest_reconcile_windows.iter().copied());
        run.host_swam |= h
            .host_registry
            .get_component::<PlayerMovementComponent>(h.host_pawn)
            .is_ok_and(|movement| movement.movement_state.kind() == MovementStateKind::Swim);
        run.host_reached_low_gravity |= in_box(h.host_position(), LOW_GRAVITY_MIN, LOW_GRAVITY_MAX);

        let pawn = h.client_pawn.expect("armed");
        let movement = h
            .client_registry
            .get_component::<PlayerMovementComponent>(pawn)
            .expect("client pawn movement")
            .clone();
        let position = h.client_position().expect("armed");
        // The client's movement drain input, built from forward prediction as
        // the frame loop builds it. Reconcile replay has no route here.
        let mut emissions = Vec::new();
        emission::append_movement_emissions(
            &h.latest_predicted_movement_events,
            &emission::entity_emitter(&h.client_registry, pawn),
            &mut emissions,
        );
        let tick = h.last_sent_client_tick.expect("sent this step");
        run.swim_emissions.extend(
            emissions
                .iter()
                .filter(|emission| emission.address.starts_with("swim_"))
                .map(|emission| (tick, emission.address, emission.fluid)),
        );
        let view = h.client_environment_view().expect("tuning installed");
        let resolved = resolve_environment(
            view,
            &BodyProbe {
                position,
                half_height: movement.capsule.half_height,
                radius: movement.capsule.radius,
                eye_height: movement.capsule.eye_height,
            },
        );
        run.ticks.push(ClientTick {
            tick: h.last_sent_client_tick.expect("sent this step"),
            state: movement.movement_state.kind(),
            latched: movement.swim_reentry_latched,
            immersion: resolved.immersion,
            in_low_gravity: in_box(position, LOW_GRAVITY_MIN, LOW_GRAVITY_MAX),
        });
    }
    run
}

fn assert_no_correction(run: &SwimRun) {
    assert!(
        run.corrections.len() > 30,
        "reconciles must run throughout; saw {}",
        run.corrections.len()
    );
    let worst = run.corrections.iter().copied().fold(0.0_f32, f32::max);
    assert!(
        worst <= NO_CORRECTION_M,
        "every correction must be at most {NO_CORRECTION_M} m; worst {worst}"
    );
}

fn state_at(run: &SwimRun, tick: u32) -> Option<MovementStateKind> {
    run.ticks
        .iter()
        .find(|record| record.tick == tick)
        .map(|record| record.state)
}

/// The windows `(acked, last_sent]` whose replay covers every tick in `ticks`,
/// with the acked baseline in `baseline` as the client recorded it.
fn windows_spanning(run: &SwimRun, ticks: &[u32], baseline: MovementStateKind) -> usize {
    run.windows
        .iter()
        .filter(|window| match **window {
            (Some(acked), Some(last_sent)) => {
                ticks.iter().all(|tick| acked < *tick && *tick <= last_sent)
                    && state_at(run, acked) == Some(baseline)
            }
            _ => false,
        })
        .count()
}

/// The first client tick whose predicted state became `to` from another state.
fn first_edge_into(run: &SwimRun, to: MovementStateKind) -> u32 {
    run.ticks
        .windows(2)
        .find(|pair| pair[0].state != to && pair[1].state == to)
        .map(|pair| pair[1].tick)
        .unwrap_or_else(|| panic!("the client never entered {to:?}"))
}

fn first_edge_out_of(run: &SwimRun, from: MovementStateKind) -> u32 {
    run.ticks
        .windows(2)
        .find(|pair| pair[0].state == from && pair[1].state != from)
        .map(|pair| pair[1].tick)
        .unwrap_or_else(|| panic!("the client never left {from:?}"))
}

/// Run along the course; hop only once past the shallows, in the low-gravity
/// room.
fn swim_through_then_hop(position: Vec3) -> SimCommand {
    let mut command = forward_command(false);
    command.movement.jump_pressed = position.z < -21.0;
    command
}

// N1 (fluid half) and N2 (P3, P4): a client swims into a pool, on through
// shallows that only a swimmer stays swimming in, out of their far face and
// straight into a low-gravity room, with zero corrections; reconciles replay
// across the swim entry from a `Normal` baseline and across the exit plus the
// low-gravity entry from a `Swim` baseline.
#[test]
fn swim_and_low_gravity_crossings_reconcile_without_correction_under_light_link() {
    let run = run_course(host_fluids(), 300, swim_through_then_hop);
    assert!(run.host_swam, "fixture: the host pawn must swim");
    assert!(
        run.host_reached_low_gravity,
        "fixture: the host must reach the room"
    );
    assert_no_correction(&run);

    let swim_entry = first_edge_into(&run, MovementStateKind::Swim);
    let swim_exit = first_edge_out_of(&run, MovementStateKind::Swim);
    let low_gravity_entry = run
        .ticks
        .iter()
        .find(|record| record.in_low_gravity)
        .map(|record| record.tick)
        .expect("the client reached the low-gravity room");
    assert!(swim_entry < swim_exit && swim_exit <= low_gravity_entry + 1);

    // P3: acked baseline `Normal`, replay enters swim.
    assert!(
        windows_spanning(&run, &[swim_entry], MovementStateKind::Normal) > 0,
        "a replay from a Normal baseline must span swim entry tick {swim_entry}"
    );
    // P4: acked baseline `Swim`, replay exits swim and enters the room.
    assert!(
        windows_spanning(
            &run,
            &[swim_exit, low_gravity_entry],
            MovementStateKind::Swim
        ) > 0,
        "a replay from a Swim baseline must span exit {swim_exit} and room entry {low_gravity_entry}"
    );
    // In the shallows the swim state is not derivable from immersion: only the
    // replicated `Swim` baseline keeps the replay swimming there.
    let band: Vec<u32> = run
        .ticks
        .iter()
        .filter(|record| {
            record.state == MovementStateKind::Swim
                && record.immersion >= swim().exit_depth
                && record.immersion < swim().enter_depth
        })
        .map(|record| record.tick)
        .collect();
    assert!(
        band.iter()
            .any(|tick| windows_spanning(&run, &[*tick], MovementStateKind::Swim) > 0),
        "a replay from a Swim baseline must cross the between-threshold shallows: {band:?}"
    );
    // The client actually hopped under the room's gravity.
    assert!(
        run.ticks
            .iter()
            .any(|record| record.in_low_gravity && record.state == MovementStateKind::Normal),
        "the client must move through the room after leaving the pool"
    );
}

// P5, replay half: the client's movement drain receives exactly one swim
// entry and one swim exit, each from the forward-predicted tick that crossed,
// naming the host's `water` (index 0). Reconciles replay across both crossings
// (from a `Normal` baseline into swim, from a `Swim` baseline out of it) and
// through the between-threshold shallows, yet add no emission: the drain's only
// client input is forward prediction, so no replay can reach a sound request.
#[test]
fn reconcile_replay_across_swim_edges_adds_no_swim_emission() {
    let run = run_course(host_fluids(), 300, swim_through_then_hop);
    assert_no_correction(&run);
    let swim_entry = first_edge_into(&run, MovementStateKind::Swim);
    let swim_exit = first_edge_out_of(&run, MovementStateKind::Swim);
    let replays_into_swim = windows_spanning(&run, &[swim_entry], MovementStateKind::Normal);
    let replays_out_of_swim = windows_spanning(&run, &[swim_exit], MovementStateKind::Swim);
    assert!(
        replays_into_swim > 0 && replays_out_of_swim > 0,
        "fixture: replays must re-run entry {swim_entry} ({replays_into_swim}) and exit \
         {swim_exit} ({replays_out_of_swim})"
    );
    assert_eq!(
        run.swim_emissions,
        [
            (swim_entry, "swim_started", Some(0)),
            (swim_exit, "swim_ended", Some(0)),
        ],
        "one emission per crossing, from forward prediction only"
    );
}

/// Wade into the pool, then hold jump from 3 m in: the pawn swims up, leaps
/// out, falls back in and leaps again.
fn swim_with_jump_held(position: Vec3) -> SimCommand {
    let mut command = forward_command(false);
    command.movement.jump_pressed = position.z < -9.0;
    command
}

/// Ticks on which the client's predicted pawn was latched out of swim while
/// immersed at or past `enter_depth`: the latch alone kept it from swimming.
fn latched_deep_ticks(run: &SwimRun) -> Vec<u32> {
    run.ticks
        .iter()
        .filter(|record| record.latched && record.immersion >= swim().enter_depth)
        .map(|record| record.tick)
        .collect()
}

// Fluid jumps and the P10 re-entry latch predict exactly, and a reconcile
// whose replay starts from a latched baseline restores the latch from the wire.
#[test]
fn fluid_jump_and_reentry_latch_reconcile_without_correction_under_light_link() {
    let run = run_course(host_fluids(), 240, swim_with_jump_held);
    assert!(run.host_swam, "fixture: the host pawn must swim");
    assert_no_correction(&run);

    // Each fluid jump arms the latch. Holding jump, a pawn that falls back in
    // with its eye still out leaps again on its entry tick.
    let leaps = run
        .ticks
        .windows(2)
        .filter(|pair| !pair[0].latched && pair[1].latched)
        .count();
    assert!(
        leaps >= 2,
        "the client must leap out more than once: {leaps}"
    );
    let latched = latched_deep_ticks(&run);
    assert!(
        !latched.is_empty(),
        "fixture: some tick must be latched while past enter_depth"
    );
    // Some reconcile replays from a latched baseline across such a tick.
    assert!(
        latched
            .iter()
            .any(|tick| run.windows.iter().any(|window| matches!(
                *window,
                (Some(acked), Some(last_sent))
                    if acked < *tick
                        && *tick <= last_sent
                        && run.ticks.iter().any(|r| r.tick == acked && r.latched)
            ))),
        "a replay must start from a latched baseline and cross latched-deep ticks {latched:?}"
    );
}

// N10, corrections half: the client predicts swim with the host's fluid,
// whatever its own manifest declares.
#[test]
fn client_with_differing_fluid_declarations_swims_with_zero_corrections() {
    for (label, client_fluids) in [
        ("no water", FluidRegistry::default()),
        ("different numbers", manifest(vec![fluid("water", 9.0)])),
        (
            "different order",
            manifest(vec![
                fluid("sludge", 6.5),
                fluid("water", HOST_WATER_VISCOSITY),
            ]),
        ),
    ] {
        let run = run_course(client_fluids, 240, swim_with_jump_held);
        assert!(run.host_swam, "{label}: fixture: the host pawn must swim");
        assert!(
            run.ticks
                .iter()
                .any(|record| record.state == MovementStateKind::Swim),
            "{label}: the client must predict swim"
        );
        assert_no_correction(&run);
    }
}
