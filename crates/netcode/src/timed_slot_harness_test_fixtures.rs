use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
};

use glam::{Vec2, Vec3};
use postretro_entities::components::weapon::WeaponComponent;
use postretro_entities::{EntityRegistry, SlotValue, Transform};
use postretro_foundation::WeaponDescriptor;
use postretro_net::harness::PacketConditioner;
use postretro_net::timesync::MonotonicClock;
use postretro_net::wire::{self, ClientMessage, ServerMessage};

use super::super::{CLIENT_A, StateSlotHarness, mandated_link, snapshot_with_state};
use crate::client::ClientReplication;
use crate::endpoint::ClientTimeSync;
use crate::host::complete_host_fixed_tick;
use crate::kinematic_mover::MoverTickStateTable;
use crate::movement::MovementInput;
use crate::scripting_systems::hit_zones::HitZoneStore;
use crate::sim::{PostMovementCommand, RemotePawnCommand, SimCommand, simulate_tick};
use crate::weapon::FireButtonState;

const CLIENT_ORIGIN_US: u64 = 7_000_000;
const HOST_FRAME_TICKS: [u32; 7] = [2, 1, 3, 1, 1, 2, 3];
const DT: f32 = 1.0 / 60.0;

pub(super) struct ClientFrame {
    pub(super) now_us: u64,
    pub(super) applied_ticks: Vec<u32>,
    pub(super) reload_target: f32,
    pub(super) reload_active: bool,
}

pub(super) struct TimedSlotHarness {
    base: StateSlotHarness,
    client_replication: ClientReplication,
    client_registry: EntityRegistry,
    pub(super) time_sync: ClientTimeSync,
    client_clock_us: Arc<AtomicU64>,
    // Time-sync traffic has its own bidirectional conditioned lane: a probe
    // cannot initialize the estimator until both actual flights have elapsed.
    sync_to_host: PacketConditioner,
    sync_to_client: PacketConditioner,
    now_us: u64,
    host_tick: u32,
    host_frame: usize,
    next_host_tick: u32,
    client_frame: u64,
    client_hz: u64,
    sent_ticks: Vec<u32>,
    simulated_tick_times: Vec<u64>,
    timer_steps_ms: Vec<u32>,
    rejected_snapshots: usize,
    pub(super) frames: Vec<ClientFrame>,
}

impl TimedSlotHarness {
    pub(super) fn new(client_hz: u64) -> Self {
        let mut base =
            StateSlotHarness::new(CLIENT_A, mandated_link(0x71a3), mandated_link(0x71a4));
        base.set_host_ammo(0, 0, 120);
        let mut reload = base
            .registry
            .get_component::<WeaponComponent>(base.shell_weapon)
            .unwrap()
            .clone();
        reload.ammo.as_mut().unwrap().reload_ms = 4_000;
        base.registry
            .set_component(base.shell_weapon, reload)
            .unwrap();

        // Real passive resource timers also run on every authoritative tick.
        // They are holstered during the reload reproduction; later scenarios
        // can select them without substituting synthetic slot values.
        for resource in [
            serde_json::json!({"kind":"heat", "heatPerShot":10.0, "overheatAt":100.0,
                "coolPerSecond":5.0, "coolDelayMs":0.0}),
            serde_json::json!({"kind":"cell", "capacity":100.0, "costPerShot":10.0,
                "regenPerSecond":5.0, "regenDelayMs":0.0}),
        ] {
            let mut descriptor: WeaponDescriptor = serde_json::from_value(serde_json::json!({
                "damage":1.0, "range":64.0, "resolution":"hitscan",
                "primary":{"trigger":"press", "recoveryMs":100.0, "steps":[{"kind":"shot"}]},
                "resource":resource,
            }))
            .unwrap();
            descriptor = descriptor.validate().unwrap();
            let mut weapon = WeaponComponent::from_descriptor(&descriptor);
            if let Some(heat) = weapon.heat.as_mut() {
                heat.heat = 80.0;
            }
            if let Some(cell) = weapon.cell.as_mut() {
                cell.charge = 0.0;
            }
            let entity = base.registry.spawn(Transform::default());
            base.registry.set_component(entity, weapon).unwrap();
        }
        let client_clock_us = Arc::new(AtomicU64::new(CLIENT_ORIGIN_US));
        Self {
            base,
            client_replication: ClientReplication::new(),
            client_registry: EntityRegistry::new(),
            time_sync: ClientTimeSync::with_virtual_clock(Arc::clone(&client_clock_us)),
            client_clock_us,
            sync_to_host: PacketConditioner::new(mandated_link(0x71a5)),
            sync_to_client: PacketConditioner::new(mandated_link(0x71a6)),
            now_us: 0,
            host_tick: 0,
            host_frame: 0,
            next_host_tick: HOST_FRAME_TICKS[0],
            client_frame: 0,
            client_hz,
            sent_ticks: Vec::new(),
            simulated_tick_times: Vec::new(),
            timer_steps_ms: Vec::new(),
            rejected_snapshots: 0,
            frames: Vec::new(),
        }
    }

    // Use rational deadlines rather than repeatedly adding 16 or 16_667 ms/us.
    // Every third tick is exactly 50 ms; a host catch-up frame still records
    // each event at its tick's deadline, not at the frame that executes it.
    fn host_tick_due_us(tick: u32) -> u64 {
        u64::from(tick) * 1_000_000 / 60
    }

    fn advance_clock(&mut self, now_us: u64) {
        assert!(now_us >= self.now_us);
        let elapsed_ms = now_us / 1000 - self.now_us / 1000;
        self.base.to_client.advance(elapsed_ms);
        self.base.to_server.advance(elapsed_ms);
        self.sync_to_host.advance(elapsed_ms);
        self.sync_to_client.advance(elapsed_ms);
        self.now_us = now_us;
        self.client_clock_us
            .store(CLIENT_ORIGIN_US + now_us, Ordering::Relaxed);
    }

    pub(super) fn run_until_us(&mut self, until_us: u64) {
        loop {
            let client_due = self.client_frame * 1_000_000 / self.client_hz;
            let host_due = Self::host_tick_due_us(self.next_host_tick);
            let next = client_due.min(host_due);
            if next > until_us {
                break;
            }
            self.advance_clock(next);
            if host_due <= client_due {
                self.host_step();
            } else {
                self.client_step();
            }
        }
    }

    fn host_step(&mut self) {
        let mut snapshot_due = false;
        while self.host_tick < self.next_host_tick {
            let tick = self.host_tick + 1;
            self.simulated_tick_times.push(Self::host_tick_due_us(tick));
            self.simulate_host_tick(tick);
            complete_host_fixed_tick(&mut self.host_tick, &mut snapshot_due);
        }
        for packet in self.base.to_server.take_ready() {
            match wire::decode::<ClientMessage>(&packet).unwrap() {
                ClientMessage::Ack(ack) => self.base.host.apply_ack(
                    CLIENT_A,
                    ack.latest_snapshot_sequence,
                    &ack.slot_baselines,
                    ack.faction_sentiment_baseline,
                ),
                ClientMessage::StateBaselineRefresh(req) => {
                    self.base
                        .host
                        .request_refresh(CLIENT_A, req.slot_id, req.missing_baseline_ref)
                }
                _ => panic!("unexpected message on state return lane"),
            }
        }
        for packet in self.sync_to_host.take_ready() {
            let ClientMessage::TimeSync(probe) = wire::decode(&packet).unwrap() else {
                panic!("time-sync lane carried a different message");
            };
            self.sync_to_client
                .enqueue(wire::encode(&ServerMessage::TimeSync(
                    probe.echo(self.host_tick, self.now_us),
                )));
        }
        if snapshot_due {
            let sampled = self.base.host.ingest_frame_and_collect_sampled_weapons(
                &self.base.host_ctx.slot_table.borrow(),
                &self.base.replication_identity,
                &self.base.registry,
                &self.base.owners,
                &self.base.weapon_owners,
            );
            let seq = self.base.sequence;
            let records = self.base.host.produce_for_client(CLIENT_A, seq).unwrap();
            let mut snapshot = snapshot_with_state(seq, self.base.fingerprint, records);
            snapshot.server_tick = self.host_tick;
            snapshot.server_script_time = f64::from(self.host_tick) / 60.0;
            self.base.to_client.enqueue(wire::encode(&snapshot));
            crate::sim::clear_owner_reload_feedback_for_weapons(&mut self.base.registry, &sampled);
            self.sent_ticks.push(self.host_tick);
            self.base.sequence += 1;
        }
        self.host_frame += 1;
        self.next_host_tick += HOST_FRAME_TICKS[self.host_frame % HOST_FRAME_TICKS.len()];
    }

    fn simulate_host_tick(&mut self, tick: u32) {
        let registry = std::rc::Rc::new(std::cell::RefCell::new(std::mem::take(
            &mut self.base.registry,
        )));
        let command = idle_command(tick, tick == 60);
        let before = registry
            .borrow()
            .get_component::<WeaponComponent>(self.base.shell_weapon)
            .unwrap()
            .state_remaining_ms;
        let mut ai = postretro_ai::AiRuntime::new();
        simulate_tick(
            registry.clone(),
            &crate::collision::CollisionWorld::new(),
            &HitZoneStore::new(),
            None,
            postretro_collision::EnvironmentView::level_only(-20.0),
            None,
            f64::from(tick) / 60.0,
            &mut postretro_scripting_core::reaction_dispatch::ProgressTracker::new(),
            postretro_ai::tick_runner!(&mut ai),
            &[],
            &mut MoverTickStateTable::default(),
            &[RemotePawnCommand {
                real_command: true,
                rejected_activation: None,
                pawn: self.base.owner_pawn,
                owner_client_id: CLIENT_A,
                weapon: Some(self.base.shell_weapon),
                shot_id: None,
                fire_tick: tick,
                client_tick: tick,
                aim_pitch: 0.0,
                start_aim: None,
                command,
            }],
            &idle_command(tick, false),
            |_| PostMovementCommand {
                aim_origin: Vec3::ZERO,
                aim_direction: Vec3::NEG_Z,
            },
            DT,
            None,
            |_| {},
        );
        let after = registry
            .borrow()
            .get_component::<WeaponComponent>(self.base.shell_weapon)
            .unwrap()
            .state_remaining_ms;
        if after > 0 && before > after {
            self.timer_steps_ms.push(before - after);
        }
        self.base.registry = std::mem::take(&mut *registry.borrow_mut());
    }

    fn client_step(&mut self) {
        if let Some(probe) = self
            .time_sync
            .maybe_send_probe((self.now_us * 60 / 1_000_000) as u32)
        {
            self.sync_to_host
                .enqueue(wire::encode(&ClientMessage::TimeSync(probe)));
        }
        for packet in self.sync_to_client.take_ready() {
            let ServerMessage::TimeSync(echo) = wire::decode(&packet).unwrap() else {
                panic!("time-sync lane carried a different message");
            };
            self.time_sync
                .estimator
                .ingest_echo(&echo, self.time_sync.clock.now_micros());
        }
        let mut applied_ticks = Vec::new();
        for packet in self.base.to_client.take_ready() {
            let snapshot = crate::decode_snapshot(&packet).expect("host snapshot decodes");
            let gate = self
                .client_replication
                .apply_snapshot(&mut self.client_registry, &snapshot);
            if !gate.accepted_newest_snapshot {
                self.rejected_snapshots += 1;
                continue;
            }
            let raw: wire::RawSnapshotMessage = wire::decode(&packet).unwrap();
            let outcome = self.base.client.apply_snapshot_state(
                &mut self.base.client_table,
                &self.base.replication_identity,
                raw.sequence,
                &raw.state_schema_fingerprint,
                &raw.state_records,
            );
            applied_ticks.push(raw.server_tick);
            self.base
                .to_server
                .enqueue(wire::encode(&ClientMessage::Ack(wire::AckMessage {
                    latest_snapshot_sequence: raw.sequence,
                    acked_server_tick: raw.server_tick,
                    entity_baselines: Vec::new(),
                    despawn_tombstones: Vec::new(),
                    slot_baselines: outcome.slot_baselines,
                    faction_sentiment_baseline: None,
                })));
            for request in outcome.refresh_requests {
                self.base
                    .to_server
                    .enqueue(wire::encode(&ClientMessage::StateBaselineRefresh(request)));
            }
        }
        // The baseline implementation builds the UI target directly from raw
        // state. The green slice substitutes through ClientStateApply's display
        // seam here, after each accepted sample and once per client frame.
        self.frames.push(ClientFrame {
            now_us: self.now_us,
            applied_ticks,
            reload_target: number(&self.base.client_table, "player.reloadProgress"),
            reload_active: self.base.client_value("player.reloadActive")
                == Some(SlotValue::Boolean(true)),
        });
        self.client_frame += 1;
    }

    pub(super) fn assert_proof_setup(&self) {
        assert_eq!(Self::host_tick_due_us(3), 50_000);
        assert!(
            self.simulated_tick_times
                .windows(2)
                .any(|p| p[1] - p[0] == 16_666)
        );
        assert!(
            self.simulated_tick_times
                .windows(2)
                .any(|p| p[1] - p[0] == 16_667)
        );
        for gap in 1..=4 {
            assert!(
                self.sent_ticks.windows(2).any(|p| p[1] - p[0] == gap),
                "host frame pattern must exercise {gap}-tick snapshot spacing"
            );
        }
        assert!(
            self.timer_steps_ms.contains(&16) && self.timer_steps_ms.contains(&17),
            "production reload must carry its fractional millisecond between ticks"
        );
        assert!(
            self.rejected_snapshots > 0,
            "conditioned snapshots must exercise newest gate"
        );
        assert!(
            self.time_sync.estimated_server_tick().is_some(),
            "real round trip must initialize sync"
        );
        assert!(self.time_sync.rtt_micros().unwrap() >= 90_000.0);
        assert_eq!(self.frames.len(), 4 * self.client_hz as usize + 1);
    }
}

fn number(table: &postretro_entities::SlotTable, name: &str) -> f32 {
    match table.get(name).and_then(|slot| slot.value.as_ref()) {
        Some(SlotValue::Number(value)) => *value,
        other => panic!("expected numeric {name}, got {other:?}"),
    }
}

fn idle_command(tick: u32, reload: bool) -> SimCommand {
    SimCommand {
        input_tick: tick,
        movement: MovementInput {
            wish_dir: Vec2::ZERO,
            jump_pressed: false,
            dash_pressed: false,
            running: false,
            crouch_intent: false,
            facing_yaw: 0.0,
            facing_pitch: 0.0,
            use_pressed: false,
            drop_pressed: false,
        },
        fire_button: FireButtonState {
            pressed: false,
            active: false,
        },
        secondary_button: FireButtonState {
            pressed: false,
            active: false,
        },
        activation: Default::default(),
        reload,
        firing_slot: 0,
        select_slot: None,
        use_pressed: false,
        drop_pressed: false,
    }
}
