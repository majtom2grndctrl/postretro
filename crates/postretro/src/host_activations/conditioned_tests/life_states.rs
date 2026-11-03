//! Life-state profiles over a conditioned link: fire authority, owner prediction, join, look, respawn.
//! See: context/lib/scripting.md §12 (Player life states) · context/lib/networking.md §Combat authority

use super::receive_alternate::ReceiveSetup;
use super::*;
use postretro_entities::ScriptCtx;
use postretro_entities::data_descriptors::{
    EntityTypeDescriptor, HealthDescriptor, InventoryDescriptor, LifeStatesDescriptor,
    NamedReaction, PlayerEventDescriptor, PlayerEventEdge, PlayerEventTrigger, PrimitiveDescriptor,
    ReactionDescriptor,
};
use postretro_foundation::{
    Capability, IrNode, IrValue, LifeStateProfile, Seat, player_capability,
};
use postretro_sim::player_events::PlayerEventTable;

/// The host's local movement pawn (Seat 0) is the listen host; the remote
/// shooter (Seat 1) is the connected client's pawn. Only the shooter's
/// zero-health edge enters `down`. Both are real player-event subjects.
struct LifeFixture {
    fixture: Fixture,
    ctx: ScriptCtx,
    events: PlayerEventTable,
    host_player: EntityId,
    down_id: u8,
    alive_id: u8,
    next_tick: u32,
}

fn entity(canonical: &str) -> EntityTypeDescriptor {
    EntityTypeDescriptor {
        canonical_name: Some(canonical.into()),
        ghost: false,
        faction: None,
        tolerance: None,
        inventory: None,
        light: None,
        emitter: None,
        weapon: None,
        touchable: None,
        mesh: None,
        health: None,
        behavior: None,
        movement: None,
        life_states: None,
    }
}

fn movement_descriptor() -> PlayerMovementDescriptor {
    let component = movement();
    PlayerMovementDescriptor {
        capsule: component.capsule,
        ground: component.ground_params,
        air: component.air,
        fall: component.fall,
        dash: component.dash,
        crouch: component.crouch,
        slide: component.slide,
        swim: component.swim,
        vault: component.vault,
        view_feel: component.view_feel,
        sounds: component.sounds,
        knockback: component.knockback,
        forgiveness: None,
        stuck_stop_enabled: component.stuck_stop_enabled,
        stuck_stop_threshold: component.stuck_stop_threshold,
    }
}

impl LifeFixture {
    fn new(weapon: WeaponDescriptor, down_fire: bool) -> Self {
        let mut player = entity("player");
        player.movement = Some(movement_descriptor());
        player.inventory = Some(InventoryDescriptor {
            loadout: vec!["conditioned-weapon".into()],
        });
        player.health = Some(HealthDescriptor {
            max: TARGET_HP,
            hitbox: None,
            zone_multipliers: Default::default(),
        });
        player.life_states = Some(LifeStatesDescriptor {
            initial: "alive".into(),
            states: BTreeMap::from([
                ("alive".into(), LifeStateProfile::default()),
                (
                    "down".into(),
                    LifeStateProfile {
                        fire: down_fire,
                        ..Default::default()
                    },
                ),
            ]),
            rejoin: Default::default(),
            state_ids: Default::default(),
            table_revision: 0,
        });
        let mut weapon_entity = entity("conditioned-weapon");
        weapon_entity.weapon = Some(weapon.clone());
        let mut ctx = ScriptCtx::new();
        ctx.data_registry
            .borrow_mut()
            .replace_entity_types(vec![player, weapon_entity]);
        let descriptors = ctx.data_registry.borrow().entities.clone();
        let mut fixture = Fixture::with_snapshot_receive(
            ScriptCtx::new(),
            ReceiveSetup {
                host_descriptors: descriptors.clone(),
                client_descriptors: descriptors,
                carried: None,
                weapon,
                hold_tuning: false,
            },
        );
        ctx.registry = fixture.host.clone();
        // Match the weapon fixture's fixed eye at y=0.5 on both peers. The
        // empty collision world deliberately permits a centre at the origin.
        place(
            &mut fixture.host.borrow_mut(),
            fixture.host_actors.pawn,
            Vec3::ZERO,
        );
        {
            let mut local = fixture.local.borrow_mut();
            place(&mut local, fixture.local_actors.pawn, Vec3::ZERO);
            // The real owner is normally health-slot-driven. Give the local
            // pawn a health component too, which `zero_shooter(true)` zeroes,
            // so the prediction and sweep regressions cover the independent
            // component-based liveness check.
            local
                .set_component(fixture.local_actors.pawn, health(None))
                .unwrap();
        }
        let host_player = ctx.registry.borrow().local_player_movement_pawn().unwrap();
        {
            let mut registry = ctx.registry.borrow_mut();
            registry.bind_pawn_seat(host_player, Seat(0));
            registry.bind_pawn_seat(fixture.host_actors.pawn, Seat(1));
            registry
                .set_component(
                    host_player,
                    DescriptorProvenance {
                        canonical_name: "player".into(),
                        owned_components: Default::default(),
                        map_overrides: Default::default(),
                        spawn_path: DescriptorSpawnPath::PlayerSpawn,
                    },
                )
                .unwrap();
            postretro_sim::life_state_commands::set_life_state(
                &ctx,
                &mut registry,
                &[host_player],
                "alive",
            );
        }
        let (alive_id, down_id) = {
            let data = ctx.data_registry.borrow();
            let states = data
                .entities
                .iter()
                .find_map(|entity| entity.life_states.as_ref())
                .unwrap();
            (
                states.state("alive").unwrap().0,
                states.state("down").unwrap().0,
            )
        };
        {
            let mut data = ctx.data_registry.borrow_mut();
            data.set_level_reactions(vec![NamedReaction {
                name: "enter-down".into(),
                descriptor: ReactionDescriptor::Primitive(PrimitiveDescriptor {
                    primitive: "setLifeState".into(),
                    target: Some("@player".into()),
                    kind: None,
                    tag: None,
                    on_complete: None,
                    args: json!({"state":"down"}),
                }),
            }]);
            data.set_level_player_events(vec![PlayerEventDescriptor {
                trigger: PlayerEventTrigger::Edge {
                    edge: PlayerEventEdge::Becomes,
                    condition: IrNode::Le {
                        a: Box::new(IrNode::Input {
                            name: "player.health".into(),
                            owner: None,
                        }),
                        b: Box::new(IrNode::Const {
                            value: IrValue::Number(0.0),
                        }),
                    },
                },
                fire: vec!["enter-down".into()],
                levels: Vec::new(),
                authored_index: 0,
            }]);
            data.recompose(&[]);
        }
        let events = PlayerEventTable::build(&ctx, Default::default(), Default::default(), None);
        // Admission and the first baseline finish on a clean link. Gameplay,
        // state changes and outcomes then cross 75 ms in each direction.
        let link = LinkConfig {
            delay: 75,
            ..LinkConfig::perfect()
        };
        fixture.up = PacketConditioner::new(link);
        fixture.down = PacketConditioner::new(LinkConfig {
            seed: link.seed ^ 0x5a5a,
            ..link
        });
        let mut result = Self {
            fixture,
            ctx,
            events,
            host_player,
            down_id,
            alive_id,
            next_tick: 0,
        };
        for _ in 0..32 {
            result.step(neutral());
        }
        result
    }

    fn step(&mut self, command: sim::SimCommand) {
        self.fixture.step(self.next_tick, command);
        self.next_tick += 1;
        let mut residuals = Vec::new();
        self.events.run_tick(&self.ctx, &[], &mut residuals);
        assert!(
            residuals.is_empty(),
            "setLifeState is an in-tick player command"
        );
        self.fixture.publish_receive_snapshots();
    }

    fn zero_shooter(&mut self, local_too: bool) {
        for (registry, pawn) in std::iter::once((&self.fixture.host, self.fixture.host_actors.pawn))
            .chain(local_too.then_some((&self.fixture.local, self.fixture.local_actors.pawn)))
        {
            let mut registry = registry.borrow_mut();
            let ComponentValue::Health(health) = registry
                .get_component_value_mut(pawn, ComponentKind::Health)
                .unwrap()
            else {
                unreachable!()
            };
            health.current = 0.0;
        }
    }

    fn repeated_death_sweeps(&self) {
        for registry in [&self.fixture.host, &self.fixture.local] {
            for _ in 0..2 {
                postretro_sim::scripting_systems::health::sweep_deaths_for_test(
                    &mut registry.borrow_mut(),
                );
            }
        }
    }

    fn assert_owner_down(&self, fire: bool) {
        let registry = self.fixture.local.borrow();
        let pawn = registry
            .get_component::<PlayerMovementComponent>(self.fixture.local_actors.pawn)
            .unwrap();
        assert_eq!(pawn.life_state, self.down_id);
        assert_eq!(player_capability(pawn, Capability::Fire), Some(fire));
        let data = self.ctx.data_registry.borrow();
        assert_eq!(
            pawn.life_state_table_revision,
            data.life_state_table_revision
        );
    }

    fn assert_other_player_untouched(&self) {
        let registry = self.ctx.registry.borrow();
        assert_eq!(
            registry
                .get_component::<PlayerMovementComponent>(self.host_player)
                .unwrap()
                .life_state,
            self.alive_id
        );
        near(
            registry
                .get_component::<HealthComponent>(self.host_player)
                .unwrap()
                .current,
            TARGET_HP,
        );
    }

    fn host_cursor(&self) -> Option<postretro_foundation::ActivationCursor> {
        self.fixture
            .host
            .borrow()
            .get_component::<WeaponComponent>(self.fixture.host_actors.weapon)
            .unwrap()
            .state
            .activation_cursor()
    }

    fn owner_cursor(&self) -> Option<postretro_foundation::ActivationCursor> {
        self.fixture
            .local
            .borrow()
            .get_component::<WeaponComponent>(self.fixture.local_actors.weapon)
            .unwrap()
            .state
            .activation_cursor()
    }

    fn owner_received_down(&self) -> bool {
        self.fixture
            .local
            .borrow()
            .get_component::<PlayerMovementComponent>(self.fixture.local_actors.pawn)
            .unwrap()
            .life_state
            == self.down_id
    }

    /// Enters `down` on the host at full HP, as a trigger reaction does, so
    /// only the profile can refuse fire.
    fn host_enter_down(&self) {
        postretro_sim::life_state_commands::set_life_state(
            &self.ctx,
            &mut self.ctx.registry.borrow_mut(),
            &[self.fixture.host_actors.pawn],
            "down",
        );
        near(
            self.ctx
                .registry
                .borrow()
                .get_component::<HealthComponent>(self.fixture.host_actors.pawn)
                .unwrap()
                .current,
            TARGET_HP,
        );
    }

    /// The owner liveness pass the client runs whenever a snapshot lands. Its
    /// `player.health` slot reads full HP, so only the declared profile can
    /// refuse fire.
    fn owner_receipt_liveness(&mut self) -> bool {
        let mut slots = postretro_entities::SlotTable::default();
        slots.get_mut("player.health").unwrap().value =
            Some(postretro_entities::SlotValue::Number(TARGET_HP));
        let mut capture = crate::input::ActivationInputCapture::default();
        capture.set_active(self.owner_cursor().map(|cursor| cursor.token));
        self.fixture.frame.apply_owner_liveness(
            &mut self.fixture.local.borrow_mut(),
            &slots,
            &mut capture,
        )
    }

    /// Holds `lane` until the host's `down` record reaches the owner, running
    /// the receipt liveness pass after every step. It must permit fire until
    /// the record lands and refuse it from that step on.
    fn hold_until_owner_receipt(&mut self, lane: ActivationLane) {
        for _ in 0..40 {
            self.step(held(lane, false));
            let received = self.owner_received_down();
            assert_eq!(
                self.owner_receipt_liveness(),
                !received,
                "at full HP only the received fire:false profile refuses the owner"
            );
            if received {
                return;
            }
        }
        panic!("the host's down record never reached the owner");
    }
}

#[test]
fn conditioned_player_event_down_refuses_remote_fire_and_reaches_its_owner() {
    let mut life = LifeFixture::new(descriptor(false, false, 1, "press"), false);
    life.zero_shooter(false);
    life.step(neutral());
    {
        let registry = life.ctx.registry.borrow();
        let pawn = registry
            .get_component::<PlayerMovementComponent>(life.fixture.host_actors.pawn)
            .unwrap();
        assert_eq!(pawn.life_state, life.down_id);
        assert_eq!(
            pawn.life_state_elapsed_ticks, 0,
            "the player-event entry pass ends at zero"
        );
    }
    let attempted = token(life.next_tick, ActivationLane::Primary);
    life.step(held(attempted.lane, true));
    assert_eq!(
        life.ctx
            .registry
            .borrow()
            .get_component::<PlayerMovementComponent>(life.fixture.host_actors.pawn)
            .unwrap()
            .life_state_elapsed_ticks,
        1,
        "the next host movement tick advances the new state's clock once"
    );
    for _ in 0..100 {
        life.step(neutral());
    }
    assert!(
        life.fixture
            .sent
            .iter()
            .any(|command| command.activation.initiation == Some(wire_token(attempted))),
        "the owner still thought fire was permitted and sent its start before the state arrived"
    );
    assert!(life.fixture.outcomes.iter().any(|outcome| matches!(outcome,
        wire::ActivationOutcome::InitiationRejected { token, .. } if *token == wire_token(attempted))),
        "the production host path refuses the in-flight start");
    life.fixture.assert_no_host_shot();
    assert_eq!(
        life.fixture.snapshots.len(),
        1,
        "only the pre-update owner predicted the shot"
    );
    life.assert_owner_down(false);
    let before = life.fixture.snapshots.len();
    life.step(held(ActivationLane::Primary, true));
    assert_eq!(
        life.fixture.snapshots.len(),
        before,
        "received fire:false prevents local prediction"
    );
    life.assert_other_player_untouched();
}

#[test]
fn life_state_o12_owner_refuses_locally_until_host_fire_permission_arrives() {
    let mut life = LifeFixture::new(descriptor(false, false, 1, "press"), false);
    life.zero_shooter(false);
    for _ in 0..80 {
        life.step(neutral());
    }
    life.assert_owner_down(false);
    postretro_sim::life_state_commands::set_life_state(
        &life.ctx,
        &mut life.ctx.registry.borrow_mut(),
        &[life.fixture.host_actors.pawn],
        "alive",
    );
    let before_sent = life.fixture.sent.len();
    let before_predicted = life.fixture.snapshots.len();
    life.step(held(ActivationLane::Primary, true));
    life.step(held(ActivationLane::Primary, false));
    assert_eq!(
        life.fixture.snapshots.len(),
        before_predicted,
        "the still-blocked owner predicts no shot in the return window"
    );
    assert!(
        life.fixture.sent[before_sent..]
            .iter()
            .all(|command| command.activation.initiation.is_none()),
        "a locally refused press sends no activation start"
    );
    life.fixture.assert_no_host_shot();
    for _ in 0..80 {
        life.step(neutral());
    }
    assert_eq!(
        life.fixture
            .local
            .borrow()
            .get_component::<PlayerMovementComponent>(life.fixture.local_actors.pawn)
            .unwrap()
            .life_state,
        life.alive_id
    );
    life.step(held(ActivationLane::Primary, true));
    for _ in 0..80 {
        life.step(neutral());
    }
    assert_eq!(
        life.fixture.authorized.len(),
        1,
        "a fresh press fires after the authoritative permission arrives"
    );
    life.assert_other_player_untouched();
}

#[test]
fn conditioned_fire_false_mid_burst_and_mid_hold_cancels_on_host_and_at_owner_receipt() {
    // Twelve rounds 50 ms apart keep each execution running from the host's
    // transition until well after the state reaches the owner.
    for trigger in ["press", "hold"] {
        let mut life = LifeFixture::new(descriptor(false, false, 12, trigger), false);
        let start = token(life.next_tick, ActivationLane::Primary);
        life.step(held(start.lane, true));
        for _ in 0..40 {
            if !life.fixture.authorized.is_empty() {
                break;
            }
            life.step(held(start.lane, false));
        }
        let host_rounds = life.fixture.authorized.len();
        assert!(
            (1..12).contains(&host_rounds) && life.host_cursor().is_some(),
            "{trigger}: the host is mid-execution when the state changes"
        );
        let predicted_at_transition = life.fixture.snapshots.len();
        life.host_enter_down();
        assert_eq!(
            life.host_cursor(),
            None,
            "{trigger}: entering fire:false cancels the host execution"
        );

        life.hold_until_owner_receipt(start.lane);
        assert!(
            life.fixture.snapshots.len() > predicted_at_transition,
            "{trigger}: the owner kept predicting rounds while the state was in flight"
        );
        assert_eq!(
            life.owner_cursor(),
            None,
            "{trigger}: the owner's execution ends at receipt"
        );

        let predicted_at_receipt = life.fixture.snapshots.len();
        let sent_at_receipt = life.fixture.sent.len();
        for _ in 0..40 {
            life.step(held(start.lane, false));
        }
        life.step(held(start.lane, true));
        for _ in 0..80 {
            life.step(neutral());
        }
        assert_eq!(
            life.fixture.snapshots.len(),
            predicted_at_receipt,
            "{trigger}: neither the held trigger nor a fresh press predicts after receipt"
        );
        assert!(
            life.fixture.sent[sent_at_receipt..]
                .iter()
                .all(|command| command.activation.initiation.is_none()),
            "{trigger}: the refused owner sends no activation start"
        );
        assert_eq!(
            life.fixture.authorized.len(),
            host_rounds,
            "{trigger}: the host authorizes no round after its transition"
        );
        life.assert_owner_down(false);
        life.assert_other_player_untouched();
    }
}

#[test]
fn conditioned_fire_false_mid_charge_cancels_on_host_and_at_owner_receipt() {
    let mut life = LifeFixture::new(descriptor(false, true, 1, "press"), false);
    let start = token(life.next_tick, ActivationLane::Secondary);
    life.step(held(start.lane, true));
    for _ in 0..40 {
        if life.fixture.host_charging() {
            break;
        }
        life.step(held(start.lane, false));
    }
    assert!(
        life.fixture.host_charging(),
        "the host is mid-charge when the state changes"
    );
    life.host_enter_down();
    assert_eq!(
        life.host_cursor(),
        None,
        "entering fire:false cancels the host charge"
    );

    life.hold_until_owner_receipt(start.lane);
    assert_eq!(
        life.owner_cursor(),
        None,
        "the owner's charge ends at receipt"
    );
    let mut release = neutral();
    release.activation.release = Some(ActivationRelease {
        token: start,
        release_tick: life.next_tick,
    });
    life.step(release);
    for _ in 0..100 {
        life.step(neutral());
    }
    assert!(
        life.fixture.snapshots.is_empty(),
        "the cancelled charge predicts no release shot"
    );
    life.fixture.assert_no_host_shot();
    assert!(!life.fixture.host_charging());
    life.assert_owner_down(false);
    life.assert_other_player_untouched();
}

#[test]
fn conditioned_zero_hp_fire_true_hold_and_burst_survive_repeated_death_sweeps() {
    for (shots, trigger, ticks) in [(1, "hold", 150), (3, "press", 24)] {
        let mut life = LifeFixture::new(descriptor(false, false, shots, trigger), true);
        let start = token(life.next_tick, ActivationLane::Primary);
        life.step(held(start.lane, true));
        life.zero_shooter(true);
        for _ in 1..ticks {
            life.step(held(start.lane, false));
            life.repeated_death_sweeps();
        }
        for _ in 0..80 {
            life.step(neutral());
            life.repeated_death_sweeps();
        }
        assert!(
            !life.fixture.cancelled(start),
            "zero HP must not cancel a permitted {trigger} execution"
        );
        if trigger == "press" {
            assert_eq!(life.fixture.snapshots.len(), 3);
            assert_eq!(
                life.fixture.authorized.len(),
                3,
                "every burst round crosses a zero-HP sweep"
            );
        } else {
            assert!(life.fixture.snapshots.len() >= 4);
            assert!(
                life.fixture.authorized.len() >= 4,
                "holding fire restarts through repeated zero-HP sweeps"
            );
        }
        assert!(
            life.fixture
                .host
                .borrow()
                .get_component::<HealthComponent>(life.fixture.host_actors.pawn)
                .unwrap()
                .death_handled,
            "the normal host death sweep observed zero HP"
        );
        life.assert_owner_down(true);
        life.assert_other_player_untouched();
    }
}

#[test]
fn conditioned_zero_hp_fire_true_charge_survives_sweeps_and_fires_its_release() {
    let mut life = LifeFixture::new(descriptor(false, true, 1, "press"), true);
    let start = token(life.next_tick, ActivationLane::Secondary);
    life.step(held(start.lane, true));
    life.zero_shooter(true);
    for _ in 1..72 {
        life.step(held(start.lane, false));
        life.repeated_death_sweeps();
    }
    assert!(life.fixture.host_charging());
    assert!(
        life.fixture
            .local
            .borrow()
            .get_component::<WeaponComponent>(life.fixture.local_actors.weapon)
            .unwrap()
            .state
            .activation_cursor()
            .is_some_and(|cursor| cursor.token == start)
    );
    life.fixture.assert_no_host_shot();
    life.assert_owner_down(true);
    let mut release = neutral();
    release.activation.release = Some(ActivationRelease {
        token: start,
        release_tick: life.next_tick,
    });
    life.step(release);
    for _ in 0..100 {
        life.step(neutral());
        life.repeated_death_sweeps();
    }
    assert!(!life.fixture.cancelled(start));
    assert_eq!(life.fixture.snapshots.len(), 1);
    assert_eq!(
        life.fixture.authorized.len(),
        1,
        "the permitted charge releases after repeated death sweeps"
    );
    life.assert_other_player_untouched();
}

mod late_baseline_tests;
mod life_state_clock_trim_tests;
mod look_receipt_tests;
mod respawn;
