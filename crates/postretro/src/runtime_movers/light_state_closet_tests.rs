// C9, game half, on the compiled `mover-closet`: the door driven by ticks,
// not forced, opens, reverses at mid-travel and closes, and every frame hands
// the renderer the t′ its author ramp gives that frame's post-tick phase. The
// ramp comes from the PRL (id 57's ramp record), so the 0.6/0.9 variant
// proves the keys parsed from the map. The renderer half composes and writes
// live tiles at these t′ (`renderer` `mover_channel::closet_tests`).
// See: context/lib/rendering_pipeline.md §4 (Mover state lighting)

use postretro_entities::{EntityRegistry, KinematicMoverComponent, MoverCommand, Transform};
use postretro_level_loader::LevelWorld;

use super::{KinematicMoverRenderCollector, mover_entity, spawn_loaded_kinematic_movers};
use crate::kinematic_mover::{MoverTickStateTable, apply_mover_command, run_kinematic_mover_tick};

const TICK_DT: f32 = 1.0 / 60.0;
/// Frames presented per tick: the second must hand over the same t′ (P10).
const FRAMES_PER_TICK: usize = 2;
const EPSILON: f32 = 2.0e-4;

fn load(var: &str) -> LevelWorld {
    let path = std::env::var(var).unwrap_or_else(|_| panic!("set {var} to a compiled PRL"));
    postretro_level_loader::load_prl(&path).unwrap_or_else(|err| panic!("load {path}: {err}"))
}

/// The door's travel fraction read from its drawn transform alone: the door
/// slides straight up from its first waypoint to its last.
fn drawn_fraction(registry: &EntityRegistry, world: &LevelWorld, mover_id: u32) -> f32 {
    let entity = mover_entity(registry, mover_id).expect("the door is present");
    let mover = registry
        .get_component::<KinematicMoverComponent>(entity)
        .expect("door component");
    let position = registry
        .get_component::<Transform>(entity)
        .expect("door transform")
        .position;
    let (from, to) = (mover.waypoints[0], mover.waypoints[1]);
    assert!(
        world.kinematic_geometry.movers.len() == 1 && mover.waypoints.len() == 2,
        "the closet holds one two-waypoint door"
    );
    (position - from).dot(to - from) / (to - from).length_squared()
}

fn ramped(t: f32, start: f32, end: f32) -> f32 {
    ((t - start) / (end - start)).clamp(0.0, 1.0)
}

/// Drive the closet door by ticks: open from load, reverse once its drawn t
/// reaches `reverse_at`, then close until it docks at its first waypoint.
/// Every presented frame checks the handed-over t′. Returns (drawn t, t′)
/// per tick.
fn drive(world: &LevelWorld, ramp: (f32, f32), reverse_at: f32) -> Vec<(f32, f32)> {
    let door = &world.kinematic_geometry.movers[0];
    assert_eq!(
        (door.light_ramp.start, door.light_ramp.end),
        ramp,
        "the ramp the PRL carries"
    );
    let mover_id = door.mover_id;
    let mut registry = EntityRegistry::new();
    spawn_loaded_kinematic_movers(&mut registry, world, super::ENGINE_AUTO_CLOSE_MS)
        .expect("closet movers spawn");
    let mut table = MoverTickStateTable::default();
    let mut collector = KinematicMoverRenderCollector::new();
    let mut handed = Vec::new();
    let mut reversed = false;
    for tick in 0..2_000 {
        if tick > 0 {
            run_kinematic_mover_tick(&mut registry, &mut table, TICK_DT);
        }
        let t = drawn_fraction(&registry, world, mover_id);
        for frame in 0..FRAMES_PER_TICK {
            collector.collect_light_states(&registry, Some(world));
            let states = collector.light_states();
            assert_eq!(states.len(), 1, "tick {tick}: one present mover");
            assert_eq!(states[0].mover_id, mover_id);
            let expected = ramped(t, ramp.0, ramp.1);
            assert!(
                (states[0].t - expected).abs() <= EPSILON,
                "tick {tick} frame {frame}: handed {} for drawn t {t} (expected {expected})",
                states[0].t
            );
            if frame == 0 {
                handed.push((t, states[0].t));
            }
        }
        if !reversed && t >= reverse_at {
            let entity = mover_entity(&registry, mover_id).expect("the door");
            let mut component = registry
                .get_component::<KinematicMoverComponent>(entity)
                .expect("door component")
                .clone();
            apply_mover_command(&mut component, &MoverCommand::Reverse);
            registry.set_component(entity, component).expect("reverse");
            reversed = true;
        } else if reversed && t <= 0.0 {
            return handed;
        }
    }
    panic!("the door never closed again");
}

#[test]
#[ignore = "requires MOVER_CLOSET_PRL and MOVER_CLOSET_RAMP_PRL: content/dev/maps/mover-closet.map compiled, and the same map with `light_ramp_start` 0.6 and `light_ramp_end` 0.9 on `closet_door`"]
fn c9_a_tick_driven_closet_door_hands_over_its_ramped_post_tick_t_every_frame() {
    let world = load("MOVER_CLOSET_PRL");
    let handed: Vec<f32> = drive(&world, (0.0, 1.0), 0.5)
        .into_iter()
        .map(|(_, t)| t)
        .collect();
    let peak = handed.iter().copied().fold(0.0f32, f32::max);
    assert!(
        (0.5..0.55).contains(&peak),
        "reverses just past mid-travel: {peak}"
    );
    let peak_at = handed.iter().position(|&t| t == peak).unwrap();
    assert!(
        handed[..=peak_at].windows(2).all(|pair| pair[1] >= pair[0]),
        "rises to the reversal"
    );
    assert!(
        handed[peak_at..].windows(2).all(|pair| pair[1] <= pair[0]),
        "falls after the reversal with no jump"
    );
    let step = handed
        .windows(2)
        .map(|pair| (pair[1] - pair[0]).abs())
        .fold(0.0f32, f32::max);
    assert!(step < 0.01, "no frame jumps: largest step {step}");
    assert_eq!(*handed.last().unwrap(), 0.0, "closes to t′ = 0");
    eprintln!(
        "C9 closet: {} ticks, peak t′ {peak:.4}, largest step {step:.5}",
        handed.len()
    );

    // The ramp variant reverses at t = 0.75, so t′ rises to about 0.5 and
    // returns; every frame at t ≤ 0.6 hands over t′ = 0.
    let ramp_world = load("MOVER_CLOSET_RAMP_PRL");
    let handed = drive(&ramp_world, (0.6, 0.9), 0.75);
    assert!(
        handed
            .iter()
            .filter(|(t, _)| *t <= 0.6)
            .all(|(_, ramped)| *ramped == 0.0),
        "frames at t ≤ 0.6 hand over t′ = 0"
    );
    let peak = handed
        .iter()
        .map(|(_, ramped)| *ramped)
        .fold(0.0f32, f32::max);
    assert!((0.5..0.6).contains(&peak), "ramped peak {peak}");
    eprintln!(
        "C9 closet, ramp 0.6/0.9: {} ticks, peak t′ {peak:.4}",
        handed.len()
    );
}
