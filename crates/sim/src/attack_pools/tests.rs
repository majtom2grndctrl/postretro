// Attack-pool requests through the drain: start, stop and spawn joins apply in
// issue order; membership comes from events, never from live tags.
// See: context/lib/entity_model.md §7c (Attack pools)

use postretro_entities::components::attack_pool::{AttackPoolComponent, AttackPoolRequest};
use postretro_entities::components::brain::BrainComponent;
use postretro_entities::{ComponentValue, EntityId, EntityRegistry, Transform};
use postretro_scripting_core::sequence::SequencedPrimitiveRegistry;

use super::commands::register_sequenced_attack_pool_primitives;
use super::*;
use crate::scripting::builtins::data_archetype_test_fixtures::behavior_enemy_descriptor;

fn pool(registry: &mut EntityRegistry, tag: &str, member_tag: &str) -> EntityId {
    let id = registry
        .try_spawn(Transform::default(), &[tag.to_string()])
        .unwrap();
    registry
        .set_component(
            id,
            AttackPoolComponent::new(member_tag.to_string(), String::new()),
        )
        .unwrap();
    id
}

fn enemy(registry: &mut EntityRegistry, tags: &[&str]) -> EntityId {
    let tags: Vec<String> = tags.iter().map(|tag| tag.to_string()).collect();
    let id = registry.try_spawn(Transform::default(), &tags).unwrap();
    let graph = behavior_enemy_descriptor("cultist").behavior.unwrap();
    registry
        .set_component(id, BrainComponent::from_graph(&graph))
        .unwrap();
    id
}

fn assigned(registry: &EntityRegistry, enemy: EntityId) -> Option<EntityId> {
    registry
        .get_component::<BrainComponent>(enemy)
        .unwrap()
        .attack_pool
}

fn state(registry: &EntityRegistry, pool: EntityId) -> AttackPoolComponent {
    registry
        .get_component::<AttackPoolComponent>(pool)
        .unwrap()
        .clone()
}

fn apply(registry: &mut EntityRegistry, requests: &[AttackPoolRequest]) {
    for &request in requests {
        match request {
            AttackPoolRequest::Start(pool) => {
                issue_attack_pool_request(registry, pool, AttackPoolVerb::Start)
            }
            AttackPoolRequest::Stop(pool) => {
                issue_attack_pool_request(registry, pool, AttackPoolVerb::Stop)
            }
            AttackPoolRequest::Join(enemy) => queue_spawn_join(registry, enemy),
        }
    }
    apply_attack_pool_requests(registry);
}

// A5: a tag holder present at start and a spawn carrying the tag while the
// pool runs are members. A tag added or removed later moves no one, and an
// untagged spawn joins nothing.
#[test]
fn pool_membership_is_assigned_by_start_and_spawn_only() {
    use AttackPoolRequest::{Join, Start};
    let mut registry = EntityRegistry::new();
    let arena = pool(&mut registry, "arena", "guard");
    let holder = enemy(&mut registry, &["guard"]);
    let bystander = enemy(&mut registry, &["patrol"]);

    apply(&mut registry, &[Start(arena)]);
    assert!(state(&registry, arena).running);
    assert_eq!(assigned(&registry, holder), Some(arena));
    assert_eq!(assigned(&registry, bystander), None);

    let spawn = enemy(&mut registry, &["guard", "wave_1"]);
    let untagged = enemy(&mut registry, &[]);
    apply(&mut registry, &[Join(spawn), Join(untagged)]);
    assert_eq!(
        assigned(&registry, spawn),
        Some(arena),
        "a tagged spawn joins"
    );
    assert_eq!(assigned(&registry, untagged), None);

    registry
        .set_tags(bystander, vec!["guard".to_string()])
        .unwrap();
    registry.set_tags(holder, Vec::new()).unwrap();
    apply(&mut registry, &[]);
    assert_eq!(
        assigned(&registry, bystander),
        None,
        "a tag added later moves no one"
    );
    assert_eq!(
        assigned(&registry, holder),
        Some(arena),
        "a tag removed later moves no one"
    );
}

// A4 (assignment half): two pools run side by side, each assigning its own
// tag holders; stopping one returns only its members, and `stop` on a stopped
// pool changes nothing.
#[test]
fn two_pools_assign_side_by_side_and_stop_independently() {
    use AttackPoolRequest::{Start, Stop};
    let mut registry = EntityRegistry::new();
    let (east, west) = (
        pool(&mut registry, "east", "east_guard"),
        pool(&mut registry, "west", "west_guard"),
    );
    let (e, w) = (
        enemy(&mut registry, &["east_guard"]),
        enemy(&mut registry, &["west_guard"]),
    );
    apply(&mut registry, &[Start(east), Start(west)]);
    assert_eq!(
        (assigned(&registry, e), assigned(&registry, w)),
        (Some(east), Some(west))
    );
    apply(&mut registry, &[Stop(west)]);
    assert_eq!(
        (assigned(&registry, e), assigned(&registry, w)),
        (Some(east), None)
    );
    let before = (state(&registry, east), state(&registry, west));
    apply(&mut registry, &[Stop(west)]);
    assert_eq!(
        (state(&registry, east), state(&registry, west)),
        before,
        "stop on a stopped pool is a no-op"
    );
}

// A3 (assignment half) and A6: starting B over A's members moves them to B;
// stopping B returns them to the default pool, not to A; a start on running A
// moves them back without a new run. Two starts in one drain assign in issue
// order (P15), and a start then stop of one pool leaves it stopped.
#[test]
fn latest_pool_assignment_wins() {
    use AttackPoolRequest::{Start, Stop};
    let mut registry = EntityRegistry::new();
    let (calm, frenzy) = (
        pool(&mut registry, "calm", "guard"),
        pool(&mut registry, "frenzy", "guard"),
    );
    let guard = enemy(&mut registry, &["guard"]);

    apply(&mut registry, &[Start(calm)]);
    assert_eq!(assigned(&registry, guard), Some(calm));
    apply(&mut registry, &[Start(frenzy)]);
    assert_eq!(assigned(&registry, guard), Some(frenzy), "B escalates A");
    assert!(
        state(&registry, calm).running,
        "the first pool keeps running"
    );
    apply(&mut registry, &[Stop(frenzy)]);
    assert_eq!(
        assigned(&registry, guard),
        None,
        "stopping B returns its members to the default pool, not to A"
    );
    let run = state(&registry, calm).run;
    apply(&mut registry, &[Start(calm)]);
    assert_eq!(assigned(&registry, guard), Some(calm));
    assert_eq!(
        state(&registry, calm).run,
        run,
        "a start on a running pool keeps its run"
    );

    apply(&mut registry, &[Start(calm), Start(frenzy)]);
    assert_eq!(assigned(&registry, guard), Some(frenzy));
    apply(&mut registry, &[Start(frenzy), Start(calm)]);
    assert_eq!(
        assigned(&registry, guard),
        Some(calm),
        "the later-issued start assigns the shared member"
    );

    apply(&mut registry, &[Start(frenzy), Stop(frenzy)]);
    assert!(
        !state(&registry, frenzy).running,
        "start then stop of one pool on one tick leaves it stopped"
    );
    assert_eq!(
        assigned(&registry, guard),
        None,
        "the start assigned the guard, and the stop returned it to the default pool"
    );
    apply(&mut registry, &[Stop(calm), Start(calm)]);
    assert!(state(&registry, calm).running);
    assert_eq!(
        state(&registry, calm).run,
        run + 1,
        "a stop then start begins a new run"
    );
}

/// Serialize the pools' and enemies' components and restore them into a fresh
/// registry under the same ids, as a level save would.
fn restored(registry: &EntityRegistry, ids: &[EntityId]) -> EntityRegistry {
    let mut fresh = EntityRegistry::new();
    for &id in ids {
        let tags = registry.get_tags(id).unwrap().to_vec();
        let restored = fresh.try_spawn(Transform::default(), &tags).unwrap();
        assert_eq!(restored, id, "the fresh registry reuses each slot in order");
        if let Ok(pool) = registry.get_component::<AttackPoolComponent>(id) {
            let json = serde_json::to_string(&ComponentValue::AttackPool(pool.clone())).unwrap();
            fresh
                .set_component_value(id, serde_json::from_str(&json).unwrap())
                .unwrap();
        }
        if let Ok(brain) = registry.get_component::<BrainComponent>(id) {
            let json = serde_json::to_string(&ComponentValue::Brain(brain.clone())).unwrap();
            fresh
                .set_component_value(id, serde_json::from_str(&json).unwrap())
                .unwrap();
        }
    }
    fresh
}

// P33: a spawn whose tags match two running pools joins the one started or
// re-started latest. A start on the spawn's tick counts in either order, and a
// restore between the starts and the spawn joins the same pool.
#[test]
fn spawn_matching_two_pools_joins_latest_started() {
    use AttackPoolRequest::{Join, Start};
    let mut registry = EntityRegistry::new();
    let (a, b) = (
        pool(&mut registry, "a", "guard_a"),
        pool(&mut registry, "b", "guard_b"),
    );
    apply(&mut registry, &[Start(a), Start(b)]);
    let both = ["guard_a", "guard_b"];

    let first = enemy(&mut registry, &both);
    apply(&mut registry, &[Join(first)]);
    assert_eq!(assigned(&registry, first), Some(b), "B started later");

    let mut restored = restored(&registry, &[a, b, first]);
    let after_restore = enemy(&mut restored, &both);
    apply(&mut restored, &[Join(after_restore)]);
    assert_eq!(
        assigned(&restored, after_restore),
        Some(b),
        "start order survives a restore"
    );
    assert_eq!(assigned(&restored, first), Some(b), "so does assignment");

    apply(&mut registry, &[Start(a)]);
    let second = enemy(&mut registry, &both);
    apply(&mut registry, &[Join(second)]);
    assert_eq!(
        assigned(&registry, second),
        Some(a),
        "a re-start ranks A latest"
    );

    for spawn_first in [true, false] {
        let mut registry = EntityRegistry::new();
        let (a, b) = (
            pool(&mut registry, "a", "guard_a"),
            pool(&mut registry, "b", "guard_b"),
        );
        apply(&mut registry, &[Start(a)]);
        let spawn = enemy(&mut registry, &both);
        let requests = if spawn_first {
            [Join(spawn), Start(b)]
        } else {
            [Start(b), Join(spawn)]
        };
        apply(&mut registry, &requests);
        assert_eq!(
            assigned(&registry, spawn),
            Some(b),
            "a start on the spawn's tick counts in either order"
        );
    }
}

// P34 at the drain: pool requests and spawn joins apply in issue order. A
// spawn before B's stop joins B and returns to the default pool with B's
// members; a spawn after it joins A.
#[test]
fn spawn_and_stop_in_one_drain_apply_in_issue_order() {
    use AttackPoolRequest::{Join, Start, Stop};
    for spawn_first in [true, false] {
        let mut registry = EntityRegistry::new();
        let (a, b) = (
            pool(&mut registry, "a", "guard_a"),
            pool(&mut registry, "b", "guard_b"),
        );
        apply(&mut registry, &[Start(a), Start(b)]);
        let spawn = enemy(&mut registry, &["guard_a", "guard_b"]);
        let requests = if spawn_first {
            [Join(spawn), Stop(b)]
        } else {
            [Stop(b), Join(spawn)]
        };
        apply(&mut registry, &requests);
        let expected = if spawn_first { None } else { Some(a) };
        assert_eq!(
            assigned(&registry, spawn),
            expected,
            "spawn first: {spawn_first}"
        );
    }
}

// A3 (client half): every pool step is host-only. A connected client issues
// nothing, so its drain leaves the pool stopped and its members unassigned.
#[test]
fn attack_pool_start_on_client_changes_nothing() {
    let ctx = postretro_entities::ScriptCtx::new();
    let (arena, guard) = {
        let mut registry = ctx.registry.borrow_mut();
        let arena = pool(&mut registry, "arena", "guard");
        (arena, enemy(&mut registry, &["guard"]))
    };
    let mut steps = SequencedPrimitiveRegistry::new();
    register_sequenced_attack_pool_primitives(&mut steps, ctx.clone());
    let run = |ctx: &postretro_entities::ScriptCtx| {
        steps
            .get(postretro_foundation::START_ATTACK_POOL)
            .expect("the start step is registered")(arena, &serde_json::Value::Null)
        .unwrap();
        apply_attack_pool_requests(&mut ctx.registry.borrow_mut());
    };

    ctx.owner_slot_writes_enabled.set(false);
    run(&ctx);
    assert!(!state(&ctx.registry.borrow(), arena).running);
    assert_eq!(assigned(&ctx.registry.borrow(), guard), None);

    ctx.owner_slot_writes_enabled.set(true);
    run(&ctx);
    assert!(
        state(&ctx.registry.borrow(), arena).running,
        "the host applies it"
    );
    assert_eq!(assigned(&ctx.registry.borrow(), guard), Some(arena));
}

// The drain with nothing queued, and with a start on a running pool, touches
// no heap once warm: membership never rides a per-tick allocation.
#[test]
fn an_idle_drain_allocates_nothing() {
    let mut registry = EntityRegistry::new();
    let arena = pool(&mut registry, "arena", "guard");
    for _ in 0..64 {
        enemy(&mut registry, &["guard"]);
    }
    apply(&mut registry, &[AttackPoolRequest::Start(arena)]);
    apply_attack_pool_requests(&mut registry);
    let snapshot = crate::alloc_probe::AllocSnapshot::arm();
    for _ in 0..8 {
        apply_attack_pool_requests(&mut registry);
    }
    assert_eq!(
        snapshot.allocs_since(),
        0,
        "an empty drain must not allocate"
    );
}
