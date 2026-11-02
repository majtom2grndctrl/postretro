// Orderings between a late remote hit's `source.setLifeState` and the
// player-event pass: a real sim tick, the post-tick hit flush (an in-tick
// impact evaluated against the live registry), then `PlayerEventTable::run_tick`.
// See: context/lib/scripting.md §12 (Player life states)

use super::tests::{evaluate_pending, event, grant_health, hit_from, number, owned_input, target};
use super::*;
use crate::collision::CollisionWorld;
use crate::life_state_commands::tests::install_states;
use crate::player_events::life_state_ordering_tests::tick;
use crate::player_events::tests::{World, input, on_player, player_event};
use glam::Vec3;
use postretro_entities::RespawnPlacement;
use postretro_entities::components::health::set_health_absolute;
use postretro_foundation::{IrNode, PlayerMovementComponent, Seat};
use postretro_scripting_core::data_descriptors::PlayerEventEdge;
use serde_json::json;

const RESPAWN_HEALTH: f32 = 25.0;
const SECOND_WIND_HEALTH: f32 = 40.0;

/// Spec A's down / bleed-out policy with a two-tick bleed-out (33 ms ≥ 32),
/// plus the Borderlands self-revive impact policy on the same `ScriptCtx`.
fn install() -> (World, ImpactPolicyRuntime, EntityId, EntityId) {
    let mut world = World::new();
    let pawn = world.spawn_player(Some(Seat(0)), 100.0);
    world
        .script_ctx
        .registry
        .borrow_mut()
        .mark_local_player_pawn(pawn)
        .unwrap();
    install_states(&world, &[pawn]);
    world.script_ctx.respawn_placements.borrow_mut().map_order = vec![RespawnPlacement {
        map_index: 0,
        foot_anchor: Vec3::ZERO,
        yaw: 0.0,
    }];
    world.install(
        vec![
            on_player("goDown", "setLifeState", json!({ "state": "down" })),
            on_player(
                "bleedOut",
                "respawn",
                json!({ "health": RESPAWN_HEALTH, "state": "alive" }),
            ),
        ],
        vec![
            player_event(
                PlayerEventEdge::Becomes,
                IrNode::Le {
                    a: Box::new(input("player.health")),
                    b: Box::new(number_node(0.0)),
                },
                &["goDown"],
            ),
            player_event(
                PlayerEventEdge::Becomes,
                IrNode::And {
                    a: Box::new(input("player.inLifeState:down")),
                    b: Box::new(IrNode::Ge {
                        a: Box::new(input("player.lifeStateMs")),
                        b: Box::new(number_node(32.0)),
                    }),
                },
                &["bleedOut"],
            ),
        ],
    );
    let mut runtime = ImpactPolicyRuntime::new(world.script_ctx.clone());
    runtime.replace_global_events(vec![event(
        "second-wind",
        "enemy",
        vec![json!({
            "when": owned_input("player.inLifeState:down"),
            "do": [
                { "primitive": "setLifeState", "target": IMPACT_SOURCE_TOKEN, "args": { "state": "alive" } },
                grant_health(number(SECOND_WIND_HEALTH)),
            ],
        })],
    )]);
    let target = target(&world.script_ctx, &["enemy"]);
    (world, runtime, pawn, target)
}

fn number_node(value: f32) -> IrNode {
    crate::player_events::tests::number(value)
}

fn life_state(world: &World, pawn: EntityId) -> (String, u32) {
    let registry = world.script_ctx.registry.borrow();
    let movement = registry
        .get_component::<PlayerMovementComponent>(pawn)
        .unwrap();
    let name = world.script_ctx.data_registry.borrow().life_state_names
        [usize::from(movement.life_state)]
    .clone();
    (name, movement.life_state_elapsed_ticks)
}

/// The pawn goes down in one pass and has spent one more tick down: the next
/// sim tick is the one whose player-event pass sees bleed-out turn true.
fn down_one_tick_before_bleed_out(world: &mut World, pawn: EntityId) {
    tick(world, |registry| set_health_absolute(registry, pawn, 0.0));
    world.tick();
    assert_eq!(life_state(world, pawn), ("down".into(), 0));
    tick(world, |_| {});
    world.tick();
    assert_eq!(life_state(world, pawn), ("down".into(), 1));
}

/// The remote hit's late flush: the policy runs against the live registry
/// after the sim tick, as `host_flush_pending_hit_declarations` does.
fn flush_remote_hit(
    world: &World,
    runtime: &mut ImpactPolicyRuntime,
    target: EntityId,
    pawn: EntityId,
) {
    hit_from(
        &world.script_ctx,
        target,
        Some(pawn),
        DamageProducer::InTick,
    );
    evaluate_pending(&world.script_ctx, runtime);
}

/// Respawn drains at the start of the next authoritative tick, ahead of the
/// simulation body (`begin_tick` / `finish_tick_start`).
fn start_tick_draining_respawns(world: &World) {
    let pending = crate::sim::begin_tick(&world.script_ctx.registry, Some(&world.script_ctx));
    crate::sim::finish_tick_start(
        pending,
        &mut world.script_ctx.registry.borrow_mut(),
        &CollisionWorld::new(),
        |_| None,
    );
}

/// O2: bleed-out fires in tick N's pass, respawn drains at tick N+1's start,
/// and the self-revive hit lands in a later post-tick flush. Respawn wins: the
/// hit reads `source.inLifeState("down")` false and its row does not fire.
#[test]
fn source_life_state_o2_bleed_out_respawn_beats_a_later_self_revive_hit() {
    let (mut world, mut runtime, pawn, target) = install();
    down_one_tick_before_bleed_out(&mut world, pawn);

    // Tick N: bleed-out turns true in the pass and queues the respawn.
    tick(&world, |_| {});
    world.tick();
    assert!(
        world
            .script_ctx
            .pending_respawns
            .borrow()
            .contains_key(&pawn)
    );
    assert_eq!(life_state(&world, pawn).0, "down");

    // Tick N+1: the respawn drains before the body.
    start_tick_draining_respawns(&world);
    assert!(world.script_ctx.pending_respawns.borrow().is_empty());
    assert_eq!(life_state(&world, pawn), ("alive".into(), 0));
    assert_eq!(world.health(pawn), RESPAWN_HEALTH);
    tick(&world, |_| {});

    // Post-tick flush of tick N+k (k = 1).
    flush_remote_hit(&world, &mut runtime, target, pawn);
    world.tick();

    assert_eq!(
        world.health(pawn),
        RESPAWN_HEALTH,
        "the self-revive row did not fire: no second-wind heal"
    );
    assert_eq!(
        life_state(&world, pawn),
        ("alive".into(), 1),
        "the respawn's state and clock stand"
    );
    assert!(world.script_ctx.pending_respawns.borrow().is_empty());
}

/// O7: the self-revive hit is flushed after tick N's sim and before tick N's
/// player-event pass, the pass whose bleed-out condition would turn true.
/// Self-revive wins: bleed-out reads `inLifeState("down")` false, so no
/// respawn is queued.
#[test]
fn source_life_state_o7_self_revive_flushed_before_the_bleed_out_pass_wins() {
    let (mut world, mut runtime, pawn, target) = install();
    down_one_tick_before_bleed_out(&mut world, pawn);

    // Tick N: sim, then the remote-hit flush, then the player-event pass.
    tick(&world, |_| {});
    assert_eq!(
        life_state(&world, pawn),
        ("down".into(), 2),
        "without the hit, this pass's 33 ms bleed-out condition is true"
    );
    flush_remote_hit(&world, &mut runtime, target, pawn);
    assert_eq!(life_state(&world, pawn), ("alive".into(), 0));
    assert_eq!(world.health(pawn), SECOND_WIND_HEALTH);
    world.tick();

    assert_eq!(
        life_state(&world, pawn),
        ("alive".into(), 0),
        "the pass left the revived pawn alone"
    );
    assert!(
        world.script_ctx.pending_respawns.borrow().is_empty(),
        "no respawn is queued"
    );

    start_tick_draining_respawns(&world);
    tick(&world, |_| {});
    world.tick();
    assert_eq!(life_state(&world, pawn), ("alive".into(), 1));
    assert_eq!(world.health(pawn), SECOND_WIND_HEALTH);
}
