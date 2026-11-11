// Attack-pool request timing through the authoritative tick: a trigger's start or stop
// applies before that tick's AI pass, every other request latches to the next, in issue order.
// See: context/lib/scripting.md §12 (Entity addressing) · context/lib/entity_model.md §7c

use glam::Vec3;
use postretro_entities::components::attack_pool::AttackPoolComponent;
use postretro_entities::components::brain::BrainComponent;
use postretro_entities::components::spawner::SpawnerComponent;
use postretro_entities::{ComponentKind, EntityId, Transform, TriggerCommand};
use postretro_foundation::{START_ATTACK_POOL, STOP_ATTACK_POOL};

use super::member_tick_test_fixtures::*;
use crate::scripting::builtins::data_archetype_test_fixtures::behavior_enemy_descriptor;

/// A stopped pool reached by `tag`, assigning holders of `member_tag`.
fn pool(rig: &Rig, tag: &str, member_tag: &str) -> EntityId {
    let mut registry = rig.ctx.registry.borrow_mut();
    let id = registry
        .try_spawn(Transform::default(), &[tag.to_string()])
        .unwrap();
    registry
        .set_component(
            id,
            AttackPoolComponent::new(member_tag.to_string(), "arena".to_string()),
        )
        .unwrap();
    id
}

/// A placed enemy carrying `tag`.
fn guard(rig: &Rig, tag: &str) -> EntityId {
    let mut registry = rig.ctx.registry.borrow_mut();
    let id = registry
        .try_spawn(
            Transform {
                position: AWAY,
                ..Transform::default()
            },
            &[tag.to_string()],
        )
        .unwrap();
    let graph = behavior_enemy_descriptor("cultist").behavior.unwrap();
    registry
        .set_component(id, BrainComponent::from_graph(&graph))
        .unwrap();
    id
}

/// The pool `enemy` took turns in on `tick`'s AI pass.
fn pool_seen(rig: &Rig, tick: u32, enemy: EntityId) -> Option<EntityId> {
    let (_, seen) = rig
        .ai_seen
        .iter()
        .find(|(seen_tick, _)| *seen_tick == tick)
        .unwrap_or_else(|| panic!("no AI pass on tick {tick}"));
    seen.iter()
        .find(|(id, _)| *id == enemy)
        .unwrap_or_else(|| panic!("{enemy:?} absent from the AI pass"))
        .1
}

fn running(rig: &Rig, pool: EntityId) -> bool {
    rig.ctx
        .registry
        .borrow()
        .get_component::<AttackPoolComponent>(pool)
        .unwrap()
        .running
}

/// A pawn waits away for two frames, then steps onto the plate: the Enter
/// lands on tick 2.
fn enter_on_tick_two(rig: &mut Rig) {
    let pawn = rig.local(AWAY);
    rig.frames(2);
    rig.place(pawn, Vec3::ZERO);
    rig.frame();
}

// A1 (sim half) and A10 / P15: a trigger's `start` command, and a
// trigger-bound `start` step, each run the pool and assign its tag holders
// before the Enter tick's AI pass. A script `start` from the frame-end drain
// latches to the next tick and applies first there. The pool's members then
// arbitrate under its profile (`running_pool_arbitrates_under_its_profile`).
#[test]
fn attack_pool_start_by_trigger_and_script_applies_profile() {
    // The trigger's no-script command.
    let mut rig = Rig::new();
    let arena = pool(&rig, "arena", "guard");
    let member = guard(&rig, "guard");
    rig.trigger("arena", TriggerCommand::Start, "");
    rig.bind(Vec::new());
    enter_on_tick_two(&mut rig);
    assert_eq!(pool_seen(&rig, 1, member), None);
    assert_eq!(
        pool_seen(&rig, 2, member),
        Some(arena),
        "the command applies before the Enter tick's AI pass"
    );

    // A trigger-bound step before any `wait`.
    let mut rig = Rig::new();
    let arena = pool(&rig, "arena", "guard");
    let member = guard(&rig, "guard");
    rig.trigger("plate", TriggerCommand::Start, "go");
    rig.bind(vec![reaction("go", vec![step(arena, START_ATTACK_POOL)])]);
    enter_on_tick_two(&mut rig);
    assert_eq!(
        pool_seen(&rig, 2, member),
        Some(arena),
        "a trigger-bound step applies on its own tick"
    );

    // A script step issued by the frame-end drain latches to the next tick.
    let mut rig = Rig::new();
    let arena = pool(&rig, "arena", "guard");
    let member = guard(&rig, "guard");
    rig.frame();
    rig.latch(arena, &step(arena, START_ATTACK_POOL));
    assert!(
        !running(&rig, arena),
        "a latched start waits for the next tick"
    );
    rig.frame();
    assert_eq!(pool_seen(&rig, 1, member), Some(arena));
    assert!(running(&rig, arena));
}

// A10 / P15, alias for the plan's name: a trigger-bound start applies before
// that tick's arbitration, after the trigger stage.
#[test]
fn trigger_bound_pool_start_applies_before_arbitration() {
    let mut rig = Rig::new();
    let arena = pool(&rig, "arena", "guard");
    let member = guard(&rig, "guard");
    rig.trigger("plate", TriggerCommand::Start, "go");
    rig.bind(vec![reaction(
        "go",
        vec![step(arena, START_ATTACK_POOL), wait(100.0)],
    )]);
    enter_on_tick_two(&mut rig);
    assert_eq!(pool_seen(&rig, 2, member), Some(arena));
}

// A10 / P15: a start and a stop of one pool on one tick apply in issue
// order. The trigger's direct command is issued first, then its bound step.
#[test]
fn pool_start_and_stop_same_tick_apply_in_issue_order() {
    for (command, bound, ends_running) in [
        (TriggerCommand::Start, STOP_ATTACK_POOL, false),
        (TriggerCommand::Stop, START_ATTACK_POOL, true),
    ] {
        let mut rig = Rig::new();
        let arena = pool(&rig, "arena", "guard");
        let member = guard(&rig, "guard");
        rig.trigger("arena", command, "plate");
        rig.bind(vec![reaction("plate", vec![step(arena, bound)])]);
        enter_on_tick_two(&mut rig);
        assert_eq!(running(&rig, arena), ends_running, "{bound}");
        assert_eq!(
            pool_seen(&rig, 2, member),
            ends_running.then_some(arena),
            "{bound}: the AI pass sees the later-issued request"
        );
    }
}

// P15: a request latched by the frame-end drain applies before the next
// tick's trigger requests.
#[test]
fn latched_pool_requests_apply_before_trigger_requests() {
    for (latched, command, ends_running) in [
        (STOP_ATTACK_POOL, TriggerCommand::Start, true),
        (START_ATTACK_POOL, TriggerCommand::Stop, false),
    ] {
        let mut rig = Rig::new();
        let arena = pool(&rig, "arena", "guard");
        guard(&rig, "guard");
        rig.trigger("arena", command.clone(), "");
        rig.bind(Vec::new());
        let pawn = rig.local(AWAY);
        rig.frames(2);
        if latched == STOP_ATTACK_POOL {
            rig.latch(arena, &step(arena, START_ATTACK_POOL));
            rig.frame();
            assert!(running(&rig, arena), "fixture: the pool runs");
        }
        rig.latch(arena, &step(arena, latched));
        rig.place(pawn, Vec3::ZERO);
        rig.frame();
        assert_eq!(
            running(&rig, arena),
            ends_running,
            "the latched {latched} applies first, then the trigger's {command:?}"
        );
    }
}

// A6 / P34: pools A then B run. On one tick a trigger-bound sequence stops B
// and fires a spawner whose `spawned_tags` carry both member tags. Spawn
// before stop: the spawns join B and return with B's members to the default
// pool. Stop before spawn: they join A. A latched stop of B applies before
// both. A replay agrees.
#[test]
fn spawn_on_pool_stop_tick_follows_issue_order() {
    #[derive(Clone, Copy, Debug, PartialEq)]
    enum Order {
        SpawnThenStop,
        StopThenSpawn,
        LatchedStop,
    }
    let run = |order: Order| -> (EntityId, Vec<Option<EntityId>>) {
        let mut rig = Rig::new();
        let a = pool(&rig, "a", "guard_a");
        let b = pool(&rig, "b", "guard_b");
        let spawner = rig.spawner("closet");
        {
            let mut registry = rig.ctx.registry.borrow_mut();
            let mut component = registry
                .get_component::<SpawnerComponent>(spawner)
                .unwrap()
                .clone();
            component.spawned_tags = vec!["guard_a".into(), "guard_b".into()];
            registry.set_component(spawner, component).unwrap();
        }
        let steps = match order {
            Order::SpawnThenStop => {
                vec![step(spawner, "spawnFromSpawner"), step(b, STOP_ATTACK_POOL)]
            }
            Order::StopThenSpawn => {
                vec![step(b, STOP_ATTACK_POOL), step(spawner, "spawnFromSpawner")]
            }
            Order::LatchedStop => vec![step(spawner, "spawnFromSpawner")],
        };
        rig.trigger("plate", TriggerCommand::Start, "go");
        rig.bind(vec![reaction("go", steps)]);
        let pawn = rig.local(AWAY);
        rig.latch(a, &step(a, START_ATTACK_POOL));
        rig.latch(b, &step(b, START_ATTACK_POOL));
        rig.frames(2);
        if order == Order::LatchedStop {
            rig.latch(b, &step(b, STOP_ATTACK_POOL));
        }
        rig.place(pawn, Vec3::ZERO);
        rig.frame();
        let spawned: Vec<EntityId> = rig
            .ctx
            .registry
            .borrow()
            .query_by_component_and_tag(ComponentKind::Brain, Some("guard_a"))
            .map(|(id, _)| id)
            .collect();
        assert_eq!(
            spawned.len(),
            SPAWN_COUNT as usize,
            "{order:?}: the batch spawned"
        );
        let seen = spawned
            .into_iter()
            .map(|spawn| pool_seen(&rig, 2, spawn))
            .collect();
        (a, seen)
    };
    for (order, joins_a) in [
        (Order::SpawnThenStop, false),
        (Order::StopThenSpawn, true),
        (Order::LatchedStop, true),
    ] {
        let (a, first) = run(order);
        let expected = joins_a.then_some(a);
        assert!(
            first.iter().all(|&pool| pool == expected),
            "{order:?}: spawns take turns in {expected:?}, saw {first:?}"
        );
        assert_eq!(run(order).1, first, "{order:?}: a replay agrees");
    }
}
