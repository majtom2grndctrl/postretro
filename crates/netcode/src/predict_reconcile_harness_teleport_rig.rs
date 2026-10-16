// Teleporter scenario rig over the conditioned-link prediction/reconcile harness.
// Installs one level's teleporters on both peers (each binding its own entities),
// drives the production predicted tick, host resolution, host movement and client
// reconcile, and logs what the app would present: view yaw, cues and cuts.
// See: context/lib/networking.md §Host input command queue · context/lib/movement.md §6
//
// Test infrastructure only (`#[cfg(test)]`). The app-side reactions the rig models
// (`camera.yaw += yaw_delta` on a predicted event or an adoption, one cue per
// predicted event or `cue_teleporter`) mirror `frame_loop` and
// `runtime_teleporters::apply_local_adoptions`; they are not re-proved here.

#![cfg(test)]

use std::collections::HashSet;

use glam::{Vec2, Vec3};
use postretro_entities::{EntityId, ScriptCtx, TeleporterComponent, Transform};
use postretro_foundation::{PlayerMovementComponent, TeleportState};
use postretro_level_format::environment_volumes::{EnvironmentBrush, EnvironmentPlane};
use postretro_level_format::teleporters::{TeleportMode, TeleporterRecord, TeleportersSection};
use postretro_net::harness::LinkConfig;
use postretro_physics::movement::TeleportEvent;
use postretro_physics::teleporters::Teleporters;

use super::predict_reconcile_harness_test_fixtures::{
    HostTickTrace, LoopbackHarness, floor_world, forward_command, idle_command,
};
use super::prediction::{CorrectionClass, LocalTeleportAdoption};
use super::replication::host_register_loaded_teleporters;
use crate::sim::SimCommand;

/// 100 ms round trip with no jitter or loss: the brief's clean co-op profile.
pub(super) fn clean_link() -> LinkConfig {
    LinkConfig {
        delay: 50,
        jitter: 0,
        loss_probability: 0.0,
        seed: 0x7e1e,
    }
}

pub(super) const FORWARD: Vec2 = Vec2::new(0.0, 1.0);
pub(super) const IDLE: Vec2 = Vec2::ZERO;

/// One authored teleporter: a single box brush plus its destination poses.
#[derive(Debug, Clone, Copy)]
pub(super) struct Spec {
    mode: TeleportMode,
    enabled_on_spawn: bool,
    region: (Vec3, Vec3),
    destination: (Vec3, f32),
    landmark: (Vec3, f32),
}

impl Spec {
    pub(super) fn classic(region: (Vec3, Vec3), destination: Vec3, yaw: f32) -> Self {
        Self {
            mode: TeleportMode::Classic,
            enabled_on_spawn: true,
            region,
            destination: (destination, yaw),
            landmark: (Vec3::ZERO, 0.0),
        }
    }

    pub(super) fn seamless(
        region: (Vec3, Vec3),
        landmark: (Vec3, f32),
        target: (Vec3, f32),
    ) -> Self {
        Self {
            mode: TeleportMode::Seamless,
            enabled_on_spawn: true,
            region,
            destination: target,
            landmark,
        }
    }

    pub(super) fn disarmed(mut self) -> Self {
        self.enabled_on_spawn = false;
        self
    }

    fn record(&self, index: usize) -> TeleporterRecord {
        let (min, max) = self.region;
        let mut planes = Vec::new();
        for axis in 0..3 {
            let mut normal = Vec3::ZERO;
            normal[axis] = 1.0;
            planes.push(EnvironmentPlane {
                normal: normal.to_array(),
                distance: max[axis],
            });
            planes.push(EnvironmentPlane {
                normal: (-normal).to_array(),
                distance: -min[axis],
            });
        }
        let classic = self.mode == TeleportMode::Classic;
        TeleporterRecord {
            source_entity: 100 + index as u32,
            mode: self.mode,
            enabled_on_spawn: self.enabled_on_spawn,
            brushes: vec![EnvironmentBrush {
                aabb_min: min.to_array(),
                aabb_max: max.to_array(),
                planes,
            }],
            destination_origin: self.destination.0.to_array(),
            destination_yaw: self.destination.1,
            landmark_origin: if classic {
                [0.0; 3]
            } else {
                self.landmark.0.to_array()
            },
            landmark_yaw: if classic { 0.0 } else { self.landmark.1 },
            arrival_sound: format!("sfx/teleport{index}"),
            on_teleport: String::new(),
        }
    }
}

/// An axis box in the floor world's playable band.
pub(super) fn region(x: (f32, f32), z: (f32, f32)) -> (Vec3, Vec3) {
    (Vec3::new(x.0, 0.0, z.0), Vec3::new(x.1, 3.0, z.1))
}

/// A cue the traveller's machine plays: from forward prediction, or from an
/// adoption whose post-replay generation is ahead.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Cue {
    Predicted(u16),
    Adopted(u16),
}

/// One forward-predicted client tick, as sampled and as predicted.
#[derive(Debug, Clone)]
pub(super) struct ClientTick {
    pub(super) client_tick: u32,
    /// The command's sampled facing: the app camera yaw at sampling.
    pub(super) facing: f32,
    pub(super) teleport: Option<TeleportEvent>,
    pub(super) offset_before: Vec3,
    pub(super) offset_after: Vec3,
    pub(super) position_after: Vec3,
    pub(super) yaw_after: f32,
}

/// One local reconcile the client applied.
#[derive(Debug, Clone, Copy)]
pub(super) struct Correction {
    pub(super) class: CorrectionClass,
    pub(super) magnitude: f32,
    /// The host's acked client tick it was reconciled from; every later
    /// predicted tick was replayed.
    pub(super) acked: Option<u32>,
    /// How many client ticks had been predicted when it landed.
    pub(super) after_client_ticks: usize,
}

pub(super) struct Rig {
    pub(super) h: LoopbackHarness,
    host_teleporters: Vec<EntityId>,
    host_tracked: HashSet<EntityId>,
    /// The app camera yaw: what the client samples each command's facing from.
    pub(super) camera_yaw: f32,
    pub(super) ticks: Vec<ClientTick>,
    pub(super) cues: Vec<Cue>,
    pub(super) adoptions: Vec<LocalTeleportAdoption>,
    pub(super) corrections: Vec<Correction>,
    /// The first command tick the client sent after its first pawn record.
    pub(super) first_record_client_tick: Option<u32>,
    /// A host-only displacement applied after every host tick: an authoritative
    /// effect the client never predicts, which keeps corrections settling.
    pub(super) host_nudge: Vec3,
}

impl Rig {
    pub(super) fn new(link: LinkConfig, specs: &[Spec]) -> Self {
        let mut rig = Self::with_unreplicated_teleporters(link, specs);
        rig.replicate_teleporters();
        rig
    }

    /// Installs the teleporters on both peers but leaves their armed state off
    /// the wire until [`Self::replicate_teleporters`]: a joiner whose armed
    /// records have not yet arrived, as the join baseline budget defers them.
    pub(super) fn with_unreplicated_teleporters(link: LinkConfig, specs: &[Spec]) -> Self {
        let section = TeleportersSection {
            teleporters: specs
                .iter()
                .enumerate()
                .map(|(index, spec)| spec.record(index))
                .collect(),
        };
        // Through the PRL codec, as each peer's loader reads it.
        let section = TeleportersSection::from_bytes(&section.to_bytes()).unwrap();
        let mut h = LoopbackHarness::new(link);
        h.world.teleporters = Teleporters::from_section(&section);
        let mut client_world = floor_world();
        client_world.teleporters = Teleporters::from_section(&section);
        let mut host_teleporters = Vec::new();
        for (index, record) in section.teleporters.iter().enumerate() {
            let component = TeleporterComponent {
                index: index as u16,
                source_entity: record.source_entity,
                armed: record.enabled_on_spawn,
                arrival_sound: record.arrival_sound.clone(),
                on_teleport: String::new(),
            };
            let host = h.host_registry.spawn(Transform::default());
            h.host_registry
                .set_component(host, component.clone())
                .unwrap();
            h.world.teleporters.bind_entity(index, host);
            host_teleporters.push(host);
            let client = h.client_registry.spawn(Transform::default());
            h.client_registry.set_component(client, component).unwrap();
            client_world.teleporters.bind_entity(index, client);
        }
        h.client_world = Some(client_world);
        Self {
            h,
            host_teleporters,
            host_tracked: HashSet::new(),
            camera_yaw: 0.0,
            ticks: Vec::new(),
            cues: Vec::new(),
            adoptions: Vec::new(),
            corrections: Vec::new(),
            first_record_client_tick: None,
            host_nudge: Vec3::ZERO,
        }
    }

    pub(super) fn replicate_teleporters(&mut self) {
        host_register_loaded_teleporters(
            &self.h.host_registry,
            &mut self.h.allocator,
            &mut self.h.replicable,
            &mut self.host_tracked,
        );
    }

    /// Idle until the client holds its first pawn record and the host tuning,
    /// then let the stream settle.
    pub(super) fn join(&mut self) {
        let mut guard = 0;
        while (self.h.client_pawn.is_none() || self.h.client_tuning.is_none()) && guard < 200 {
            self.step(IDLE);
            guard += 1;
        }
        assert!(self.h.client_pawn.is_some(), "the client armed its pawn");
        for _ in 0..12 {
            self.step(IDLE);
        }
    }

    pub(super) fn command(&self, wish: Vec2) -> SimCommand {
        let mut command = if wish == IDLE {
            idle_command()
        } else {
            forward_command(false)
        };
        command.movement.wish_dir = wish;
        command.movement.facing_yaw = self.camera_yaw;
        command
    }

    /// Predict and send one tick from the camera, as the app's fixed tick does,
    /// turning the camera on a predicted teleport. `deliver: false` loses the
    /// command on the uplink.
    pub(super) fn predict(&mut self, wish: Vec2, deliver: bool) -> Option<TeleportEvent> {
        let command = self.command(wish);
        let client_tick = self.h.prediction.peek_next_client_tick();
        let offset_before = self.h.prediction.presentation_offset();
        self.h
            .client_predict_and_send_with_delivery(&command, deliver);
        let teleport = self.h.latest_predicted_movement_events.teleport;
        if let Some(event) = teleport {
            self.camera_yaw += event.yaw_delta;
            self.cues.push(Cue::Predicted(event.teleporter));
        }
        let pose = self
            .h
            .client_pawn
            .and_then(|pawn| self.h.client_registry.get_component::<Transform>(pawn).ok())
            .copied()
            .unwrap_or_default();
        self.ticks.push(ClientTick {
            client_tick,
            facing: command.movement.facing_yaw,
            teleport,
            offset_before,
            offset_after: self.h.prediction.presentation_offset(),
            position_after: pose.position,
            yaw_after: yaw_of(pose.rotation),
        });
        teleport
    }

    /// Deliver due snapshots through the production apply + reconcile, then
    /// present what the app would: adoptions turn the camera and cue.
    pub(super) fn receive(&mut self) {
        let acks = self.h.client_receive();
        self.h.apply_acks(&acks);
        let classes = &self.h.latest_local_correction_classes;
        let magnitudes = &self.h.latest_local_corrections;
        let windows = &self.h.latest_reconcile_windows;
        assert!(
            classes.len() == magnitudes.len() && classes.len() == windows.len(),
            "every local reconcile in these scenarios applies"
        );
        for ((&class, &magnitude), &(acked, _)) in classes.iter().zip(magnitudes).zip(windows) {
            self.corrections.push(Correction {
                class,
                magnitude,
                acked,
                after_client_ticks: self.ticks.len(),
            });
        }
        for adoption in std::mem::take(&mut self.h.prediction.teleport_adoptions) {
            self.camera_yaw += adoption.yaw_delta;
            if let Some(teleporter) = adoption.cue_teleporter {
                self.cues.push(Cue::Adopted(teleporter));
            }
            self.adoptions.push(adoption);
        }
        if self.first_record_client_tick.is_none() && self.h.client_pawn.is_some() {
            self.first_record_client_tick = Some(self.h.prediction.peek_next_client_tick());
        }
    }

    /// One client tick, one link step, one host tick, one receive.
    pub(super) fn step(&mut self, wish: Vec2) -> Option<TeleportEvent> {
        self.step_with(wish, true, &[])
    }

    /// [`Self::step`], losing the command on the uplink when `deliver` is false
    /// and applying `armed` changes on the host at the start of its tick.
    pub(super) fn step_with(
        &mut self,
        wish: Vec2,
        deliver: bool,
        armed: &[(usize, bool)],
    ) -> Option<TeleportEvent> {
        self.host_set_armed(armed);
        let event = self.predict(wish, deliver);
        self.h.advance_clock();
        self.host_ticks(1);
        self.receive();
        event
    }

    /// `ticks` host fixed ticks in one frame (one snapshot), with the host-only
    /// nudge after each.
    pub(super) fn host_ticks(&mut self, ticks: u32) {
        self.host_ticks_arming(ticks, &[]);
    }

    /// Host ticks in one frame; `arm_before[i]` applies at the start of tick `i`.
    pub(super) fn host_ticks_arming(&mut self, ticks: u32, arm_before: &[&[(usize, bool)]]) {
        let nudge = self.host_nudge;
        let changes: Vec<Vec<(EntityId, bool)>> = (0..ticks as usize)
            .map(|tick| {
                arm_before
                    .get(tick + 1)
                    .map(|changes| self.entity_changes(changes))
                    .unwrap_or_default()
            })
            .collect();
        if let Some(first) = arm_before.first() {
            let first = self.entity_changes(first);
            apply_armed_changes(&mut self.h, &first);
        }
        self.h.host_frame(ticks, |h, tick| {
            if nudge != Vec3::ZERO {
                let pawn = h.host_pawn;
                let mut transform = *h.host_registry.get_component::<Transform>(pawn).unwrap();
                transform.position += nudge;
                h.host_registry.set_component(pawn, transform).unwrap();
            }
            apply_armed_changes(h, &changes[tick as usize]);
        });
    }

    /// Queue script `arm`/`disarm` changes in listed order and apply them as the
    /// host's next movement tick start does (`apply_pending_teleporter_mutations`).
    pub(super) fn host_set_armed(&mut self, changes: &[(usize, bool)]) {
        let changes = self.entity_changes(changes);
        apply_armed_changes(&mut self.h, &changes);
    }

    fn entity_changes(&self, changes: &[(usize, bool)]) -> Vec<(EntityId, bool)> {
        changes
            .iter()
            .map(|&(index, armed)| (self.host_teleporters[index], armed))
            .collect()
    }

    /// Step until a tick predicts a teleport; returns its index in [`Self::ticks`].
    pub(super) fn walk_until_predicted_teleport(&mut self, limit: usize) -> usize {
        for _ in 0..limit {
            if self.step(FORWARD).is_some() {
                return self.ticks.len() - 1;
            }
        }
        panic!("no predicted teleport within {limit} ticks");
    }

    pub(super) fn idle(&mut self, ticks: usize) {
        for _ in 0..ticks {
            self.step(IDLE);
        }
    }

    pub(super) fn client_teleport(&self) -> TeleportState {
        self.client_movement().teleport
    }

    pub(super) fn client_movement(&self) -> PlayerMovementComponent {
        self.h
            .client_registry
            .get_component::<PlayerMovementComponent>(self.h.client_pawn.unwrap())
            .unwrap()
            .clone()
    }

    pub(super) fn client_armed(&self, index: usize) -> bool {
        self.h
            .client_world
            .as_ref()
            .unwrap()
            .teleporters
            .armed(&self.h.client_registry, index)
    }

    pub(super) fn host_teleport(&self) -> TeleportState {
        self.h
            .host_registry
            .get_component::<PlayerMovementComponent>(self.h.host_pawn)
            .unwrap()
            .teleport
            .clone()
    }

    pub(super) fn host_yaw(&self) -> f32 {
        let transform = self
            .h
            .host_registry
            .get_component::<Transform>(self.h.host_pawn)
            .unwrap();
        yaw_of(transform.rotation)
    }

    /// Host trace entries whose tick changed the pawn's teleport generation.
    pub(super) fn host_teleports(&self) -> Vec<HostTickTrace> {
        let mut previous: Option<u8> = None;
        let mut out = Vec::new();
        for entry in &self.h.host_trace {
            if previous.is_some_and(|generation| generation != entry.teleport.generation) {
                out.push(entry.clone());
            }
            previous = Some(entry.teleport.generation);
        }
        out
    }

    pub(super) fn teleport_corrections(&self) -> Vec<Correction> {
        self.corrections
            .iter()
            .copied()
            .filter(|correction| correction.class == CorrectionClass::Teleport)
            .collect()
    }
}

fn apply_armed_changes(h: &mut LoopbackHarness, changes: &[(EntityId, bool)]) {
    if changes.is_empty() {
        return;
    }
    let ctx = ScriptCtx::new();
    std::mem::swap(&mut *ctx.registry.borrow_mut(), &mut h.host_registry);
    ctx.pending_teleporter_mutations
        .borrow_mut()
        .extend_from_slice(changes);
    postretro_sim::teleporter_commands::apply_pending_teleporter_mutations(&ctx);
    std::mem::swap(&mut *ctx.registry.borrow_mut(), &mut h.host_registry);
}

/// Yaw of `Quat::from_rotation_y(yaw)`: its forward is (-sin, 0, -cos).
pub(super) fn yaw_of(rotation: glam::Quat) -> f32 {
    let forward = rotation * Vec3::NEG_Z;
    (-forward.x).atan2(-forward.z)
}

/// Angular distance between two yaws, modulo a full turn.
pub(super) fn yaw_gap(a: f32, b: f32) -> f32 {
    use std::f32::consts::{PI, TAU};
    ((a - b + PI).rem_euclid(TAU) - PI).abs()
}

/// The horizontal forward the movement substrate walks for `yaw`.
pub(super) fn forward_of(yaw: f32) -> Vec3 {
    Vec3::new(-yaw.sin(), 0.0, -yaw.cos())
}
