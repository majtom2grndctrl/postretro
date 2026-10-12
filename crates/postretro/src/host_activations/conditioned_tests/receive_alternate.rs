//! Opt-in coverage of snapshot receive during fixed weapon prediction. The
//! ordinary conditioned fixture intentionally covers the command/outcome lanes
//! without applying snapshots; these scenarios add the production receive path.

use super::*;
use crate::input::{Action, ActionSnapshot, ActivationInputCapture, ButtonState};
use postretro_combat_model::TuningPayload;
use postretro_entities::presentation_binding::{
    PresentationBindingKey, PresentationBindingResolver,
};
use postretro_entities::{ScriptCtx, SlotValue};
use postretro_foundation::{ActivationCursor, ActivationPhase};
use postretro_sim::scripting_systems::ui_proxy::PlayerHudStatePublisher;

struct NoBindings;
impl PresentationBindingResolver for NoBindings {
    fn resolve(&self, _: &EntityRegistry, _: PresentationBindingKey) -> Option<(EntityId, Vec3)> {
        None
    }
    fn key_for_entity(
        &self,
        _: &EntityRegistry,
        _: EntityId,
    ) -> Option<(PresentationBindingKey, Vec3)> {
        None
    }
}

pub(super) struct SnapshotReceive {
    pub(super) tuning: Option<TuningPayload>,
    replication: netcode::ClientReplication,
    state: netcode::ClientStateApply,
    pub(super) prediction: netcode::ClientPrediction,
    set: netcode::ReplicableSet,
    last_snapshot_tick: Option<u32>,
    ctx: ScriptCtx,
    hud: PlayerHudStatePublisher,
    control_installs: usize,
    local_applies: usize,
    acknowledged_baseline: Option<u32>,
    /// Number of issued rounds when an ordinary local-pawn record applied
    /// during a live activation. This proves receive interleaves its waits.
    active_applies: Vec<usize>,
    active_applies_after_baseline_ack: usize,
    /// The production pull-effects boundary, not an audio-device playback log.
    pub(super) fire_cues: Vec<(ShotId, Option<String>)>,
    pub(super) declarations: Vec<ShotId>,
}

impl SnapshotReceive {
    pub(super) fn control(&mut self, message: &wire::ServerControlMessage) {
        if let wire::ServerControlMessage::Tuning(bytes) = message {
            // The app's private Control installer is outside this test seam.
            // Use the actual host-produced payload, validating its rows before
            // passing it into the production snapshot materializer.
            let payload: TuningPayload = serde_json::from_slice(bytes).unwrap();
            for row in payload.wieldables.iter().flatten() {
                row.validate().unwrap();
            }
            self.tuning = Some(payload);
            self.control_installs += 1;
        }
    }
}

impl Fixture {
    fn with_received_dev_weapon(canonical: &str) -> Self {
        use crate::startup::lifecycle::tests::{script_ctx, test_app};
        use postretro_scripting_core::primitives_registry::PrimitiveRegistry;

        let mut app = test_app();
        let mut ctx = script_ctx(&app);
        let mut primitives = PrimitiveRegistry::new();
        crate::scripting::primitives::register_all(&mut primitives, ctx.clone());
        let manifest = crate::coop_fixture_tests::initialize_dev_mod(&mut app, &primitives);
        let weapon = manifest
            .entities
            .iter()
            .find(|entity| entity.canonical_name.as_deref() == Some(canonical))
            .unwrap()
            .clone();
        let mut player = manifest
            .entities
            .iter()
            .find(|entity| entity.canonical_name.as_deref() == Some("player"))
            .unwrap()
            .clone();
        // Only loadout breadth changes; weapon actions, charge expressions,
        // resources, accuracy, muzzle offset and presentation stay authored.
        player.inventory.as_mut().unwrap().loadout = vec![canonical.into()];
        let mut fixture = Self::new(LinkConfig::perfect(), weapon.weapon.clone().unwrap());
        fixture.descriptors = vec![player, weapon];
        fixture
            .host
            .borrow_mut()
            .despawn(fixture.host_actors.pawn)
            .unwrap();
        fixture.allocator.forget(fixture.host_actors.pawn);
        fixture.owners = Default::default();
        fixture
            .local
            .borrow_mut()
            .despawn(fixture.local_actors.pawn)
            .unwrap();
        ctx.registry = fixture.local.clone();
        let mut set = netcode::ReplicableSet::default();
        let pawn = netcode::host_handle_accept_descriptor_at_placement(
            &mut fixture.host.borrow_mut(),
            &mut fixture.allocator,
            &mut set,
            &mut Default::default(),
            &mut fixture.queues,
            &mut fixture.owners,
            &mut fixture.weapon_owners,
            &mut fixture.open,
            &mut fixture.pending,
            &mut fixture.weaponless,
            CLIENT,
            &[crate::scripting::map_entity::MapEntity {
                map_ordinal: None,
                classname: "player_spawn".into(),
                origin: Vec3::ZERO,
                angles: Vec3::ZERO,
                key_values: Default::default(),
                tags: Vec::new(),
            }],
            0,
            &fixture.descriptors,
            None,
            None,
        )
        .expect("real slot admission composes the remote player's loadout");
        fixture.host_actors.pawn = pawn;
        fixture.host_actors.weapon = fixture
            .host
            .borrow()
            .get_component::<Inventory>(pawn)
            .unwrap()
            .active_wieldable()
            .unwrap();
        // The fixture host simulates under `EnvironmentView::level_only(0.0)`.
        let tuning = netcode::tuning_payload_for_pawn(
            &fixture.host.borrow(),
            pawn,
            &fixture.descriptors,
            None,
            &postretro_combat_model::EnvironmentTuning {
                level_gravity: 0.0,
                fluids: Default::default(),
            },
        );
        netcode::host_send_tuning_if_changed(
            &mut fixture.server,
            &mut Default::default(),
            CLIENT,
            tuning,
        );
        fixture.receive = Some(SnapshotReceive {
            tuning: None,
            replication: Default::default(),
            state: Default::default(),
            prediction: Default::default(),
            set,
            last_snapshot_tick: None,
            hud: PlayerHudStatePublisher::new(ctx.clone()),
            ctx,
            control_installs: 0,
            local_applies: 0,
            acknowledged_baseline: None,
            active_applies: Vec::new(),
            active_applies_after_baseline_ack: 0,
            fire_cues: Vec::new(),
            declarations: Vec::new(),
        });
        fixture.publish_receive_snapshots();
        // Finish the initial FullBaseline's real ACK round trip before any
        // simulation can ingest another candidate baseline for this pawn.
        for _ in 0..8 {
            fixture.transport();
            netcode::host_handle_client_messages(
                &mut fixture.server,
                &mut fixture.replication,
                &mut fixture.slots,
                &mut fixture.queues,
                &mut fixture.pending,
                &mut fixture.open,
                CLIENT,
                fixture.tick,
                u64::from(fixture.tick) * 16_667,
            );
        }
        assert_eq!(fixture.receive.as_ref().unwrap().control_installs, 1);
        assert!(fixture.receive.as_ref().unwrap().local_applies > 0);
        assert!(
            fixture.pawn_baseline_acked(),
            "initial pawn baseline {:?} for network {} must be acknowledged; applies={}, latest_received_tick={:?}, host/client participating={}/{}",
            fixture
                .replication
                .current_baseline_id(fixture.pawn_network()),
            fixture.pawn_network(),
            fixture.receive.as_ref().unwrap().local_applies,
            fixture
                .receive
                .as_ref()
                .unwrap()
                .replication
                .latest_server_tick(),
            fixture.server.is_participating(CLIENT),
            fixture.client.is_participating(),
        );
        fixture.receive.as_mut().unwrap().acknowledged_baseline = fixture
            .replication
            .current_baseline_id(fixture.pawn_network());
        fixture.idle(0, 20);
        fixture
    }

    fn pawn_baseline_acked(&self) -> bool {
        let network = self.pawn_network();
        self.replication.has_registered_clients()
            && self
                .replication
                .current_baseline_id(network)
                .is_some_and(|baseline| {
                    self.replication
                        .baseline_acked_by_all_recipients(network, baseline)
                })
    }

    fn local_cursor(&self) -> Option<ActivationCursor> {
        self.local
            .borrow()
            .get_component::<WeaponComponent>(self.local_actors.weapon)
            .ok()?
            .state
            .activation_cursor()
    }

    pub(super) fn receive_snapshots(&mut self) {
        let before = self.local_cursor();
        let baseline_acked = self
            .receive
            .as_ref()
            .and_then(|receive| receive.acknowledged_baseline)
            .is_some_and(|baseline| {
                self.replication
                    .baseline_acked_by_all_recipients(self.pawn_network(), baseline)
            });
        let Some(receive) = self.receive.as_mut() else {
            return;
        };
        let tuning = receive.tuning.as_ref();
        let no_volumes = postretro_collision::EnvironmentVolumes::empty();
        let outcome = netcode::client_receive_and_apply(
            &mut self.local.borrow_mut(),
            &mut receive.ctx.slot_table.borrow_mut(),
            &Default::default(),
            &Default::default(),
            &Default::default(),
            &mut self.client,
            &mut receive.replication,
            &mut receive.state,
            &mut receive.prediction,
            &self.descriptors,
            &self.zones,
            None,
            &self.world,
            &no_volumes,
            DT,
            Duration::from_secs_f32(DT),
            None,
            tuning.and_then(|payload| payload.movement.as_ref()),
            tuning,
            false,
            &mut NoBindings,
            f64::from(self.tick) * f64::from(DT),
            Some(0.0),
        );
        if let Some(armed) = outcome.armed_local_pawn {
            receive.local_applies += 1;
            self.local_actors.pawn = armed.entity_id;
            self.local_actors.weapon = self
                .local
                .borrow()
                .get_component::<Inventory>(armed.entity_id)
                .unwrap()
                .active_wieldable()
                .unwrap();
            if before.is_some() {
                receive.active_applies.push(self.snapshots.len());
                // Setup has acknowledged the pawn baseline; subsequent ordinary
                // production records are deltas, not initial materialization.
                if baseline_acked {
                    receive.active_applies_after_baseline_ack += 1;
                }
                assert_eq!(
                    self.local
                        .borrow()
                        .get_component::<WeaponComponent>(self.local_actors.weapon)
                        .unwrap()
                        .state
                        .activation_cursor(),
                    before,
                    "ordinary local-pawn snapshot erased a live alternate-fire cursor"
                );
            }
        }
        self.projection.merge(&outcome.owner_private_weapon);
    }

    pub(super) fn publish_receive_snapshots(&mut self) {
        let Some(receive) = self.receive.as_mut() else {
            return;
        };
        netcode::host_replicate(
            &self.host.borrow(),
            &receive.ctx.slot_table.borrow(),
            &Default::default(),
            &Default::default(),
            &mut self.server,
            &mut self.allocator,
            &mut self.replication,
            &mut self.slots,
            &receive.set,
            &self.owners,
            &self.weapon_owners,
            &self.queues,
            None,
            self.tick,
            f64::from(self.tick) * f64::from(DT),
            self.tick.is_multiple_of(2),
            &mut receive.last_snapshot_tick,
            &Default::default(),
        );
    }

    fn alternate_command(
        &self,
        capture: &mut ActivationInputCapture,
        tick: u32,
        state: ButtonState,
        moving: bool,
    ) -> sim::SimCommand {
        capture.set_active(self.local_cursor().map(|cursor| cursor.token));
        capture.observe(&ActionSnapshot::with_button_state(Action::AltFire, state));
        let mut command = if matches!(state, ButtonState::Pressed | ButtonState::Held) {
            held(ActivationLane::Secondary, state == ButtonState::Pressed)
        } else {
            neutral()
        };
        command.activation = capture.command(tick);
        if moving {
            command.movement.wish_dir = Vec2::X;
        }
        command
    }
}

// Regression: ordinary movement snapshots cancelled a client's remaining burst
// rounds while the host spent all three rounds and had no HIT for ordinals 1/2.
#[test]
fn receive_shipped_rifle_secondary_keeps_three_declared_rounds_across_snapshots() {
    for moving in [false, true] {
        let mut fixture = Fixture::with_received_dev_weapon("reference_rifle");
        if moving {
            // One bounded realistic-link case: admission is already complete,
            // then the existing mandated delay/jitter/loss conditions apply to
            // every command, ordinary snapshot and outcome during the burst.
            let up = mandated_link();
            let mut down = up;
            down.seed ^= 0x5a5a;
            fixture.up = PacketConditioner::new(up);
            fixture.down = PacketConditioner::new(down);
            // Establish normal in-flight snapshots before the short burst;
            // an empty delayed pipeline can outlast its entire execution.
            fixture.idle(20, 20);
        }
        let mut capture = ActivationInputCapture::default();
        let start_tick = if moving { 40 } else { 20 };
        let start = token(start_tick, ActivationLane::Secondary);
        let before_applies = fixture.receive.as_ref().unwrap().local_applies;
        for offset in 0..24 {
            let tick = start_tick + offset;
            let state = match offset {
                0 => ButtonState::Pressed,
                1 => ButtonState::Released,
                _ => ButtonState::Inactive,
            };
            let command = fixture.alternate_command(&mut capture, tick, state, moving);
            fixture.step(tick, command);
            // A render-only frame can receive/reconcile without producing an
            // input command or advancing the weapon's fixed-tick cursor.
            if offset == 0 || offset == 6 {
                let applies = fixture.receive.as_ref().unwrap().local_applies;
                let cursor = fixture.local_cursor();
                let shots = fixture.snapshots.len();
                fixture.transport();
                assert_eq!(fixture.local_cursor(), cursor);
                assert_eq!(fixture.snapshots.len(), shots);
                if !moving {
                    assert!(
                        fixture.receive.as_ref().unwrap().local_applies > applies,
                        "perfect-link render-only transport did not apply a new snapshot"
                    );
                }
            }
        }
        if moving {
            fixture.idle(start_tick + 24, 60);
        }
        let expected: Vec<_> = (0..3)
            .map(|ordinal| fixture.shot_id(start, ordinal))
            .collect();
        assert_eq!(
            fixture.snapshot_ticks,
            [start_tick, start_tick + 5, start_tick + 10]
        );
        assert_eq!(
            fixture
                .snapshots
                .iter()
                .map(|shot| shot.activation.shot_id)
                .collect::<Vec<_>>(),
            expected
        );
        assert_eq!(
            fixture
                .authorized
                .iter()
                .map(|shot| shot.shot_id)
                .collect::<Vec<_>>(),
            expected
        );
        let receive = fixture.receive.as_ref().unwrap();
        assert_eq!(receive.declarations, expected);
        assert!(receive.local_applies > before_applies + 3);
        if moving {
            // Delayed snapshots can skip either short wait. Require an actual
            // active receive without prescribing its arrival within the burst.
            assert!(
                receive.active_applies_after_baseline_ack >= 1,
                "conditioned burst received no active snapshot: {:?}",
                receive.active_applies
            );
        } else {
            assert!(receive.active_applies.contains(&1));
            assert!(receive.active_applies.contains(&2));
            assert!(receive.active_applies_after_baseline_ack >= 2);
        }
        assert_eq!(
            receive.fire_cues,
            expected
                .iter()
                .map(|id| (*id, Some("weapons/rifle_fire".into())))
                .collect::<Vec<_>>()
        );
        // Three frozen Secondary shot cues inherit the rifle's authored sound.
        // This checks eligibility/identity at the real production effects
        // boundary, not audible playback, and makes no primary-fallback claim.
        assert!(
            fixture
                .snapshots
                .iter()
                .all(|shot| shot.sounds.fire.as_deref() == Some("weapons/rifle_fire"))
        );
        assert_eq!(fixture.accepted(start), 1);
        assert!(fixture.hit_refusals.is_empty());
        assert_eq!(fixture.ammo(), 27);
        near(fixture.damage(), 27.0);
        for shot in &fixture.authorized {
            near(shot.damage, 9.0);
        }
        for id in expected {
            assert!(fixture.verdicts.iter().any(|verdict| verdict.shot_id
                == netcode::wire_convert::shot_id_to_wire(id)
                && verdict.accept
                && verdict.hit_accepted));
        }
        assert_eq!(
            fixture.sent[start_tick as usize].activation.initiation,
            Some(wire_token(start))
        );
        assert_eq!(
            fixture.sent[start_tick as usize + 1]
                .activation
                .release
                .unwrap()
                .token,
            wire_token(start)
        );
        assert!(
            fixture
                .sent
                .iter()
                .all(|input| !input.fire_button.active && !input.fire_button.pressed)
        );
    }
}

impl Fixture {
    fn charge_hud(&mut self) -> (bool, f32) {
        let receive = self.receive.as_mut().unwrap();
        receive
            .hud
            .tick_for_role_and_report_sampled_weapon(true, None);
        let slots = receive.ctx.slot_table.borrow();
        let Some(SlotValue::Boolean(charging)) = slots
            .get("player.weaponCharging")
            .and_then(|slot| slot.value.as_ref())
        else {
            panic!("connected-client publisher must expose charging");
        };
        let Some(SlotValue::Number(progress)) = slots
            .get("player.weaponChargeProgress")
            .and_then(|slot| slot.value.as_ref())
        else {
            panic!("connected-client publisher must expose fixed-tick progress");
        };
        (*charging, *progress)
    }

    fn host_cell(&self) -> f32 {
        self.host
            .borrow()
            .get_component::<WeaponComponent>(self.host_actors.weapon)
            .unwrap()
            .cell
            .unwrap()
            .charge
    }
}

// Regression: ordinary local-pawn snapshots erased Charging and the original
// token, so the physical secondary release could no longer name that execution.
#[test]
fn receive_shipped_plasma_secondary_keeps_charge_hud_and_original_release_across_snapshots() {
    // Early, exact minimum, partial, full-plus-hold, and both edges before one
    // fixed command. All timing/scaling uses the shipped dev action program.
    for elapsed in [0, 11, 12, 30, 65] {
        let mut fixture = Fixture::with_received_dev_weapon("reference_plasma_bolt");
        let mut capture = ActivationInputCapture::default();
        let start = token(20, ActivationLane::Secondary);
        let timing = weapon::execution::action_program(
            fixture
                .local
                .borrow()
                .get_component::<WeaponComponent>(fixture.local_actors.weapon)
                .unwrap(),
            ActivationLane::Secondary,
        )
        .unwrap()
        .timing
        .charge
        .unwrap();
        assert_eq!((timing.min_ticks, timing.full_ticks), (12, 60));
        let before_applies = fixture.receive.as_ref().unwrap().local_applies;
        let mut last_progress = 0.0;
        for offset in 0..elapsed {
            let tick = start.start_tick + offset;
            let command = fixture.alternate_command(
                &mut capture,
                tick,
                if offset == 0 {
                    ButtonState::Pressed
                } else {
                    ButtonState::Held
                },
                false,
            );
            fixture.step(tick, command);
            let cursor = fixture
                .local_cursor()
                .expect("secondary remains charging until explicit release");
            assert_eq!(cursor.token, start);
            assert_eq!(cursor.phase, ActivationPhase::Charging);
            let (charging, progress) = fixture.charge_hud();
            assert!(charging);
            assert!(progress + 1e-6 >= last_progress);
            near(
                progress,
                offset.min(timing.full_ticks) as f32 / timing.full_ticks as f32,
            );
            last_progress = progress;
            assert!(fixture.snapshots.is_empty());
            assert!(fixture.authorized.is_empty());
            assert!(fixture.receive.as_ref().unwrap().declarations.is_empty());
            near(fixture.host_cell(), 100.0);
            if offset == 2 || offset == 62 {
                let applies = fixture.receive.as_ref().unwrap().local_applies;
                let cursor = fixture.local_cursor();
                let charging_before = charging;
                let progress_before = progress;
                fixture.transport();
                assert!(
                    fixture.receive.as_ref().unwrap().local_applies > applies,
                    "render-only transport did not apply a new snapshot"
                );
                assert_eq!(
                    fixture.local_cursor(),
                    cursor,
                    "render-only receive cannot advance charge"
                );
                let (charging_after, progress_after) = fixture.charge_hud();
                assert_eq!(charging_after, charging_before);
                near(progress_after, progress_before);
            }
        }
        let release_tick = start.start_tick + elapsed;
        let command = if elapsed == 0 {
            capture.observe(&ActionSnapshot::with_button_state(
                Action::AltFire,
                ButtonState::Pressed,
            ));
            capture.observe(&ActionSnapshot::with_button_state(
                Action::AltFire,
                ButtonState::Released,
            ));
            let mut command = neutral();
            command.activation = capture.command(release_tick);
            command
        } else {
            fixture.alternate_command(&mut capture, release_tick, ButtonState::Released, false)
        };
        assert_eq!(
            command.activation.release,
            Some(ActivationRelease {
                token: start,
                release_tick
            })
        );
        fixture.step(release_tick, command);
        fixture.idle(release_tick + 1, 20);
        assert_eq!(
            fixture.sent[release_tick as usize]
                .activation
                .release
                .unwrap()
                .token,
            wire_token(start)
        );
        assert_eq!(
            fixture.sent[20].activation.initiation,
            Some(wire_token(start))
        );
        assert!(
            fixture
                .sent
                .iter()
                .all(|input| !input.fire_button.pressed && !input.fire_button.active)
        );
        assert!(
            fixture
                .sent
                .iter()
                .filter_map(|input| input.activation.initiation)
                .all(|token| token == wire_token(start))
        );
        assert!(!fixture.charge_hud().0);
        near(fixture.charge_hud().1, 0.0);
        if elapsed < timing.min_ticks {
            assert!(fixture.snapshots.is_empty());
            assert!(fixture.authorized.is_empty());
            assert!(fixture.receive.as_ref().unwrap().declarations.is_empty());
            near(fixture.host_cell(), 100.0);
            near(fixture.damage(), 0.0);
            continue;
        }
        let charge = elapsed.min(timing.full_ticks) as f32 / timing.full_ticks as f32;
        let damage = 10.0 * (1.0 + 9.0 * charge);
        let cost = 5.0 * (1.0 + 9.0 * charge);
        let size = 1.5 * (1.0 + charge);
        let shot_id = fixture.shot_id(start, 0);
        assert_eq!(fixture.snapshot_ticks, [release_tick]);
        assert_eq!(fixture.snapshots.len(), 1);
        assert_eq!(fixture.authorized.len(), 1);
        let shot = &fixture.snapshots[0];
        assert_eq!(shot.activation.shot_id, shot_id);
        near(shot.activation.charge, charge);
        near(shot.activation.values.damage, damage);
        assert!(
            matches!(shot.activation.values.resource_cost, postretro_foundation::ShotResourceCost::Cell(actual) if (actual - cost).abs() < 0.001)
        );
        let ProjectileBodyVisual::Sprite {
            size: actual_size, ..
        } = shot.projectile.as_ref().unwrap().visual.body
        else {
            panic!("shipped plasma uses a sprite body");
        };
        near(actual_size, size);
        assert_eq!(fixture.authorized[0].shot_id, shot_id);
        near(fixture.authorized[0].damage, damage);
        assert_eq!(fixture.presentations.len(), 1);
        let ProjectileBodyVisual::Sprite {
            size: host_size, ..
        } = fixture.presentations[0].projectile.visual.body
        else {
            panic!("host plasma presentation retains the scaled sprite");
        };
        near(host_size, size);
        assert_eq!(fixture.receive.as_ref().unwrap().declarations, [shot_id]);
        assert!(fixture.receive.as_ref().unwrap().local_applies > before_applies + 3);
        assert!(fixture.receive.as_ref().unwrap().active_applies.len() >= 4);
        assert!(fixture.verdicts.iter().any(|verdict| verdict.shot_id
            == netcode::wire_convert::shot_id_to_wire(shot_id)
            && verdict.accept
            && verdict.hit_accepted));
        assert!(fixture.hit_refusals.is_empty());
        near(fixture.host_cell(), 100.0 - cost);
        near(fixture.damage(), damage);
    }
}
