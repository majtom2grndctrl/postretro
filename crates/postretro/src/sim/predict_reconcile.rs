//! Dev-tools-quality seed harness for in-process prediction/reconciliation.
//!
//! This is deliberately not wired into the engine frame path. It exists to
//! exercise the `simulate_tick` seam with two local sims, injected latency, and
//! client-side rewind/replay so Phase 2 has concrete reconciliation material to
//! extend.

#![cfg_attr(not(test), allow(dead_code))]

use std::cell::RefCell;
use std::collections::{HashSet, VecDeque};
use std::rc::Rc;

use glam::{Vec2, Vec3};
use parry3d::math::{Isometry, Point};
use parry3d::shape::TriMesh;

use super::{SimCommand, simulate_tick};
use crate::collision::CollisionWorld;
use crate::movement::MovementInput;
use crate::scripting_systems::hit_zones::HitZoneStore;
use crate::weapon::FireButtonState;
use postretro_entities::{EntityId, EntityRegistry, Transform};
use postretro_foundation::{
    AirParams, BoolOrIr, CapsuleParams, DashParams, FallParams, ForgivenessParams, GroundParams,
    NumberOrIr, PlayerMovementComponent, PlayerMovementDescriptor, SpeedParams,
};
use postretro_scripting_core::reaction_dispatch::ProgressTracker;

const DT: f32 = 1.0 / 60.0;
const GRAVITY: f32 = -20.0;
const DEFAULT_START: Vec3 = Vec3::new(0.0, 1.21, 0.0);
const RECONCILE_EPSILON: f32 = 0.001;

/// Compact command source for the replay prototype. It is intentionally smaller
/// than the eventual network command; `to_sim_command` expands it at the seam.
#[derive(Debug, Clone, Copy)]
pub(crate) struct PrototypeCommand {
    pub(crate) wish_dir: Vec2,
    pub(crate) jump_pressed: bool,
    pub(crate) dash_pressed: bool,
    pub(crate) running: bool,
    pub(crate) crouch_intent: bool,
    pub(crate) facing_yaw: f32,
}

impl PrototypeCommand {
    fn to_sim_command(self) -> SimCommand {
        SimCommand {
            movement: MovementInput {
                wish_dir: self.wish_dir,
                jump_pressed: self.jump_pressed,
                dash_pressed: self.dash_pressed,
                running: self.running,
                crouch_intent: self.crouch_intent,
                facing_yaw: self.facing_yaw,
            },
            fire_button: FireButtonState {
                pressed: false,
                active: false,
            },
        }
    }

    fn to_post_movement_command(self) -> super::PostMovementCommand {
        super::PostMovementCommand {
            aim_origin: Vec3::new(0.0, 2.0, 0.0),
            aim_direction: Vec3::new(self.facing_yaw.sin(), 0.0, -self.facing_yaw.cos())
                .normalize(),
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct PrototypeConfig {
    pub(crate) one_way_latency_ticks: u32,
    pub(crate) jitter_ticks: Vec<i32>,
    pub(crate) client_start_offset: Vec3,
}

impl PrototypeConfig {
    fn delay_for(&self, tick: u32, salt: u32) -> u32 {
        let base = self.one_way_latency_ticks as i32;
        let jitter = if self.jitter_ticks.is_empty() {
            0
        } else {
            let index = ((tick + salt) as usize) % self.jitter_ticks.len();
            self.jitter_ticks[index]
        };
        (base + jitter).max(0) as u32
    }
}

#[derive(Debug, Clone)]
struct HarnessSnapshot {
    tick: u32,
    transform: Transform,
    movement: PlayerMovementComponent,
}

impl HarnessSnapshot {
    fn horizontal_position(&self) -> Vec3 {
        self.transform.position
    }
}

struct PrototypeHarness {
    registry: Rc<RefCell<EntityRegistry>>,
    world: CollisionWorld,
    hit_zones: HitZoneStore,
    progress: ProgressTracker,
    ai_warned: HashSet<String>,
    player_id: EntityId,
}

impl PrototypeHarness {
    fn new(start: Vec3) -> Self {
        let registry = Rc::new(RefCell::new(EntityRegistry::new()));
        let player_id = {
            let mut registry = registry.borrow_mut();
            spawn_player(&mut registry, start)
        };
        Self {
            registry,
            world: floor_world(),
            hit_zones: HitZoneStore::new(),
            progress: ProgressTracker::new(),
            ai_warned: HashSet::new(),
            player_id,
        }
    }

    fn tick(&mut self, command: PrototypeCommand) {
        let sim_command = command.to_sim_command();
        simulate_tick(
            self.registry.clone(),
            &self.world,
            &self.hit_zones,
            None,
            GRAVITY,
            None,
            0.0,
            &mut self.progress,
            &mut self.ai_warned,
            &sim_command,
            |_| command.to_post_movement_command(),
            DT,
        );
    }

    fn snapshot(&self, tick: u32) -> HarnessSnapshot {
        let registry = self.registry.borrow();
        let transform = *registry
            .get_component::<Transform>(self.player_id)
            .expect("prototype player must keep its transform");
        let movement = registry
            .get_component::<PlayerMovementComponent>(self.player_id)
            .expect("prototype player must keep movement state")
            .clone();
        HarnessSnapshot {
            tick,
            transform,
            movement,
        }
    }

    fn restore(&mut self, snapshot: HarnessSnapshot) {
        let mut registry = self.registry.borrow_mut();
        registry
            .set_component(self.player_id, snapshot.transform)
            .expect("prototype player transform restore must succeed");
        registry
            .set_component(self.player_id, snapshot.movement)
            .expect("prototype player movement restore must succeed");
    }
}

#[derive(Debug, Clone)]
struct ClientHistoryEntry {
    tick: u32,
    command: PrototypeCommand,
    predicted: HarnessSnapshot,
}

#[derive(Debug, Clone, Copy)]
struct ScheduledCommand {
    arrival_tick: u32,
    input_tick: u32,
    command: PrototypeCommand,
}

#[derive(Debug, Clone)]
struct ScheduledSnapshot {
    arrival_tick: u32,
    snapshot: HarnessSnapshot,
}

#[derive(Debug, Clone)]
pub(crate) struct ReconcileCorrection {
    pub(crate) ack_tick: u32,
    pub(crate) ack_error: f32,
    pub(crate) visible_delta: f32,
    pub(crate) replayed_ticks: u32,
    pub(crate) replay_included_dash: bool,
}

#[derive(Debug, Clone)]
pub(crate) struct PrototypeReport {
    pub(crate) final_position_error: f32,
    pub(crate) corrections: Vec<ReconcileCorrection>,
    pub(crate) max_visible_correction: f32,
    pub(crate) max_dash_visible_correction: f32,
}

pub(crate) fn run_replay(
    commands: &[PrototypeCommand],
    config: &PrototypeConfig,
) -> PrototypeReport {
    let mut prototype = PredictReconcilePrototype::new(config.clone());
    prototype.run(commands)
}

struct PredictReconcilePrototype {
    config: PrototypeConfig,
    client: PrototypeHarness,
    server: PrototypeHarness,
    history: Vec<ClientHistoryEntry>,
    server_commands: VecDeque<ScheduledCommand>,
    authoritative_snapshots: VecDeque<ScheduledSnapshot>,
    corrections: Vec<ReconcileCorrection>,
    last_server_snapshot: HarnessSnapshot,
}

impl PredictReconcilePrototype {
    fn new(config: PrototypeConfig) -> Self {
        let client = PrototypeHarness::new(DEFAULT_START + config.client_start_offset);
        let server = PrototypeHarness::new(DEFAULT_START);
        let last_server_snapshot = server.snapshot(0);
        Self {
            config,
            client,
            server,
            history: Vec::new(),
            server_commands: VecDeque::new(),
            authoritative_snapshots: VecDeque::new(),
            corrections: Vec::new(),
            last_server_snapshot,
        }
    }

    fn run(&mut self, commands: &[PrototypeCommand]) -> PrototypeReport {
        for (tick, command) in commands.iter().copied().enumerate() {
            let tick = tick as u32;
            self.deliver_authority(tick);
            self.predict_client(tick, command);
            self.enqueue_command(tick, command);
            self.advance_server(tick);
        }

        let mut drain_tick = commands.len() as u32;
        while !self.server_commands.is_empty() || !self.authoritative_snapshots.is_empty() {
            self.deliver_authority(drain_tick);
            self.advance_server(drain_tick);
            drain_tick += 1;
        }

        let final_client = self
            .client
            .snapshot(commands.len().saturating_sub(1) as u32);
        let final_position_error = position_error(&final_client, &self.last_server_snapshot);
        let max_visible_correction = self
            .corrections
            .iter()
            .map(|c| c.visible_delta)
            .fold(0.0, f32::max);
        let max_dash_visible_correction = self
            .corrections
            .iter()
            .filter(|c| c.replay_included_dash)
            .map(|c| c.visible_delta)
            .fold(0.0, f32::max);

        PrototypeReport {
            final_position_error,
            corrections: self.corrections.clone(),
            max_visible_correction,
            max_dash_visible_correction,
        }
    }

    fn predict_client(&mut self, tick: u32, command: PrototypeCommand) {
        self.client.tick(command);
        let predicted = self.client.snapshot(tick);
        self.history.push(ClientHistoryEntry {
            tick,
            command,
            predicted,
        });
    }

    fn enqueue_command(&mut self, tick: u32, command: PrototypeCommand) {
        self.server_commands.push_back(ScheduledCommand {
            arrival_tick: tick + self.config.delay_for(tick, 0),
            input_tick: tick,
            command,
        });
    }

    fn advance_server(&mut self, tick: u32) {
        while self
            .server_commands
            .front()
            .is_some_and(|scheduled| scheduled.arrival_tick <= tick)
        {
            let scheduled = self
                .server_commands
                .pop_front()
                .expect("front checked above");
            self.server.tick(scheduled.command);
            let snapshot = self.server.snapshot(scheduled.input_tick);
            self.last_server_snapshot = snapshot.clone();
            self.authoritative_snapshots.push_back(ScheduledSnapshot {
                arrival_tick: tick + self.config.delay_for(scheduled.input_tick, 7),
                snapshot,
            });
        }
    }

    fn deliver_authority(&mut self, tick: u32) {
        while self
            .authoritative_snapshots
            .front()
            .is_some_and(|scheduled| scheduled.arrival_tick <= tick)
        {
            let scheduled = self
                .authoritative_snapshots
                .pop_front()
                .expect("front checked above");
            self.reconcile(scheduled.snapshot);
        }
    }

    fn reconcile(&mut self, authoritative: HarnessSnapshot) {
        let Some(acked_index) = self
            .history
            .iter()
            .position(|entry| entry.tick == authoritative.tick)
        else {
            return;
        };
        let predicted_at_ack = self.history[acked_index].predicted.clone();
        let ack_error = position_error(&predicted_at_ack, &authoritative);
        if ack_error <= RECONCILE_EPSILON {
            return;
        }

        let pre_reconcile_current = self.client.snapshot(authoritative.tick);
        self.client.restore(authoritative.clone());

        let replay_start = acked_index + 1;
        let replay_included_dash = self.history[acked_index..]
            .iter()
            .any(|entry| entry.command.dash_pressed);
        for index in replay_start..self.history.len() {
            let tick = self.history[index].tick;
            let command = self.history[index].command;
            self.client.tick(command);
            self.history[index].predicted = self.client.snapshot(tick);
        }

        let ack_tick = authoritative.tick;
        self.history[acked_index].predicted = authoritative;
        let post_reconcile_current = self.client.snapshot(ack_tick);
        self.corrections.push(ReconcileCorrection {
            ack_tick,
            ack_error,
            visible_delta: position_error(&pre_reconcile_current, &post_reconcile_current),
            replayed_ticks: self.history.len().saturating_sub(replay_start) as u32,
            replay_included_dash,
        });
    }
}

fn spawn_player(registry: &mut EntityRegistry, position: Vec3) -> EntityId {
    let id = registry.spawn(Transform {
        position,
        ..Transform::default()
    });
    registry
        .set_component(
            id,
            PlayerMovementComponent::from_descriptor(&player_descriptor()),
        )
        .expect("prototype player movement component should attach");
    id
}

fn player_descriptor() -> PlayerMovementDescriptor {
    PlayerMovementDescriptor {
        capsule: CapsuleParams {
            radius: 0.4,
            half_height: 0.8,
            eye_height: 0.5,
        },
        ground: GroundParams {
            speed: SpeedParams {
                walk: 7.0,
                run: 11.0,
                crouch: 3.0,
            },
            accel: 10.0,
            step_height: 0.3,
            max_slope: 45.0,
        },
        air: AirParams {
            forward_steer: 0.0,
            accel: 0.7,
            max_control_speed: 0.5,
            bunny_hop: false,
            jumps: 0,
            jump_velocity: 5.5,
            jump_ceiling: 0.0,
        },
        fall: FallParams {
            terminal_velocity: 40.0,
        },
        stuck_stop_enabled: PlayerMovementDescriptor::DEFAULT_STUCK_STOP_ENABLED,
        stuck_stop_threshold: PlayerMovementDescriptor::DEFAULT_STUCK_STOP_THRESHOLD,
        dash: Some(DashParams {
            boost_speed: NumberOrIr::Literal(18.0),
            momentum_retention: NumberOrIr::Literal(0.65),
            steer_control: NumberOrIr::Literal(0.2),
            dash_drag: NumberOrIr::Literal(18.0),
            cooldown_ms: NumberOrIr::Literal(250.0),
            air_dashes: 0,
            preserve_vertical: BoolOrIr::Literal(false),
        }),
        forgiveness: Some(ForgivenessParams {
            coyote_ms: 0.0,
            jump_buffer_ms: 0.0,
        }),
        crouch: None,
        view_feel: None,
    }
}

fn floor_world() -> CollisionWorld {
    let points = vec![
        Point::new(-500.0, 0.0, -500.0),
        Point::new(500.0, 0.0, -500.0),
        Point::new(500.0, 0.0, 500.0),
        Point::new(-500.0, 0.0, 500.0),
    ];
    let triangles = vec![[0, 2, 1], [0, 3, 2]];
    CollisionWorld {
        mesh: TriMesh::new(points, triangles),
        isometry: Isometry::identity(),
    }
}

fn position_error(a: &HarnessSnapshot, b: &HarnessSnapshot) -> f32 {
    (a.horizontal_position() - b.horizontal_position()).length()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn replay_commands() -> Vec<PrototypeCommand> {
        (0..120)
            .map(|tick| PrototypeCommand {
                wish_dir: if tick < 70 {
                    Vec2::new(0.0, 1.0)
                } else if tick < 95 {
                    Vec2::new(0.35, 0.8)
                } else {
                    Vec2::ZERO
                },
                jump_pressed: false,
                dash_pressed: matches!(tick, 4 | 48),
                running: tick < 90,
                crouch_intent: false,
                facing_yaw: if tick < 80 { 0.0 } else { 0.3 },
            })
            .collect()
    }

    #[test]
    fn predict_reconcile_rewinds_to_authority_and_replays_local_history() {
        let report = run_replay(
            &replay_commands(),
            &PrototypeConfig {
                one_way_latency_ticks: 6,
                jitter_ticks: vec![0, 2, -1, 1],
                client_start_offset: Vec3::new(0.35, 0.0, 0.0),
            },
        );

        assert!(
            report.final_position_error <= RECONCILE_EPSILON,
            "client should converge after the last authoritative ack; error={}",
            report.final_position_error
        );
        assert!(
            !report.corrections.is_empty(),
            "initial client offset should force at least one reconciliation"
        );
        assert!(
            report.corrections.iter().any(|c| c.replayed_ticks > 0),
            "delayed authority should require replaying predicted commands"
        );
        assert!(
            report.corrections.iter().all(|c| c.ack_error > 0.0),
            "recorded corrections should include the ack-time prediction error"
        );
    }

    #[test]
    fn predict_reconcile_records_dash_replay_corrections_under_jitter() {
        let report = run_replay(
            &replay_commands(),
            &PrototypeConfig {
                one_way_latency_ticks: 8,
                jitter_ticks: vec![2, -2, 1, 0, -1],
                client_start_offset: Vec3::new(0.35, 0.0, 0.0),
            },
        );

        assert!(
            report.max_visible_correction > 0.25,
            "offset should produce a visible correction metric"
        );
        assert!(
            report.max_dash_visible_correction > 0.25,
            "first authoritative ack should rewind across an already-predicted dash"
        );
        assert!(
            report
                .corrections
                .iter()
                .any(|c| c.replay_included_dash && c.replayed_ticks > 0),
            "dash correction should be measured on a replay window, not only an ack replacement"
        );
        assert!(
            report.corrections.iter().any(|c| c.ack_tick <= 4),
            "early delayed acks should cover the first dash window"
        );
    }
}
