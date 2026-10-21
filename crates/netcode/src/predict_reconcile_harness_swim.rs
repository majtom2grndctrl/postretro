// Co-op swim parity through the loopback harness: swim edges, the fluid jump and
// its latch, low-gravity crossings, and push-force currents, none needing correction.
// See: context/lib/networking.md · context/lib/movement.md §4 (Swim), §6 (Regional gravity and push)

#![cfg(test)]

use glam::Vec3;
use postretro_collision::{BodyProbe, EnvironmentVolume, resolve_environment};
use postretro_foundation::{
    FluidDescriptor, FluidRegistry, MovementStateKind, PlayerMovementComponent, SwimParams,
};

use super::predict_reconcile_harness_test_fixtures::{
    LoopbackHarness, NO_CORRECTION_M, NamedEnvironmentVolume, forward_command, gravity_box_volume,
    idle_command, light_link,
};
use crate::emission;
use crate::sim::SimCommand;

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

fn swim() -> SwimParams {
    SwimParams {
        speed: 4.5,
        accel: 6.0,
        sink_speed: 0.8,
        enter_depth: 0.5,
        exit_depth: 0.35,
        fluid_jump_velocity: Some(6.5),
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
        fog_distance: None,
        fog_color: None,
        wave_period: None,
        wave_amplitude: 8.0,
        wave_length: [64.0, 80.0],
        surface_reflectivity: 1.0,
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
    /// The predicted drift layer's vertical part and total vertical velocity,
    /// and its horizontal x part.
    drift_x: f32,
    drift_y: f32,
    velocity_y: f32,
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
    run_records(course(), client_fluids, steps, command_at, |_, _| {})
}

/// [`run_course`] over `records`, calling `before_step(h, step)` before each
/// driven step.
fn run_records(
    records: Vec<NamedEnvironmentVolume>,
    client_fluids: FluidRegistry,
    steps: u32,
    command_at: impl Fn(Vec3) -> SimCommand,
    mut before_step: impl FnMut(&mut LoopbackHarness, &SwimRun),
) -> SwimRun {
    let mut h =
        LoopbackHarness::with_fluid_tables(light_link(), records, host_fluids(), client_fluids)
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
        before_step(&mut h, &run);
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
            drift_x: movement.push_velocity.x,
            drift_y: movement.push_velocity.y,
            velocity_y: movement.velocity.y,
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

// A client swims into a pool, on through
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

    // Acked baseline `Normal`, replay enters swim.
    assert!(
        windows_spanning(&run, &[swim_entry], MovementStateKind::Normal) > 0,
        "a replay from a Normal baseline must span swim entry tick {swim_entry}"
    );
    // Acked baseline `Swim`, replay exits swim and enters the room.
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

// Swim sounds never come from a replay: the client's movement drain receives exactly one swim
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

// Fluid jumps and the re-entry latch predict exactly, and a reconcile
// whose replay starts from a latched baseline restores the latch from the wire.
#[test]
fn fluid_jump_and_reentry_latch_reconcile_without_correction_under_light_link() {
    let run = run_course(host_fluids(), 240, swim_with_jump_held);
    assert!(run.host_swam, "fixture: the host pawn must swim");
    assert_no_correction(&run);

    // Each fluid jump arms the latch, and clears the water so the latch clears
    // on the way up. Holding jump, a pawn that falls back in swims, then leaps
    // again from the surface.
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

// The client predicts swim with the host's fluid,
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

/// An upward current filling the pool and reaching well above its surface: the
/// pool's footprint, from its floor to 6 m.
const CURRENT_MIN: Vec3 = Vec3::new(-50.0, -1.0, -14.0);
const CURRENT_MAX: Vec3 = Vec3::new(50.0, 6.0, -6.0);
const CURRENT_PUSH: Vec3 = Vec3::new(0.0, 9.0, 0.0);

/// The pool with a coincident vertical current; no shallows or room.
fn current_course() -> Vec<NamedEnvironmentVolume> {
    vec![
        NamedEnvironmentVolume {
            volume: EnvironmentVolume {
                gravity: None,
                ..gravity_box_volume(POOL_MIN, POOL_MAX, 0.0)
            },
            fluid: Some("water".to_string()),
        },
        NamedEnvironmentVolume {
            volume: EnvironmentVolume {
                gravity: None,
                push: Some(CURRENT_PUSH),
                ..gravity_box_volume(CURRENT_MIN, CURRENT_MAX, 0.0)
            },
            fluid: None,
        },
    ]
}

/// Walk into the pool and stop inside the current.
fn walk_into_the_current(position: Vec3) -> SimCommand {
    if position.z > -10.0 {
        forward_command(false)
    } else {
        idle_command()
    }
}

// A vertical current while swimming lands in the 3D drift
// layer, lifts the swimmer out of the water (folding the drift into vertical
// velocity) and back in, over and over, and the client predicts all of it with
// zero corrections: the drift's vertical part crosses the wire.
#[test]
fn vertical_current_lifts_a_swimmer_out_and_reconciles_without_correction_under_light_link() {
    let run = run_records(
        current_course(),
        host_fluids(),
        300,
        walk_into_the_current,
        |_, _| {},
    );
    assert!(run.host_swam, "fixture: the host pawn must swim");
    assert_no_correction(&run);
    let settled = CURRENT_PUSH.y / HOST_WATER_VISCOSITY;
    assert!(
        run.ticks
            .iter()
            .any(|record| record.state == MovementStateKind::Swim
                && record.drift_y > 0.5 * settled),
        "the client's swimmer must carry a vertical drift"
    );
    let lifted_out = run
        .ticks
        .windows(2)
        .filter(|pair| {
            pair[0].state == MovementStateKind::Swim
                && pair[1].state == MovementStateKind::Normal
                && pair[1].velocity_y > 0.0
                && pair[1].drift_y == 0.0
        })
        .count();
    assert!(
        lifted_out >= 2,
        "the current must lift the swimmer out of the water repeatedly: {lifted_out}"
    );
    // Some reconcile replays from a `Swim` baseline carrying a vertical drift.
    assert!(
        run.windows.iter().any(|window| matches!(
            *window,
            (Some(acked), Some(_)) if run.ticks.iter().any(|record| record.tick == acked
                && record.state == MovementStateKind::Swim
                && record.drift_y > 0.5 * settled)
        )),
        "a replay must start from a swimming baseline with a vertical drift"
    );
}

/// Walk into the pool and keep swimming forward through it.
fn swim_forward(_position: Vec3) -> SimCommand {
    forward_command(false)
}

// A pawn-tuning change (a weapon pickup) arriving while the
// client swims rebuilds its movement component through the production rebuild,
// which carries the live tick state: no correction, and no second swim entry,
// even when the rebuild lands in a frame with no snapshot to reconcile against.
#[test]
fn pawn_tuning_rebuild_on_a_swimming_client_costs_no_correction_and_no_swim_edge() {
    let mut rebuilds = 0;
    let run = run_records(course(), host_fluids(), 200, swim_forward, |h, run| {
        let swimming_for = run
            .ticks
            .iter()
            .rev()
            .take_while(|record| record.state == MovementStateKind::Swim)
            .count();
        if [10, 40].contains(&swimming_for) {
            h.client_rebuild_movement_from_installed_tuning();
            rebuilds += 1;
        }
    });
    assert_eq!(
        rebuilds, 2,
        "fixture: the client rebuilt twice while swimming"
    );
    assert!(run.host_swam, "fixture: the host pawn must swim");
    assert_no_correction(&run);
    let entries = run
        .swim_emissions
        .iter()
        .filter(|(_, address, _)| *address == "swim_started")
        .count();
    assert_eq!(entries, 1, "one swim entry: {:?}", run.swim_emissions);
}

const SIDEWAYS_PUSH_X: f32 = 6.0;

/// The pool under a sideways current of the given force.
fn sideways_current_course(force: f32) -> Vec<NamedEnvironmentVolume> {
    let mut course = current_course();
    let current = &mut course[1].volume;
    current.push = Some(Vec3::new(SIDEWAYS_PUSH_X, 0.0, 0.0));
    current.force = force;
    course
}

/// Run the sideways current at `force`; the swimmer walks in, then idles in the
/// current until it settles.
fn run_sideways_current(force: f32) -> SwimRun {
    run_records(
        sideways_current_course(force),
        host_fluids(),
        300,
        walk_into_the_current,
        |_, _| {},
    )
}

/// The client's mean drift along the push over the last swimming ticks, once
/// the current has settled (the swimmer idles in it for well over the drift's
/// 1 / viscosity time constant before the run ends).
fn settled_drift_x(run: &SwimRun) -> f32 {
    const WINDOW: usize = 30;
    let tail = &run.ticks[run.ticks.len() - WINDOW..];
    assert!(
        tail.iter()
            .all(|record| record.state == MovementStateKind::Swim),
        "fixture: the swimmer must still be swimming when the drift is read"
    );
    tail.iter().map(|record| record.drift_x).sum::<f32>() / WINDOW as f32
}

// Host movement, client prediction and reconcile replay all read the felt
// push (push × force) from the same per-volume force, so a half-force current
// carries the swimmer at half its settled drift with zero corrections.
#[test]
fn half_force_current_reconciles_without_correction_under_light_link() {
    let half = run_sideways_current(0.5);
    assert!(half.host_swam, "fixture: the host pawn must swim");
    assert_no_correction(&half);
    assert!(
        half.windows.iter().any(|window| window.0.is_some()),
        "reconcile replays must run through the current"
    );

    // A resolve that ignored force would also reconcile cleanly, so compare the
    // settled drift against the same course at full force.
    let full = run_sideways_current(1.0);
    assert!(full.host_swam, "fixture: the host pawn must swim");
    assert_no_correction(&full);
    let full_drift = settled_drift_x(&full);
    let half_drift = settled_drift_x(&half);
    let full_expected = SIDEWAYS_PUSH_X / HOST_WATER_VISCOSITY;
    assert!(
        (full_drift - full_expected).abs() <= 0.05 * full_expected,
        "fixture: a full-force current settles at push / viscosity {full_expected}; got \
         {full_drift}"
    );
    assert!(
        (half_drift - 0.5 * full_drift).abs() <= 0.03 * full_drift,
        "half force must settle at half the full-force drift: half {half_drift}, full {full_drift}"
    );
}
