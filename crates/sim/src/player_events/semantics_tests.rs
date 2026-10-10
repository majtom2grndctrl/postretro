// Player-event evaluation semantics: edges, first sight, unobserved states,
// edge memory across holds, reclaims, releases and recomposes, and the
// per-tick snapshot.
// See: context/lib/scripting.md §12 (Player events)

use postretro_entities::components::inventory::Inventory;
use postretro_entities::components::weapon::WeaponComponent;
use postretro_entities::data_descriptors::WeaponDescriptor;
use postretro_entities::{
    EntityId, GroupKind, ReplicationScope, SlotOwnership, SlotRecord, SlotSchema, SlotType,
    SlotValue, Transform,
};
use postretro_foundation::{IrNode, IrValue, Seat};
use postretro_scripting_core::data_descriptors::{
    NamedReaction, PlayerEventEdge, PrimitiveDescriptor, ReactionDescriptor,
};
use postretro_test_log_capture::LogCapture;
use serde_json::json;

use super::tests::{World, input, lt, number, on_player, play_sound, player_event};
use super::{PlayerEventTable, PlayerKey};
use crate::mover_commands::MoverCommandDiagnostics;
use crate::spawner::SpawnContext;

const XP: &str = "progression.xp";
const LEVEL: &str = "leveling.level";
const ALARM: &str = "closet.alarm";

fn and(a: IrNode, b: IrNode) -> IrNode {
    IrNode::And {
        a: Box::new(a),
        b: Box::new(b),
    }
}

fn ge(a: IrNode, b: IrNode) -> IrNode {
    IrNode::Ge {
        a: Box::new(a),
        b: Box::new(b),
    }
}

fn low_health() -> IrNode {
    lt(input("player.health"), number(50.0))
}

fn declare_number(world: &World, name: &str, per_owner: bool, default: f32) {
    world
        .script_ctx
        .slot_table
        .borrow_mut()
        .insert(
            name.to_string(),
            SlotRecord::new(SlotSchema {
                slot_type: SlotType::Number,
                default: Some(SlotValue::Number(default)),
                range: None,
                persist: false,
                readonly: false,
                ownership: SlotOwnership::Mod,
                network: ReplicationScope::None,
                per_owner,
                accumulate: None,
            }),
        )
        .unwrap();
}

fn set_owner(world: &World, name: &str, seat: Seat, value: f32) {
    world
        .script_ctx
        .slot_table
        .borrow_mut()
        .get_mut(name)
        .unwrap()
        .set_per_seat_value(seat, SlotValue::Number(value));
}

fn owner(world: &World, name: &str, seat: Seat) -> f32 {
    match world
        .script_ctx
        .slot_table
        .borrow()
        .get(name)
        .unwrap()
        .per_seat_value(seat)
    {
        Some(SlotValue::Number(value)) => *value,
        other => panic!("{name} for {seat:?} is {other:?}"),
    }
}

fn set_global(world: &World, name: &str, value: f32) {
    world
        .script_ctx
        .slot_table
        .borrow_mut()
        .get_mut(name)
        .unwrap()
        .write_value(Some(SlotValue::Number(value)));
}

fn residual_players(world: &mut World) -> Vec<PlayerKey> {
    let players = world.residuals.iter().map(|residual| residual.player).collect();
    world.residuals.clear();
    players
}

/// `xp ≥ 100 && level < 2`, firing `on.player.addSlot(level, 1)` and a chime.
fn install_guarded_milestone(world: &mut World, guarded: bool) {
    declare_number(world, XP, true, 0.0);
    declare_number(world, LEVEL, true, 1.0);
    let reached = ge(input(XP), number(100.0));
    let condition = if guarded {
        and(reached, lt(input(LEVEL), number(2.0)))
    } else {
        reached
    };
    world.install(
        vec![
            on_player("levelUp", "addSlot", json!({ "slot": LEVEL, "delta": 1.0 })),
            play_sound("fanfare", "level_up"),
        ],
        vec![player_event(
            PlayerEventEdge::Becomes,
            condition,
            &["levelUp", "fanfare"],
        )],
    );
}

fn rebind(world: &World, pawn: EntityId, seat: Seat) {
    world.script_ctx.registry.borrow_mut().bind_pawn_seat(pawn, seat);
}

fn hold(world: &World, pawn: EntityId) {
    world.script_ctx.registry.borrow_mut().clear_pawn_seat(pawn);
}

#[test]
fn guarded_milestone_fires_once_per_player_and_credits_only_that_seat() {
    let mut world = World::new();
    world.spawn_player(Some(Seat(1)), 100.0);
    world.spawn_player(Some(Seat(2)), 100.0);
    install_guarded_milestone(&mut world, true);

    set_owner(&world, XP, Seat(1), 100.0);
    world.tick();
    assert_eq!(residual_players(&mut world), vec![PlayerKey::Seat(Seat(1))]);
    assert_eq!(owner(&world, LEVEL, Seat(1)), 2.0, "the crossing player levels up");
    assert_eq!(owner(&world, LEVEL, Seat(2)), 1.0, "the other player is untouched");

    world.tick();
    assert!(residual_players(&mut world).is_empty(), "the guard extinguishes the condition");

    set_owner(&world, XP, Seat(2), 120.0);
    world.tick();
    assert_eq!(residual_players(&mut world), vec![PlayerKey::Seat(Seat(2))]);
    assert_eq!(owner(&world, LEVEL, Seat(2)), 2.0);
    assert_eq!(owner(&world, LEVEL, Seat(1)), 2.0);
}

#[test]
fn same_tick_fires_follow_group_resolution_order_and_repeat_identically() {
    let run = || {
        let mut world = World::new();
        // Registry slot order, not seat order, is the group's order.
        world.spawn_player(Some(Seat(7)), 100.0);
        world.spawn_player(Some(Seat(3)), 100.0);
        world.spawn_player(None, 100.0);
        install_guarded_milestone(&mut world, true);
        set_owner(&world, XP, Seat(7), 100.0);
        set_owner(&world, XP, Seat(3), 100.0);
        world.tick();
        residual_players(&mut world)
    };
    let first = run();
    assert_eq!(
        first,
        vec![PlayerKey::Seat(Seat(7)), PlayerKey::Seat(Seat(3))],
        "fires in the players() group order"
    );
    assert_eq!(run(), first, "identical registry histories give identical orders");
}

#[test]
fn same_tick_conditions_evaluate_before_any_fire_applies() {
    let mut world = World::new();
    let first = world.spawn_player(Some(Seat(1)), 40.0);
    let second = world.spawn_player(Some(Seat(2)), 45.0);
    let heal_everyone = NamedReaction {
        name: "medic".to_string(),
        descriptor: ReactionDescriptor::Primitive(PrimitiveDescriptor {
            primitive: "grantHealth".to_string(),
            target: None,
            kind: Some(GroupKind::Player),
            tag: None,
            on_complete: None,
            args: json!({ "amount": 30.0 }),
        }),
    };
    world.install(
        vec![heal_everyone, play_sound("chime", "medic")],
        vec![player_event(PlayerEventEdge::Becomes, low_health(), &["medic", "chime"])],
    );

    world.tick();
    assert_eq!(
        residual_players(&mut world),
        vec![PlayerKey::Seat(Seat(1)), PlayerKey::Seat(Seat(2))],
        "the first fire's heal does not suppress the second player's same-tick edge"
    );
    assert_eq!(world.health(first), 100.0, "both fires applied: 40 + 30 + 30");
    assert_eq!(world.health(second), 100.0, "45 + 30 + 30, capped");

    world.tick();
    assert!(residual_players(&mut world).is_empty(), "the heal is seen next tick");
}

#[test]
fn becomes_and_ceases_fire_on_their_edge_only() {
    let mut world = World::new();
    let pawn = world.spawn_player(Some(Seat(1)), 100.0);
    world.install(
        vec![play_sound("bleed", "bleed"), play_sound("mend", "mend")],
        vec![
            player_event(PlayerEventEdge::Becomes, low_health(), &["bleed"]),
            player_event(PlayerEventEdge::Ceases, low_health(), &["mend"]),
        ],
    );
    world.tick();
    assert!(world.take_residual_reactions().is_empty(), "false at first sight: neither edge");
    world.set_health(pawn, 30.0);
    world.tick();
    assert_eq!(world.take_residual_reactions(), vec!["bleed".to_string()]);
    world.tick();
    assert!(world.take_residual_reactions().is_empty(), "a held condition fires nothing");
    world.set_health(pawn, 80.0);
    world.tick();
    assert_eq!(world.take_residual_reactions(), vec!["mend".to_string()]);
}

#[test]
fn condition_sees_a_write_before_the_seam_this_tick_and_a_later_write_next_tick() {
    let mut world = World::new();
    world.spawn_player(Some(Seat(1)), 100.0);
    declare_number(&world, ALARM, false, 0.0);
    world.install(
        vec![play_sound("klaxon", "alarm")],
        vec![player_event(
            PlayerEventEdge::Becomes,
            ge(input(ALARM), number(1.0)),
            &["klaxon"],
        )],
    );
    world.tick();

    // An in-tick trigger `setState` lands inside the tick, before the seam.
    set_global(&world, ALARM, 1.0);
    world.tick();
    assert_eq!(world.take_residual_reactions(), vec!["klaxon".to_string()]);

    set_global(&world, ALARM, 0.0);
    world.tick();
    // A frame-end drain write lands after this frame's ticks evaluated.
    world.tick();
    set_global(&world, ALARM, 1.0);
    assert!(world.take_residual_reactions().is_empty(), "not seen by an earlier tick");
    world.tick();
    assert_eq!(world.take_residual_reactions(), vec!["klaxon".to_string()]);
}

#[test]
fn two_ticks_in_one_frame_leave_becomes_then_ceases_in_tick_order() {
    let mut world = World::new();
    let pawn = world.spawn_player(Some(Seat(1)), 100.0);
    world.install(
        vec![play_sound("bleed", "bleed"), play_sound("mend", "mend")],
        vec![
            player_event(PlayerEventEdge::Ceases, low_health(), &["mend"]),
            player_event(PlayerEventEdge::Becomes, low_health(), &["bleed"]),
        ],
    );
    world.tick();
    world.set_health(pawn, 20.0);
    world.tick();
    world.set_health(pawn, 90.0);
    world.tick();
    assert_eq!(
        world.take_residual_reactions(),
        vec!["bleed".to_string(), "mend".to_string()],
        "both residuals wait for the frame drain in tick order"
    );

    // Over and back between two evaluations is never observed.
    world.set_health(pawn, 20.0);
    world.set_health(pawn, 90.0);
    world.tick();
    assert!(world.take_residual_reactions().is_empty());
}

#[test]
fn no_player_pawn_fires_nothing_and_logs_nothing_above_debug() {
    let mut world = World::new();
    let capture = LogCapture::start();
    world.install(
        vec![play_sound("bleed", "bleed")],
        vec![player_event(PlayerEventEdge::Becomes, low_health(), &["bleed"])],
    );
    world.tick();
    world.tick();
    assert!(world.take_residual_reactions().is_empty());
    let noisy: Vec<_> = capture
        .records()
        .into_iter()
        .filter(|record| record.level <= log::Level::Info)
        .collect();
    assert!(noisy.is_empty(), "nothing above debug: {noisy:?}");
}

#[test]
fn first_sight_at_install_fires_becomes_only() {
    let mut world = World::new();
    world.spawn_player(Some(Seat(1)), 30.0);
    world.spawn_player(Some(Seat(2)), 90.0);
    world.install(
        vec![play_sound("bleed", "bleed"), play_sound("mend", "mend")],
        vec![
            player_event(PlayerEventEdge::Becomes, low_health(), &["bleed"]),
            player_event(PlayerEventEdge::Ceases, low_health(), &["mend"]),
        ],
    );
    world.tick();
    assert_eq!(
        world.take_residual_reactions(),
        vec!["bleed".to_string()],
        "a holding player fires becomes at first sight; a false one fires no ceases"
    );
}

#[test]
fn a_joining_player_is_first_seen_at_their_first_bound_tick() {
    let mut world = World::new();
    world.spawn_player(Some(Seat(1)), 90.0);
    world.install(
        vec![play_sound("bleed", "bleed")],
        vec![player_event(PlayerEventEdge::Becomes, low_health(), &["bleed"])],
    );
    world.tick();
    // Bound to a seat but never sent input: the group, not the tick's
    // resolved-command list, decides who is observed.
    world.spawn_player(Some(Seat(2)), 30.0);
    world.tick();
    assert_eq!(residual_players(&mut world), vec![PlayerKey::Seat(Seat(2))]);
    world.spawn_player(Some(Seat(3)), 90.0);
    world.tick();
    assert!(residual_players(&mut world).is_empty(), "a player joining false fires nothing");
}

#[test]
fn a_disconnect_hold_fires_nothing_and_a_reclaim_while_holding_fires_again() {
    let mut world = World::new();
    let pawn = world.spawn_player(Some(Seat(1)), 30.0);
    world.install(
        vec![play_sound("bleed", "bleed"), play_sound("mend", "mend")],
        vec![
            player_event(PlayerEventEdge::Becomes, low_health(), &["bleed"]),
            player_event(PlayerEventEdge::Ceases, low_health(), &["mend"]),
        ],
    );
    world.tick();
    assert_eq!(world.take_residual_reactions(), vec!["bleed".to_string()]);

    hold(&world, pawn);
    world.tick();
    assert!(world.take_residual_reactions().is_empty(), "becoming unobserved fires no ceases");

    // A reclaim rebinds the same seat to a replacement pawn.
    let replacement = world.spawn_unbound(30.0);
    rebind(&world, replacement, Seat(1));
    world.tick();
    assert_eq!(world.take_residual_reactions(), vec!["bleed".to_string()]);

    hold(&world, replacement);
    world.tick();
    let healthy = world.spawn_unbound(90.0);
    rebind(&world, healthy, Seat(1));
    world.tick();
    assert!(world.take_residual_reactions().is_empty(), "reclaiming while false fires nothing");
}

#[test]
fn seat_release_drops_edge_memory_and_a_new_seat_fires_once() {
    let mut world = World::new();
    let leaving = world.spawn_player(Some(Seat(1)), 30.0);
    world.spawn_player(Some(Seat(2)), 90.0);
    world.install(
        vec![play_sound("bleed", "bleed")],
        vec![player_event(PlayerEventEdge::Becomes, low_health(), &["bleed"])],
    );
    world.tick();
    world.take_residual_reactions();
    assert_eq!(world.table.edge_memory_len(), 2, "one entry per live player per event");

    // Release seat 1 and admit seat 3 before the next tick.
    world.script_ctx.registry.borrow_mut().despawn(leaving).unwrap();
    world.spawn_player(Some(Seat(3)), 30.0);
    world.tick();
    assert_eq!(residual_players(&mut world), vec![PlayerKey::Seat(Seat(3))]);
    assert_eq!(world.table.edge_memory_len(), 2, "memory returns to the live player count");
}

#[test]
fn a_guarded_milestone_survives_level_transitions_and_reclaims_an_unguarded_one_refires() {
    for (guarded, expected_fires) in [(true, 1), (false, 3)] {
        let mut world = World::new();
        let pawn = world.spawn_player(Some(Seat(1)), 100.0);
        install_guarded_milestone(&mut world, guarded);
        set_owner(&world, XP, Seat(1), 100.0);
        let mut fires = 0;
        world.tick();
        fires += residual_players(&mut world).len();

        // A level transition rebuilds the table with no edge memory.
        world.table = PlayerEventTable::build(
            &world.script_ctx,
            MoverCommandDiagnostics::default(),
            SpawnContext::default(),
            None,
        );
        world.tick();
        fires += residual_players(&mut world).len();

        // A disconnect hold and a reclaim.
        hold(&world, pawn);
        world.tick();
        let replacement = world.spawn_unbound(100.0);
        rebind(&world, replacement, Seat(1));
        world.tick();
        fires += residual_players(&mut world).len();

        assert_eq!(fires, expected_fires, "guarded: {guarded}");
    }
}

#[test]
fn a_zero_health_player_stays_observed_and_post_sweep_damage_reports_death_next_tick() {
    let mut world = World::new();
    let pawn = world.spawn_player(Some(Seat(1)), 10.0);
    let dead = IrNode::Le {
        a: Box::new(input("player.health")),
        b: Box::new(number(0.0)),
    };
    world.install(
        vec![
            on_player("finish", "applyDamage", json!({ "amount": 100.0 })),
            play_sound("toll", "bell"),
        ],
        vec![
            player_event(PlayerEventEdge::Becomes, low_health(), &["finish"]),
            player_event(PlayerEventEdge::Becomes, dead, &["toll"]),
        ],
    );
    // This tick's death sweep already ran before the seam; the fire's damage
    // lands after it.
    world.tick();
    assert_eq!(world.health(pawn), 0.0);
    assert!(world.take_residual_reactions().is_empty());

    let died = crate::sim::run_death_sweep(&world.script_ctx.registry);
    assert_eq!(died.iter().filter(|event| *event == "playerDied").count(), 1);
    world.tick();
    assert_eq!(
        world.take_residual_reactions(),
        vec!["toll".to_string()],
        "the dead player is still observed: becomes fires once at death"
    );
    assert!(crate::sim::run_death_sweep(&world.script_ctx.registry).is_empty());
    world.tick();
    assert!(world.take_residual_reactions().is_empty(), "no first-sight re-fire follows");
}

#[test]
fn a_recompose_keeping_a_condition_fires_nothing_and_a_changed_condition_starts_unobserved() {
    let mut world = World::new();
    world.spawn_player(Some(Seat(1)), 30.0);
    world.install(
        vec![play_sound("bleed", "bleed"), play_sound("drip", "drip")],
        vec![player_event(PlayerEventEdge::Becomes, low_health(), &["bleed"])],
    );
    world.tick();
    assert_eq!(world.take_residual_reactions(), vec!["bleed".to_string()]);

    let recompose = |world: &mut World, condition: IrNode| {
        {
            let mut data = world.script_ctx.data_registry.borrow_mut();
            data.clear();
            data.set_level_reactions(vec![play_sound("bleed", "bleed"), play_sound("drip", "drip")]);
            data.set_level_player_events(vec![player_event(
                PlayerEventEdge::Becomes,
                condition,
                &["drip"],
            )]);
            data.recompose(&[]);
        }
        let previous = std::mem::take(&mut world.table);
        world.table = PlayerEventTable::build(
            &world.script_ctx,
            MoverCommandDiagnostics::default(),
            SpawnContext::default(),
            Some(&previous),
        );
    };
    // Same condition and edge, edited fire list.
    recompose(&mut world, low_health());
    world.tick();
    assert!(world.take_residual_reactions().is_empty(), "edge memory survives by content");

    recompose(&mut world, lt(input("player.health"), number(60.0)));
    world.tick();
    assert_eq!(world.take_residual_reactions(), vec!["drip".to_string()]);
}

fn ammo_weapon(magazine: u32) -> WeaponComponent {
    let descriptor: WeaponDescriptor = serde_json::from_value(json!({
        "damage": 10.0,
        "range": 64.0,
        "primary": { "trigger": "hold", "recoveryMs": 100.0, "steps": [{ "kind": "shot" }] },
        "resolution": "hitscan",
        "resource": { "kind": "ammo", "type": "rounds", "magazine": 8, "reserve": 0 },
    }))
    .unwrap();
    let mut weapon = WeaponComponent::from_descriptor(&descriptor);
    weapon.magazine = magazine;
    weapon
}

fn wield(world: &World, pawn: EntityId, weapon: Option<WeaponComponent>) {
    let mut registry = world.script_ctx.registry.borrow_mut();
    let mut inventory = Inventory::default();
    if let Some(weapon) = weapon {
        let id = registry.spawn(Transform::default());
        registry.set_component(id, weapon).unwrap();
        inventory.wieldables[0] = Some(id);
    }
    inventory.active_slot = 0;
    registry.set_component(pawn, inventory).unwrap();
}

#[test]
fn a_missing_weapon_value_is_unobserved_and_regaining_it_fires_once() {
    let mut world = World::new();
    let pawn = world.spawn_player(Some(Seat(1)), 100.0);
    world.install(
        vec![play_sound("click", "dry"), play_sound("full", "full")],
        vec![
            player_event(PlayerEventEdge::Becomes, lt(input("player.ammo"), number(5.0)), &["click"]),
            player_event(PlayerEventEdge::Ceases, lt(input("player.ammo"), number(5.0)), &["full"]),
        ],
    );
    world.tick();
    assert!(world.take_residual_reactions().is_empty(), "no active weapon: unobserved");

    wield(&world, pawn, Some(ammo_weapon(3)));
    world.tick();
    assert_eq!(world.take_residual_reactions(), vec!["click".to_string()]);

    wield(&world, pawn, None);
    world.tick();
    assert!(world.take_residual_reactions().is_empty(), "losing the value fires nothing");

    wield(&world, pawn, Some(ammo_weapon(2)));
    world.tick();
    assert_eq!(world.take_residual_reactions(), vec!["click".to_string()]);
}

#[test]
fn a_connected_client_registers_and_fires_nothing() {
    let mut world = World::new();
    world.spawn_player(Some(Seat(1)), 30.0);
    world.script_ctx.owner_slot_writes_enabled.set(false);
    world.install(
        vec![play_sound("bleed", "bleed")],
        vec![player_event(PlayerEventEdge::Becomes, low_health(), &["bleed"])],
    );
    assert!(world.table.is_empty(), "nothing registers on a connected client");
    world.tick();
    assert!(world.take_residual_reactions().is_empty());
}

#[test]
fn constant_conditions_fire_once_at_first_sight() {
    let mut world = World::new();
    world.spawn_player(Some(Seat(1)), 100.0);
    world.install(
        vec![play_sound("hello", "hello")],
        vec![player_event(
            PlayerEventEdge::Becomes,
            IrNode::Const {
                value: IrValue::Bool(true),
            },
            &["hello"],
        )],
    );
    world.tick();
    world.tick();
    assert_eq!(world.take_residual_reactions(), vec!["hello".to_string()]);
}

#[test]
fn steady_state_evaluation_allocates_nothing() {
    let mut world = World::new();
    let first = world.spawn_player(Some(Seat(1)), 90.0);
    world.spawn_player(Some(Seat(2)), 90.0);
    world.install(
        vec![play_sound("bleed", "bleed"), play_sound("mend", "mend")],
        vec![
            player_event(PlayerEventEdge::Becomes, low_health(), &["bleed"]),
            player_event(PlayerEventEdge::Ceases, low_health(), &["mend"]),
        ],
    );
    // Warm every reused buffer, fires included.
    for health in [90.0, 20.0, 90.0] {
        world.set_health(first, health);
        world.tick();
        world.residuals.clear();
    }
    let mut healths = [20.0_f32, 90.0].into_iter().cycle();
    let mut allocations = 0;
    for _ in 0..8 {
        let health = healths.next().unwrap();
        world.set_health(first, health);
        let probe = crate::alloc_probe::AllocSnapshot::arm();
        world.tick();
        allocations += probe.allocs_since();
        assert_eq!(world.residuals.len(), 1, "one edge per tick");
        world.residuals.clear();
    }
    assert_eq!(allocations, 0, "per-tick evaluation and fire bookkeeping allocate nothing");
}
