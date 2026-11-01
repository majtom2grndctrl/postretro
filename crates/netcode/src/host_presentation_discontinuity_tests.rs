// Host-seat discontinuity proof: a relocated connected-client pawn cuts on the
// delayed presentation timeline, never blends, whatever relocated it.
// See: context/lib/networking.md §Host-side remote-pawn presentation

use glam::{Quat, Vec3};
use postretro_entities::{RespawnPlacement, RespawnPlacements, RespawnRequest, ScriptCtx};
use postretro_foundation::{PlayerMovementComponent, Seat};
use postretro_level_format::environment_volumes::{EnvironmentBrush, EnvironmentPlane};
use postretro_level_format::teleporters::{TeleportMode, TeleporterRecord, TeleportersSection};
use postretro_physics::movement::MovementInput;
use postretro_physics::teleporters::Teleporters;

use super::*;

use crate::predict_reconcile_harness_test_fixtures::{
    GRAVITY, component, entity_descriptors, floor_world, forward_command, idle_command,
    net_slot_provenance,
};

const TICK: f32 = 1.0 / 60.0;
const ALPHAS: [f32; 5] = [0.0, 0.25, 0.5, 0.75, 0.99];
const FRAMES: u32 = 30;
const POS_EPS: f32 = 1.0e-4;
const YAW_EPS: f32 = 1.0e-4;
/// The cut tick's end pose may settle by a skin distance after relocation.
const ARRIVAL_EPS: f32 = 1.0e-2;
const CLIENT: u64 = 7;
const SEAT: Seat = Seat(1);

const TELEPORT_START: Vec3 = Vec3::new(0.0, 1.21, -5.0);
const TELEPORT_DESTINATION: Vec3 = Vec3::new(40.0, 1.25, 0.0);
/// Turns the traveller to +X, so it walks on away from the source after arrival.
const TELEPORT_YAW: f32 = -std::f32::consts::FRAC_PI_2;
const DEATH_SPOT: Vec3 = Vec3::new(-20.0, 1.21, 10.0);
const PLACEMENT_FOOT: Vec3 = Vec3::new(12.0, 0.0, -4.0);
const PLACEMENT_YAW: f32 = -0.6;
const RESPAWN_TICK: u32 = 6;

/// One cause of a connected-client pawn's relocation, driven through its
/// production path. A new relocation cause adds one case and inherits the proof.
struct Discontinuity {
    name: &'static str,
    start: Vec3,
    velocity: Vec3,
    /// The pawn's resolved movement command, before any relocation turns it.
    input: MovementInput,
    /// Runs before each fixed tick with the tick about to run.
    arm: fn(&Rig, u32),
    /// Body point and facing the relocation ends at.
    arrival: Vec3,
    arrival_yaw: f32,
}

fn teleporter_arrival() -> Discontinuity {
    Discontinuity {
        name: "teleporter arrival",
        start: TELEPORT_START,
        velocity: Vec3::NEG_Z * 11.0,
        input: forward_command(false).movement,
        arm: |_, _| {},
        arrival: TELEPORT_DESTINATION,
        arrival_yaw: TELEPORT_YAW,
    }
}

fn respawn() -> Discontinuity {
    Discontinuity {
        name: "respawn",
        start: DEATH_SPOT,
        velocity: Vec3::ZERO,
        input: idle_command().movement,
        // The verb queues during one tick; the next tick's start drains it.
        arm: |rig, tick| {
            if tick == RESPAWN_TICK {
                rig.ctx.pending_respawns.borrow_mut().insert(
                    rig.pawn,
                    RespawnRequest {
                        pawn: rig.pawn,
                        health: None,
                        state: None,
                    },
                );
            }
        },
        // Foot anchor plus the fixture capsule's radius and half height.
        arrival: PLACEMENT_FOOT + Vec3::Y * (0.4 + 0.8),
        arrival_yaw: PLACEMENT_YAW,
    }
}

/// A classic teleporter across the traveller's path, z in [-8, -6].
fn teleporter() -> TeleporterRecord {
    let (min, max) = (Vec3::new(-3.0, 0.0, -8.0), Vec3::new(3.0, 3.0, -6.0));
    let planes = (0..3)
        .flat_map(|axis| {
            let mut normal = Vec3::ZERO;
            normal[axis] = 1.0;
            [
                EnvironmentPlane {
                    normal: normal.to_array(),
                    distance: max[axis],
                },
                EnvironmentPlane {
                    normal: (-normal).to_array(),
                    distance: -min[axis],
                },
            ]
        })
        .collect();
    TeleporterRecord {
        source_entity: 100,
        mode: TeleportMode::Classic,
        enabled_on_spawn: true,
        brushes: vec![EnvironmentBrush {
            aabb_min: min.to_array(),
            aabb_max: max.to_array(),
            planes,
        }],
        destination_origin: TELEPORT_DESTINATION.to_array(),
        destination_yaw: TELEPORT_YAW,
        landmark_origin: [0.0; 3],
        landmark_yaw: 0.0,
        arrival_sound: String::new(),
        on_teleport: String::new(),
    }
}

/// One authoritative end-of-tick pose as the host recorded it.
struct Sample {
    tick: u32,
    transform: Transform,
    generation: u8,
}

/// One drawn pose: the delayed target it was sampled at and what the render read.
struct Drawn {
    target: f64,
    alpha: f32,
    transform: Transform,
}

/// The host seat with one connected client's pawn, the teleporter installed and
/// a respawn placement published.
struct Rig {
    ctx: ScriptCtx,
    world: CollisionWorld,
    owners: MovementOwners,
    allocator: NetworkIdAllocator,
    buffer: RemoteInterpolationBuffer,
    pawn: EntityId,
    input: MovementInput,
    tick: u32,
}

impl Rig {
    fn new(case: &Discontinuity) -> Self {
        let ctx = ScriptCtx::new();
        ctx.data_registry
            .borrow_mut()
            .replace_entity_types(entity_descriptors());
        *ctx.respawn_placements.borrow_mut() = RespawnPlacements {
            seats: [(SEAT, 0)].into_iter().collect(),
            map_order: vec![RespawnPlacement {
                map_index: 0,
                foot_anchor: PLACEMENT_FOOT,
                yaw: PLACEMENT_YAW,
            }],
            first_player: Some(0),
        };
        let mut world = floor_world();
        world.teleporters = Teleporters::from_section(&TeleportersSection {
            teleporters: vec![teleporter()],
        });
        let pawn = {
            let mut registry = ctx.registry.borrow_mut();
            let pawn = registry.spawn(Transform {
                position: case.start,
                ..Default::default()
            });
            let mut movement = component();
            movement.velocity = case.velocity;
            registry.set_component(pawn, movement).unwrap();
            registry.set_component(pawn, net_slot_provenance()).unwrap();
            registry.bind_pawn_seat(pawn, SEAT);
            pawn
        };
        let mut owners = MovementOwners::default();
        owners.set(pawn, CLIENT);
        let mut allocator = NetworkIdAllocator::new();
        let _network_id = allocator.stamp(pawn);
        Self {
            ctx,
            world,
            owners,
            allocator,
            buffer: RemoteInterpolationBuffer::default(),
            pawn,
            input: case.input.clone(),
            tick: 0,
        }
    }

    /// One authoritative tick in the host frame loop's order: tick start (snapshot,
    /// teleporter mutations, respawn drain), arrival yaw against the resolved
    /// facing, movement, then the pose record and observer drain.
    fn fixed_tick(&mut self) -> Sample {
        let pending = crate::sim::begin_tick(&self.ctx.registry, Some(&self.ctx));
        let (pawn, facing) = (self.pawn, self.input.facing_yaw);
        let (_started, turns) = crate::sim::finish_tick_start(
            pending,
            &mut self.ctx.registry.borrow_mut(),
            &self.world,
            move |id| (id == pawn).then_some(facing),
        );
        for turn in turns.iter().filter(|turn| turn.pawn == pawn) {
            self.input.facing_yaw += turn.yaw_delta;
        }
        let events = crate::sim::run_host_movement_tick(
            &mut self.ctx.registry.borrow_mut(),
            &self.world,
            postretro_collision::EnvironmentView::level_only(GRAVITY),
            &[(pawn, self.input.clone())],
            TICK,
        );
        // The client's view turns through a teleport; its next commands carry it.
        for (_, events) in events.iter().filter(|(id, _)| *id == pawn) {
            if let Some(teleport) = events.teleport {
                self.input.facing_yaw += teleport.yaw_delta;
            }
        }
        let registry = self.ctx.registry.borrow();
        crate::host_record_client_pawn_poses(
            &mut self.buffer,
            &self.owners,
            &self.allocator,
            &registry,
            self.tick,
        );
        crate::host_take_client_pawn_teleports(&mut self.buffer, &self.owners, &self.allocator);
        let sample = Sample {
            tick: self.tick,
            transform: *registry.get_component::<Transform>(pawn).unwrap(),
            generation: registry
                .get_component::<PlayerMovementComponent>(pawn)
                .unwrap()
                .teleport
                .generation,
        };
        drop(registry);
        self.tick += 1;
        sample
    }

    /// One host frame of one tick: restore the authoritative pose, tick, then
    /// present at each render alpha and read what the render collector draws.
    fn frame(&mut self, arm: fn(&Rig, u32), samples: &mut Vec<Sample>, drawn: &mut Vec<Drawn>) {
        crate::host_restore_client_pawn_authoritative_poses(
            &self.buffer,
            &self.owners,
            &self.allocator,
            &mut self.ctx.registry.borrow_mut(),
        );
        arm(self, self.tick);
        samples.push(self.fixed_tick());
        for alpha in ALPHAS {
            let mut registry = self.ctx.registry.borrow_mut();
            crate::host_present_client_pawns(
                &self.buffer,
                &self.owners,
                &self.allocator,
                &mut registry,
                self.tick,
                alpha,
            );
            drawn.push(Drawn {
                target: presentation_target_tick(self.tick, alpha, PRESENTATION_DELAY_TICKS),
                alpha,
                transform: registry.interpolated_transform(self.pawn, alpha).unwrap(),
            });
        }
    }
}

fn segment_distance(point: Vec3, a: Vec3, b: Vec3) -> f32 {
    let span = b - a;
    let s = if span.length_squared() > 0.0 {
        ((point - a).dot(span) / span.length_squared()).clamp(0.0, 1.0)
    } else {
        0.0
    };
    (point - (a + span * s)).length()
}

/// Distance from `point` to the path through `samples`' positions in tick order.
fn path_distance(samples: &[Sample], point: Vec3) -> f32 {
    match samples {
        [only] => (point - only.transform.position).length(),
        _ => samples
            .windows(2)
            .map(|pair| {
                segment_distance(
                    point,
                    pair[0].transform.position,
                    pair[1].transform.position,
                )
            })
            .fold(f32::INFINITY, f32::min),
    }
}

/// Compares facing directions: `Quat::angle_between` goes through `acos` near 1,
/// where one ulp of rounding reads as ~1e-3 rad.
fn same_facing(a: Quat, b: Quat) -> bool {
    (a * Vec3::NEG_Z - b * Vec3::NEG_Z).length() < YAW_EPS
}

fn same_pose(a: &Transform, b: &Transform) -> bool {
    (a.position - b.position).length() < POS_EPS && same_facing(a.rotation, b.rotation)
}

/// A discontinuity is a cut, never a blend, on the host's delayed timeline. Until
/// the delayed target reaches the relocation tick the host draws the pawn on its
/// pre-relocation path, holding the last pre-relocation pose across the final
/// span; at the relocation tick it draws exactly the arrival pose; after it, only
/// the post-relocation path. No drawn pose lies inside the source→arrival segment.
fn assert_host_cuts_never_blends(case: &Discontinuity) {
    let name = case.name;
    let mut rig = Rig::new(case);
    let mut samples = Vec::new();
    let mut drawn = Vec::new();
    for _ in 0..FRAMES {
        rig.frame(case.arm, &mut samples, &mut drawn);
    }

    let changes: Vec<usize> = (1..samples.len())
        .filter(|&index| samples[index].generation != samples[index - 1].generation)
        .collect();
    assert_eq!(
        changes.len(),
        1,
        "{name}: fixture relocates exactly once within the run"
    );
    let (pre, post) = samples.split_at(changes[0]);
    let source = pre.last().unwrap().transform;
    let arrival = post[0].transform;
    let arrival_tick = f64::from(post[0].tick);
    assert!(
        (arrival.position - case.arrival).length() < ARRIVAL_EPS,
        "{name}: fixture's cut tick ends at the relocation point, got {:?}",
        arrival.position
    );
    assert!(
        same_facing(arrival.rotation, Quat::from_rotation_y(case.arrival_yaw)),
        "{name}: fixture's cut tick ends facing the relocation yaw"
    );
    assert!(
        (arrival.position - source.position).length() > 10.0,
        "{name}: fixture relocates across the map"
    );

    let span = arrival.position - source.position;
    let mut seen = [false; 4];
    for shown in &drawn {
        let at = shown.transform;
        let (target, alpha, position) = (shown.target, shown.alpha, at.position);
        let context = format!("{name}: target {target:.2} (alpha {alpha}) drew {position:?}");
        let s = (position - source.position).dot(span) / span.length_squared();
        let on_segment = segment_distance(position, source.position, arrival.position) < 0.05;
        assert!(
            !((0.05..=0.95).contains(&s) && on_segment),
            "{context}: blended between source and arrival"
        );
        if shown.target < arrival_tick - 1.0 {
            assert!(
                path_distance(pre, at.position) < POS_EPS,
                "{context}: left the pre-relocation path"
            );
            seen[0] = true;
        } else if shown.target < arrival_tick {
            assert!(
                same_pose(&at, &source),
                "{context}: must hold the last pre-relocation pose {:?}",
                source.position
            );
            seen[1] = true;
        } else if (shown.target - arrival_tick).abs() < 1.0e-9 {
            assert!(
                same_pose(&at, &arrival),
                "{context}: must cut to the arrival pose {:?}",
                arrival.position
            );
            seen[2] = true;
        } else {
            assert!(
                path_distance(post, at.position) < POS_EPS,
                "{context}: left the post-relocation path"
            );
            seen[3] = true;
        }
    }
    assert_eq!(
        seen, [true; 4],
        "{name}: frames cover the lead-in, the held source, the cut and the arrival side"
    );
}

#[test]
fn host_seat_cuts_a_teleporter_arrival_on_the_delayed_timeline_without_blending() {
    assert_host_cuts_never_blends(&teleporter_arrival());
}

// The host draws a respawning remote pawn at its death spot until the delayed
// timeline reaches the respawn tick, then at the placement; never in between.
#[test]
fn host_seat_cuts_a_respawn_on_the_delayed_timeline_without_blending_from_the_death_spot() {
    assert_host_cuts_never_blends(&respawn());
}
