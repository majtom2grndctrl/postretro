// Profiles and mod reloads through the tick: a profile's overrides, a reload
// changing budgets (revokes nothing, applies next tick), and a reload
// changing what an attack draws, resolved from the live descriptor.

use std::sync::Arc;

use postretro_entities::components::brain::AttackPoolId;
use postretro_entities::provenance::{DescriptorProvenance, DescriptorSpawnPath};
use serde_json::json;

use super::*;

const DEFAULT_ID: AttackTokenId = AttackTokenId(0);
const HEAVY_ID: AttackTokenId = AttackTokenId(2);

/// A token declaration crossing as `{ kind, name, budget }`.
fn declare(name: &str, limit: u32, refresh_ms: f32) -> serde_json::Value {
    json!({ "kind": "attackToken", "name": name,
            "budget": { "limit": limit, "refreshMs": refresh_ms } })
}

/// `token.with({ ... })` in a profile.
fn override_ref(token: &str, fields: serde_json::Value) -> serde_json::Value {
    let mut entry = json!({ "token": { "kind": "attackToken", "name": token } });
    for (key, value) in fields.as_object().expect("override fields") {
        entry[key] = value.clone();
    }
    entry
}

fn commit(reg: &mut EntityRegistry, tokens: serde_json::Value, budgets: serde_json::Value) {
    let vocabulary = AttackBudgetVocabulary::from_manifest(Some(&tokens), Some(&budgets))
        .expect("a valid registration");
    reg.set_attack_budgets(Arc::new(vocabulary));
}

// B17: a profile override changes only the fields it names. Through the
// default pool: `default: [melee.with({ limit: 1 })]` admits one jab at a
// time, keeps `melee`'s declared 4 s refresh, and leaves `heavy` at its
// declared limit 2. The pool-profile half is
// `pool_profile_override_keeps_unnamed_fields_and_tokens`.
#[test]
fn profile_override_keeps_unnamed_fields_and_tokens() {
    let graph = marked_graph(vec![
        ("jab", held_attack(vec![token("melee")])),
        ("rocket", held_attack(vec![token("heavy")])),
    ]);
    let mut reg = EntityRegistry::new();
    commit(
        &mut reg,
        json!([declare("melee", 3, 4000.0), declare("heavy", 2, 0.0)]),
        json!({ "default": [override_ref("melee", json!({ "limit": 1 }))] }),
    );
    let mut runtime = AiRuntime::new();
    spawn_player(&mut reg, player_spot());
    let jabbers = [
        spawn_at(&mut reg, 0, &graph, "jab_act"),
        spawn_at(&mut reg, 1, &graph, "jab_act"),
    ];
    let rockets = [
        spawn_at(&mut reg, 2, &graph, "rocket_act"),
        spawn_at(&mut reg, 3, &graph, "rocket_act"),
    ];

    tick(&mut reg, &mut runtime);
    assert_eq!(held(&reg, jabbers[0]), vec!["jab"]);
    assert!(held(&reg, jabbers[1]).is_empty(), "the profile's limit 1");
    for rocket in rockets {
        assert_eq!(held(&reg, rocket), vec!["rocket"], "heavy keeps limit 2");
    }

    enter(&mut reg, jabbers[0], "idle");
    tick(&mut reg, &mut runtime);
    for elapsed in 1..250 {
        tick(&mut reg, &mut runtime);
        assert!(
            held(&reg, jabbers[1]).is_empty(),
            "{elapsed} ticks after: melee keeps its declared refreshMs"
        );
    }
    tick(&mut reg, &mut runtime);
    assert_eq!(held(&reg, jabbers[1]), vec!["jab"]);
}

// B26: a reload lowering `heavy` from 2 to 1 while two slams hold revokes
// neither turn, and no slam is granted until both release. A reload lowering
// `refreshMs` frees a refreshing count once the new delay has passed. The
// removed-profile case is `reload_removing_a_pool_profile_falls_back_to_default_next_tick`.
#[test]
fn reload_lowering_budgets_revokes_no_turn_and_applies_next_tick() {
    let graph = marked_graph(vec![("slam", held_attack(vec![token("heavy")]))]);
    let mut reg = EntityRegistry::new();
    let tokens = |heavy_limit: u32, heavy_refresh_ms: f32| {
        json!([
            declare("melee", 3, 0.0),
            declare("heavy", heavy_limit, heavy_refresh_ms)
        ])
    };
    commit(&mut reg, tokens(2, 0.0), json!({}));
    let mut runtime = AiRuntime::new();
    spawn_player(&mut reg, player_spot());
    let holders = [
        spawn_at(&mut reg, 0, &graph, "slam_act"),
        spawn_at(&mut reg, 1, &graph, "slam_act"),
    ];
    let waiter = spawn_at(&mut reg, 2, &graph, "slam_act");
    tick(&mut reg, &mut runtime);
    assert!(holders.iter().all(|&holder| held(&reg, holder) == ["slam"]));

    commit(&mut reg, tokens(1, 0.0), json!({}));
    tick(&mut reg, &mut runtime);
    assert!(
        holders.iter().all(|&holder| held(&reg, holder) == ["slam"]),
        "lowering the limit revokes no turn"
    );
    enter(&mut reg, holders[0], "idle");
    tick(&mut reg, &mut runtime);
    assert!(
        held(&reg, waiter).is_empty(),
        "one slam still holds the one heavy"
    );
    enter(&mut reg, holders[1], "idle");
    tick(&mut reg, &mut runtime);
    assert_eq!(
        held(&reg, waiter),
        vec!["slam"],
        "granted once both release"
    );

    // Refresh measured against the current value: a 4 s refresh lowered to
    // 1 s ten ticks in frees the count 1 s after the turn ended.
    commit(&mut reg, tokens(1, 4000.0), json!({}));
    tick(&mut reg, &mut runtime);
    enter(&mut reg, holders[0], "slam_act");
    enter(&mut reg, waiter, "idle");
    tick(&mut reg, &mut runtime);
    assert!(held(&reg, holders[0]).is_empty(), "heavy is refreshing");
    for _ in 1..10 {
        tick(&mut reg, &mut runtime);
    }
    commit(&mut reg, tokens(1, 1000.0), json!({}));
    // 62 ticks is 992 ms, 63 is 1008 ms.
    for elapsed in 10..63 {
        tick(&mut reg, &mut runtime);
        assert!(
            held(&reg, holders[0]).is_empty(),
            "{elapsed} ticks after: still inside the lowered delay"
        );
    }
    tick(&mut reg, &mut runtime);
    assert_eq!(
        held(&reg, holders[0]),
        vec!["slam"],
        "freed once the lowered delay has passed"
    );
}

/// The `brute` descriptor carrying `graph`, as the live data registry would.
fn brute(graph: &BehaviorGraphDescriptor) -> EntityTypeDescriptor {
    EntityTypeDescriptor {
        life_states: None,
        ghost: false,
        faction: None,
        tolerance: None,
        canonical_name: Some("brute".to_string()),
        inventory: None,
        light: None,
        emitter: None,
        movement: None,
        weapon: None,
        touchable: None,
        mesh: None,
        health: None,
        behavior: Some(graph.clone()),
    }
}

fn spawn_brute(
    reg: &mut EntityRegistry,
    index: usize,
    graph: &BehaviorGraphDescriptor,
    state: &str,
) -> EntityId {
    let enemy = spawn_at(reg, index, graph, state);
    reg.set_component(
        enemy,
        DescriptorProvenance {
            canonical_name: "brute".to_string(),
            owned_components: Default::default(),
            map_overrides: Default::default(),
            spawn_path: DescriptorSpawnPath::MapPlacement,
        },
    )
    .unwrap();
    enemy
}

fn tick_at(
    reg: &mut EntityRegistry,
    runtime: &mut AiRuntime,
    descriptors: &[EntityTypeDescriptor],
    generation: u64,
) {
    run_ai_tick_with_navigation_and_impact(
        reg,
        runtime,
        DT,
        AiTickInputs {
            mover_nav: crate::nav::MoverNav::none(),
            nav_graph: None,
            collision_world: Some(&CollisionWorld::new()),
            descriptors,
            descriptor_generation: generation,
            factions: &FactionRegistry::default(),
            faction_sentiment: &RefCell::new(FactionSentimentState::default()),
        },
        |_| {},
    );
}

fn held_draws(reg: &EntityRegistry, enemy: EntityId) -> Vec<ResolvedDraws> {
    turns(reg, enemy).held().map(|turn| turn.draws).collect()
}

// B27: a hot reload that changes an attack's resolved `draws` releases its
// holders' turns, and they re-request under the new draws. Draws resolve
// from the live descriptor (the brain's graph keeps its spawn-time attacks)
// and compare resolved: no `draws` to `[defaultToken]` leaves the turn
// standing; `[]` to `[melee]` releases the undrawn holder.
#[test]
fn reload_changing_resolved_draws_releases_holders() {
    // `jab` has no `draws`, so it draws `defaultToken`; `chip` draws nothing.
    let original = marked_graph(vec![
        ("slam", held_attack(vec![token("heavy")])),
        ("jab", contact(1.0, 100_000.0)),
        ("chip", held_attack(Vec::new())),
    ]);
    let mut reg = EntityRegistry::new();
    // Refreshing `heavy` and `defaultToken` show which turns ended.
    commit(
        &mut reg,
        json!([declare("melee", 1, 0.0), declare("heavy", 1, 4000.0)]),
        json!({ "default": [override_ref("default", json!({ "refreshMs": 4000 }))] }),
    );
    let mut runtime = AiRuntime::new();
    let pawn = spawn_player(&mut reg, player_spot());
    let slammer = spawn_brute(&mut reg, 0, &original, "slam_act");
    let jabber = spawn_brute(&mut reg, 1, &original, "jab_act");
    let chipper = spawn_brute(&mut reg, 2, &original, "chip_act");
    let mut descriptors = vec![brute(&original)];

    tick_at(&mut reg, &mut runtime, &descriptors, 0);
    for enemy in [slammer, jabber, chipper] {
        assert_eq!(held(&reg, enemy).len(), 1, "fixture: every attack holds");
    }
    let jab_draws = held_draws(&reg, jabber);

    let mut reloaded = original.clone();
    reloaded.attacks.get_mut("slam").unwrap().draws = Some(vec![token("melee")]);
    reloaded.attacks.get_mut("jab").unwrap().draws = Some(vec![token("default")]);
    reloaded.attacks.get_mut("chip").unwrap().draws = Some(vec![token("melee")]);
    descriptors[0] = brute(&reloaded);
    tick_at(&mut reg, &mut runtime, &descriptors, 1);

    let group = (AttackPoolId::Default, pawn);
    assert_eq!(
        runtime.turns.refreshing_now(group, HEAVY_ID),
        1,
        "the slam turn ended"
    );
    assert_eq!(
        runtime.turns.refreshing_now(group, DEFAULT_ID),
        0,
        "the jab turn stands: no draws and [defaultToken] resolve alike"
    );
    assert_eq!(held_draws(&reg, jabber), jab_draws);
    assert_eq!(
        held_draws(&reg, slammer),
        vec![draws(&[(1, 1)])],
        "the slam re-requested under its new draws and took the one melee"
    );
    assert!(
        held(&reg, chipper).is_empty(),
        "the undrawn chip was released and now waits on melee"
    );
    assert_eq!(requested(&reg, chipper).as_deref(), Some("chip"));
}

// A reload that removes a token an attack still draws drops the draw, so the
// token no longer limits that attack; the author hears it on that reload and
// on every later one that still lacks the token, naming attack and token.
#[test]
fn reload_removing_a_drawn_token_warns_on_each_reload() {
    let graph = marked_graph(vec![("slam", held_attack(vec![token("heavy")]))]);
    let mut reg = EntityRegistry::new();
    commit(
        &mut reg,
        json!([declare("melee", 1, 0.0), declare("heavy", 1, 4000.0)]),
        json!({}),
    );
    let mut runtime = AiRuntime::new();
    spawn_player(&mut reg, player_spot());
    spawn_brute(&mut reg, 0, &graph, "slam_act");
    let descriptors = vec![brute(&graph)];
    let capture = postretro_test_log_capture::LogCapture::start();
    tick_at(&mut reg, &mut runtime, &descriptors, 0);
    capture.assert_not_logged(log::Level::Warn, "is not a registered attack token");

    let warning = "attack `slam` draws `heavy`, which is not a registered attack token; the draw is ignored, so `heavy` no longer limits this attack";
    commit(&mut reg, json!([declare("melee", 1, 0.0)]), json!({}));
    tick_at(&mut reg, &mut runtime, &descriptors, 0);
    tick_at(&mut reg, &mut runtime, &descriptors, 0);
    capture.assert_logged_once(log::Level::Warn, warning);

    capture.clear();
    commit(&mut reg, json!([declare("melee", 1, 0.0)]), json!({}));
    tick_at(&mut reg, &mut runtime, &descriptors, 0);
    capture.assert_logged_once(log::Level::Warn, warning);
}

// B27, compared resolved: a reload that only reorders an attack's draws is
// no draws change, so its turn stands and `heavy` starts no refresh.
// Regression: draws compared in list order, so a reorder released holders.
#[test]
fn reload_reordering_draws_keeps_the_turn() {
    let original = marked_graph(vec![(
        "slam",
        held_attack(vec![token("melee"), token("heavy")]),
    )]);
    let mut reg = EntityRegistry::new();
    commit(
        &mut reg,
        json!([declare("melee", 3, 0.0), declare("heavy", 1, 4000.0)]),
        json!({}),
    );
    let mut runtime = AiRuntime::new();
    let pawn = spawn_player(&mut reg, player_spot());
    let slammer = spawn_brute(&mut reg, 0, &original, "slam_act");
    let mut descriptors = vec![brute(&original)];
    tick_at(&mut reg, &mut runtime, &descriptors, 0);
    assert_eq!(held(&reg, slammer), vec!["slam"]);

    let mut reloaded = original.clone();
    reloaded.attacks.get_mut("slam").unwrap().draws = Some(vec![token("heavy"), token("melee")]);
    descriptors[0] = brute(&reloaded);
    tick_at(&mut reg, &mut runtime, &descriptors, 1);

    assert_eq!(
        held(&reg, slammer),
        vec!["slam"],
        "a reorder keeps the turn"
    );
    assert_eq!(
        runtime
            .turns
            .refreshing_now((AttackPoolId::Default, pawn), HEAVY_ID),
        0,
        "the turn never ended"
    );
}

// Token remap through the tick: a commit that inserts a token ahead of a held
// turn's token shifts its id. The held turn survives, re-keyed to the new id,
// and the refreshing count moves with it.
#[test]
fn vocabulary_shift_keeps_held_turns_and_refreshing_counts() {
    const SHIFTED_HEAVY_ID: AttackTokenId = AttackTokenId(3);
    let graph = marked_graph(vec![("slam", held_attack(vec![token("heavy")]))]);
    let mut reg = EntityRegistry::new();
    commit(
        &mut reg,
        json!([declare("melee", 3, 0.0), declare("heavy", 2, 4000.0)]),
        json!({}),
    );
    let mut runtime = AiRuntime::new();
    let pawn = spawn_player(&mut reg, player_spot());
    let holder = spawn_at(&mut reg, 0, &graph, "slam_act");
    let ender = spawn_at(&mut reg, 1, &graph, "slam_act");
    tick(&mut reg, &mut runtime);
    assert_eq!(held(&reg, holder), vec!["slam"]);
    assert_eq!(held(&reg, ender), vec!["slam"]);
    enter(&mut reg, ender, "idle");
    tick(&mut reg, &mut runtime);
    let group = (AttackPoolId::Default, pawn);
    assert_eq!(
        runtime.turns.refreshing_now(group, HEAVY_ID),
        1,
        "fixture: the ender's heavy refreshes"
    );

    // `ranged` lands between `melee` and `heavy`: heavy's id moves from 2 to 3.
    commit(
        &mut reg,
        json!([
            declare("melee", 3, 0.0),
            declare("ranged", 2, 0.0),
            declare("heavy", 2, 4000.0)
        ]),
        json!({}),
    );
    tick(&mut reg, &mut runtime);

    assert_eq!(held(&reg, holder), vec!["slam"], "the held turn survives");
    assert_eq!(
        held_draws(&reg, holder),
        vec![draws(&[(SHIFTED_HEAVY_ID.0, 1)])],
        "re-keyed to heavy's new id"
    );
    assert_eq!(
        runtime.turns.refreshing_now(group, SHIFTED_HEAVY_ID),
        1,
        "the refreshing count moved with heavy"
    );
    assert_eq!(
        runtime.turns.refreshing_entries(),
        1,
        "and the holder's turn started none"
    );
}
