use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
};

use glam::{Vec2, Vec3};
use postretro_entities::components::inventory::Inventory;
use postretro_entities::components::weapon::WeaponComponent;
use postretro_entities::{EntityId, EntityRegistry, SlotValue, Transform};
use postretro_foundation::{
    ActivationLane, ActivationToken, ReloadStyle, ShotId, WeaponDescriptor,
};
use postretro_net::harness::PacketConditioner;
use postretro_net::timesync::{DEFAULT_MICROS_PER_TICK, EchoOutcome, MonotonicClock};
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
// The consecutive (even, odd-only, +3) pairs produce adjacent four-tick
// stamp gaps. Losing their shared snapshot exercises an eight-tick interval.
const HOST_FRAME_TICKS: [u32; 9] = [2, 1, 3, 1, 3, 1, 1, 2, 3];
const DT: f32 = 1.0 / 60.0;
pub(super) const NAMES: [&str; 4] = [
    "player.reloadProgress",
    "player.weaponCooldownMs",
    "player.heat",
    "player.cell",
];

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum Scenario {
    Reload,
    Cooldown,
    CooldownReset,
    Heat,
    Cell,
    HeatShots,
    HeatDelay,
    HeatJump,
    SlowingHeat,
    Overheat,
    Shells,
    Cancel,
}

impl Scenario {
    pub(super) fn slot(self) -> usize {
        match self {
            Self::Reload | Self::Shells | Self::Cancel => 0,
            Self::Cooldown | Self::CooldownReset => 1,
            Self::Cell => 3,
            _ => 2,
        }
    }
    pub(super) fn meter(self) -> usize {
        match self {
            Self::Reload | Self::Shells | Self::Cancel => 0,
            Self::Cooldown | Self::CooldownReset => 1,
            Self::Cell => 3,
            _ => 2,
        }
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub(super) struct RampTrace {
    pub(super) target: Option<f64>,
    pub(super) newest_tick: Option<f64>,
    pub(super) newest_value: Option<f64>,
    pub(super) host_slot: Option<usize>,
    pub(super) slope: Option<f64>,
    pub(super) fed_tick: Option<f64>,
    pub(super) cap_expired: bool,
    pub(super) samples: usize,
    pub(super) bound: Option<f64>,
}

#[derive(Debug)]
pub(super) struct IntakeTrace {
    pub(super) tick: u32,
    pub(super) now_us: u64,
    pub(super) raw: [Option<f64>; 4],
    pub(super) host_slots: [Option<usize>; 4],
    pub(super) bounds: [Option<f64>; 4],
    pub(super) ramps: [RampTrace; 4],
    pub(super) records: usize,
    pub(super) timed_record_present: [bool; 4],
    pub(super) held_resend: [bool; 4],
    pub(super) only_held_resends: bool,
}

#[derive(Debug)]
pub(super) struct HostEvent {
    pub(super) tick: u32,
    pub(super) due_us: u64,
    pub(super) executed_us: u64,
    pub(super) outcome: crate::sim::ReloadOutcome,
}

#[derive(Debug)]
pub(super) struct ClientFrame {
    pub(super) now_us: u64,
    pub(super) applied_ticks: Vec<u32>,
    pub(super) reload_target: f32,
    pub(super) reload_active: bool,
    pub(super) targets: [Option<f64>; 4],
    pub(super) raw: [Option<f64>; 4],
    pub(super) host_slots: [Option<usize>; 4],
    pub(super) bounds: [Option<f64>; 4],
    pub(super) ramps: [RampTrace; 4],
    pub(super) playback_tick: Option<f64>,
    pub(super) overheated: bool,
    pub(super) overheat_at: Option<f64>,
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
    pub(super) intakes: Vec<IntakeTrace>,
    pub(super) events: Vec<HostEvent>,
    pub(super) scenario: Scenario,
    weapons: [EntityId; 4],
    client_weapons: Vec<(EntityId, WeaponComponent)>,
    pub(super) echo_fates: Vec<(u64, EchoOutcome)>,
    pub(super) drop_ticks: Vec<u32>,
    pub(super) drop_started: bool,
    pub(super) drop_completed: bool,
    pub(super) dropped_endpoints: [usize; 2],
    pub(super) hold_completed_until_us: Option<u64>,
    held_completed_packet: Option<Vec<u8>>,
    pub(super) rejected_ticks: Vec<u32>,
    pub(super) heat_zero_due_us: Option<u64>,
    pub(super) snapshot_outage: Option<(u64, u64)>,
    pub(super) duplicate_packets: bool,
    pub(super) sync_enabled_after_us: u64,
    // Deliberately stall one issued round trip before the ramp, then return
    // to the mandated profile. Real Input retransmission can create this RTT
    // asymmetry; the estimator must actually report a reseed after it clears.
    pub(super) stall_first_echo_until_us: Option<u64>,
    delayed_echo: Option<Vec<u8>>,
    timed_ids: [u16; 4],
    held_baselines: std::collections::HashMap<u16, u32>,
}

impl TimedSlotHarness {
    pub(super) fn new(client_hz: u64) -> Self {
        Self::scenario(client_hz, Scenario::Reload, true)
    }

    pub(super) fn scenario(client_hz: u64, scenario: Scenario, lossy: bool) -> Self {
        let profile = |seed| {
            let mut link = mandated_link(seed);
            if !lossy {
                link.loss_probability = 0.0;
            }
            link
        };
        let mut base = StateSlotHarness::new(CLIENT_A, profile(0x71a3), profile(0x71a4));
        base.set_host_ammo(0, 0, 120);
        let mut reload = base
            .registry
            .get_component::<WeaponComponent>(base.shell_weapon)
            .unwrap()
            .clone();
        reload.ammo.as_mut().unwrap().reload_ms = 4_000;
        if matches!(scenario, Scenario::Shells | Scenario::Cancel) {
            let ammo = reload.ammo.as_mut().unwrap();
            ammo.reload_style = ReloadStyle::PerShell;
            ammo.reload_ms = 1000;
            ammo.capacity = 3;
        }
        base.registry
            .set_component(base.shell_weapon, reload)
            .unwrap();

        // Real passive resource timers also run on every authoritative tick.
        // They are holstered during the reload reproduction; later scenarios
        // can select them without substituting synthetic slot values.
        let mut weapons = vec![base.shell_weapon, base.cell_weapon];
        let mut cooldown = base
            .registry
            .get_component::<WeaponComponent>(base.cell_weapon)
            .unwrap()
            .clone();
        let mut cooldown_descriptor = resource_descriptor(serde_json::Value::Null, 4000.0);
        cooldown_descriptor.resource = None;
        cooldown.refresh_from_descriptor(&cooldown_descriptor);
        base.registry
            .set_component(base.cell_weapon, cooldown)
            .unwrap();
        for resource in [
            serde_json::json!({"kind":"heat", "heatPerShot":10.0, "overheatAt":100.0,
                "coolPerSecond":5.0, "coolDelayMs":0.0}),
            serde_json::json!({"kind":"cell", "capacity":100.0, "costPerShot":10.0,
                "regenPerSecond":5.0, "regenDelayMs":0.0}),
        ] {
            let descriptor = resource_descriptor(resource, 15.0);
            let mut weapon = WeaponComponent::from_descriptor(&descriptor);
            if let Some(heat) = weapon.heat.as_mut() {
                heat.heat = 80.0;
                if matches!(scenario, Scenario::HeatShots) {
                    heat.tuning.overheat_at = 1000.0;
                    heat.tuning.cool_delay_ms = 1000.0;
                    heat.idle_ms = 1000.0;
                }
                if matches!(scenario, Scenario::HeatDelay) {
                    heat.tuning.cool_delay_ms = 75.0;
                }
                if matches!(scenario, Scenario::Overheat) {
                    heat.heat = 0.0;
                    heat.tuning.heat_per_shot = 100.0;
                    heat.tuning.cool_per_second = 100.0;
                }
            }
            if let Some(cell) = weapon.cell.as_mut() {
                cell.charge = 0.0;
            }
            let entity = base.registry.spawn(Transform::default());
            base.registry.set_component(entity, weapon).unwrap();
            weapons.push(entity);
        }
        let weapons: [EntityId; 4] = weapons.try_into().unwrap();
        let mut inventory = base
            .registry
            .get_component::<Inventory>(base.owner_pawn)
            .unwrap()
            .clone();
        for (index, weapon) in weapons.into_iter().enumerate() {
            inventory.wieldables[index] = Some(weapon);
        }
        inventory.active_slot = scenario.slot();
        base.registry
            .set_component(base.owner_pawn, inventory)
            .unwrap();
        let mut client_registry = EntityRegistry::new();
        let client_weapons = weapons
            .into_iter()
            .map(|host_weapon| {
                let mut installed = base
                    .registry
                    .get_component::<WeaponComponent>(host_weapon)
                    .unwrap()
                    .clone();
                if let Some(ammo) = installed.ammo.as_mut() {
                    ammo.reload_ms *= 2;
                }
                if let Some(heat) = installed.heat.as_mut() {
                    heat.tuning.cool_per_second *= 2.0;
                }
                let entity = client_registry.spawn(Transform::default());
                client_registry
                    .set_component(entity, installed.clone())
                    .unwrap();
                (entity, installed)
            })
            .collect();
        let client_clock_us = Arc::new(AtomicU64::new(CLIENT_ORIGIN_US));
        let schema = crate::state_slots::ReplicatedSlotSchema::build(
            &base.client_table,
            &base.replication_identity,
        );
        let timed_ids = NAMES.map(|name| schema.id_for(name).unwrap().0);
        Self {
            base,
            client_replication: ClientReplication::new(),
            client_registry,
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
            intakes: Vec::new(),
            events: Vec::new(),
            scenario,
            weapons,
            client_weapons,
            echo_fates: Vec::new(),
            drop_ticks: Vec::new(),
            drop_started: false,
            drop_completed: false,
            dropped_endpoints: [0; 2],
            hold_completed_until_us: None,
            held_completed_packet: None,
            rejected_ticks: Vec::new(),
            heat_zero_due_us: None,
            snapshot_outage: None,
            duplicate_packets: false,
            sync_enabled_after_us: 0,
            stall_first_echo_until_us: None,
            delayed_echo: None,
            timed_ids,
            held_baselines: std::collections::HashMap::new(),
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

    pub(super) fn fixed_snapshot_delay(&mut self) {
        assert_eq!(self.now_us, 0, "configure before the first host frame");
        self.base.to_client = PacketConditioner::new(postretro_net::harness::LinkConfig {
            delay: 45,
            jitter: 0,
            loss_probability: 0.0,
            seed: 1,
        });
    }

    fn host_step(&mut self) {
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
            let echo = wire::encode(&ServerMessage::TimeSync(
                probe.echo(self.host_tick, self.now_us),
            ));
            if self.stall_first_echo_until_us.is_some()
                && !self.time_sync.estimator.is_initialized()
            {
                self.delayed_echo = Some(echo);
            } else {
                self.sync_to_client.enqueue(echo);
            }
        }
        if self
            .stall_first_echo_until_us
            .is_some_and(|until| self.now_us >= until)
            && let Some(echo) = self.delayed_echo.take()
        {
            self.sync_to_client.enqueue(echo);
            self.stall_first_echo_until_us = None;
        }
        // Production polls and echoes before this frame runs its fixed ticks.
        let mut snapshot_due = false;
        while self.host_tick < self.next_host_tick {
            let tick = self.host_tick + 1;
            self.simulated_tick_times.push(Self::host_tick_due_us(tick));
            self.simulate_host_tick(tick);
            complete_host_fixed_tick(&mut self.host_tick, &mut snapshot_due);
        }
        if snapshot_due {
            use postretro_entities::components::weapon::{ReloadFeedback, ReloadFeedbackConsumer};
            let endpoint = self
                .base
                .registry
                .get_component::<WeaponComponent>(self.weapons[self.scenario.slot()])
                .unwrap()
                .reload_feedback_sample(ReloadFeedbackConsumer::OwnerProjection)
                .endpoint
                .map(|e| e.feedback);
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
            let dropped = self.drop_ticks.contains(&self.host_tick)
                || self
                    .snapshot_outage
                    .is_some_and(|(start, end)| (start..end).contains(&self.now_us))
                || match endpoint {
                    Some(ReloadFeedback::Started) if self.drop_started => {
                        self.dropped_endpoints[0] += 1;
                        true
                    }
                    Some(ReloadFeedback::Completed) if self.drop_completed => {
                        self.dropped_endpoints[1] += 1;
                        true
                    }
                    _ => false,
                };
            if endpoint == Some(ReloadFeedback::Completed) && self.hold_completed_until_us.is_some()
            {
                self.held_completed_packet = Some(wire::encode(&snapshot));
            } else if !dropped {
                self.base.to_client.enqueue(wire::encode(&snapshot));
                if self.duplicate_packets {
                    self.base.to_client.enqueue(wire::encode(&snapshot));
                }
            }
            crate::sim::clear_owner_reload_feedback_for_weapons(&mut self.base.registry, &sampled);
            self.sent_ticks.push(self.host_tick);
            self.base.sequence += 1;
        }
        if self
            .hold_completed_until_us
            .is_some_and(|at| self.now_us >= at)
            && let Some(packet) = self.held_completed_packet.take()
        {
            self.base.to_client.enqueue(packet);
            self.hold_completed_until_us = None;
        }
        self.host_frame += 1;
        self.next_host_tick += HOST_FRAME_TICKS[self.host_frame % HOST_FRAME_TICKS.len()];
    }

    fn simulate_host_tick(&mut self, tick: u32) {
        let registry = std::rc::Rc::new(std::cell::RefCell::new(std::mem::take(
            &mut self.base.registry,
        )));
        let slot = self.scenario.slot();
        let active_weapon = self.weapons[slot];
        if tick == 120 && matches!(self.scenario, Scenario::HeatJump | Scenario::SlowingHeat) {
            let mut weapon = registry
                .borrow()
                .get_component::<WeaponComponent>(active_weapon)
                .unwrap()
                .clone();
            let resource = if self.scenario == Scenario::HeatJump {
                serde_json::json!({"kind":"heat", "heatPerShot":10.0, "overheatAt":40.0, "coolPerSecond":5.0, "coolDelayMs":0.0})
            } else {
                serde_json::json!({"kind":"heat", "heatPerShot":10.0, "overheatAt":100.0, "coolPerSecond":2.5, "coolDelayMs":0.0})
            };
            weapon.refresh_from_descriptor(&resource_descriptor(resource, 15.0));
            registry
                .borrow_mut()
                .set_component(active_weapon, weapon)
                .unwrap();
        }
        let reload = tick == 60
            && matches!(
                self.scenario,
                Scenario::Reload | Scenario::Shells | Scenario::Cancel
            );
        let fire = match self.scenario {
            Scenario::Cooldown | Scenario::HeatDelay | Scenario::Overheat => tick == 60,
            Scenario::CooldownReset => tick == 60 || tick == 360,
            Scenario::HeatShots => (60..=120).contains(&tick) && tick.is_multiple_of(2),
            Scenario::Cancel => tick == 160,
            _ => false,
        };
        let mut command = idle_command(tick, reload);
        command.firing_slot = slot as u8;
        if fire {
            command.fire_button = FireButtonState {
                pressed: true,
                active: true,
            };
            command.activation.initiation = Some(ActivationToken {
                start_tick: tick,
                lane: ActivationLane::Primary,
            });
        }
        let before = registry
            .borrow()
            .get_component::<WeaponComponent>(self.base.shell_weapon)
            .unwrap()
            .state_remaining_ms;
        let mut ai = postretro_ai::AiRuntime::new();
        let events = simulate_tick(
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
                weapon: Some(active_weapon),
                shot_id: fire.then_some(ShotId::from_parts(1, tick, ActivationLane::Primary, 0)),
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
        self.events
            .extend(events.reload_deliveries.into_iter().map(|event| HostEvent {
                tick,
                due_us: Self::host_tick_due_us(tick),
                executed_us: self.now_us,
                outcome: event.outcome,
            }));
        if self.scenario == Scenario::Overheat
            && tick > 60
            && self.heat_zero_due_us.is_none()
            && registry
                .borrow()
                .get_component::<WeaponComponent>(active_weapon)
                .unwrap()
                .heat
                .is_some_and(|heat| !heat.overheated && heat.heat == 0.0)
        {
            self.heat_zero_due_us = Some(Self::host_tick_due_us(tick));
        }
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
        let host_before_display = self.host_timer_values();
        if self.now_us >= self.sync_enabled_after_us
            && (self.delayed_echo.is_none() || self.time_sync.estimator.is_initialized())
            && let Some(probe) = self
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
            let fate = self
                .time_sync
                .estimator
                .echo_fates()
                .last()
                .unwrap()
                .outcome;
            self.echo_fates.push((self.now_us, fate));
        }
        let mut applied_ticks = Vec::new();
        for packet in self.base.to_client.take_ready() {
            let snapshot = crate::decode_snapshot(&packet).expect("host snapshot decodes");
            let gate = self
                .client_replication
                .apply_snapshot(&mut self.client_registry, &snapshot);
            if !gate.accepted_newest_snapshot {
                self.rejected_snapshots += 1;
                self.rejected_ticks.push(snapshot.server_tick);
                continue;
            }
            let raw: wire::RawSnapshotMessage = wire::decode(&packet).unwrap();
            let timed_record_present = self
                .timed_ids
                .map(|id| raw.state_records.iter().any(|r| r.slot_id == id));
            let held_resend = self.timed_ids.map(|id| {
                raw.state_records.iter().any(|r| {
                    r.slot_id == id
                        && self
                            .held_baselines
                            .get(&id)
                            .is_some_and(|held| r.baseline_id <= *held)
                })
            });
            let only_held_resends = !raw.state_records.is_empty()
                && raw.state_records.iter().all(|r| {
                    self.held_baselines
                        .get(&r.slot_id)
                        .is_some_and(|held| r.baseline_id <= *held)
                });
            let outcome = self.base.client.apply_snapshot_state(
                &mut self.base.client_table,
                &self.base.replication_identity,
                raw.sequence,
                &raw.state_schema_fingerprint,
                &raw.state_records,
            );
            self.base.client.accept_timed_slot_sample(
                raw.server_tick,
                &self.base.client_table,
                &outcome,
            );
            let (_, ramps) = self.debug();
            self.intakes.push(IntakeTrace {
                tick: raw.server_tick,
                now_us: self.now_us,
                raw: self.raw(),
                host_slots: self.committed_host_slots(),
                bounds: self.committed_bounds(),
                ramps,
                records: raw.state_records.len(),
                timed_record_present,
                held_resend,
                only_held_resends,
            });
            for &(id, baseline) in &outcome.slot_baselines {
                self.held_baselines
                    .entry(id)
                    .and_modify(|held| *held = (*held).max(baseline))
                    .or_insert(baseline);
            }
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
        let raw = self.raw();
        let sample_tick = self
            .time_sync
            .estimated_server_tick()
            .zip(self.time_sync.rtt_micros())
            .map(|(tick, rtt)| tick - 0.5 * rtt / DEFAULT_MICROS_PER_TICK as f64);
        let dt = self.frames.last().map_or(0.0, |frame| {
            (self.now_us - frame.now_us) as f64 / 1_000_000.0
        });
        self.base.client.advance_timed_slot_targets(sample_tick, dt);
        let mut display: std::collections::HashMap<String, SlotValue> = self
            .base
            .client_table
            .iter()
            .filter_map(|(name, slot)| slot.value.clone().map(|value| (name.to_owned(), value)))
            .collect();
        self.base.client.apply_timed_slot_targets(&mut display);
        let targets = NAMES.map(|name| display.get(name).and_then(slot_number));
        let (playback_tick, ramps) = self.debug();
        assert_eq!(self.raw(), raw, "display projection wrote the raw store");
        assert_eq!(
            self.host_timer_values(),
            host_before_display,
            "client display changed authoritative host components"
        );
        for (entity, installed) in &self.client_weapons {
            let live = self
                .client_registry
                .get_component::<WeaponComponent>(*entity)
                .unwrap();
            assert_eq!(
                live.heat, installed.heat,
                "projection changed installed client heat"
            );
            assert_eq!(
                live.cell, installed.cell,
                "projection changed installed client cell"
            );
            assert_eq!(live.cooldown_remaining_ms, installed.cooldown_remaining_ms);
            assert_eq!(live.state_remaining_ms, installed.state_remaining_ms);
        }
        for (name, slot) in self.base.client_table.iter() {
            if !NAMES.contains(&name)
                && let Some(value) = &slot.value
            {
                assert_eq!(
                    display.get(name),
                    Some(value),
                    "nonprojected slot {name} changed"
                );
            }
        }
        self.frames.push(ClientFrame {
            now_us: self.now_us,
            applied_ticks,
            reload_target: targets[0].unwrap_or(0.0) as f32,
            reload_active: self.base.client_value("player.reloadActive")
                == Some(SlotValue::Boolean(true)),
            targets,
            raw,
            host_slots: self.committed_host_slots(),
            bounds: self.committed_bounds(),
            ramps,
            playback_tick,
            overheat_at: display.get("player.overheatAt").and_then(slot_number),
            overheated: self.base.client_value("player.overheated")
                == Some(SlotValue::Boolean(true)),
        });
        self.client_frame += 1;
    }

    fn committed_host_slots(&self) -> [Option<usize>; 4] {
        let raw = self.base.client.weapon_projection();
        [
            raw.reload_progress.map(|s| s.slot),
            raw.cooldown.map(|s| s.slot),
            raw.heat.map(|s| s.slot),
            raw.cell.map(|s| s.slot),
        ]
    }

    fn committed_bounds(&self) -> [Option<f64>; 4] {
        let raw = self.base.client.weapon_projection();
        [
            Some(1.0),
            Some(0.0),
            Some(0.0),
            raw.cell_capacity
                .filter(|capacity| raw.cell.is_some_and(|cell| cell.slot == capacity.slot))
                .and_then(|capacity| capacity.value)
                .map(f64::from),
        ]
    }

    pub(super) fn assert_queued_probe_echoes_before_frame_ticks(ticks: u32) {
        // Regression: catch-up ticks were incorrectly included in the echo,
        // biasing the fixture's sample timeline by as much as three ticks.
        let mut harness = Self::scenario(144, Scenario::Heat, false);
        let clean = postretro_net::harness::LinkConfig {
            delay: 0,
            jitter: 0,
            loss_probability: 0.0,
            seed: 1,
        };
        harness.sync_to_host = PacketConditioner::new(clean);
        harness.sync_to_client = PacketConditioner::new(clean);
        harness.base.to_client = PacketConditioner::new(clean);
        // Both 1 -> 2 and 3 -> 6 cross the real two-tick snapshot cadence.
        harness.host_tick = if ticks == 1 { 1 } else { 3 };
        harness.next_host_tick = harness.host_tick + ticks;
        let before = harness.host_tick;
        let probe = harness.time_sync.maybe_send_probe(0).unwrap();
        harness
            .sync_to_host
            .enqueue(wire::encode(&ClientMessage::TimeSync(probe)));
        harness.advance_clock(Self::host_tick_due_us(harness.next_host_tick));
        harness.host_step();
        let packets = harness.sync_to_client.take_ready();
        assert_eq!(packets.len(), 1);
        let ServerMessage::TimeSync(echo) = wire::decode(&packets[0]).unwrap() else {
            panic!("expected the queued probe's echo");
        };
        assert_eq!(echo.server_tick, before, "echo includes this frame's ticks");
        let snapshots = harness.base.to_client.take_ready();
        assert_eq!(snapshots.len(), 1, "frame must cross snapshot cadence");
        let snapshot = crate::decode_snapshot(&snapshots[0]).unwrap();
        assert_eq!(snapshot.server_tick, before + ticks);
        assert_eq!(harness.sent_ticks, vec![before + ticks]);
    }

    fn raw(&self) -> [Option<f64>; 4] {
        NAMES.map(|name| {
            self.base
                .client_table
                .get(name)
                .and_then(|slot| slot.value.as_ref())
                .and_then(slot_number)
        })
    }

    fn host_timer_values(&self) -> [[f32; 5]; 4] {
        self.weapons.map(|id| {
            let weapon = self
                .base
                .registry
                .get_component::<WeaponComponent>(id)
                .unwrap();
            [
                weapon.cooldown_remaining_ms,
                weapon.state_remaining_ms as f32,
                weapon.state_total_ms as f32,
                weapon.heat.map_or(-1.0, |v| v.heat),
                weapon.cell.map_or(-1.0, |v| v.charge),
            ]
        })
    }

    fn debug(&self) -> (Option<f64>, [RampTrace; 4]) {
        let debug = self.base.client.timed_slot_debug();
        (
            debug.playback_tick,
            debug.slots.map(|r| RampTrace {
                target: r.target,
                newest_tick: r.newest_tick,
                newest_value: r.newest_value,
                host_slot: r.host_slot,
                slope: r.agreed_slope,
                fed_tick: r.fed_tick,
                cap_expired: r.cap_expired,
                samples: r.samples,
                bound: r.bound,
            }),
        )
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

fn slot_number(value: &SlotValue) -> Option<f64> {
    if let SlotValue::Number(value) = value {
        Some(f64::from(*value))
    } else {
        None
    }
}

fn resource_descriptor(resource: serde_json::Value, recovery_ms: f32) -> WeaponDescriptor {
    serde_json::from_value::<WeaponDescriptor>(serde_json::json!({
        "damage":1.0, "range":64.0, "resolution":"hitscan",
        "primary":{"trigger":"press", "recoveryMs":recovery_ms, "steps":[{"kind":"shot"}]},
        "resource":resource,
    }))
    .unwrap()
    .validate()
    .unwrap()
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
