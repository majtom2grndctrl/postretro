// A scheduled agent stepping every 30th tick (the AI tick's dormant tier, 2 Hz):
// its mover carries it and doors push it every tick, its own step covers the
// ticks since its last, and at 2 Hz it passes no geometry it stops at under
// 60 Hz.
// See: context/plans/in-progress/E10--ai-tick-scheduling/index.md (Mover carry
//      and pushes reach a dormant NPC every tick; P29)
//      context/lib/movement.md §6

use std::collections::HashMap;

use glam::Vec3;
use postretro_foundation::{GroundRef, KnockbackResponse};

use super::mover_contact_tests::{
    CARRY_TOLERANCE, DT, ENV, MOVER, MoverTrack, RADIUS, agent, floor_world, position, rest_y,
    solid_box, spawn,
};
use super::*;
use crate::collision::CollisionWorld;
use crate::collision::moving::{CombinedCollisionWorld, MoverCollider};

/// Ticks between a dormant agent's steps: 2 Hz.
const STRIDE: u32 = 30;

/// The real NPC tick scheduler over a two-cell level: the agents stand in cell
/// 0, which no perceiver sees; a far pawn in cell 1 sees only its own cell. A
/// scheduled agent carries a brain whose identity puts its dormant run on the
/// requested phase, so the tiers choose its steps as they do in production.
struct Schedule {
    scheduler: crate::npc_tiers::NpcScheduler,
    level: crate::npc_perception::test_support::Row,
    pawn: Option<EntityId>,
    identities: HashMap<EntityId, u64>,
    /// The ticks stepped so far: the upcoming step is tick `tick`.
    tick: u32,
}

impl Schedule {
    fn new() -> Self {
        let level = crate::npc_perception::test_support::Row {
            width: 10_000.0,
            residency: Some(crate::npc_perception::test_support::residency(
                2,
                32.0,
                |camera| vec![(camera, 0.0)],
            )),
        };
        Self {
            scheduler: crate::npc_tiers::NpcScheduler::new(
                crate::npc_perception::NpcPerception::new(
                    2,
                    0,
                    crate::npc_perception::GhostLinkCells::default(),
                ),
            ),
            level,
            pawn: None,
            identities: HashMap::new(),
            tick: 0,
        }
    }

    /// Make `id` an unseen NPC whose dormant runs fall on upcoming ticks
    /// `phase`, `phase + 30`, and so on.
    fn dormant(&mut self, registry: &mut EntityRegistry, id: EntityId, phase: u32) {
        let phase = u64::from(phase % STRIDE);
        // The tiers count from 1 on the first step, so step `t` is tier tick
        // `t + 1`.
        let identity = (0..)
            .find(|&identity| crate::npc_tiers::dormant_runs_on(phase + 1, identity))
            .expect("some identity takes every phase");
        let graph =
            crate::scripting::builtins::data_archetype_test_fixtures::behavior_enemy_descriptor(
                "dormant_walker",
            )
            .behavior
            .unwrap();
        let mut brain = postretro_entities::components::brain::BrainComponent::from_graph(&graph);
        brain.schedule_identity = Some(identity);
        registry.set_component(id, brain).unwrap();
        if self.pawn.is_none() {
            self.pawn = Some(registry.spawn(postretro_entities::Transform {
                position: Vec3::new(15_000.0, 0.0, 0.0),
                ..Default::default()
            }));
        }
        self.identities.insert(id, identity);
    }

    /// Choose this tick's tiers, as the sim tick does before the AI stage.
    fn choose(&mut self, registry: &EntityRegistry) {
        let crate::npc_tiers::NpcScheduler { perception, tiers } = &mut self.scheduler;
        perception.step(&self.level, registry, self.pawn);
        tiers.choose(
            registry,
            perception,
            &crate::npc_tiers::TierInputs {
                level: &self.level,
                descriptors: &[],
                descriptor_generation: 0,
            },
        );
    }

    /// The step `id` takes on the tick just chosen.
    fn step(&self, id: EntityId) -> AgentStep {
        self.scheduler.tiers.agent_step(id)
    }

    fn finish_tick(&mut self) {
        self.tick += 1;
    }

    /// Whether `id` steps on the upcoming tick.
    fn steps_on(&self, id: EntityId) -> bool {
        self.identities.get(&id).is_none_or(|&identity| {
            crate::npc_tiers::dormant_runs_on(u64::from(self.tick) + 1, identity)
        })
    }
}

fn step_scheduled(
    registry: &mut EntityRegistry,
    world: &CollisionWorld,
    movers: &[MoverCollider],
    track: &MoverTrack,
    schedule: &mut Schedule,
) {
    schedule.choose(registry);
    let collision = CombinedCollisionWorld::new(world, movers, track);
    let lookup = |id| schedule.step(id);
    tick_with_steps(
        registry,
        &collision,
        None,
        AgentSteps::scheduled(&lookup),
        ENV,
        DT,
    );
    schedule.finish_tick();
}

/// A lift top at y = 0.5 that starts, reverses and stops under a rider.
fn lift_course(tick: u32) -> Vec3 {
    match tick {
        20..80 => Vec3::Y * 2.0,
        80..110 => Vec3::NEG_Y * 2.0,
        _ => Vec3::ZERO,
    }
}

fn dormant_ride(goal: Option<Vec3>, phase: u32) {
    let world = floor_world();
    let movers = [solid_box(
        Vec3::new(-20.0, -0.5, -8.0),
        Vec3::new(20.0, 0.0, 8.0),
    )];
    let mut lift = MoverTrack::at(Vec3::new(0.0, 0.5, 0.0));
    let mut registry = EntityRegistry::new();
    let id = spawn(&mut registry, Vec3::new(0.0, rest_y(0.5), 0.0), goal);
    let mut schedule = Schedule::new();
    schedule.dormant(&mut registry, id, phase);

    // Settle onto the lift with one ordinary tick.
    let lookup = |_| AgentStep::Ticks(1);
    let collision = CombinedCollisionWorld::new(&world, &movers, &lift);
    tick_with_steps(
        &mut registry,
        &collision,
        None,
        AgentSteps::scheduled(&lookup),
        ENV,
        DT,
    );
    assert_eq!(agent(&registry, id).ground, GroundRef::Mover(MOVER));

    let mut before = position(&registry, id);
    for tick_index in 0..140 {
        let velocity = lift_course(tick_index);
        lift.advance(velocity);
        step_scheduled(&mut registry, &world, &movers, &lift, &mut schedule);
        let at = position(&registry, id);
        assert_eq!(
            agent(&registry, id).ground,
            GroundRef::Mover(MOVER),
            "phase {phase}, tick {tick_index}: mover ground"
        );
        // Carried exactly once each tick, step ticks included.
        let drift = (at.y - before.y) - velocity.y * DT;
        assert!(
            drift.abs() < CARRY_TOLERANCE,
            "phase {phase}, tick {tick_index}: rider drifted {drift} from the carry"
        );
        before = at;
    }
}

#[test]
fn dormant_rider_is_carried_once_per_tick() {
    for phase in [0, 1, 7, 19, 29] {
        dormant_ride(None, phase);
        dormant_ride(Some(Vec3::new(15.0, rest_y(0.5), 0.0)), phase);
    }
}

#[test]
fn dormant_agent_pushed_by_a_door_is_never_embedded() {
    let world = floor_world();
    // A sweeper's near face at local x = 0.5, moving +X into an idle agent.
    let movers = [solid_box(
        Vec3::new(0.0, 0.0, -20.0),
        Vec3::new(0.5, 3.0, 20.0),
    )];
    for phase in [0, 13, 29] {
        let mut sweeper = MoverTrack::at(Vec3::new(-2.0, 0.0, 0.0));
        let mut registry = EntityRegistry::new();
        let id = spawn(&mut registry, Vec3::new(0.0, rest_y(0.0), 0.0), None);
        let mut schedule = Schedule::new();
        schedule.dormant(&mut registry, id, phase);

        for tick_index in 0..120 {
            sweeper.advance(Vec3::X * 2.0);
            step_scheduled(&mut registry, &world, &movers, &sweeper, &mut schedule);
            let face = sweeper.pose.transform.position.x + 0.5;
            let at = position(&registry, id);
            assert!(
                at.x - RADIUS >= face - 1.0e-3,
                "phase {phase}, tick {tick_index}: the sweeper embedded the agent: \
                 face {face}, agent {at:?}"
            );
            assert!(
                at.y > rest_y(0.0) - 0.05,
                "phase {phase}, tick {tick_index}: pushed through the floor, at {at:?}"
            );
        }
        assert!(
            position(&registry, id).x > 1.0,
            "phase {phase}: the sweeper pushed the agent along"
        );
    }
}

#[test]
fn dormant_rider_released_by_a_leaving_mover_no_later_than_its_next_step() {
    let world = floor_world();
    // A 2 m platform 0.2 m over the floor that runs out from under its rider.
    let movers = [solid_box(
        Vec3::new(-1.0, -0.2, -8.0),
        Vec3::new(1.0, 0.0, 8.0),
    )];
    let drive = Vec3::X * 3.0;
    for phase in [0, 11, 29] {
        let mut lift = MoverTrack::at(Vec3::new(0.0, 0.2, 0.0));
        let mut registry = EntityRegistry::new();
        // Walking off sideways, as `stepping_off_a_moving_mover_adds_release_
        // velocity_that_decays_with_knockback_drag` does at 60 Hz; an idle
        // rider rides on.
        let id = spawn(
            &mut registry,
            Vec3::new(0.0, rest_y(0.2), 0.0),
            Some(Vec3::new(0.0, rest_y(0.0), 40.0)),
        );
        let lookup = |_| AgentStep::Ticks(1);
        let collision = CombinedCollisionWorld::new(&world, &movers, &lift);
        tick_with_steps(
            &mut registry,
            &collision,
            None,
            AgentSteps::scheduled(&lookup),
            ENV,
            DT,
        );
        let mut schedule = Schedule::new();
        schedule.dormant(&mut registry, id, phase);

        let mut releases = 0;
        let mut previous_ground = agent(&registry, id).ground;
        for tick_index in 0..200 {
            lift.advance(drive);
            let steps_now = schedule.steps_on(id);
            step_scheduled(&mut registry, &world, &movers, &lift, &mut schedule);
            let rider = agent(&registry, id);
            if previous_ground == GroundRef::Mover(MOVER) && rider.ground != previous_ground {
                assert!(
                    steps_now,
                    "phase {phase}, tick {tick_index}: released between steps"
                );
                releases += 1;
                assert!(
                    (rider.knockback_velocity.x - drive.x).abs() < 1.0e-3,
                    "phase {phase}: release velocity applied once: {:?}",
                    rider.knockback_velocity
                );
            }
            previous_ground = rider.ground;
            let at = position(&registry, id);
            assert!(
                at.y > rest_y(0.0) - 0.05,
                "phase {phase}, tick {tick_index}: dropped through the floor, at {at:?}"
            );
        }
        assert_eq!(releases, 1, "phase {phase}: released exactly once");
    }
}

/// The floor plus a wall across +X at `wall_x`, 3 m high.
fn walled_world(wall_x: f32) -> CollisionWorld {
    let span = 60.0;
    CollisionWorld::from_triangles_for_test(
        vec![
            Vec3::new(-span, 0.0, -span),
            Vec3::new(span, 0.0, -span),
            Vec3::new(span, 0.0, span),
            Vec3::new(-span, 0.0, span),
            Vec3::new(wall_x, 0.0, -span),
            Vec3::new(wall_x, 3.0, -span),
            Vec3::new(wall_x, 3.0, span),
            Vec3::new(wall_x, 0.0, span),
        ],
        vec![[0, 2, 1], [0, 3, 2], [4, 5, 6], [4, 6, 7]],
    )
}

#[test]
fn dormant_walker_and_faller_never_pass_a_wall_or_floor() {
    let wall_x = 5.0;
    let world = walled_world(wall_x);
    for phase in [0, 17] {
        let mut registry = EntityRegistry::new();
        // A walker heading through the wall, and a faller dropped from 12 m.
        let walker = spawn(
            &mut registry,
            Vec3::new(0.0, rest_y(0.0), 0.0),
            Some(Vec3::new(20.0, rest_y(0.0), 0.0)),
        );
        let faller = spawn(&mut registry, Vec3::new(-10.0, 12.0, 0.0), None);
        let mut schedule = Schedule::new();
        schedule.dormant(&mut registry, walker, phase);
        schedule.dormant(&mut registry, faller, phase + 3);
        let track = MoverTrack::at(Vec3::new(0.0, -100.0, 0.0));
        for tick_index in 0..240 {
            step_scheduled(&mut registry, &world, &[], &track, &mut schedule);
            let at = position(&registry, walker);
            assert!(
                at.x + RADIUS <= wall_x + 1.0e-3,
                "phase {phase}, tick {tick_index}: the walker passed the wall, at {at:?}"
            );
            let fell = position(&registry, faller);
            assert!(
                fell.y >= rest_y(0.0) - 0.05,
                "phase {phase}, tick {tick_index}: the faller passed the floor, at {fell:?}"
            );
        }
        assert!(
            position(&registry, walker).x + RADIUS > wall_x - 0.2,
            "phase {phase}: the walker reached the wall"
        );
        assert!(
            agent(&registry, faller).ground.is_grounded(),
            "phase {phase}: the faller landed"
        );
    }
}

/// How far an idle agent shoved at `shove` travels in `ticks` ticks.
fn knocked_travel(drag: f32, shove: Vec3, dormant: bool, ticks: u32) -> f32 {
    let world = floor_world();
    let track = MoverTrack::at(Vec3::new(0.0, -100.0, 0.0));
    let mut registry = EntityRegistry::new();
    let start = Vec3::new(0.0, rest_y(0.0), 0.0);
    let id = spawn(&mut registry, start, None);
    // Ground the agent first.
    let mut schedule = Schedule::new();
    step_scheduled(&mut registry, &world, &[], &track, &mut schedule);
    let mut shoved = agent(&registry, id).clone();
    shoved.knockback = KnockbackResponse {
        ground_drag: drag,
        air_drag: drag,
        ..KnockbackResponse::default()
    };
    shoved.knockback_velocity = shove;
    shoved.velocity += shove;
    registry.set_component(id, shoved).unwrap();
    let from = position(&registry, id);
    if dormant {
        // Its first step lands at the end of the first full stride: the
        // tiers count its last step as the tick before it became dormant.
        let phase = (schedule.tick + STRIDE - 1) % STRIDE;
        schedule.dormant(&mut registry, id, phase);
    }
    for _ in 0..ticks {
        step_scheduled(&mut registry, &world, &[], &track, &mut schedule);
    }
    (position(&registry, id) - from).length()
}

#[test]
fn dormant_knockback_travels_as_far_as_at_60_hz_for_every_drag() {
    let shove = Vec3::new(8.0, 0.0, 0.0);
    // Two whole strides, so both bodies have moved for the same ticks.
    let ticks = STRIDE * 2;
    let one_step = shove.length() * DT * STRIDE as f32;
    for drag in [0.0, 0.5, 1.0, 2.0, 4.0, 8.0, 30.0] {
        let ticked = knocked_travel(drag, shove, false, ticks);
        let stepped = knocked_travel(drag, shove, true, ticks);
        assert!(
            (ticked - stepped).abs() <= one_step,
            "drag {drag}: 60 Hz {ticked} m, 2 Hz {stepped} m, one step {one_step} m"
        );
        assert!(
            (ticked - stepped).abs() <= 0.05 * ticked.max(0.05),
            "drag {drag}: 60 Hz {ticked} m, 2 Hz {stepped} m differ past 5%"
        );
    }
}

/// The floor plus a staircase climbing +X from `x0`: `count` steps, each
/// `rise` high and `run` deep, then a landing.
fn stair_world(x0: f32, count: u32, rise: f32, run: f32) -> CollisionWorld {
    let span = 20.0;
    let mut points = vec![
        Vec3::new(-span, 0.0, -span),
        Vec3::new(x0, 0.0, -span),
        Vec3::new(x0, 0.0, span),
        Vec3::new(-span, 0.0, span),
    ];
    let mut triangles = vec![[0, 2, 1], [0, 3, 2]];
    let mut quad = |points: &mut Vec<Vec3>, corners: [Vec3; 4], up: bool| {
        let base = points.len() as u32;
        points.extend(corners);
        if up {
            triangles.push([base, base + 2, base + 1]);
            triangles.push([base, base + 3, base + 2]);
        } else {
            triangles.push([base, base + 1, base + 2]);
            triangles.push([base, base + 2, base + 3]);
        }
    };
    for step in 0..=count {
        let x = x0 + step as f32 * run;
        let low = step as f32 * rise;
        let high = (step + 1).min(count) as f32 * rise;
        let depth = if step == count { span } else { run };
        if step < count {
            // Riser facing -X.
            quad(
                &mut points,
                [
                    Vec3::new(x, low, -span),
                    Vec3::new(x, high, -span),
                    Vec3::new(x, high, span),
                    Vec3::new(x, low, span),
                ],
                false,
            );
        }
        // Tread.
        quad(
            &mut points,
            [
                Vec3::new(x, high, -span),
                Vec3::new(x + depth, high, -span),
                Vec3::new(x + depth, high, span),
                Vec3::new(x, high, span),
            ],
            true,
        );
    }
    CollisionWorld::from_triangles_for_test(points, triangles)
}

/// Ticks a walker takes up a staircase, at 60 Hz or dormant.
fn stair_climb(dormant: bool, phase: u32) -> Option<u32> {
    let (x0, count, rise, run) = (2.0, 8, 0.25, 0.4);
    let world = stair_world(x0, count, rise, run);
    let top = count as f32 * rise;
    let landing_x = x0 + count as f32 * run + 1.0;
    let track = MoverTrack::at(Vec3::new(0.0, -100.0, 0.0));
    let mut registry = EntityRegistry::new();
    let id = spawn(
        &mut registry,
        Vec3::new(0.0, rest_y(0.0), 0.0),
        Some(Vec3::new(landing_x + 4.0, rest_y(top), 0.0)),
    );
    let mut schedule = Schedule::new();
    if dormant {
        schedule.dormant(&mut registry, id, phase);
    }
    for tick_index in 0..600 {
        step_scheduled(&mut registry, &world, &[], &track, &mut schedule);
        let at = position(&registry, id);
        // Never inside the staircase: below the tread under it.
        let steps_under = ((at.x - x0) / run).floor().clamp(-1.0, count as f32 - 1.0);
        let tread = (steps_under + 1.0).max(0.0) * rise;
        let under_floor = if at.x < x0 { 0.0 } else { tread };
        assert!(
            at.y >= rest_y(under_floor) - rise - 0.05,
            "dormant {dormant}, tick {tick_index}: inside the stairs at {at:?}"
        );
        if at.x >= landing_x && (at.y - rest_y(top)).abs() < 0.1 {
            return Some(tick_index);
        }
    }
    None
}

#[test]
fn dormant_walker_climbs_stairs_about_as_fast_as_at_60_hz() {
    let ticked = stair_climb(false, 0).expect("climbs at 60 Hz");
    for phase in [0, 9, 23] {
        let stepped = stair_climb(true, phase);
        eprintln!("60 Hz {ticked} ticks; dormant phase {phase}: {stepped:?}");
        let stepped = stepped.expect("climbs dormant");
        assert!(
            stepped <= ticked + 2 * STRIDE,
            "phase {phase}: 60 Hz {ticked} ticks, dormant {stepped}"
        );
    }
}

// P18: stuck detection counts elapsed ticks: a dormant walker wedged on a
// wall fires recovery on the first step after its count reaches the
// threshold, never on an earlier one. Recovery clears the plan latch, and a
// 2 Hz step spends the whole tangent window inside it.
#[test]
fn dormant_wedge_fires_recovery_on_the_first_step_past_the_threshold() {
    let world = walled_world(2.0);
    let track = MoverTrack::at(Vec3::new(0.0, -100.0, 0.0));
    let mut registry = EntityRegistry::new();
    let goal = Vec3::new(20.0, rest_y(0.0), 0.0);
    let walker = spawn(&mut registry, Vec3::new(0.0, rest_y(0.0), 0.0), Some(goal));
    let mut schedule = Schedule::new();
    schedule.dormant(&mut registry, walker, 0);
    let mut stuck_before_step = 0;
    let mut fired = Vec::new();
    for tick_index in 0..600 {
        let steps_now = schedule.steps_on(walker);
        step_scheduled(&mut registry, &world, &[], &track, &mut schedule);
        if !steps_now {
            continue;
        }
        let mut walker_state = agent(&registry, walker).clone();
        if walker_state.planned_destination.is_none() {
            assert!(
                stuck_before_step >= STUCK_TICKS_THRESHOLD,
                "tick {tick_index}: recovery fired at {stuck_before_step} stuck ticks"
            );
            fired.push(tick_index);
            // Re-latch the plan, as an admitted replan would.
            walker_state.planned_destination = Some(goal);
            registry
                .set_component(walker, walker_state.clone())
                .unwrap();
        }
        stuck_before_step = walker_state.stuck_ticks;
    }
    assert!(!fired.is_empty(), "the wedged walker tried to recover");
}
