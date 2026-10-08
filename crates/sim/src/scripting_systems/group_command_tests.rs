// Group commands (`npcs`/`players` lowered to `kind` on the wire) on the named
// and scheduled-sequence paths, against the production reaction handlers and
// the real seat ledger. The trigger-tick path is covered beside the binder.
// See: context/lib/scripting.md §12.

#![cfg(test)]

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use postretro_entities::components::brain::BrainComponent;
use postretro_entities::components::health::HealthComponent;
use postretro_entities::data_descriptors::HealthDescriptor;
use postretro_entities::{
    AmmoReserve, DataRegistry, EntityId, GroupKind, GroupTarget, NamedReaction,
    PrimitiveDescriptor, ReactionDescriptor, ReplicationScope, ScriptCtx, SequenceStep,
    SequenceTarget, SlotOwnership, SlotRecord, SlotSchema, SlotType, SlotValue, Transform,
};
use postretro_foundation::Seat;
use postretro_net::wire::{ConnectClaim, PlayerClaimId};
use postretro_netcode::SeatTable;
use postretro_scripting_core::reaction_dispatch::{
    fire_named_event_with_sequences, validate_sequence_primitives,
};
use postretro_scripting_core::reaction_registry::{
    ReactionPrimitiveRegistry, SystemReactionCommand, SystemReactionRegistry,
};
use postretro_scripting_core::sequence::SequencedPrimitiveRegistry;
use postretro_test_log_capture::LogCapture;

use super::reaction_scheduler::{ReactionScheduler, register_reaction_control_primitives};
use crate::scripting::reactions::registry::{
    register_emitter_reaction_primitives, register_grant_reactions,
    register_npc_state_reaction_primitives,
};
use crate::trigger_system::PlayerId;

const MAX_HEALTH: f32 = 100.0;
const START_HEALTH: f32 = 50.0;
const XP_SLOT: &str = "currency.xp";

/// A `ScriptCtx` wired like `session/mod.rs`: the production entity-targeted
/// handlers (`applyDamage`, `grantHealth`, `grantAmmo`, `updateNpcState`), a
/// live scheduler with `wait`/`fire`, and a `note` member step whose ordered
/// log observes authored order against group steps.
struct Fixture {
    ctx: ScriptCtx,
    scheduler: ReactionScheduler,
    data: DataRegistry,
    sequence_registry: SequencedPrimitiveRegistry,
    reaction_registry: ReactionPrimitiveRegistry,
    system_registry: SystemReactionRegistry,
    log: Rc<RefCell<Vec<String>>>,
}

impl Fixture {
    fn new() -> Self {
        let ctx = ScriptCtx::new();
        let scheduler = ReactionScheduler::default();
        scheduler.set_enabled(true);
        let log: Rc<RefCell<Vec<String>>> = Rc::new(RefCell::new(Vec::new()));

        let mut sequence_registry = SequencedPrimitiveRegistry::new();
        register_reaction_control_primitives(&mut sequence_registry, scheduler.clone());
        {
            let log = log.clone();
            sequence_registry.register("note", move |_id, args| {
                let label = args["label"].as_str().unwrap_or_default();
                log.borrow_mut().push(label.to_string());
                Ok(())
            });
        }

        let mut reaction_registry = ReactionPrimitiveRegistry::new();
        register_emitter_reaction_primitives(&mut reaction_registry);
        register_npc_state_reaction_primitives(&mut reaction_registry);
        register_grant_reactions(&mut reaction_registry);
        {
            // Records each handoff's target list in order, so a test can read
            // which entities a group step reached and when it ran.
            let log = log.clone();
            reaction_registry.register("recordTargets", move |_registry, targets, args| {
                let label = args["label"].as_str().unwrap_or_default();
                let ids: Vec<String> = targets.iter().map(|id| id.to_raw().to_string()).collect();
                log.borrow_mut()
                    .push(format!("{label}:[{}]", ids.join(",")));
                Ok(())
            });
        }

        ctx.slot_table
            .borrow_mut()
            .insert(
                XP_SLOT.to_string(),
                SlotRecord::new(SlotSchema {
                    slot_type: SlotType::Number,
                    default: Some(SlotValue::Number(0.0)),
                    range: None,
                    persist: false,
                    readonly: false,
                    ownership: SlotOwnership::Mod,
                    network: ReplicationScope::None,
                    per_owner: true,
                    accumulate: None,
                }),
            )
            .expect("fixture slot is vacant");

        Self {
            ctx,
            scheduler,
            data: DataRegistry::new(),
            sequence_registry,
            reaction_registry,
            system_registry: SystemReactionRegistry::new(),
            log,
        }
    }

    /// Install through `validate_sequence_primitives`, as `setupLevel` does.
    fn install(&mut self, reactions: Vec<NamedReaction>) {
        let reactions = validate_sequence_primitives(reactions, &self.sequence_registry);
        self.ctx
            .data_registry
            .borrow_mut()
            .populate_level(reactions.clone(), Vec::new(), &[]);
        self.data.populate_level(reactions, Vec::new(), &[]);
    }

    fn fire(&self, name: &str) {
        let _ = fire_named_event_with_sequences(
            name,
            &self.data,
            &self.sequence_registry,
            &self.reaction_registry,
            &self.system_registry,
            &self.ctx,
            None,
        );
    }

    /// Advance past a 1 ms wait (one tick in a later frame) and run the
    /// frame-end landing drain.
    fn land(&self) {
        self.scheduler.begin_frame();
        self.scheduler.evaluate(&[]);
        self.drain();
    }

    fn drain(&self) {
        self.scheduler.drain_landings(
            &self.data,
            &self.sequence_registry,
            &self.reaction_registry,
            &self.system_registry,
            &self.ctx,
        );
    }

    fn set_client(&self) {
        // The role the session derives from a `Client` endpoint.
        self.ctx.owner_slot_writes_enabled.set(false);
        self.scheduler.set_enabled(false);
    }

    fn spawn_with_health(&self, tags: &[&str]) -> EntityId {
        let tags: Vec<String> = tags.iter().map(|tag| tag.to_string()).collect();
        let mut registry = self.ctx.registry.borrow_mut();
        let id = registry
            .try_spawn(Transform::default(), &tags)
            .expect("registry has room");
        let mut health = HealthComponent::from_descriptor(&HealthDescriptor {
            max: MAX_HEALTH,
            hitbox: None,
            zone_multipliers: HashMap::new(),
        });
        health.current = START_HEALTH;
        registry.set_component(id, health).unwrap();
        registry.set_component(id, AmmoReserve::new()).unwrap();
        id
    }

    fn spawn_npc(&self, tags: &[&str]) -> EntityId {
        let id = self.spawn_with_health(tags);
        self.ctx
            .registry
            .borrow_mut()
            .set_component(id, brain())
            .unwrap();
        id
    }

    fn health(&self, id: EntityId) -> f32 {
        self.ctx
            .registry
            .borrow()
            .get_component::<HealthComponent>(id)
            .unwrap()
            .current
    }

    fn ammo(&self, id: EntityId) -> u32 {
        self.ctx
            .registry
            .borrow()
            .get_component::<AmmoReserve>(id)
            .unwrap()
            .available("shells")
    }

    fn aggro(&self, id: EntityId) -> bool {
        self.ctx
            .registry
            .borrow()
            .get_component::<BrainComponent>(id)
            .unwrap()
            .aggro_armed
    }

    fn log(&self) -> Vec<String> {
        self.log.borrow().clone()
    }

    fn queued_xp_seats(&self) -> Vec<Seat> {
        self.ctx
            .system_commands
            .take()
            .into_iter()
            .flat_map(|command| match command {
                SystemReactionCommand::AddOwnerSlot { seats, .. } => seats,
                _ => Vec::new(),
            })
            .collect()
    }
}

/// The seat ledger the host runs: the local pawn binds Seat 0, a remote joins
/// through admission and binds its pawn, and a disconnect starts a hold.
struct Seats {
    table: SeatTable,
}

impl Seats {
    fn new() -> Self {
        Self {
            table: SeatTable::local_only(),
        }
    }

    fn bind_local(&mut self, fx: &Fixture, pawn: EntityId) {
        let mut registry = fx.ctx.registry.borrow_mut();
        registry.mark_local_player_pawn(pawn).unwrap();
        self.table.bind_pawn(&mut registry, Seat(0), pawn);
    }

    fn join(&mut self, fx: &Fixture, client_id: u64, player: u8, pawn: EntityId) -> Seat {
        let seat = self
            .table
            .admit_or_reclaim(client_id, Some(claim(player)), false)
            .expect("seat namespace has room")
            .seat;
        self.table
            .bind_pawn(&mut fx.ctx.registry.borrow_mut(), seat, pawn);
        seat
    }

    fn disconnect(&mut self, fx: &Fixture, client_id: u64) {
        assert!(
            self.table
                .hold_disconnected_client(&mut fx.ctx.registry.borrow_mut(), client_id)
                .is_some()
        );
    }
}

fn claim(player: u8) -> ConnectClaim {
    ConnectClaim {
        player_id: PlayerClaimId([player; 16]),
        display_name: format!("player{player}"),
    }
}

/// A brain that starts with aggro disarmed, so an `updateNpcState({ aggro:
/// true })` that lands is observable.
fn brain() -> BrainComponent {
    let mut brain = BrainComponent::from_graph(&postretro_foundation::BehaviorGraphDescriptor {
        knockback: Default::default(),
        envelope: postretro_foundation::BehaviorGraphEnvelope {
            initial: "idle".to_string(),
            activities: std::collections::BTreeMap::from([(
                "idle".to_string(),
                postretro_foundation::BehaviorActivityDescriptor {
                    sound: None,
                    animation: None,
                    motion: Some(postretro_foundation::MotionVerb::Hold),
                    action: None,
                    on_enter: None,
                    layers: Default::default(),
                },
            )]),
            transitions: Default::default(),
        },
        candidate_filter: None,
        retaliation: None,
        patrol: None,
        attacks: Default::default(),
        engagement_radius: None,
        move_speed: 1.0,
    });
    brain.aggro_armed = false;
    brain
}

fn group(kind: GroupKind, tag: Option<&str>) -> GroupTarget {
    GroupTarget {
        kind,
        tag: tag.map(str::to_string),
    }
}

fn group_step(
    kind: GroupKind,
    tag: Option<&str>,
    primitive: &str,
    args: serde_json::Value,
) -> SequenceStep {
    SequenceStep {
        id: SequenceTarget::Group(group(kind, tag)),
        primitive: primitive.to_string(),
        args,
    }
}

fn group_primitive(
    name: &str,
    kind: GroupKind,
    tag: Option<&str>,
    primitive: &str,
    args: serde_json::Value,
) -> NamedReaction {
    NamedReaction {
        name: name.to_string(),
        descriptor: ReactionDescriptor::Primitive(PrimitiveDescriptor {
            primitive: primitive.to_string(),
            target: None,
            kind: Some(kind),
            tag: tag.map(str::to_string),
            on_complete: None,
            args,
        }),
    }
}

fn raw_tag_primitive(
    name: &str,
    tag: &str,
    primitive: &str,
    args: serde_json::Value,
) -> NamedReaction {
    NamedReaction {
        name: name.to_string(),
        descriptor: ReactionDescriptor::Primitive(PrimitiveDescriptor {
            primitive: primitive.to_string(),
            target: None,
            kind: None,
            tag: Some(tag.to_string()),
            on_complete: None,
            args,
        }),
    }
}

fn wait_step(interruptible: bool) -> SequenceStep {
    SequenceStep {
        id: SequenceTarget::Wait,
        primitive: "wait".to_string(),
        args: serde_json::json!({ "durationMs": 1.0, "interruptible": interruptible }),
    }
}

fn note_step(id: EntityId, label: &str) -> SequenceStep {
    SequenceStep {
        id: SequenceTarget::Entity(id),
        primitive: "note".to_string(),
        args: serde_json::json!({ "label": label }),
    }
}

fn sequence(name: &str, steps: Vec<SequenceStep>) -> NamedReaction {
    NamedReaction {
        name: name.to_string(),
        descriptor: ReactionDescriptor::Sequence(steps),
    }
}

fn damage(amount: f32) -> serde_json::Value {
    serde_json::json!({ "amount": amount })
}

// ---------------------------------------------------------------------------
// NPC half — the first-slice falsification. An NPC group step after a
// wait resolves when it lands: an NPC spawned during the wait is reached, one
// despawned during it is not, and a player pawn carrying the tag never is (after-wait
// path).
// ---------------------------------------------------------------------------
#[test]
fn npc_group_step_after_wait_reaches_npcs_present_at_landing() {
    let mut fx = Fixture::new();
    let mut seats = Seats::new();
    let resident = fx.spawn_npc(&["closet"]);
    let doomed = fx.spawn_npc(&["closet"]);
    let pawn = fx.spawn_with_health(&["closet"]);
    seats.bind_local(&fx, pawn);
    fx.install(vec![sequence(
        "closet.timedReveal",
        vec![
            wait_step(true),
            group_step(GroupKind::Npc, Some("closet"), "applyDamage", damage(5.0)),
        ],
    )]);

    fx.fire("closet.timedReveal");
    assert_eq!(
        fx.scheduler.pending_len(),
        1,
        "the group tail parks at the wait"
    );
    assert_eq!(
        fx.health(resident),
        START_HEALTH,
        "nothing past the wait runs at fire"
    );

    // During the wait a spawner releases an NPC carrying the tag, and one
    // tagged NPC despawns. The released NPC takes the despawned one's slot.
    fx.ctx.registry.borrow_mut().despawn(doomed).unwrap();
    let released = fx.spawn_npc(&["closet"]);

    fx.land();
    assert_eq!(fx.scheduler.pending_len(), 0);
    assert_eq!(fx.health(resident), START_HEALTH - 5.0);
    assert_eq!(
        fx.health(released),
        START_HEALTH - 5.0,
        "the NPC spawned during the wait is reached at landing"
    );
    assert_eq!(
        fx.health(pawn),
        START_HEALTH,
        "a player pawn is never an npc"
    );
    assert!(!fx.ctx.registry.borrow().exists(doomed));
}

// Player half: a players() step after a wait reaches a player who
// joined during the wait and skips one whose seat went into a disconnect hold.
#[test]
fn player_group_step_after_wait_reaches_joiners_and_skips_held_seats() {
    let mut fx = Fixture::new();
    let mut seats = Seats::new();
    let local = fx.spawn_with_health(&[]);
    seats.bind_local(&fx, local);
    let leaver = fx.spawn_with_health(&[]);
    seats.join(&fx, 41, 1, leaver);
    let npc = fx.spawn_npc(&[]);
    fx.install(vec![sequence(
        "resupply",
        vec![
            wait_step(false),
            group_step(GroupKind::Player, None, "grantHealth", damage(10.0)),
        ],
    )]);

    fx.fire("resupply");
    let joiner = fx.spawn_with_health(&[]);
    seats.join(&fx, 42, 2, joiner);
    // The leaver's pawn outlives the hold edge here, which is the stricter
    // case: only its seat binding is gone.
    seats.disconnect(&fx, 41);

    fx.land();
    assert_eq!(fx.health(local), START_HEALTH + 10.0);
    assert_eq!(
        fx.health(joiner),
        START_HEALTH + 10.0,
        "the joiner is reached"
    );
    assert_eq!(
        fx.health(leaver),
        START_HEALTH,
        "a held seat's pawn is skipped"
    );
    assert_eq!(fx.health(npc), START_HEALTH, "an npc is never a player");
}

// A group step with zero matches at landing is a debug no-op — no
// handler warning, nothing above debug from the group dispatch.
#[test]
fn group_step_with_no_matches_at_landing_is_a_debug_noop() {
    let mut fx = Fixture::new();
    let npc = fx.spawn_npc(&["elsewhere"]);
    fx.install(vec![sequence(
        "emptyRoom",
        vec![
            wait_step(false),
            group_step(
                GroupKind::Npc,
                Some("closet"),
                "updateNpcState",
                serde_json::json!({ "aggro": true }),
            ),
        ],
    )]);
    fx.fire("emptyRoom");

    let capture = LogCapture::start();
    fx.land();
    capture.assert_logged_once(log::Level::Debug, "matched nothing; no-op");
    assert!(
        capture
            .records()
            .iter()
            .all(|record| record.level >= log::Level::Debug),
        "nothing above debug: {:?}",
        capture.records()
    );
    assert!(!fx.aggro(npc));
}

// ---------------------------------------------------------------------------
// Name-fired path: an npc group with tag "x" skips a player pawn
// tagged "x" — as a primitive body and as a sequence step — and a tagless npc
// group reaches every NPC.
// ---------------------------------------------------------------------------
#[test]
fn npc_group_damage_skips_a_tagged_pawn_and_tagless_reaches_every_npc() {
    let mut fx = Fixture::new();
    let mut seats = Seats::new();
    let tagged = fx.spawn_npc(&["x"]);
    let untagged = fx.spawn_npc(&[]);
    let pawn = fx.spawn_with_health(&["x"]);
    seats.bind_local(&fx, pawn);
    fx.install(vec![
        group_primitive(
            "hitX",
            GroupKind::Npc,
            Some("x"),
            "applyDamage",
            damage(5.0),
        ),
        sequence(
            "hitXStep",
            vec![group_step(
                GroupKind::Npc,
                Some("x"),
                "applyDamage",
                damage(5.0),
            )],
        ),
        group_primitive("hitAll", GroupKind::Npc, None, "applyDamage", damage(1.0)),
    ]);

    fx.fire("hitX");
    fx.fire("hitXStep");
    assert_eq!(fx.health(tagged), START_HEALTH - 10.0);
    assert_eq!(fx.health(untagged), START_HEALTH);
    assert_eq!(
        fx.health(pawn),
        START_HEALTH,
        "the pawn tagged x is not an npc"
    );

    fx.fire("hitAll");
    assert_eq!(fx.health(tagged), START_HEALTH - 11.0);
    assert_eq!(
        fx.health(untagged),
        START_HEALTH - 1.0,
        "tagless reaches every NPC"
    );
    assert_eq!(fx.health(pawn), START_HEALTH);
}

// A raw kindless tag descriptor keeps today's Transform scan and reaches
// every tagged entity, a player pawn included.
#[test]
fn raw_kindless_tag_descriptor_reaches_every_tagged_entity_as_before() {
    let mut fx = Fixture::new();
    let mut seats = Seats::new();
    let npc = fx.spawn_npc(&["x"]);
    let pawn = fx.spawn_with_health(&["x"]);
    seats.bind_local(&fx, pawn);
    let other = fx.spawn_npc(&["y"]);
    fx.install(vec![raw_tag_primitive(
        "raw",
        "x",
        "applyDamage",
        damage(5.0),
    )]);

    fx.fire("raw");
    assert_eq!(fx.health(npc), START_HEALTH - 5.0);
    assert_eq!(fx.health(pawn), START_HEALTH - 5.0);
    assert_eq!(fx.health(other), START_HEALTH);
}

// The retired `updateEnemyState` name is an unknown primitive — no alias —
// on every path, while `updateNpcState` applies.
#[test]
fn retired_update_enemy_state_is_rejected_as_unknown_and_update_npc_state_applies() {
    let mut fx = Fixture::new();
    let npc = fx.spawn_npc(&["x"]);
    let aggro = serde_json::json!({ "aggro": true });
    let capture = LogCapture::start();
    fx.install(vec![
        raw_tag_primitive("oldRaw", "x", "updateEnemyState", aggro.clone()),
        group_primitive(
            "oldGroup",
            GroupKind::Npc,
            None,
            "updateEnemyState",
            aggro.clone(),
        ),
        sequence(
            "oldMember",
            vec![SequenceStep {
                id: SequenceTarget::Entity(npc),
                primitive: "updateEnemyState".to_string(),
                args: aggro.clone(),
            }],
        ),
    ]);
    capture.assert_logged_once(
        log::Level::Error,
        "sequence step 0 names unknown primitive \"updateEnemyState\"",
    );
    fx.fire("oldRaw");
    fx.fire("oldGroup");
    fx.fire("oldMember");
    capture.assert_logged(
        log::Level::Warn,
        "primitive 'updateEnemyState' is not registered",
    );
    assert!(!fx.aggro(npc), "the old name changes nothing");

    fx.install(vec![group_primitive(
        "new",
        GroupKind::Npc,
        Some("x"),
        "updateNpcState",
        aggro,
    )]);
    fx.fire("new");
    assert!(fx.aggro(npc));
}

// ---------------------------------------------------------------------------
// Players() half on the named path: grantHealth credits every seat-bound
// pawn exactly once and nothing else.
// ---------------------------------------------------------------------------
#[test]
fn player_group_grant_health_credits_every_seat_bound_pawn_once() {
    let mut fx = Fixture::new();
    let mut seats = Seats::new();
    let local = fx.spawn_with_health(&["players"]);
    seats.bind_local(&fx, local);
    let remote = fx.spawn_with_health(&[]);
    seats.join(&fx, 41, 1, remote);
    let npc = fx.spawn_npc(&["players"]);
    let unbound = fx.spawn_with_health(&["players"]);
    fx.install(vec![group_primitive(
        "resupply",
        GroupKind::Player,
        None,
        "grantHealth",
        damage(10.0),
    )]);

    fx.fire("resupply");
    assert_eq!(fx.health(local), START_HEALTH + 10.0);
    assert_eq!(fx.health(remote), START_HEALTH + 10.0);
    assert_eq!(fx.health(npc), START_HEALTH);
    assert_eq!(
        fx.health(unbound),
        START_HEALTH,
        "a tag alone never makes a player"
    );
}

// ---------------------------------------------------------------------------
// Matches reach the handler in slot order, identically on every run with
// identical inputs, including after a despawn frees a slot a later spawn reuses.
// ---------------------------------------------------------------------------
#[test]
fn group_matches_reach_the_handler_in_the_same_order_across_identical_runs() {
    fn run() -> (Vec<String>, Vec<EntityId>) {
        let mut fx = Fixture::new();
        let a = fx.spawn_npc(&["x"]);
        let b = fx.spawn_npc(&["x"]);
        let c = fx.spawn_npc(&["x"]);
        fx.ctx.registry.borrow_mut().despawn(a).unwrap();
        let d = fx.spawn_npc(&["x"]);
        fx.install(vec![group_primitive(
            "record",
            GroupKind::Npc,
            Some("x"),
            "recordTargets",
            serde_json::json!({ "label": "x" }),
        )]);
        fx.fire("record");
        (fx.log(), vec![d, b, c])
    }
    let (first, slot_order) = run();
    let (second, _) = run();
    let expected: Vec<String> = slot_order
        .iter()
        .map(|id| id.to_raw().to_string())
        .collect();
    assert_eq!(first, vec![format!("x:[{}]", expected.join(","))]);
    assert_eq!(first, second);
}

// ---------------------------------------------------------------------------
// During a disconnect hold players() skips that pawn for every verb, and
// reaches the seat's pawn again after reclaim. The local pawn is reached
// throughout (single player is the local seat alone).
// ---------------------------------------------------------------------------
#[test]
fn disconnect_hold_skips_the_pawn_for_every_player_verb_and_reclaim_restores_it() {
    let mut fx = Fixture::new();
    let mut seats = Seats::new();
    let local = fx.spawn_with_health(&[]);
    seats.bind_local(&fx, local);
    let held = fx.spawn_with_health(&[]);
    let remote_seat = seats.join(&fx, 41, 1, held);
    fx.install(vec![
        group_primitive("hurt", GroupKind::Player, None, "applyDamage", damage(5.0)),
        group_primitive("heal", GroupKind::Player, None, "grantHealth", damage(20.0)),
        group_primitive(
            "ammo",
            GroupKind::Player,
            None,
            "grantAmmo",
            serde_json::json!({ "type": "shells", "amount": 4 }),
        ),
        group_primitive(
            "xp",
            GroupKind::Player,
            None,
            "addSlot",
            serde_json::json!({ "slot": XP_SLOT, "delta": 1 }),
        ),
    ]);
    let fire_all = |fx: &Fixture| {
        for name in ["hurt", "heal", "ammo", "xp"] {
            fx.fire(name);
        }
    };

    seats.disconnect(&fx, 41);
    fire_all(&fx);
    assert_eq!(
        fx.health(held),
        START_HEALTH,
        "damage and heal skip the held pawn"
    );
    assert_eq!(fx.ammo(held), 0);
    assert_eq!(
        fx.health(local),
        START_HEALTH + 15.0,
        "the local pawn is reached"
    );
    assert_eq!(fx.ammo(local), 4);
    assert_eq!(
        fx.queued_xp_seats(),
        vec![Seat(0)],
        "addSlot skips the held seat"
    );

    // Reclaim: the same player identity takes its held seat back with a fresh
    // pawn.
    let reclaimed_pawn = fx.spawn_with_health(&[]);
    let reclaimed_seat = seats.join(&fx, 43, 1, reclaimed_pawn);
    assert_eq!(reclaimed_seat, remote_seat);
    fire_all(&fx);
    assert_eq!(fx.health(reclaimed_pawn), START_HEALTH + 15.0);
    assert_eq!(fx.ammo(reclaimed_pawn), 4);
    assert_eq!(fx.queued_xp_seats(), vec![Seat(0), remote_seat]);
}

// ---------------------------------------------------------------------------
// On a connected client, a reaction with no wait applies nothing from its
// npc update or players() grant — not even to the client's own pawn — and logs
// nothing above debug. The same reactions on the host apply to every match. A
// group step after a wait never runs on the client.
// ---------------------------------------------------------------------------
#[test]
fn connected_client_group_commands_apply_nothing_and_stay_below_debug() {
    let aggro = serde_json::json!({ "aggro": true });
    let reactions = |fx: &mut Fixture| {
        fx.install(vec![
            group_primitive(
                "aggro",
                GroupKind::Npc,
                None,
                "updateNpcState",
                aggro.clone(),
            ),
            group_primitive("heal", GroupKind::Player, None, "grantHealth", damage(10.0)),
            sequence(
                "lateAggro",
                vec![
                    wait_step(false),
                    group_step(GroupKind::Npc, None, "updateNpcState", aggro.clone()),
                ],
            ),
        ]);
    };

    // Client: the client holds its own pawn, marked local, with no seat ledger.
    let mut client = Fixture::new();
    let npc = client.spawn_npc(&[]);
    let own_pawn = client.spawn_with_health(&[]);
    client
        .ctx
        .registry
        .borrow_mut()
        .mark_local_player_pawn(own_pawn)
        .unwrap();
    reactions(&mut client);
    client.set_client();
    let capture = LogCapture::start();
    client.fire("aggro");
    client.fire("heal");
    assert!(
        capture
            .records()
            .iter()
            .all(|record| record.level >= log::Level::Debug),
        "nothing above debug on the client: {:?}",
        capture.records()
    );
    drop(capture);
    assert!(!client.aggro(npc));
    assert_eq!(
        client.health(own_pawn),
        START_HEALTH,
        "not even the client's own pawn"
    );

    client.fire("lateAggro");
    client.land();
    client.land();
    assert!(
        !client.aggro(npc),
        "a step after a wait never runs on the client"
    );

    // Host: the same reactions apply to every match.
    let mut host = Fixture::new();
    let mut seats = Seats::new();
    let npc_a = host.spawn_npc(&[]);
    let npc_b = host.spawn_npc(&[]);
    let local = host.spawn_with_health(&[]);
    seats.bind_local(&host, local);
    let remote = host.spawn_with_health(&[]);
    seats.join(&host, 41, 1, remote);
    reactions(&mut host);
    host.fire("aggro");
    host.fire("heal");
    assert!(host.aggro(npc_a) && host.aggro(npc_b));
    assert_eq!(host.health(local), START_HEALTH + 10.0);
    assert_eq!(host.health(remote), START_HEALTH + 10.0);
}

// On a connected client, a member step in the same reaction as a
// skipped group step still applies.
#[test]
fn connected_client_runs_member_steps_beside_a_skipped_group_step() {
    let mut fx = Fixture::new();
    let door = fx.ctx.registry.borrow_mut().spawn(Transform::default());
    let npc = fx.spawn_npc(&[]);
    fx.install(vec![sequence(
        "levelLoad",
        vec![
            note_step(door, "door.start"),
            group_step(
                GroupKind::Npc,
                None,
                "updateNpcState",
                serde_json::json!({ "aggro": true }),
            ),
            note_step(door, "door.after"),
        ],
    )]);
    fx.set_client();

    fx.fire("levelLoad");
    assert_eq!(
        fx.log(),
        vec!["door.start".to_string(), "door.after".to_string()]
    );
    assert!(!fx.aggro(npc));
}

// ---------------------------------------------------------------------------
// A mover member step followed by a group step run in authored order on
// one drain — at fire and after a wait. An Exit that cancels the interruptible
// wait runs neither tail step. (The Exit is fed to the scheduler directly; the
// trigger-volume plumbing that produces it is covered by the trigger suites.)
// ---------------------------------------------------------------------------
#[test]
fn member_and_group_steps_run_in_authored_order_and_an_exit_cancel_runs_neither() {
    let mut fx = Fixture::new();
    let door = fx.ctx.registry.borrow_mut().spawn(Transform::default());
    let npc = fx.spawn_npc(&["closet"]);
    let ordered = |label: &str| {
        vec![
            note_step(door, &format!("{label}.door")),
            group_step(
                GroupKind::Npc,
                Some("closet"),
                "recordTargets",
                serde_json::json!({ "label": format!("{label}.npcs") }),
            ),
            note_step(door, &format!("{label}.after")),
        ]
    };
    let mut body = ordered("now");
    body.push(wait_step(true));
    body.extend(ordered("landed"));
    fx.install(vec![sequence("reveal", body.clone())]);

    fx.fire("reveal");
    fx.land();
    let npc_raw = npc.to_raw();
    assert_eq!(
        fx.log(),
        vec![
            "now.door".to_string(),
            format!("now.npcs:[{npc_raw}]"),
            "now.after".to_string(),
            "landed.door".to_string(),
            format!("landed.npcs:[{npc_raw}]"),
            "landed.after".to_string(),
        ]
    );

    // Exit cancel: the same tail parked under a trigger origin and cancelled by
    // that origin's Exit before landing runs neither step.
    fx.log.borrow_mut().clear();
    let trigger = fx.ctx.registry.borrow_mut().spawn(Transform::default());
    let player = PlayerId::Remote(7);
    fx.scheduler.enroll(
        "reveal",
        0,
        Some((trigger, player)),
        ordered("cancelled"),
        5,
        true,
    );
    fx.scheduler.begin_frame();
    fx.scheduler.evaluate(&[(trigger, player)]);
    fx.drain();
    for _ in 0..6 {
        fx.land();
    }
    assert!(
        fx.log().is_empty(),
        "a cancelled tail runs neither step: {:?}",
        fx.log()
    );
}
