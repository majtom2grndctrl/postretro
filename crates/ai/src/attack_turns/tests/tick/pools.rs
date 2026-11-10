// Attack pools through the tick: the sim's pre-AI drain assigns members, and
// each running pool arbitrates under its own profile beside the default pool
// (rows A1–A4, A7, A9, A11, A12; pins P11, P13, P26 and P30 across pool changes).
// See: context/lib/entity_model.md §7c (Attack budget, Attack pools)

use postretro_entities::components::attack_pool::AttackPoolComponent;
use postretro_entities::components::brain::AttackPoolId;
use postretro_sim::attack_pools::{
    AttackPoolVerb, apply_attack_pool_requests, issue_attack_pool_request,
};
use serde_json::json;

use super::*;

/// Registered ids: `defaultToken` is 0, then `melee`, then `heavy`.
const MELEE_ID: AttackTokenId = AttackTokenId(1);
const HEAVY_ID: AttackTokenId = AttackTokenId(2);

/// Declare `melee` and `heavy` as (limit, refreshMs), with `budgets` as the
/// mod's `attackBudgets`, and commit them as a manifest does.
fn install_budgets(
    reg: &mut EntityRegistry,
    melee: (u32, f32),
    heavy: (u32, f32),
    budgets: serde_json::Value,
) {
    let declare = |name: &str, (limit, refresh_ms): (u32, f32)| {
        json!({ "kind": "attackToken", "name": name,
                "budget": { "limit": limit, "refreshMs": refresh_ms } })
    };
    let vocabulary = AttackBudgetVocabulary::from_manifest(
        Some(&json!([declare("melee", melee), declare("heavy", heavy)])),
        Some(&budgets),
    )
    .expect("a valid registration");
    reg.set_attack_budgets(std::sync::Arc::new(vocabulary));
}

/// `token.with({ limit })` in a profile, as it crosses.
fn limit(token: &str, limit: u32) -> serde_json::Value {
    json!({ "token": { "kind": "attackToken", "name": token }, "limit": limit })
}

/// A placed `ai_attack_pool` reached by `tag`, stopped, as a level installs it.
fn spawn_pool(reg: &mut EntityRegistry, tag: &str, member_tag: &str, profile: &str) -> EntityId {
    let pool = reg
        .try_spawn(Transform::default(), &[tag.to_string()])
        .unwrap();
    reg.set_component(
        pool,
        AttackPoolComponent::new(member_tag.to_string(), profile.to_string()),
    )
    .unwrap();
    pool
}

fn tag(reg: &mut EntityRegistry, enemy: EntityId, tag: &str) {
    reg.set_tags(enemy, vec![tag.to_string()]).unwrap();
}

fn start(reg: &mut EntityRegistry, pool: EntityId) {
    issue_attack_pool_request(reg, pool, AttackPoolVerb::Start);
}

fn stop(reg: &mut EntityRegistry, pool: EntityId) {
    issue_attack_pool_request(reg, pool, AttackPoolVerb::Stop);
}

/// One authoritative tick as the sim runs it: pool requests drain, then the AI.
fn pool_tick(reg: &mut EntityRegistry, runtime: &mut AiRuntime) {
    apply_attack_pool_requests(reg);
    tick(reg, runtime);
}

/// The pool each of `enemy`'s held turns counts in.
fn held_pools(reg: &EntityRegistry, enemy: EntityId) -> Vec<AttackPoolId> {
    turns(reg, enemy).held().map(|turn| turn.pool).collect()
}

fn jabber() -> BehaviorGraphDescriptor {
    marked_graph(vec![("jab", held_attack(vec![token("melee")]))])
}

fn slam_and_jab() -> BehaviorGraphDescriptor {
    marked_graph(vec![
        ("slam", held_attack(vec![token("melee"), token("heavy")])),
        ("jab", held_attack(vec![token("melee")])),
    ])
}

fn refreshing(runtime: &AiRuntime, pool: AttackPoolId, target: EntityId) -> u32 {
    runtime.turns.refreshing_now((pool, target), HEAVY_ID)
}

fn melee_refreshing(runtime: &AiRuntime, pool: AttackPoolId, target: EntityId) -> u32 {
    runtime.turns.refreshing_now((pool, target), MELEE_ID)
}

// A1 (arbitration half): a running pool's members take turns under its
// profile's limits; an empty profile and an undeclared one both use
// `default`. The trigger and script starts that run it are proven at the sim
// seam (`attack_pool_start_by_trigger_and_script_applies_profile`).
#[test]
fn running_pool_arbitrates_under_its_profile() {
    for (profile, limit_in_pool) in [("arena", 1), ("", 3), ("missing", 3)] {
        let mut reg = EntityRegistry::new();
        install_budgets(
            &mut reg,
            (3, 0.0),
            (1, 0.0),
            json!({ "arena": [limit("melee", 1)] }),
        );
        let mut runtime = AiRuntime::new();
        spawn_player(&mut reg, player_spot());
        let graph = jabber();
        let members: Vec<EntityId> = (0..4)
            .map(|index| {
                let enemy = spawn_at(&mut reg, index, &graph, "jab_act");
                tag(&mut reg, enemy, "guard");
                enemy
            })
            .collect();
        let pool = spawn_pool(&mut reg, "arena", "guard", profile);
        start(&mut reg, pool);
        pool_tick(&mut reg, &mut runtime);
        assert_eq!(
            holders(&reg, &members),
            limit_in_pool,
            "profile `{profile}`: members take turns at its melee limit"
        );
        for &member in &members {
            assert!(
                held_pools(&reg, member)
                    .iter()
                    .all(|&counted| counted == AttackPoolId::Pool(pool)),
                "a member's turn counts in its pool"
            );
        }
    }
}

// A2: three pool members and three non-members ready on one player. The pool
// at limit 1 and the default pool at limit 2 grant exactly one member and two
// non-members. Then a head-of-line waiter in the pool, reserving the whole
// token there, never blocks a grant in the default pool.
#[test]
fn attack_pool_and_default_pool_arbitrate_independently() {
    let mut reg = EntityRegistry::new();
    install_budgets(
        &mut reg,
        (2, 0.0),
        (1, 0.0),
        json!({ "arena": [limit("melee", 1)] }),
    );
    let mut runtime = AiRuntime::new();
    spawn_player(&mut reg, player_spot());
    let graph = marked_graph(vec![
        ("jab", held_attack(vec![token("melee")])),
        ("crush", held_attack(vec![costing("melee", 2)])),
    ]);
    let members: Vec<EntityId> = (0..3)
        .map(|index| {
            let enemy = spawn_at(&mut reg, index, &graph, "jab_act");
            tag(&mut reg, enemy, "guard");
            enemy
        })
        .collect();
    let others: Vec<EntityId> = (3..6)
        .map(|index| spawn_at(&mut reg, index, &graph, "jab_act"))
        .collect();
    let pool = spawn_pool(&mut reg, "arena", "guard", "arena");
    start(&mut reg, pool);
    pool_tick(&mut reg, &mut runtime);
    assert_eq!(
        holders(&reg, &members),
        1,
        "one member at the pool's limit 1"
    );
    assert_eq!(
        holders(&reg, &others),
        2,
        "two non-members at default's limit 2"
    );

    // A waiting member switches to a cost-2 crush, keeping its clock: it heads
    // melee in the pool and reserves all of it. It is ahead of the waiting
    // non-member by clock and slot, so one shared line would hold that
    // non-member back; in its own pool it is granted the tick room frees.
    let crusher = *members
        .iter()
        .find(|&&member| held(&reg, member).is_empty())
        .unwrap();
    enter(&mut reg, crusher, "crush_act");
    pool_tick(&mut reg, &mut runtime);
    assert_eq!(requested(&reg, crusher).as_deref(), Some("crush"));
    let waiting_other = *others
        .iter()
        .find(|&&other| held(&reg, other).is_empty())
        .unwrap();
    assert!(
        wait_since(&reg, crusher) <= wait_since(&reg, waiting_other) && crusher < waiting_other,
        "fixture: the crusher would head one shared line"
    );
    let freed = *others
        .iter()
        .find(|&&other| !held(&reg, other).is_empty())
        .unwrap();
    enter(&mut reg, freed, "idle");
    pool_tick(&mut reg, &mut runtime);
    assert_eq!(
        held(&reg, waiting_other),
        vec!["jab"],
        "the pool's head-of-line crusher blocks nothing in the default pool"
    );
    assert!(
        held(&reg, crusher).is_empty(),
        "the crusher still waits in its pool"
    );
}

// A3: with "calm" running and "frenzy" started over its members, stopping
// "frenzy" returns them to the default pool, and `start` on "calm" moves them
// back. Through every move the held turn, the waiter's clock and calm's
// refreshing count stand.
#[test]
fn restarting_a_running_pool_reassigns_tag_holders() {
    let mut reg = EntityRegistry::new();
    install_budgets(&mut reg, (3, 4000.0), (1, 0.0), json!({}));
    let mut runtime = AiRuntime::new();
    let pawn = spawn_player(&mut reg, player_spot());
    let graph = slam_and_jab();
    let spent = spawn_at(&mut reg, 0, &graph, "jab_act");
    let holder = spawn_at(&mut reg, 1, &graph, "slam_act");
    let waiter = spawn_at(&mut reg, 2, &graph, "idle");
    for enemy in [spent, holder, waiter] {
        tag(&mut reg, enemy, "guard");
    }
    let calm = spawn_pool(&mut reg, "calm", "guard", "");
    let frenzy = spawn_pool(&mut reg, "frenzy", "guard", "");
    start(&mut reg, calm);
    pool_tick(&mut reg, &mut runtime);
    assert_eq!(held(&reg, spent), vec!["jab"]);
    assert_eq!(held(&reg, holder), vec!["slam"]);
    // The jab ends in calm and its melee refreshes there; the waiter asks for
    // a slam behind the holder's heavy.
    enter(&mut reg, spent, "idle");
    enter(&mut reg, waiter, "slam_act");
    pool_tick(&mut reg, &mut runtime);
    assert_eq!(
        melee_refreshing(&runtime, AttackPoolId::Pool(calm), pawn),
        1
    );
    let since = wait_since(&reg, waiter);
    assert!(since.is_some(), "fixture: the waiter waits on heavy");

    let check = |reg: &mut EntityRegistry, runtime: &mut AiRuntime, pool: AttackPoolId| {
        pool_tick(reg, runtime);
        assert_eq!(
            held_pools(reg, holder),
            vec![pool],
            "the held turn counts in {pool:?}"
        );
        assert_eq!(
            wait_since(reg, waiter),
            since,
            "the wait clock stands in {pool:?}"
        );
        assert!(held(reg, waiter).is_empty());
    };
    start(&mut reg, frenzy);
    check(&mut reg, &mut runtime, AttackPoolId::Pool(frenzy));
    stop(&mut reg, frenzy);
    check(&mut reg, &mut runtime, AttackPoolId::Default);
    start(&mut reg, calm);
    check(&mut reg, &mut runtime, AttackPoolId::Pool(calm));
    assert_eq!(
        melee_refreshing(&runtime, AttackPoolId::Pool(calm), pawn),
        1,
        "a start on running calm leaves its refreshing count"
    );
}

// A4: two pools run on one level, each member taking turns in its own pool.
// Stopping one returns its members to the default pool on that tick without
// touching the other, and `stop` on a stopped pool changes nothing.
#[test]
fn two_pools_run_side_by_side_and_stop_independently() {
    let mut reg = EntityRegistry::new();
    install_budgets(
        &mut reg,
        (3, 0.0),
        (1, 0.0),
        json!({ "duel": [limit("melee", 1)] }),
    );
    let mut runtime = AiRuntime::new();
    spawn_player(&mut reg, player_spot());
    let graph = jabber();
    let squad = |reg: &mut EntityRegistry, first: usize, member_tag: &str| -> Vec<EntityId> {
        (first..first + 2)
            .map(|index| {
                let enemy = spawn_at(reg, index, &graph, "jab_act");
                tag(reg, enemy, member_tag);
                enemy
            })
            .collect()
    };
    let east_guards = squad(&mut reg, 0, "east_guard");
    let west_guards = squad(&mut reg, 2, "west_guard");
    let east = spawn_pool(&mut reg, "east", "east_guard", "duel");
    let west = spawn_pool(&mut reg, "west", "west_guard", "duel");
    start(&mut reg, east);
    start(&mut reg, west);
    pool_tick(&mut reg, &mut runtime);
    assert_eq!(holders(&reg, &east_guards), 1, "east grants one at limit 1");
    assert_eq!(holders(&reg, &west_guards), 1, "west grants one at limit 1");

    stop(&mut reg, west);
    pool_tick(&mut reg, &mut runtime);
    assert_eq!(
        holders(&reg, &west_guards),
        2,
        "west's members take turns in the default pool from the stop's tick"
    );
    for &guard in &west_guards {
        assert_eq!(held_pools(&reg, guard), vec![AttackPoolId::Default]);
    }
    assert_eq!(holders(&reg, &east_guards), 1, "east is untouched");

    let before: Vec<AttackTurnState> = east_guards
        .iter()
        .chain(&west_guards)
        .map(|&enemy| turns(&reg, enemy))
        .collect();
    stop(&mut reg, west);
    pool_tick(&mut reg, &mut runtime);
    let after: Vec<AttackTurnState> = east_guards
        .iter()
        .chain(&west_guards)
        .map(|&enemy| turns(&reg, enemy))
        .collect();
    let held_only = |states: &[AttackTurnState]| -> Vec<Vec<AttackPoolId>> {
        states
            .iter()
            .map(|state| state.held().map(|turn| turn.pool).collect())
            .collect()
    };
    assert_eq!(
        held_only(&before),
        held_only(&after),
        "stop on a stopped pool changes nothing"
    );
}

// A7 / P13: a pool at limit 1 starts while two cost-1 members hold
// default-pool turns on one target. Neither turn is revoked; both count
// against the pool, so no grant there lands until both have ended. A stop
// while members hold the pool's turns revokes nothing either.
#[test]
fn enemy_changing_pool_keeps_turn_counted_in_new_pool() {
    let mut reg = EntityRegistry::new();
    install_budgets(
        &mut reg,
        (2, 0.0),
        (1, 0.0),
        json!({ "arena": [limit("melee", 1)] }),
    );
    let mut runtime = AiRuntime::new();
    spawn_player(&mut reg, player_spot());
    let graph = jabber();
    let first = spawn_at(&mut reg, 0, &graph, "jab_act");
    let second = spawn_at(&mut reg, 1, &graph, "jab_act");
    let late = spawn_at(&mut reg, 2, &graph, "idle");
    for enemy in [first, second, late] {
        tag(&mut reg, enemy, "guard");
    }
    let pool = spawn_pool(&mut reg, "arena", "guard", "arena");
    pool_tick(&mut reg, &mut runtime);
    assert_eq!(held_pools(&reg, first), vec![AttackPoolId::Default]);
    assert_eq!(held_pools(&reg, second), vec![AttackPoolId::Default]);

    start(&mut reg, pool);
    enter(&mut reg, late, "jab_act");
    pool_tick(&mut reg, &mut runtime);
    for enemy in [first, second] {
        assert_eq!(
            held_pools(&reg, enemy),
            vec![AttackPoolId::Pool(pool)],
            "the turn stands and counts in the new pool"
        );
    }
    assert!(held(&reg, late).is_empty(), "two counted against limit 1");
    enter(&mut reg, first, "idle");
    pool_tick(&mut reg, &mut runtime);
    assert!(
        held(&reg, late).is_empty(),
        "one still counted: no grant fits around it"
    );
    enter(&mut reg, second, "idle");
    pool_tick(&mut reg, &mut runtime);
    assert_eq!(
        held(&reg, late),
        vec!["jab"],
        "granted once nothing is counted"
    );

    stop(&mut reg, pool);
    pool_tick(&mut reg, &mut runtime);
    assert_eq!(
        held_pools(&reg, late),
        vec![AttackPoolId::Default],
        "a stop revokes nothing; the turn counts in the default pool"
    );
}

// P26 / P30 across a pool change: a turn granted in the default pool that
// ends after its holder joined pool A refreshes in A, never in the default
// pool. A refreshing count stays with the pool it landed in when its holder
// changes pool, and stopping and restarting A drops A's.
#[test]
fn refresh_lands_in_the_pool_a_turn_ends_in_and_stays_there() {
    let mut reg = EntityRegistry::new();
    install_budgets(&mut reg, (3, 0.0), (1, 4000.0), json!({}));
    let mut runtime = AiRuntime::new();
    let pawn = spawn_player(&mut reg, player_spot());
    let graph = slam_and_jab();
    let holder = spawn_at(&mut reg, 0, &graph, "slam_act");
    let member = spawn_at(&mut reg, 1, &graph, "idle");
    let outsider = spawn_at(&mut reg, 2, &graph, "idle");
    tag(&mut reg, holder, "guard");
    tag(&mut reg, member, "guard");
    let arena = spawn_pool(&mut reg, "arena", "guard", "");
    let in_arena = AttackPoolId::Pool(arena);

    pool_tick(&mut reg, &mut runtime);
    assert_eq!(held_pools(&reg, holder), vec![AttackPoolId::Default]);
    start(&mut reg, arena);
    pool_tick(&mut reg, &mut runtime);
    assert_eq!(
        held_pools(&reg, holder),
        vec![in_arena],
        "the holder joined A"
    );

    // The default-pool grant ends in A.
    enter(&mut reg, holder, "idle");
    pool_tick(&mut reg, &mut runtime);
    assert_eq!(refreshing(&runtime, in_arena, pawn), 1, "it refreshes in A");
    assert_eq!(refreshing(&runtime, AttackPoolId::Default, pawn), 0);
    enter(&mut reg, member, "slam_act");
    enter(&mut reg, outsider, "slam_act");
    pool_tick(&mut reg, &mut runtime);
    assert_eq!(
        held(&reg, outsider),
        vec!["slam"],
        "the default pool's heavy is open"
    );
    assert!(held(&reg, member).is_empty(), "A's heavy is refreshing");

    // The outsider's slam ends in the default pool; then it joins A. Its
    // count stays in the default pool.
    enter(&mut reg, outsider, "idle");
    pool_tick(&mut reg, &mut runtime);
    assert_eq!(refreshing(&runtime, AttackPoolId::Default, pawn), 1);
    tag(&mut reg, outsider, "guard");
    start(&mut reg, arena);
    pool_tick(&mut reg, &mut runtime);
    assert_eq!(
        refreshing(&runtime, AttackPoolId::Default, pawn),
        1,
        "a count stays with its pool across its holder's pool change"
    );
    assert_eq!(refreshing(&runtime, in_arena, pawn), 1);

    // A stop and restart drops A's count, so A begins open.
    stop(&mut reg, arena);
    start(&mut reg, arena);
    pool_tick(&mut reg, &mut runtime);
    assert_eq!(
        refreshing(&runtime, in_arena, pawn),
        0,
        "a new run begins open"
    );
    assert_eq!(
        held(&reg, member),
        vec!["slam"],
        "granted in A on the restart's tick"
    );
    assert_eq!(
        refreshing(&runtime, AttackPoolId::Default, pawn),
        1,
        "A's restart never touches the default pool's count"
    );
}

// P30 for a departed holder: a holder leaving the AI set on the tick its pool
// stops, or stops and restarts in one drain, starts no refresh there, since
// the stop dropped that pool's counts. Regression: its refresh landed
// orphaned in the stopped pool, or leaked into the new run.
#[test]
fn departed_holder_refresh_skips_a_stopped_or_restarted_pool() {
    for restart in [false, true] {
        let mut reg = EntityRegistry::new();
        install_budgets(&mut reg, (3, 0.0), (1, 4000.0), json!({}));
        let mut runtime = AiRuntime::new();
        let pawn = spawn_player(&mut reg, player_spot());
        let holder = spawn_at(&mut reg, 0, &slam_and_jab(), "slam_act");
        tag(&mut reg, holder, "guard");
        let arena = spawn_pool(&mut reg, "arena", "guard", "");
        let in_arena = AttackPoolId::Pool(arena);
        start(&mut reg, arena);
        pool_tick(&mut reg, &mut runtime);
        assert_eq!(held_pools(&reg, holder), vec![in_arena], "fixture");

        // The holder leaves the AI set on the drain's tick.
        set_hp(&mut reg, holder, 0.0);
        stop(&mut reg, arena);
        if restart {
            start(&mut reg, arena);
        }
        pool_tick(&mut reg, &mut runtime);
        assert_eq!(
            refreshing(&runtime, in_arena, pawn),
            0,
            "restart {restart}: nothing of the old run refreshes in A"
        );
        assert_eq!(
            runtime.turns.refreshing_entries(),
            0,
            "restart {restart}: and no orphaned count is kept"
        );
    }
}

// A9: with no `ai_attack_pool`, the feed is empty and every turn counts in
// the default pool.
#[test]
fn level_without_attack_pool_uses_default_pool() {
    let mut reg = EntityRegistry::new();
    install_budgets(&mut reg, (2, 0.0), (1, 0.0), json!({}));
    let mut runtime = AiRuntime::new();
    spawn_player(&mut reg, player_spot());
    let graph = jabber();
    let enemies: Vec<EntityId> = (0..3)
        .map(|index| spawn_at(&mut reg, index, &graph, "jab_act"))
        .collect();
    pool_tick(&mut reg, &mut runtime);
    assert_eq!(crate::attack_turns::running_attack_pools(&reg).count(), 0);
    assert_eq!(
        holders(&reg, &enemies),
        2,
        "the default pool alone at limit 2"
    );
    for enemy in enemies {
        assert!(
            held_pools(&reg, enemy)
                .iter()
                .all(|&pool| pool == AttackPoolId::Default)
        );
    }
}

// A11: pools running when their level unloads are not running in the next
// install, a start queued before the unload never lands, and members take
// turns in the default pool from the next install's first tick with nothing
// refreshing.
#[test]
fn level_change_stops_attack_pools_and_clears_ledger() {
    let mut reg = EntityRegistry::new();
    install_budgets(
        &mut reg,
        (3, 0.0),
        (1, 4000.0),
        json!({ "arena": [limit("melee", 1)] }),
    );
    let mut runtime = AiRuntime::new();
    let graph = slam_and_jab();
    let install = |reg: &mut EntityRegistry| -> (EntityId, EntityId, Vec<EntityId>) {
        let pawn = spawn_player(reg, player_spot());
        let guards: Vec<EntityId> = (0..2)
            .map(|index| {
                let enemy = spawn_at(reg, index, &graph, "slam_act");
                tag(reg, enemy, "guard");
                enemy
            })
            .collect();
        (pawn, spawn_pool(reg, "arena", "guard", "arena"), guards)
    };
    let (_, arena, guards) = install(&mut reg);
    start(&mut reg, arena);
    pool_tick(&mut reg, &mut runtime);
    enter(&mut reg, guards[0], "idle");
    pool_tick(&mut reg, &mut runtime);
    assert!(
        runtime.turns.refreshing_entries() > 0,
        "fixture: a count refreshes"
    );
    start(&mut reg, arena);

    // Unload, as `clear_surface_lifetime_level_state` does, then install the
    // same level again.
    reg.clear_for_level_unload();
    runtime.clear_level_state();
    let (pawn, arena, guards) = install(&mut reg);
    pool_tick(&mut reg, &mut runtime);
    assert!(
        !reg.get_component::<AttackPoolComponent>(arena)
            .unwrap()
            .running,
        "no pool runs in the next install"
    );
    assert_eq!(runtime.turns.refreshing_entries(), 0);
    assert_eq!(held(&reg, guards[0]), vec!["slam"]);
    assert_eq!(held_pools(&reg, guards[0]), vec![AttackPoolId::Default]);
    assert_eq!(refreshing(&runtime, AttackPoolId::Default, pawn), 0);
}

// A12: while a pool uses `default`, a mod reload that changes `default`
// changes that pool's limits from the next tick.
#[test]
fn reload_changing_default_profile_reaches_pool_next_tick() {
    let mut reg = EntityRegistry::new();
    install_budgets(&mut reg, (1, 0.0), (1, 0.0), json!({}));
    let mut runtime = AiRuntime::new();
    spawn_player(&mut reg, player_spot());
    let graph = jabber();
    let members: Vec<EntityId> = (0..3)
        .map(|index| {
            let enemy = spawn_at(&mut reg, index, &graph, "jab_act");
            tag(&mut reg, enemy, "guard");
            enemy
        })
        .collect();
    let pool = spawn_pool(&mut reg, "arena", "guard", "default");
    start(&mut reg, pool);
    pool_tick(&mut reg, &mut runtime);
    assert_eq!(holders(&reg, &members), 1);

    install_budgets(
        &mut reg,
        (1, 0.0),
        (1, 0.0),
        json!({ "default": [limit("melee", 3)] }),
    );
    pool_tick(&mut reg, &mut runtime);
    assert_eq!(
        holders(&reg, &members),
        3,
        "the reloaded `default` reaches the pool on the next tick"
    );
    assert!(
        members
            .iter()
            .all(|&member| held_pools(&reg, member) == vec![AttackPoolId::Pool(pool)])
    );
}

// P11 with a pool: a serialize-and-restore keeps the running pool, its start
// order and each enemy's assignment as saved, never re-derived from tags, and
// drops turns, waits and refreshing counts.
#[test]
fn restore_keeps_running_pools_and_assignments() {
    let mut reg = EntityRegistry::new();
    install_budgets(
        &mut reg,
        (3, 0.0),
        (1, 4000.0),
        json!({ "arena": [limit("melee", 1)] }),
    );
    let mut runtime = AiRuntime::new();
    let pawn = spawn_player(&mut reg, player_spot());
    let graph = jabber();
    let member = spawn_at(&mut reg, 0, &graph, "jab_act");
    let waiter = spawn_at(&mut reg, 1, &graph, "jab_act");
    tag(&mut reg, member, "guard");
    tag(&mut reg, waiter, "guard");
    let pool = spawn_pool(&mut reg, "arena", "guard", "arena");
    start(&mut reg, pool);
    pool_tick(&mut reg, &mut runtime);
    assert_eq!(holders(&reg, &[member, waiter]), 1);
    // A tag edit after the start moves no one, before or after the restore.
    reg.set_tags(waiter, Vec::new()).unwrap();

    let kinds = [
        ComponentKind::Transform,
        ComponentKind::PlayerMovement,
        ComponentKind::Mesh,
        ComponentKind::Health,
        ComponentKind::Agent,
        ComponentKind::Brain,
        ComponentKind::EntityState,
        ComponentKind::AttackPool,
    ];
    let mut restored = EntityRegistry::new();
    restored.set_attack_budgets(std::sync::Arc::clone(reg.attack_budgets()));
    for id in [pawn, member, waiter, pool] {
        let saved: Vec<serde_json::Value> = kinds
            .iter()
            .filter_map(|&kind| {
                reg.iter_with_kind(kind)
                    .find(|(entity, _)| *entity == id)
                    .map(|(_, value)| serde_json::to_value(value).expect("component serializes"))
            })
            .collect();
        let restored_id = restored
            .try_spawn(Transform::default(), reg.get_tags(id).unwrap())
            .unwrap();
        assert_eq!(restored_id, id, "fixture: restore keeps entity ids");
        for value in saved {
            let value: ComponentValue =
                serde_json::from_value(value).expect("component deserializes");
            restored.set_component_value(restored_id, value).unwrap();
        }
    }
    assert_eq!(
        restored.get_component::<AttackPoolComponent>(pool).unwrap(),
        reg.get_component::<AttackPoolComponent>(pool).unwrap(),
        "running state, run and start rank restore as saved"
    );
    for enemy in [member, waiter] {
        assert_eq!(turns(&restored, enemy), AttackTurnState::default());
        assert_eq!(
            restored
                .get_component::<BrainComponent>(enemy)
                .unwrap()
                .attack_pool,
            Some(pool),
            "the assignment restores as saved"
        );
    }

    let mut restored_runtime = AiRuntime::new();
    pool_tick(&mut restored, &mut restored_runtime);
    assert_eq!(
        holders(&restored, &[member, waiter]),
        1,
        "the first restored tick arbitrates from scratch under the pool's profile"
    );
    for enemy in [member, waiter] {
        assert!(
            held_pools(&restored, enemy)
                .iter()
                .all(|&counted| counted == AttackPoolId::Pool(pool))
        );
    }
}

// Hot path: once the running set has synced, a steady-state tick's pool feed
// (reading every pool's run state and profile in place), the sync against it,
// and each enemy's pool lookup perform no heap allocation, for many pools and
// many members.
#[test]
fn steady_state_pool_feed_and_membership_allocate_nothing() {
    use crate::alloc_probe::AllocSnapshot;
    use crate::attack_turns::{AttackTurnRuntime, attack_pool_of, running_attack_pools};

    let mut registry = EntityRegistry::new();
    let pools: Vec<EntityId> = (0..32)
        .map(|index| {
            let pool = registry.spawn(Transform::default());
            let mut component =
                AttackPoolComponent::new(format!("guard_{index}"), format!("profile_{index}"));
            component.running = index % 4 != 0;
            component.run = 1;
            registry.set_component(pool, component).unwrap();
            pool
        })
        .collect();
    let graph = jabber();
    let brains: Vec<BrainComponent> = (0..256)
        .map(|index| {
            let mut brain = BrainComponent::from_graph(&graph);
            brain.attack_pool = (index % 3 != 0).then_some(pools[index % pools.len()]);
            brain
        })
        .collect();
    let mut runtime = AttackTurnRuntime::default();
    let pass = |runtime: &mut AttackTurnRuntime| {
        runtime.begin_tick(16.0, &registry);
        runtime.sync_pools(running_attack_pools(&registry));
        brains
            .iter()
            .filter(|brain| attack_pool_of(&runtime.budgets, brain) != AttackPoolId::Default)
            .count()
    };
    let warm = pass(&mut runtime);

    let snapshot = AllocSnapshot::arm();
    let steady = pass(&mut runtime);
    let allocs = snapshot.allocs_since();
    assert_eq!(steady, warm);
    assert!(
        warm > 0 && warm < brains.len(),
        "fixture: members of running and stopped pools"
    );
    assert_eq!(allocs, 0, "a steady-state pool feed must not allocate");
}
