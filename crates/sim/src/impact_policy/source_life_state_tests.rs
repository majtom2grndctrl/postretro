// An impact policy's `source.inLifeState` / `source.setLifeState` against real
// life-state descriptors: bind-time name checks, the source snapshot read, and
// the in-tick write through the shared life-state verb.
// See: context/lib/scripting.md §2 (Impact-policy evaluation), §12 (Player life states)

use super::tests::{evaluate_pending, event, grant_health, hit_from, number, owned_input, target};
use super::*;
use crate::life_state_commands::{set_life_state, tests::install_states};
use crate::player_events::tests::World;
use crate::sim::player_life::permits;
use log::Level;
use postretro_entities::provenance::{DescriptorProvenance, DescriptorSpawnPath};
use postretro_foundation::{Capability, PlayerMovementComponent, Seat};
use postretro_test_log_capture::LogCapture;
use serde_json::json;

fn set_life_state_effect(state: &str) -> Value {
    json!({
        "primitive": "setLifeState",
        "target": IMPACT_SOURCE_TOKEN,
        "args": { "state": state },
    })
}

fn in_life_state(state: &str) -> Value {
    owned_input(&format!("player.inLifeState:{state}"))
}

/// One seated player pawn declaring `alive`, `down` and `blocked` (fire
/// refused), entered `alive`, at half health; and an `enemy` impact target.
fn rig() -> (World, EntityId, EntityId) {
    let world = World::new();
    let pawn = world.spawn_player(Some(Seat(0)), 50.0);
    install_states(&world, &[pawn]);
    let target = target(&world.script_ctx, &["enemy"]);
    (world, pawn, target)
}

fn runtime(world: &World, events: Vec<ImpactEventDescriptor>) -> ImpactPolicyRuntime {
    let mut runtime = ImpactPolicyRuntime::new(world.script_ctx.clone());
    runtime.replace_global_events(events);
    runtime
}

fn hit(
    world: &World,
    runtime: &mut ImpactPolicyRuntime,
    target: EntityId,
    source: Option<EntityId>,
) {
    hit_from(&world.script_ctx, target, source, DamageProducer::InTick);
    evaluate_pending(&world.script_ctx, runtime);
}

fn state_id(world: &World, name: &str) -> u8 {
    world
        .script_ctx
        .data_registry
        .borrow()
        .life_state_names
        .iter()
        .position(|state| state == name)
        .expect("declared state") as u8
}

fn movement(world: &World, pawn: EntityId) -> PlayerMovementComponent {
    world
        .script_ctx
        .registry
        .borrow()
        .get_component::<PlayerMovementComponent>(pawn)
        .expect("player pawn")
        .clone()
}

fn set_elapsed(world: &World, pawn: EntityId, ticks: u32) {
    let mut registry = world.script_ctx.registry.borrow_mut();
    let mut movement = registry
        .get_component::<PlayerMovementComponent>(pawn)
        .unwrap()
        .clone();
    movement.life_state_elapsed_ticks = ticks;
    registry.set_component(pawn, movement).unwrap();
}

/// A pawn that never entered a life state: no provenance, so no descriptor.
fn stateless_pawn(world: &World) -> EntityId {
    world.spawn_player(Some(Seat(1)), 50.0)
}

#[test]
fn impact_source_in_life_state_holds_exactly_while_the_source_pawn_is_in_that_state() {
    let (world, pawn, target) = rig();
    let mut runtime = runtime(
        &world,
        vec![event(
            "second-wind",
            "enemy",
            vec![json!({ "when": in_life_state("down"), "do": [grant_health(number(5.0))] })],
        )],
    );

    hit(&world, &mut runtime, target, Some(pawn));
    assert_eq!(world.health(pawn), 50.0, "an alive source reads false");

    set_life_state(
        &world.script_ctx,
        &mut world.script_ctx.registry.borrow_mut(),
        &[pawn],
        "down",
    );
    hit(&world, &mut runtime, target, Some(pawn));
    assert_eq!(world.health(pawn), 55.0, "a down source reads true");

    set_life_state(
        &world.script_ctx,
        &mut world.script_ctx.registry.borrow_mut(),
        &[pawn],
        "alive",
    );
    hit(&world, &mut runtime, target, Some(pawn));
    assert_eq!(world.health(pawn), 55.0, "leaving `down` reads false again");
}

#[test]
fn impact_source_in_life_state_is_false_without_warning_for_a_non_player_source() {
    let (world, _pawn, target) = rig();
    // An NPC damager: health and even a movement component, but no seat.
    let npc = super::tests::source(&world.script_ctx, true, false);
    world
        .script_ctx
        .registry
        .borrow_mut()
        .set_component(npc, crate::trigger_bindings::group_tick_tests::movement())
        .unwrap();
    world.set_health(npc, 50.0);
    let mut runtime = runtime(
        &world,
        vec![event(
            "second-wind",
            "enemy",
            vec![json!({
                "when": { "op": "not", "x": in_life_state("alive") },
                "do": [grant_health(number(5.0))],
            })],
        )],
    );

    let capture = LogCapture::start();
    hit(&world, &mut runtime, target, Some(npc));
    hit(&world, &mut runtime, target, None);

    assert_eq!(
        world.health(npc),
        55.0,
        "`inLifeState(\"alive\")` is false on the NPC, so its negation fires"
    );
    capture.assert_nothing_above(Level::Info);
}

#[test]
fn impact_source_set_life_state_applies_in_tick_restarts_elapsed_and_same_state_is_a_noop() {
    let (world, pawn, target) = rig();
    let mut runtime = runtime(
        &world,
        vec![event(
            "lock-out",
            "enemy",
            vec![set_life_state_effect("blocked")],
        )],
    );
    set_elapsed(&world, pawn, 40);
    assert!(permits(
        &world.script_ctx.registry.borrow(),
        pawn,
        Capability::Fire
    ));

    hit(&world, &mut runtime, target, Some(pawn));
    let after = movement(&world, pawn);
    assert_eq!(after.life_state, state_id(&world, "blocked"));
    assert_eq!(
        after.life_state_elapsed_ticks, 0,
        "an actual change restarts elapsed"
    );
    assert!(
        !permits(&world.script_ctx.registry.borrow(), pawn, Capability::Fire),
        "the next tick's fire gate reads the new profile"
    );

    set_elapsed(&world, pawn, 7);
    hit(&world, &mut runtime, target, Some(pawn));
    assert_eq!(
        movement(&world, pawn).life_state_elapsed_ticks,
        7,
        "setting the current state is a no-op"
    );
}

#[test]
fn impact_source_set_life_state_on_a_live_non_player_source_warn_skips() {
    let (world, _pawn, target) = rig();
    let npc = super::tests::source(&world.script_ctx, true, false);
    let mut runtime = runtime(
        &world,
        vec![event(
            "revive",
            "enemy",
            vec![set_life_state_effect("down")],
        )],
    );

    let capture = LogCapture::start();
    hit(&world, &mut runtime, target, Some(npc));

    capture.assert_logged_once(Level::Warn, "does not declare state `down`; skipping");
}

#[test]
fn impact_source_set_life_state_without_a_live_source_skips_silently() {
    let (world, pawn, target) = rig();
    let mut runtime = runtime(
        &world,
        vec![event(
            "revive",
            "enemy",
            vec![set_life_state_effect("down")],
        )],
    );
    let gone = super::tests::source(&world.script_ctx, true, false);
    world
        .script_ctx
        .registry
        .borrow_mut()
        .despawn(gone)
        .unwrap();

    let capture = LogCapture::start();
    hit(&world, &mut runtime, target, None);
    hit(&world, &mut runtime, target, Some(gone));

    capture.assert_nothing_above(Level::Info);
    assert_eq!(movement(&world, pawn).life_state, state_id(&world, "alive"));
}

#[test]
fn impact_policy_naming_an_unknown_life_state_is_skipped_at_bind_while_siblings_bind() {
    let (world, pawn, target) = rig();
    let capture = LogCapture::start();
    let mut runtime = runtime(
        &world,
        vec![
            event(
                "bad-read",
                "enemy",
                vec![json!({ "when": in_life_state("ghost"), "do": [grant_health(number(1.0))] })],
            ),
            event("bad-write", "enemy", vec![set_life_state_effect("ghost")]),
            event("bonus", "enemy", vec![grant_health(number(5.0))]),
        ],
    );

    let skipped = |policy: &str| {
        capture
            .records()
            .iter()
            .filter(|record| {
                record.level == Level::Warn
                    && record
                        .message
                        .contains(&format!("policy `{policy}` was skipped during bind"))
                    && record.message.contains("ghost")
            })
            .count()
    };
    assert_eq!(skipped("bad-read"), 1, "{:?}", capture.records());
    assert_eq!(skipped("bad-write"), 1, "{:?}", capture.records());
    assert_eq!(skipped("bonus"), 0);

    hit(&world, &mut runtime, target, Some(pawn));
    assert_eq!(
        world.health(pawn),
        55.0,
        "the mod's other policy still binds"
    );
}

#[test]
fn impact_source_set_life_state_warn_skips_a_source_lacking_the_state_or_declaring_none() {
    let (world, pawn, target) = rig();
    // A second player descriptor declaring only `alive`: `down` stays known
    // mod-wide through the first, so the policy binds.
    {
        let mut data = world.script_ctx.data_registry.borrow_mut();
        let player = data.entities[0].clone();
        let mut medic = player.clone();
        medic.canonical_name = Some("medic".into());
        let states = medic.life_states.as_mut().unwrap();
        states.states.retain(|name, _| name == "alive");
        data.replace_entity_types(vec![player, medic]);
    }
    let medic = world.spawn_player(Some(Seat(2)), 50.0);
    world
        .script_ctx
        .registry
        .borrow_mut()
        .set_component(
            medic,
            DescriptorProvenance {
                canonical_name: "medic".into(),
                owned_components: Default::default(),
                map_overrides: Default::default(),
                spawn_path: DescriptorSpawnPath::PlayerSpawn,
            },
        )
        .unwrap();
    set_life_state(
        &world.script_ctx,
        &mut world.script_ctx.registry.borrow_mut(),
        &[medic],
        "alive",
    );
    let stateless = stateless_pawn(&world);
    let mut runtime = runtime(
        &world,
        vec![event("down", "enemy", vec![set_life_state_effect("down")])],
    );
    let medic_before = movement(&world, medic).life_state;
    let stateless_before = movement(&world, stateless).life_state;

    let capture = LogCapture::start();
    hit(&world, &mut runtime, target, Some(medic));
    hit(&world, &mut runtime, target, Some(stateless));
    hit(&world, &mut runtime, target, Some(pawn));

    for skipped in [medic, stateless] {
        capture.assert_logged_once(
            Level::Warn,
            &format!("target {skipped:?} does not declare state `down`; skipping"),
        );
    }
    assert_eq!(movement(&world, medic).life_state, medic_before);
    assert_eq!(movement(&world, stateless).life_state, stateless_before);
    assert_eq!(movement(&world, pawn).life_state, state_id(&world, "down"));
}
