// The refresh ledger and pool profiles at the runtime seam: where a
// refreshing count lands, what drops it, and which profile each running pool
// arbitrates under. Through-the-tick refresh proofs live in `tick/refresh.rs`.
// See: context/lib/entity_model.md §7c (Attack budget)

use std::sync::Arc;

use postretro_entities::components::brain::{AttackTurnState, HeldTurn};
use postretro_foundation::attack_budget::AttackBudgetVocabulary;
use serde_json::json;

use super::*;
use crate::attack_turns::{AttackTurnRuntime, RunningPool};

const DT_MS: f32 = 16.0;
/// Registered ids: `defaultToken` is 0, then the mod's tokens in order.
const MELEE_ID: AttackTokenId = AttackTokenId(1);
const HEAVY_ID: AttackTokenId = AttackTokenId(2);

/// `melee` and `heavy` with `budgets` (`attackBudgets`), as a committed
/// manifest registers them.
fn vocabulary(heavy_refresh_ms: f32, budgets: serde_json::Value) -> Arc<AttackBudgetVocabulary> {
    Arc::new(
        AttackBudgetVocabulary::from_manifest(
            Some(&json!([
                { "kind": "attackToken", "name": "melee", "budget": { "limit": 3 } },
                { "kind": "attackToken", "name": "heavy",
                  "budget": { "limit": 1, "refreshMs": heavy_refresh_ms } },
            ])),
            Some(&budgets),
        )
        .expect("a valid registration"),
    )
}

fn override_ref(token: &str, fields: serde_json::Value) -> serde_json::Value {
    let mut entry = json!({ "token": { "kind": "attackToken", "name": token } });
    for (key, value) in fields.as_object().expect("override fields") {
        entry[key] = value.clone();
    }
    entry
}

fn running(pool: EntityId, profile: &str, run: u64) -> RunningPool<'_> {
    RunningPool { pool, profile, run }
}

/// A turn on `target` in `pool` drawing one `heavy`.
fn heavy_turn(target: EntityId, pool: AttackPoolId) -> AttackTurnState {
    let mut turns = AttackTurnState::default();
    assert!(turns.hold(HeldTurn {
        attack: 0,
        target,
        pool,
        draws: draws(&[(HEAVY_ID.0, 1)]),
        until_cooldown: true,
    }));
    turns
}

// B15 / P30, pool half: a refreshing count lands in the pool its turn counted
// against when it ended and never counts in another. A `start` on a running
// pool keeps it; a stop drops it, so a restart begins open, even when the
// stop and the restart land between two AI ticks. Assigning members to pools
// through the tick is `ai_attack_pool`'s.
#[test]
fn refresh_lands_in_pool_at_turn_end_and_drops_on_stop() {
    let mut registry = EntityRegistry::new();
    let (target, holder, arena) = (
        registry.spawn(Transform::default()),
        registry.spawn(Transform::default()),
        registry.spawn(Transform::default()),
    );
    registry.set_attack_budgets(vocabulary(4000.0, json!({})));
    let mut runtime = AttackTurnRuntime::default();
    let pool = AttackPoolId::Pool(arena);

    let tick = |runtime: &mut AttackTurnRuntime, pools: &[RunningPool<'_>]| {
        runtime.begin_tick(DT_MS, &registry);
        runtime.sync_pools(pools.iter().copied());
    };
    tick(&mut runtime, &[running(arena, "", 1)]);
    let mut turns = heavy_turn(target, pool);
    runtime.release_unstarted_grant(holder, &mut turns, 0);
    assert!(!turns.holds_any());
    assert_eq!(runtime.refreshing_now((pool, target), HEAVY_ID), 1);
    assert_eq!(
        runtime.refreshing_now((AttackPoolId::Default, target), HEAVY_ID),
        0,
        "it counts only in the pool it ended in"
    );

    tick(&mut runtime, &[running(arena, "", 1)]);
    assert_eq!(
        runtime.refreshing_now((pool, target), HEAVY_ID),
        1,
        "a start on a running pool leaves its refreshing counts"
    );
    tick(&mut runtime, &[]);
    tick(&mut runtime, &[running(arena, "", 2)]);
    assert_eq!(
        runtime.refreshing_now((pool, target), HEAVY_ID),
        0,
        "a stop drops them; the restart begins open"
    );

    let mut turns = heavy_turn(target, pool);
    runtime.release_unstarted_grant(holder, &mut turns, 0);
    assert_eq!(runtime.refreshing_now((pool, target), HEAVY_ID), 1);
    tick(&mut runtime, &[running(arena, "", 3)]);
    assert_eq!(
        runtime.refreshing_now((pool, target), HEAVY_ID),
        0,
        "a stop and restart between ticks begins a new run too"
    );
}

// B17: a profile changes only the fields it names. `melee.with({ limit: 1 })`
// keeps `melee`'s declared `refreshMs`; `heavy`, left out, keeps its declared
// budget. A pool reads its own profile; an empty or undeclared one reads
// `default`.
#[test]
fn pool_profile_override_keeps_unnamed_fields_and_tokens() {
    let ids = targets(3);
    let mut registry = EntityRegistry::new();
    registry.set_attack_budgets(Arc::new(
        AttackBudgetVocabulary::from_manifest(
            Some(&json!([
                { "kind": "attackToken", "name": "melee",
                  "budget": { "limit": 3, "refreshMs": 500 } },
                { "kind": "attackToken", "name": "heavy",
                  "budget": { "limit": 2, "refreshMs": 4000 } },
            ])),
            Some(&json!({ "arena": [override_ref("melee", json!({ "limit": 1 }))] })),
        )
        .expect("a valid registration"),
    ));
    let mut runtime = AttackTurnRuntime::default();
    runtime.begin_tick(DT_MS, &registry);
    runtime.sync_pools(
        [
            running(ids[0], "arena", 1),
            running(ids[1], "", 1),
            running(ids[2], "missing", 1),
        ]
        .into_iter(),
    );

    let arena = runtime.budgets.for_pool(AttackPoolId::Pool(ids[0]));
    assert_eq!(arena.limit(MELEE_ID), Some(1));
    assert_eq!(
        arena.refresh_ms(MELEE_ID),
        500.0,
        "melee keeps its refreshMs"
    );
    assert_eq!(arena.limit(HEAVY_ID), Some(2), "heavy keeps its budget");
    assert_eq!(arena.refresh_ms(HEAVY_ID), 4000.0);
    assert_eq!(arena.limit(AttackTokenId(0)), Some(3), "defaultToken too");
    for pool in [
        AttackPoolId::Default,
        AttackPoolId::Pool(ids[1]),
        AttackPoolId::Pool(ids[2]),
    ] {
        assert_eq!(
            runtime.budgets.for_pool(pool).limit(MELEE_ID),
            Some(3),
            "{pool:?} reads the declared budgets as `default`"
        );
    }
}

// B26 (profile half) and A12 (engine half): a reload that removes a running
// pool's profile makes it use `default` from the next tick, and a reload that
// changes `default` reaches the default pool and every pool using `default`.
#[test]
fn reload_removing_a_pool_profile_falls_back_to_default_next_tick() {
    let ids = targets(2);
    let (arena, calm) = (ids[0], ids[1]);
    let mut registry = EntityRegistry::new();
    registry.set_attack_budgets(vocabulary(
        0.0,
        json!({ "arena": [override_ref("melee", json!({ "limit": 1 }))] }),
    ));
    let mut runtime = AttackTurnRuntime::default();
    let pools = [running(arena, "arena", 1), running(calm, "default", 1)];
    runtime.begin_tick(DT_MS, &registry);
    runtime.sync_pools(pools.iter().copied());
    let limit = |runtime: &AttackTurnRuntime, pool: AttackPoolId| {
        runtime.budgets.for_pool(pool).limit(MELEE_ID)
    };
    assert_eq!(limit(&runtime, AttackPoolId::Pool(arena)), Some(1));

    registry.set_attack_budgets(vocabulary(
        0.0,
        json!({ "default": [override_ref("melee", json!({ "limit": 2 }))] }),
    ));
    assert_eq!(
        limit(&runtime, AttackPoolId::Pool(arena)),
        Some(1),
        "a commit applies from the next tick"
    );
    runtime.begin_tick(DT_MS, &registry);
    runtime.sync_pools(pools.iter().copied());
    for pool in [
        AttackPoolId::Pool(arena),
        AttackPoolId::Pool(calm),
        AttackPoolId::Default,
    ] {
        assert_eq!(
            limit(&runtime, pool),
            Some(2),
            "{pool:?} reads the reloaded `default`"
        );
    }
}

// A reload that shifts token ids (a token inserted before `heavy`) keeps a
// refreshing count on `heavy`, now under its new id.
#[test]
fn reload_shifting_token_ids_keeps_refreshing_counts_by_name() {
    let mut registry = EntityRegistry::new();
    let (target, holder) = (
        registry.spawn(Transform::default()),
        registry.spawn(Transform::default()),
    );
    registry.set_attack_budgets(vocabulary(4000.0, json!({})));
    let mut runtime = AttackTurnRuntime::default();
    runtime.begin_tick(DT_MS, &registry);
    let group = (AttackPoolId::Default, target);
    let mut turns = heavy_turn(target, AttackPoolId::Default);
    runtime.release_unstarted_grant(holder, &mut turns, 0);
    assert_eq!(runtime.refreshing_now(group, HEAVY_ID), 1);

    registry.set_attack_budgets(Arc::new(
        AttackBudgetVocabulary::from_manifest(
            Some(&json!([
                { "kind": "attackToken", "name": "lobber", "budget": { "limit": 1 } },
                { "kind": "attackToken", "name": "melee", "budget": { "limit": 3 } },
                { "kind": "attackToken", "name": "heavy",
                  "budget": { "limit": 1, "refreshMs": 4000 } },
            ])),
            None,
        )
        .expect("a valid registration"),
    ));
    runtime.begin_tick(DT_MS, &registry);
    assert_eq!(runtime.refreshing_now(group, AttackTokenId(3)), 1);
    assert_eq!(
        runtime.refreshing_now(group, HEAVY_ID),
        0,
        "id 2 now names `melee`"
    );
}
