//! Proves the ordering between a real simulation tick and the player-event pass:
//! two zeroed pawns each fire their own life-state edge plus one legacy death event,
//! healing before the sweep suppresses the edges while a post-sweep heal rearms the
//! per-pawn death latch, and re-setting the same state continues its clock and conditions
//! through the next real tick.
//!
//! See: context/lib/scripting.md §12 Reaction Dispatch Model (player life states)
use super::tests::{World, input, number, on_player, player_event};
use super::*;
use crate::ai_host::AiTickResult;
use crate::collision::CollisionWorld;
use crate::kinematic_mover::MoverTickStateTable;
use crate::scripting_systems::hit_zones::HitZoneStore;
use crate::sim::{PostMovementCommand, TickEvents, simulate_tick};
use postretro_entities::EntityRegistry;
use postretro_entities::components::health::{HealthComponent, set_health_absolute};
use postretro_foundation::PlayerMovementComponent;
use postretro_scripting_core::reaction_dispatch::ProgressTracker;
use serde_json::json;

fn low_health() -> IrNode {
    IrNode::Le {
        a: Box::new(input("player.health")),
        b: Box::new(number(0.0)),
    }
}

fn install() -> (World, EntityId, EntityId) {
    let mut world = World::new();
    let first = world.spawn_player(Some(Seat(0)), 100.0);
    let second = world.spawn_player(Some(Seat(1)), 100.0);
    world
        .script_ctx
        .registry
        .borrow_mut()
        .mark_local_player_pawn(first)
        .unwrap();
    crate::life_state_commands::tests::install_states(&world, &[first, second]);
    world.install(
        vec![on_player("down", "setLifeState", json!({"state":"down"}))],
        vec![player_event(
            PlayerEventEdge::Becomes,
            low_health(),
            &["down"],
        )],
    );
    (world, first, second)
}

fn neutral_command() -> crate::sim::SimCommand {
    crate::sim::SimCommand {
        input_tick: 0,
        movement: crate::movement::MovementInput {
            wish_dir: glam::Vec2::ZERO,
            jump_pressed: false,
            dash_pressed: false,
            running: false,
            crouch_intent: false,
            facing_yaw: 0.0,
            facing_pitch: 0.0,
            use_pressed: false,
            drop_pressed: false,
        },
        fire_button: crate::weapon::FireButtonState {
            pressed: false,
            active: false,
        },
        secondary_button: crate::weapon::FireButtonState {
            pressed: false,
            active: false,
        },
        activation: Default::default(),
        reload: false,
        firing_slot: 0,
        select_slot: None,
        use_pressed: false,
        drop_pressed: false,
    }
}

fn tick(world: &World, mut ai: impl FnMut(&mut EntityRegistry)) -> TickEvents {
    simulate_tick(
        world.script_ctx.registry.clone(),
        &CollisionWorld::new(),
        &HitZoneStore::new(),
        None,
        postretro_collision::EnvironmentView::level_only(0.0),
        None,
        0.0,
        &mut ProgressTracker::new(),
        |registry, _, _, _| {
            ai(registry);
            AiTickResult {
                events: Vec::new(),
                projectile_spawns: Vec::new(),
            }
        },
        &[],
        &mut MoverTickStateTable::default(),
        &[],
        &neutral_command(),
        |_| PostMovementCommand {
            aim_origin: glam::Vec3::ZERO,
            aim_direction: glam::Vec3::NEG_Z,
        },
        1.0 / 60.0,
        None,
        |_| {},
    )
}
fn deaths(events: &TickEvents) -> usize {
    events
        .death
        .iter()
        .filter(|name| name.as_str() == "playerDied")
        .count()
}
fn elapsed(world: &World, pawn: EntityId) -> u32 {
    world
        .script_ctx
        .registry
        .borrow()
        .get_component::<PlayerMovementComponent>(pawn)
        .unwrap()
        .life_state_elapsed_ticks
}

#[test]
fn life_policy_o1_two_zeroed_subjects_fire_own_edges_and_one_legacy_event() {
    let (mut world, first, second) = install();
    tick(&world, |_| {});
    world.tick();
    let events = tick(&world, |registry| {
        for pawn in [first, second] {
            set_health_absolute(registry, pawn, 0.0);
        }
    });
    assert_eq!(
        deaths(&events),
        1,
        "the param-less legacy event still coalesces one sweep"
    );
    world.tick();
    let fired: Vec<_> = world
        .table
        .scratch
        .fires
        .iter()
        .map(|fire| (fire.pawn, fire.player))
        .collect();
    assert_eq!(
        fired,
        [
            (first, PlayerKey::Seat(Seat(0))),
            (second, PlayerKey::Seat(Seat(1)))
        ]
    );
    for pawn in [first, second] {
        assert_eq!(elapsed(&world, pawn), 0);
    }
    assert_eq!(deaths(&tick(&world, |_| {})), 0);
    world.tick();
    assert!(
        world.table.scratch.fires.is_empty(),
        "held zero health creates no second edge"
    );
    for pawn in [first, second] {
        assert_eq!(elapsed(&world, pawn), 1);
    }
}

#[test]
fn life_policy_o2_heal_before_sweep_suppresses_edges_and_post_sweep_heal_rearms_latch() {
    for sweep_first in [false, true] {
        let (mut world, pawn, other) = install();
        tick(&world, |_| {});
        world.tick();
        let events = tick(&world, |registry| {
            set_health_absolute(registry, pawn, 0.0);
            if !sweep_first {
                set_health_absolute(registry, pawn, 75.0);
            }
        });
        if sweep_first {
            assert_eq!(deaths(&events), 1);
            // The settled simulation latched first; the same tick's player pass
            // observes the subsequent positive write, as the frame-loop ordering does.
            set_health_absolute(&mut world.script_ctx.registry.borrow_mut(), pawn, 75.0);
        } else {
            assert_eq!(deaths(&events), 0);
        }
        world.tick();
        assert!(
            world.table.scratch.fires.is_empty(),
            "the pass sees healed health, so no zero edge"
        );
        assert!(
            !world
                .script_ctx
                .registry
                .borrow()
                .get_component::<HealthComponent>(pawn)
                .unwrap()
                .death_handled
        );
        assert_eq!(world.health(other), 100.0);
        let events = tick(&world, |registry| set_health_absolute(registry, pawn, 0.0));
        assert_eq!(
            deaths(&events),
            1,
            "positive health rearmed the pawn's death latch"
        );
        world.tick();
        assert_eq!(
            world
                .table
                .scratch
                .fires
                .iter()
                .map(|fire| fire.pawn)
                .collect::<Vec<_>>(),
            [pawn]
        );
        assert_eq!(deaths(&tick(&world, |_| {})), 0);
    }
}

#[test]
fn life_policy_o5_same_state_set_continues_through_next_real_tick_and_condition() {
    let (mut world, pawn, other) = install();
    world.install(
        vec![
            on_player("down", "setLifeState", json!({"state":"down"})),
            on_player("continued", "grantHealth", json!({"amount":10})),
        ],
        vec![
            player_event(PlayerEventEdge::Becomes, low_health(), &["down"]),
            player_event(
                PlayerEventEdge::Becomes,
                IrNode::And {
                    a: Box::new(input("player.inLifeState:down")),
                    b: Box::new(IrNode::Ge {
                        a: Box::new(input("player.lifeStateMs")),
                        b: Box::new(number(32.0)),
                    }),
                },
                &["continued"],
            ),
        ],
    );
    tick(&world, |registry| set_health_absolute(registry, pawn, 0.0));
    world.tick();
    assert_eq!(elapsed(&world, pawn), 0);
    tick(&world, |_| {});
    world.tick();
    assert_eq!(elapsed(&world, pawn), 1);
    crate::life_state_commands::set_life_state(
        &world.script_ctx,
        &mut world.script_ctx.registry.borrow_mut(),
        &[pawn],
        "down",
    );
    assert_eq!(
        elapsed(&world, pawn),
        1,
        "same-state write preserves the live clock"
    );
    tick(&world, |_| {});
    world.tick();
    assert_eq!(
        elapsed(&world, pawn),
        2,
        "next full tick continues instead of starting over"
    );
    assert_eq!(
        world.health(pawn),
        10.0,
        "the actual >=32ms condition sees the continued 33ms value"
    );
    assert_eq!(world.health(other), 100.0);
}
