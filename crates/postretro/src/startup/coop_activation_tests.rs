//! Co-op activation puzzles on a headless host: the production sim tick, then
//! the session player counts and the `occupancy` families written from its
//! trigger-stage capture, the store accumulators, and the frame's crossing pass.
//! See: context/lib/scripting.md §5 (Session players, Occupancy), §12

use std::cell::RefCell;
use std::collections::{BTreeSet, HashMap};

use glam::Vec3;
use postretro_entities::components::health::HealthComponent;
use postretro_entities::data_descriptors::{CrossingCondition, CrossingDescriptor};
use postretro_entities::slot_table::{
    ReplicationScope, SlotOwnership, SlotRecord, SlotSchema, SlotType, SlotValue,
};
use postretro_entities::{
    EntityId, ScriptCtx, Transform, TriggerActivation, TriggerCommand, TriggerFireMode,
    TriggerVolumeComponent,
};
use postretro_foundation::{IrNode, IrValue, Seat};
use postretro_scripting_core::occupancy_references::FamilyReferences;
use postretro_scripting_core::state_crossings::CrossingDetector;
use postretro_sim::ai_host::AiTickResult;
use postretro_sim::scripting_systems::slot_accumulators::{
    SlotAccumulatorBindings, evaluate_slot_accumulators,
};
use postretro_sim::scripting_systems::trigger_volume_bridge::TriggerVolumeBridge;
use postretro_sim::sim::{
    PostMovementCommand, RemotePawnCommand, TriggerTickContext, simulate_tick_with_presentation_aim,
};
use postretro_sim::trigger_bindings::TriggerBindingTable;
use postretro_sim::trigger_system::TriggerSystem;

use super::player_death_surface_tests::command;
use crate::netcode::SeatTable;
use crate::netcode::session_players::CapturedPawns;
use crate::session::occupancy_families::OccupancyFamilies;
use crate::session::players::SessionPlayers;

const DT: f32 = 1.0 / 60.0;
/// Far from every volume the tests place.
const OUTSIDE: Vec3 = Vec3::new(500.0, 0.0, 0.0);

fn movement() -> postretro_foundation::PlayerMovementComponent {
    use postretro_scripting_core::data_descriptors::{
        AirParams, CapsuleParams, FallParams, GroundParams, PlayerMovementDescriptor, SpeedParams,
    };
    postretro_foundation::PlayerMovementComponent::from_descriptor(&PlayerMovementDescriptor {
        sounds: None,
        knockback: Default::default(),
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
            friction: GroundParams::DEFAULT_FRICTION,
            stop_speed: GroundParams::DEFAULT_STOP_SPEED,
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
        dash: None,
        forgiveness: None,
        crouch: None,
        slide: None,
        swim: None,
        vault: None,
        view_feel: None,
    })
}

fn input(name: &str) -> Box<IrNode> {
    Box::new(IrNode::Input {
        name: name.to_string(),
        owner: None,
    })
}

fn number(value: f32) -> Box<IrNode> {
    Box::new(IrNode::Const {
        value: IrValue::Number(value),
    })
}

/// `onStateCrossing(occupancy.tag(tag).occupiedCount, { above }, [fire])`.
pub(super) fn occupied_above(tag: &str, above: f32, fire: &str) -> CrossingDescriptor {
    CrossingDescriptor {
        slot: Some(format!("occupancy.{tag}.occupiedCount")),
        condition: CrossingCondition::Above { threshold: above },
        max: 1.0,
        edge: None,
        fire: vec![fire.to_string()],
    }
}

/// "Everyone is in": `loading == 0 && inPlay > 0 && occupants >= inPlay`.
pub(super) fn all_players_in(tag: &str, fire: &str) -> CrossingDescriptor {
    let in_play = || input("session.playerCounts.inPlay");
    CrossingDescriptor {
        slot: None,
        condition: CrossingCondition::Ir(IrNode::And {
            a: Box::new(IrNode::Eq {
                a: input("session.playerCounts.loading"),
                b: number(0.0),
            }),
            b: Box::new(IrNode::And {
                a: Box::new(IrNode::Gt {
                    a: in_play(),
                    b: number(0.0),
                }),
                b: Box::new(IrNode::Ge {
                    a: input(&format!("occupancy.{tag}.occupants")),
                    b: in_play(),
                }),
            }),
        }),
        max: 1.0,
        edge: None,
        fire: vec![fire.to_string()],
    }
}

/// One tick's counts and each tag's `(tag, occupiedCount, occupants)`.
pub(super) type TickRecord = (Vec<f32>, Vec<(String, f32, f32)>);

/// A headless co-op host: its own pawn on seat 0 plus connected clients, the
/// level's tagged trigger volumes, and the frame's crossing pass.
pub(super) struct Host {
    pub(super) ctx: ScriptCtx,
    seats: SeatTable,
    players: SessionPlayers,
    families: OccupancyFamilies,
    triggers: TriggerSystem,
    bridge: TriggerVolumeBridge,
    world: crate::collision::CollisionWorld,
    detector: CrossingDetector,
    accumulators: SlotAccumulatorBindings,
    pub(super) host: EntityId,
    clients: Vec<(u64, EntityId)>,
    next_client: u64,
    input_tick: u32,
    /// Every tick's `(connected, loading, inPlay, down, reconnecting)` and each
    /// tag's `(occupiedCount, occupants)`, in tick order.
    pub(super) timeline: Vec<TickRecord>,
}

impl Host {
    /// Install a session level with `volumes` as `(tag, centre)` plates
    /// (4 m cubes), the mod referencing `mod_tags` and the level `level_tags`,
    /// and the given crossings. The host's pawn spawns at `host_at`.
    pub(super) fn install(
        volumes: &[(&str, Vec3)],
        mod_tags: &[&str],
        level_tags: &[&str],
        crossings: Vec<CrossingDescriptor>,
        host_at: Vec3,
    ) -> Self {
        let ctx = ScriptCtx::new();
        let mut bridge = TriggerVolumeBridge::new();
        let mut volume_tags = BTreeSet::new();
        {
            let mut registry = ctx.registry.borrow_mut();
            for (tag, at) in volumes {
                let volume = registry.spawn(Transform {
                    position: *at,
                    ..Transform::default()
                });
                registry
                    .set_component(
                        volume,
                        TriggerVolumeComponent::new(
                            TriggerActivation::Touch,
                            String::new(),
                            String::new(),
                            String::new(),
                            TriggerCommand::Start,
                            TriggerFireMode::Multiple,
                            0.0,
                            true,
                        ),
                    )
                    .unwrap();
                registry.set_tags(volume, vec![(*tag).to_string()]).unwrap();
                bridge.insert_for_test(volume, *at - Vec3::splat(2.0), *at + Vec3::splat(2.0));
                volume_tags.insert((*tag).to_string());
            }
        }
        let refs = |tags: &[&str]| {
            let mut refs = FamilyReferences::default();
            for tag in tags {
                refs.note_slot(&format!("occupancy.{tag}.occupants"));
            }
            refs
        };
        let mut families = OccupancyFamilies::default();
        families.commit_mod(&refs(mod_tags), &mut ctx.slot_table.borrow_mut());
        // Install order: families, the seats' counts, crossings over both;
        // the host's pawn spawns last.
        families.install_level(
            &refs(level_tags),
            &volume_tags,
            &mut ctx.slot_table.borrow_mut(),
        );
        let mut seats = SeatTable::local_only();
        let mut players = SessionPlayers::default();
        players.begin_level(
            true,
            Some(&mut seats),
            None,
            &ctx.registry.borrow(),
            &mut ctx.slot_table.borrow_mut(),
        );
        ctx.data_registry
            .borrow_mut()
            .populate_level(Vec::new(), crossings, &[]);
        let mut detector = CrossingDetector::new();
        detector.initialize(&ctx.data_registry.borrow(), &ctx.slot_table.borrow(), &ctx);
        let mut accumulators = SlotAccumulatorBindings::default();
        accumulators.rebuild(&ctx);

        let host = {
            let mut registry = ctx.registry.borrow_mut();
            let host = Self::spawn_pawn(&mut registry, host_at);
            registry.mark_local_player_pawn(host).unwrap();
            seats.bind_pawn(&mut registry, Seat(0), host);
            host
        };
        Self {
            ctx,
            seats,
            players,
            families,
            triggers: TriggerSystem::default(),
            bridge,
            world: crate::collision::CollisionWorld::new(),
            detector,
            accumulators,
            host,
            clients: Vec::new(),
            next_client: 1,
            input_tick: 0,
            timeline: Vec::new(),
        }
    }

    fn spawn_pawn(registry: &mut postretro_entities::EntityRegistry, at: Vec3) -> EntityId {
        let pawn = registry.spawn(Transform {
            position: at,
            ..Transform::default()
        });
        registry.set_component(pawn, movement()).unwrap();
        pawn
    }

    /// A client is admitted: its seat reads loading until `spawn` binds a pawn.
    pub(super) fn admit(&mut self) -> u64 {
        let client = self.next_client;
        self.next_client += 1;
        self.seats.admit_or_reclaim(client, None, false).unwrap();
        client
    }

    /// The admitted `client`'s pawn spawns at `at`.
    pub(super) fn spawn(&mut self, client: u64, at: Vec3) -> EntityId {
        let seat = self.seats.seat_for_client(client).unwrap();
        let pawn = {
            let mut registry = self.ctx.registry.borrow_mut();
            let pawn = Self::spawn_pawn(&mut registry, at);
            self.seats.bind_pawn(&mut registry, seat, pawn);
            pawn
        };
        self.clients.push((client, pawn));
        pawn
    }

    /// Admit a client and spawn its pawn at `at`.
    pub(super) fn join(&mut self, at: Vec3) -> EntityId {
        let client = self.admit();
        self.spawn(client, at)
    }

    /// A level change or same-map restart: every client is demoted (its pawn
    /// despawns, its seat stays bound), the level reinstalls with the same
    /// references and crossings, and the host's pawn spawns at `host_at` after
    /// install. Clients re-promote later through `respawn`.
    pub(super) fn relevel(&mut self, host_at: Vec3) {
        {
            let mut registry = self.ctx.registry.borrow_mut();
            for (_, pawn) in self.clients.drain(..) {
                registry.despawn(pawn).unwrap();
            }
            registry.despawn(self.host).unwrap();
        }
        // The same level installs again: its references are the families it
        // holds now.
        let level_tags: BTreeSet<String> = self
            .ctx
            .slot_table
            .borrow()
            .family_members(&postretro_entities::engine_state_collections::OCCUPANCY_FAMILY);
        self.triggers = TriggerSystem::default();
        self.players
            .end_level(Some(&mut self.seats), &mut self.ctx.slot_table.borrow_mut());
        self.families
            .unload_level(&mut self.ctx.slot_table.borrow_mut());
        let crossings = self.ctx.data_registry.borrow().crossings.clone();
        let mut refs = FamilyReferences::default();
        for tag in &level_tags {
            refs.note_slot(&format!("occupancy.{tag}.occupants"));
        }
        let volume_tags = level_tags.clone();
        self.families
            .install_level(&refs, &volume_tags, &mut self.ctx.slot_table.borrow_mut());
        self.players.begin_level(
            true,
            Some(&mut self.seats),
            None,
            &self.ctx.registry.borrow(),
            &mut self.ctx.slot_table.borrow_mut(),
        );
        {
            let mut registry = self.ctx.data_registry.borrow_mut();
            registry.clear();
            registry.populate_level(Vec::new(), crossings, &[]);
        }
        self.detector = CrossingDetector::new();
        self.detector.initialize(
            &self.ctx.data_registry.borrow(),
            &self.ctx.slot_table.borrow(),
            &self.ctx,
        );
        let mut registry = self.ctx.registry.borrow_mut();
        self.host = Self::spawn_pawn(&mut registry, host_at);
        registry.mark_local_player_pawn(self.host).unwrap();
        self.seats.bind_pawn(&mut registry, Seat(0), self.host);
    }

    pub(super) fn place(&self, pawn: EntityId, at: Vec3) {
        let mut registry = self.ctx.registry.borrow_mut();
        let mut transform = *registry.get_component::<Transform>(pawn).unwrap();
        transform.position = at;
        registry.set_component(pawn, transform).unwrap();
    }

    /// An undeclared pawn's health reaches zero (or recovers to full).
    pub(super) fn set_dead(&self, pawn: EntityId, dead: bool) {
        let mut health = HealthComponent::from_descriptor(&postretro_entities::HealthDescriptor {
            max: 100.0,
            ..Default::default()
        });
        health.current = if dead { 0.0 } else { 100.0 };
        self.ctx
            .registry
            .borrow_mut()
            .set_component(pawn, health)
            .unwrap();
    }

    /// Declare a mod store slot `name` with an accumulator over `delta`.
    pub(super) fn accumulate(&mut self, name: &str, start: f32, delta: IrNode) {
        let (namespace, slot) = name.split_once('.').unwrap();
        self.ctx
            .slot_table
            .borrow_mut()
            .insert_namespace(
                namespace,
                vec![(
                    slot.to_string(),
                    SlotRecord::new(SlotSchema {
                        slot_type: SlotType::Number,
                        default: Some(SlotValue::Number(start)),
                        range: None,
                        persist: false,
                        readonly: false,
                        ownership: SlotOwnership::Mod,
                        network: ReplicationScope::None,
                        per_owner: false,
                        accumulate: Some(delta),
                    }),
                )],
            )
            .unwrap();
        self.accumulators.rebuild(&self.ctx);
    }

    /// One frame of `ticks` authoritative ticks, then the frame's crossing
    /// pass. Returns the reactions it fired.
    pub(super) fn frame_of(&mut self, ticks: usize) -> Vec<String> {
        for _ in 0..ticks {
            self.tick();
        }
        self.detector
            .detect(&self.ctx.slot_table.borrow())
            .into_iter()
            .map(|fire| fire.reaction)
            .collect()
    }

    pub(super) fn frame(&mut self) -> Vec<String> {
        self.frame_of(1)
    }

    fn tick(&mut self) {
        let mut sim_command = command();
        sim_command.input_tick = self.input_tick;
        let remote: Vec<RemotePawnCommand> = self
            .clients
            .iter()
            .filter(|(_, pawn)| self.ctx.registry.borrow().exists(*pawn))
            .map(|&(owner_client_id, pawn)| RemotePawnCommand {
                reload_press_tick: None,
                equip: Default::default(),
                real_command: true,
                rejected_activation: None,
                pawn,
                owner_client_id,
                weapon: None,
                shot_id: None,
                fire_tick: self.input_tick,
                client_tick: self.input_tick,
                aim_pitch: 0.0,
                command: sim_command.clone(),
                start_aim: None,
            })
            .collect();
        self.input_tick += 1;
        let bindings = TriggerBindingTable::default();
        let edges = HashMap::new();
        let events = simulate_tick_with_presentation_aim(
            self.ctx.registry.clone(),
            &self.world,
            &postretro_sim::scripting_systems::hit_zones::HitZoneStore::new(),
            None,
            // No gravity and no input: every pawn holds where a test puts it.
            postretro_collision::EnvironmentView::level_only(0.0),
            false,
            0.0,
            (0.0, 0.0),
            &mut Default::default(),
            |_, _, _, _| AiTickResult {
                events: Vec::new(),
                projectile_spawns: Vec::new(),
            },
            &[],
            &mut Default::default(),
            &remote,
            &sim_command,
            |_| PostMovementCommand {
                aim_origin: Vec3::ZERO,
                aim_direction: Vec3::NEG_Z,
            },
            DT,
            &mut Default::default(),
            &mut Default::default(),
            &[],
            0,
            &Default::default(),
            &RefCell::new(Default::default()),
            None,
            &edges,
            &edges,
            Some(TriggerTickContext {
                teleporters: None,
                seams: None,
                system: &mut self.triggers,
                bridge: &self.bridge,
                bindings: &bindings,
                slot_table: self.ctx.slot_table.clone(),
                script_ctx: Some(self.ctx.clone()),
                auto_close_timers: None,
                use_edges: &edges,
            }),
            |_, _, _| {},
            |_| {},
            postretro_sim::sim::TimingGate::OFF,
        );
        // The frame loop's order: counts and families from this tick's
        // capture, then accumulators; crossings run once per frame.
        self.players.publish_tick(
            Some(&mut self.seats),
            &self.ctx.registry.borrow(),
            CapturedPawns {
                captured: &events.trigger_stage_pawns.captured,
                counted: &events.trigger_stage_pawns.counted,
            },
            &mut self.ctx.slot_table.borrow_mut(),
        );
        self.families.publish_tick(
            true,
            &self.ctx.registry.borrow(),
            &events.trigger_stage_pawns.occupied_volumes,
            &mut self.ctx.slot_table.borrow_mut(),
        );
        evaluate_slot_accumulators(&mut self.accumulators, DT);
        self.record();
    }

    pub(super) fn read(&self, slot: &str) -> f32 {
        match self
            .ctx
            .slot_table
            .borrow()
            .get(slot)
            .and_then(|s| s.value.clone())
        {
            Some(SlotValue::Number(value)) => value,
            other => panic!("`{slot}` reads {other:?}"),
        }
    }

    pub(super) fn count(&self, name: &str) -> f32 {
        self.read(&format!("session.playerCounts.{name}"))
    }

    pub(super) fn occupancy(&self, tag: &str) -> (f32, f32) {
        (
            self.read(&format!("occupancy.{tag}.occupiedCount")),
            self.read(&format!("occupancy.{tag}.occupants")),
        )
    }

    /// Record this tick and hold S6: no family counts more occupants than
    /// players in play.
    fn record(&mut self) {
        let counts: Vec<f32> = ["connected", "loading", "inPlay", "down", "reconnecting"]
            .iter()
            .map(|name| self.count(name))
            .collect();
        let tags: Vec<String> = self
            .ctx
            .slot_table
            .borrow()
            .family_members(&postretro_entities::engine_state_collections::OCCUPANCY_FAMILY)
            .into_iter()
            .collect();
        let mut families = Vec::new();
        for tag in tags {
            let (occupied, occupants) = self.occupancy(&tag);
            assert!(
                occupants <= counts[2],
                "S6: `{tag}` counts {occupants} occupants with {} in play",
                counts[2]
            );
            families.push((tag, occupied, occupants));
        }
        self.timeline.push((counts, families));
    }
}

const PLATE_A: Vec3 = Vec3::new(0.0, 0.0, 0.0);
const PLATE_B: Vec3 = Vec3::new(20.0, 0.0, 0.0);
const PLATE_C: Vec3 = Vec3::new(40.0, 0.0, 0.0);
const ROOM: Vec3 = Vec3::new(-40.0, 0.0, 0.0);

fn two_plates(crossings: Vec<CrossingDescriptor>) -> Host {
    Host::install(
        &[("vault-plates", PLATE_A), ("vault-plates", PLATE_B)],
        &[],
        &["vault-plates"],
        crossings,
        OUTSIDE,
    )
}

fn fired(fires: &[String], name: &str) -> bool {
    fires.iter().any(|fire| fire == name)
}

// P1: the door opens only while both plates hold an effective occupant;
// releasing one before the second lands suppresses the solve; one player
// walking plate to plate never solves.
#[test]
fn two_button_door_solves_only_with_both_plates_held() {
    let solve = || vec![occupied_above("vault-plates", 1.0, "vault.solve")];

    let mut host = two_plates(solve());
    let partner = host.join(OUTSIDE);
    assert!(host.frame().is_empty());
    host.place(host.host, PLATE_A);
    assert!(!fired(&host.frame(), "vault.solve"));
    host.place(partner, PLATE_B);
    assert!(fired(&host.frame(), "vault.solve"), "both plates held");
    assert_eq!(host.occupancy("vault-plates"), (2.0, 2.0));

    let mut host = two_plates(solve());
    let partner = host.join(OUTSIDE);
    host.place(host.host, PLATE_A);
    host.frame();
    host.place(host.host, OUTSIDE);
    host.frame();
    host.place(partner, PLATE_B);
    assert!(
        !fired(&host.frame(), "vault.solve"),
        "released before the second landed"
    );

    let mut host = two_plates(solve());
    for at in [PLATE_A, PLATE_B, PLATE_A, PLATE_B] {
        host.place(host.host, at);
        assert!(
            !fired(&host.frame(), "vault.solve"),
            "one player walks plate to plate"
        );
    }
    // A dead partner on the other plate is no effective occupant.
    let partner = host.join(PLATE_A);
    host.set_dead(partner, true);
    assert!(!fired(&host.frame(), "vault.solve"));
    host.set_dead(partner, false);
    assert!(fired(&host.frame(), "vault.solve"));
}

// P2: N separated plates sharing a tag solve only when all N are held on one tick.
#[test]
fn n_plates_solve_only_when_all_held_on_one_tick() {
    let mut host = Host::install(
        &[("ring", PLATE_A), ("ring", PLATE_B), ("ring", PLATE_C)],
        &[],
        &["ring"],
        vec![occupied_above("ring", 2.0, "ring.solve")],
        OUTSIDE,
    );
    let second = host.join(OUTSIDE);
    let third = host.join(OUTSIDE);
    host.place(host.host, PLATE_A);
    host.place(second, PLATE_B);
    assert!(!fired(&host.frame(), "ring.solve"));
    host.place(second, OUTSIDE);
    host.place(third, PLATE_C);
    assert!(
        !fired(&host.frame(), "ring.solve"),
        "two of three, never all at once"
    );
    host.place(second, PLATE_B);
    assert!(fired(&host.frame(), "ring.solve"));
}

fn room_gate(host_at: Vec3) -> Host {
    Host::install(
        &[("airlock", ROOM)],
        &[],
        &["airlock"],
        vec![all_players_in("airlock", "airlock.allIn")],
        host_at,
    )
}

// P3: the all-players gate opens in the frame the last in-play player enters,
// with one, two and three players.
#[test]
fn all_players_gate_opens_in_last_entry_frame() {
    for party in 1..=3 {
        let mut host = room_gate(OUTSIDE);
        let mut pawns = vec![host.host];
        for _ in 1..party {
            pawns.push(host.join(OUTSIDE));
        }
        assert!(host.frame().is_empty());
        for (index, &pawn) in pawns.iter().enumerate() {
            host.place(pawn, ROOM);
            let opened = fired(&host.frame(), "airlock.allIn");
            assert_eq!(
                opened,
                index + 1 == party,
                "party of {party}: opens with the last entry only"
            );
        }
    }
}

// P4 (P12): a downed player outside does not hold the gate shut; with nobody in
// play it stays closed.
#[test]
fn all_players_gate_ignores_downed_and_stays_closed_with_none_in_play() {
    let mut host = room_gate(OUTSIDE);
    let downed = host.join(OUTSIDE);
    host.set_dead(downed, true);
    host.frame();
    assert_eq!(host.count("down"), 1.0);
    host.place(host.host, ROOM);
    assert!(fired(&host.frame(), "airlock.allIn"));

    let mut host = room_gate(OUTSIDE);
    host.set_dead(host.host, true);
    for _ in 0..3 {
        assert!(host.frame().is_empty(), "nobody in play");
    }
    assert_eq!(
        (host.count("inPlay"), host.occupancy("airlock").1),
        (0.0, 0.0)
    );
    let partner = host.join(ROOM);
    host.set_dead(partner, true);
    assert!(host.frame().is_empty(), "every player downed, one inside");
}

// P6 (P18): a player admitted after the gate opened turns it false; it fires
// again when they enter.
#[test]
fn late_admission_reopens_all_players_gate() {
    let mut host = room_gate(ROOM);
    assert!(fired(&host.frame(), "airlock.allIn"));
    let client = host.admit();
    assert!(host.frame().is_empty());
    assert_eq!(host.count("loading"), 1.0);
    let joiner = host.spawn(client, OUTSIDE);
    assert!(
        host.frame().is_empty(),
        "the joiner counts in play, outside"
    );
    host.place(joiner, ROOM);
    assert!(
        fired(&host.frame(), "airlock.allIn"),
        "fires again on entry"
    );
}

// P7 (pin P1): with one tick per frame a completing set crosses in its frame,
// and a lone host standing inside at install crosses in the first
// authoritative frame, with no exit and re-entry.
#[test]
fn completed_set_crosses_in_its_frame_and_at_install() {
    let mut host = room_gate(ROOM);
    assert_eq!(
        host.count("loading"),
        1.0,
        "install counts the seat as loading"
    );
    assert!(fired(&host.frame(), "airlock.allIn"));

    let mut host = Host::install(
        &[("vault-plates", PLATE_A)],
        &[],
        &["vault-plates"],
        vec![occupied_above("vault-plates", 0.0, "plate.held")],
        PLATE_A,
    );
    assert!(fired(&host.frame(), "plate.held"));
}

// O5 (P7): a player killed inside the room, with another player outside,
// leaves `inPlay` and `occupants` on the same tick, and the gate never crosses.
#[test]
fn death_inside_leaves_counts_together_and_gate_holds() {
    let mut host = room_gate(OUTSIDE);
    let inside = host.join(ROOM);
    assert!(host.frame().is_empty());
    assert_eq!(
        (host.count("inPlay"), host.occupancy("airlock").1),
        (2.0, 1.0)
    );
    host.set_dead(inside, true);
    assert!(host.frame().is_empty());
    let (counts, families) = host.timeline.last().unwrap();
    assert_eq!(counts[2], 1.0, "in play drops on this tick");
    assert_eq!(families[0].2, 0.0, "occupants drop on the same tick");
    for _ in 0..3 {
        assert!(host.frame().is_empty(), "the gate never crosses");
    }
}

// P5 (P13): after a level change and after a same-map restart, a gate the
// party spawns inside stays closed while any client still loads and opens in
// the frame the last one counts inside.
#[test]
fn party_gate_waits_for_every_client_after_level_change_and_restart() {
    let mut host = room_gate(ROOM);
    let clients = [host.admit(), host.admit()];
    for client in clients {
        host.spawn(client, ROOM);
    }
    assert!(fired(&host.frame(), "airlock.allIn"));
    for _restart in 0..2 {
        host.relevel(ROOM);
        assert_eq!(
            host.count("loading"),
            3.0,
            "install counts every seat loading"
        );
        assert!(host.frame().is_empty(), "both clients still load");
        assert_eq!(host.count("loading"), 2.0);
        host.spawn(clients[0], ROOM);
        assert!(host.frame().is_empty(), "one client still loads");
        host.spawn(clients[1], ROOM);
        assert!(
            fired(&host.frame(), "airlock.allIn"),
            "opens in the frame the last client counts inside"
        );
    }
}

// P8: King-of-the-Hill: control drains at `dt` per tick with one player on the
// hill, twice that with two, and not at all when empty. A mod-scope reference.
#[test]
fn king_of_the_hill_drains_by_occupants() {
    let mut host = Host::install(&[("hill", PLATE_A)], &["hill"], &[], Vec::new(), OUTSIDE);
    host.accumulate(
        "kingHill.control",
        100.0,
        IrNode::Mul {
            a: input("@dt"),
            b: Box::new(IrNode::Mul {
                a: input("occupancy.hill.occupants"),
                b: number(-1.0),
            }),
        },
    );
    let partner = host.join(OUTSIDE);
    let drain = |host: &mut Host| {
        let before = host.read("kingHill.control");
        host.frame();
        before - host.read("kingHill.control")
    };
    assert!(drain(&mut host).abs() < 1e-5, "empty hill holds");
    host.place(host.host, PLATE_A);
    host.frame();
    assert!((drain(&mut host) - DT).abs() < 1e-4, "one player drains dt");
    host.place(partner, PLATE_A);
    host.frame();
    assert!(
        (drain(&mut host) - 2.0 * DT).abs() < 1e-4,
        "two drain twice that"
    );
}

// P9 (P6): two identical scripted runs on the trigger-driving harness produce
// identical timelines for every count.
#[test]
fn scripted_runs_produce_identical_count_timelines() {
    let run = || {
        let mut host = Host::install(
            &[
                ("airlock", ROOM),
                ("vault-plates", PLATE_A),
                ("vault-plates", PLATE_B),
            ],
            &["airlock"],
            &["vault-plates"],
            vec![
                all_players_in("airlock", "airlock.allIn"),
                occupied_above("vault-plates", 1.0, "vault.solve"),
            ],
            OUTSIDE,
        );
        let a = host.join(OUTSIDE);
        let client = host.admit();
        let mut fires = Vec::new();
        let script: [(usize, Option<(EntityId, Vec3)>); 6] = [
            (2, Some((host.host, PLATE_A))),
            (1, Some((a, PLATE_B))),
            (3, None),
            (2, Some((host.host, ROOM))),
            (1, Some((a, ROOM))),
            (2, None),
        ];
        for (ticks, step) in script {
            if let Some((pawn, at)) = step {
                host.place(pawn, at);
            }
            fires.push(host.frame_of(ticks));
        }
        let late = host.spawn(client, ROOM);
        host.set_dead(a, true);
        fires.push(host.frame_of(2));
        host.place(late, OUTSIDE);
        fires.push(host.frame_of(2));
        (host.timeline, fires)
    };
    let first = run();
    assert_eq!(first, run());
    assert!(first.1.iter().flatten().any(|fire| fire == "vault.solve"));
}

/// Install `map` under the real dev mod the way a session does: mod init, the
/// mod's `occupancy` commit, then the production world install with its data
/// script. Returns the app and the warnings logged on the way.
fn install_dev_map(map: &'static str, out: &std::path::Path) -> (crate::App, Vec<String>) {
    use postretro_scripting_core::primitives_registry::PrimitiveRegistry;
    let capture = postretro_test_log_capture::LogCapture::start();
    let mut app = crate::startup::lifecycle::tests::test_app();
    let ctx = crate::startup::lifecycle::tests::script_ctx(&app);
    let mut primitives = PrimitiveRegistry::new();
    crate::scripting::primitives::register_all(&mut primitives, ctx.clone());
    // The fixture runs the mod's `occupancy` commit as first init does.
    crate::coop_fixture_tests::initialize_dev_mod(&mut app, &primitives);
    let world = super::teleporters_surface_tests::compile(map, out);
    crate::startup::lifecycle::tests::install_capture_world(&mut app, &world);
    let warnings = capture
        .records()
        .into_iter()
        .filter(|record| record.level <= log::Level::Warn)
        .map(|record| record.message)
        .collect();
    (app, warnings)
}

// U1, C5, C6, C4 and P23 on the dev map, in TypeScript and Luau alike: the
// mod's hill store references `hill`; the level's script references the vault
// plates and an airlock tag it reads off the map at run time; its HUD strip
// binds that family without an unknown-slot warning; nothing warns about an
// absent tag; the hill accumulator binds and drains; and both runtimes lower
// the same crossings.
#[test]
fn dev_map_declares_families_and_matches_across_runtimes() {
    let out = tempfile::tempdir().unwrap();
    let mut lowered = Vec::new();
    for map in ["coop-activation-test", "coop-activation-test-luau"] {
        let (mut app, warnings) = install_dev_map(map, out.path());
        let ctx = crate::startup::lifecycle::tests::script_ctx(&app);
        assert_eq!(
            ctx.slot_table
                .borrow()
                .family_members(&postretro_entities::engine_state_collections::OCCUPANCY_FAMILY),
            ["airlock", "hill", "vault-plates"]
                .into_iter()
                .map(str::to_string)
                .collect::<BTreeSet<_>>(),
            "{map}"
        );
        let relevant: Vec<&String> = warnings
            .iter()
            .filter(|message| {
                message.contains("occupancy")
                    || message.contains("unknown slot")
                    || message.contains("data script")
            })
            .collect();
        assert!(relevant.is_empty(), "{map}: {relevant:#?}");

        // The hill accumulator, a mod-scope reference, binds on this level.
        ctx.slot_table
            .borrow_mut()
            .get_mut("occupancy.hill.occupants")
            .unwrap()
            .write_value(Some(SlotValue::Number(2.0)));
        let before = match ctx
            .slot_table
            .borrow()
            .get("kingHill.control")
            .unwrap()
            .value
        {
            Some(SlotValue::Number(value)) => value,
            ref other => panic!("{other:?}"),
        };
        evaluate_slot_accumulators(
            &mut app
                .session
                .as_mut()
                .unwrap()
                .scripting
                .slot_accumulator_bindings,
            DT,
        );
        let after = match ctx
            .slot_table
            .borrow()
            .get("kingHill.control")
            .unwrap()
            .value
        {
            Some(SlotValue::Number(value)) => value,
            ref other => panic!("{other:?}"),
        };
        assert!(
            (before - after - 2.0 * DT).abs() < 1e-4,
            "{map}: {before} -> {after}"
        );

        let registry = ctx.data_registry.borrow();
        let mut crossings: Vec<String> = registry
            .crossings
            .iter()
            .filter(|crossing| {
                crossing
                    .fire
                    .iter()
                    .any(|fire| fire.starts_with("vault") || fire.starts_with("airlock"))
            })
            .map(|crossing| format!("{crossing:?}"))
            .collect();
        crossings.sort();
        assert_eq!(crossings.len(), 2, "{map}: {crossings:#?}");
        lowered.push(crossings);
    }
    assert_eq!(
        lowered[0], lowered[1],
        "TypeScript and Luau lower the same crossings"
    );
}

fn workspace_file(path: &str) -> String {
    std::fs::read_to_string(super::teleporters_surface_tests::workspace().join(path))
        .unwrap_or_else(|error| panic!("{path}: {error}"))
}

// U2: the modder reference documents the counts, the player list, `forEach`,
// the `occupancy` family and the all-players pattern.
#[test]
fn scripting_reference_documents_session_players_and_occupancy() {
    let docs = workspace_file("docs/scripting-reference.md");
    for needle in [
        "### Session players",
        "session.playerCounts",
        "session.players",
        "forEach(list, (row, i) => widget)",
        "### Occupancy by tag",
        "occupancy.tag(tag)",
        "occupiedCount",
        "#### Everyone is in",
        "runtime.ge(room, inPlay)",
    ] {
        assert!(
            docs.contains(needle),
            "docs/scripting-reference.md lacks `{needle}`"
        );
    }
}

// U3: the Triggers tab's effective-occupancy column is dev-tools only: the
// field and its header live in the dev-tools diagnostics module and the
// dev-tools debug panel, which their crates compile only with the feature.
#[test]
fn effective_occupancy_column_is_dev_tools_only() {
    let root = super::teleporters_surface_tests::workspace();
    let mut namers = Vec::new();
    let mut stack = vec![root.join("crates")];
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                if path.file_name().is_some_and(|name| name != "target") {
                    stack.push(path);
                }
            } else if path.extension().is_some_and(|ext| ext == "rs")
                && !path.ends_with("startup/coop_activation_tests.rs")
            {
                let source = std::fs::read_to_string(&path).unwrap();
                if source.contains(".effective_occupancy")
                    || source.contains("effective_occupancy:")
                {
                    namers.push(path.strip_prefix(&root).unwrap().to_path_buf());
                }
            }
        }
    }
    namers.sort();
    assert_eq!(
        namers,
        vec![
            std::path::PathBuf::from("crates/postretro/src/trigger_diagnostics.rs"),
            std::path::PathBuf::from("crates/renderer/src/render/debug_ui/mod.rs"),
        ]
    );
    assert!(
        workspace_file("crates/postretro/src/main.rs")
            .contains("#[cfg(feature = \"dev-tools\")]\nmod trigger_diagnostics;")
    );
    assert!(
        workspace_file("crates/renderer/src/render/mod.rs")
            .contains("#[cfg(feature = \"dev-tools\")]\nmod debug_ui;")
    );
}
