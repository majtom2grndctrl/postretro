// Declared tokens through the tick: a registered vocabulary on the registry,
// attacks authoring `draws`, and the resolution chokepoint between them.
// Re-proves the multi-token pins (P14, P18, P19, P24, P27) beside the
// engine-level proofs in `tests/arbitration.rs`, plus per-target allowances.

use std::sync::Arc;

use postretro_foundation::attack_budget::{AttackBudgetVocabulary, TokenDraw};
use serde_json::json;

use super::*;

/// Register `tokens` (name, limit) as the mod's `attackTokens`, as a
/// committed manifest does.
fn install_tokens(reg: &mut EntityRegistry, tokens: &[(&str, u32)]) {
    let declarations: Vec<serde_json::Value> = tokens
        .iter()
        .map(|(name, limit)| {
            json!({ "kind": "attackToken", "name": name, "budget": { "limit": limit } })
        })
        .collect();
    let vocabulary = AttackBudgetVocabulary::from_manifest(Some(&json!(declarations)), None)
        .expect("a valid token registration");
    reg.set_attack_budgets(Arc::new(vocabulary));
}

/// A draw of one `name`, as a bare handle crosses.
fn token(name: &str) -> TokenDraw {
    TokenDraw(json!({ "kind": "attackToken", "name": name }))
}

/// `name.with({ cost })`, as it crosses.
fn costing(name: &str, cost: u32) -> TokenDraw {
    TokenDraw(json!({ "token": { "kind": "attackToken", "name": name }, "cost": cost }))
}

fn drawing(mut attack: AttackParams, draws: Vec<TokenDraw>) -> AttackParams {
    attack.draws = Some(draws);
    attack
}

/// A held-forever contact attack drawing `draws`.
fn held_attack(draws: Vec<TokenDraw>) -> AttackParams {
    drawing(contact(1.0, 100_000.0), draws)
}

/// Marked activities, one per attack, plus `idle`, which releases.
fn marked_graph(attacks: Vec<(&str, AttackParams)>) -> BehaviorGraphDescriptor {
    let names: Vec<String> = attacks.iter().map(|(name, _)| name.to_string()).collect();
    let acts: Vec<String> = names.iter().map(|name| format!("{name}_act")).collect();
    let mut activities = vec![("idle", idle())];
    for (name, act) in names.iter().zip(&acts) {
        activities.push((act.as_str(), marked(striking(name), name)));
    }
    graph("idle", activities, vec![], attacks)
}

/// B6 / P14: `melee` at 4, `heavy` at 1. One slam (`melee`, `heavy`) holds; a
/// second waits longest and reserves one `melee` and the `heavy`; two jabs
/// requesting after it are granted past it, and it is granted on the tick
/// the holding slam releases.
#[test]
fn declared_slam_reserves_melee_and_heavy_while_jabs_pass() {
    let graph = marked_graph(vec![
        ("slam", held_attack(vec![token("melee"), token("heavy")])),
        ("jab", held_attack(vec![token("melee")])),
    ]);
    let mut reg = EntityRegistry::new();
    install_tokens(&mut reg, &[("melee", 4), ("heavy", 1)]);
    let mut runtime = AiRuntime::new();
    spawn_player(&mut reg, player_spot());
    let first = spawn_at(&mut reg, 0, &graph, "slam_act");
    let second = spawn_at(&mut reg, 1, &graph, "slam_act");
    let jabs = [
        spawn_at(&mut reg, 2, &graph, "idle"),
        spawn_at(&mut reg, 3, &graph, "idle"),
    ];

    tick(&mut reg, &mut runtime);
    tick(&mut reg, &mut runtime);
    assert_eq!(held(&reg, first), vec!["slam"]);
    assert_eq!(wait_since(&reg, second), Some(1));

    for jab in jabs {
        enter(&mut reg, jab, "jab_act");
    }
    tick(&mut reg, &mut runtime);
    for jab in jabs {
        assert_eq!(
            held(&reg, jab),
            vec!["jab"],
            "both jabs pass the waiting slam"
        );
    }
    assert!(held(&reg, second).is_empty(), "the heavy is still held");

    enter(&mut reg, first, "idle");
    tick(&mut reg, &mut runtime);
    assert_eq!(
        held(&reg, second),
        vec!["slam"],
        "granted on the tick the holding slam releases"
    );
}

/// B7: a slam (`melee`, `heavy`) and a rocket (`ranged`, `heavy`) from two
/// archetypes, both drawing `heavy` at limit 1, never hold two turns on one
/// target at once, while each archetype takes its turn.
#[test]
fn declared_heavy_shared_by_two_archetypes_never_overlaps() {
    let brute = marked_graph(vec![(
        "slam",
        held_attack(vec![token("melee"), token("heavy")]),
    )]);
    let gunner = marked_graph(vec![(
        "rocket",
        held_attack(vec![token("ranged"), token("heavy")]),
    )]);
    let mut reg = EntityRegistry::new();
    install_tokens(&mut reg, &[("melee", 3), ("ranged", 2), ("heavy", 1)]);
    let mut runtime = AiRuntime::new();
    spawn_player(&mut reg, player_spot());
    let enemies = [
        (spawn_at(&mut reg, 0, &brute, "slam_act"), "slam_act"),
        (spawn_at(&mut reg, 1, &gunner, "rocket_act"), "rocket_act"),
        (spawn_at(&mut reg, 2, &brute, "slam_act"), "slam_act"),
        (spawn_at(&mut reg, 3, &gunner, "rocket_act"), "rocket_act"),
    ];

    let mut held_by_kind = [false, false];
    for tick_index in 1..=40 {
        tick(&mut reg, &mut runtime);
        let holders: Vec<usize> = (0..enemies.len())
            .filter(|&index| turns(&reg, enemies[index].0).holds_any())
            .collect();
        assert!(
            holders.len() <= 1,
            "tick {tick_index}: two heavy turns at once"
        );
        for &index in &holders {
            held_by_kind[index % 2] = true;
        }
        // Every fourth tick the holder leaves and comes back, so the heavy
        // turn passes along the line.
        if tick_index % 4 == 0 {
            for index in holders {
                let (enemy, act) = enemies[index];
                enter(&mut reg, enemy, "idle");
                tick(&mut reg, &mut runtime);
                enter(&mut reg, enemy, act);
            }
        }
    }
    assert_eq!(
        held_by_kind,
        [true, true],
        "both archetypes take heavy turns"
    );
}

/// B8 / P18, P32: `melee` at 3, `heavy` at 1. One slam holds; a second slam
/// heads `melee` and `heavy`; a cost-3 `melee` draw waits behind it; a later
/// cost-1 jab fits beside the slam's single reservation. Once the second slam
/// is granted, the cost-3 draw heads `melee` and reserves the whole token.
#[test]
fn declared_head_of_line_reserves_per_token_in_wait_order() {
    let graph = marked_graph(vec![
        ("slam", held_attack(vec![token("melee"), token("heavy")])),
        ("crush", held_attack(vec![costing("melee", 3)])),
        ("jab", held_attack(vec![token("melee")])),
    ]);
    let mut reg = EntityRegistry::new();
    install_tokens(&mut reg, &[("melee", 3), ("heavy", 1)]);
    let mut runtime = AiRuntime::new();
    spawn_player(&mut reg, player_spot());
    let holder = spawn_at(&mut reg, 0, &graph, "slam_act");
    let slam = spawn_at(&mut reg, 1, &graph, "slam_act");
    let crush = spawn_at(&mut reg, 2, &graph, "idle");
    let jab = spawn_at(&mut reg, 3, &graph, "idle");
    let late_jab = spawn_at(&mut reg, 4, &graph, "idle");

    tick(&mut reg, &mut runtime);
    assert_eq!(held(&reg, holder), vec!["slam"]);
    enter(&mut reg, crush, "crush_act");
    tick(&mut reg, &mut runtime);
    assert_eq!(wait_since(&reg, crush), Some(2));
    enter(&mut reg, jab, "jab_act");
    tick(&mut reg, &mut runtime);
    assert_eq!(
        held(&reg, jab),
        vec!["jab"],
        "the cost-1 jab fits beside the head slam's one reserved melee"
    );
    assert!(held(&reg, crush).is_empty() && held(&reg, slam).is_empty());

    enter(&mut reg, holder, "idle");
    tick(&mut reg, &mut runtime);
    assert_eq!(held(&reg, slam), vec!["slam"], "the head slam is granted");
    assert!(held(&reg, crush).is_empty(), "two melee are still counted");

    enter(&mut reg, late_jab, "jab_act");
    tick(&mut reg, &mut runtime);
    assert!(
        held(&reg, late_jab).is_empty(),
        "the cost-3 draw now heads melee and reserves all of it"
    );

    enter(&mut reg, slam, "idle");
    enter(&mut reg, jab, "idle");
    tick(&mut reg, &mut runtime);
    assert_eq!(
        held(&reg, crush),
        vec!["crush"],
        "granted once nothing is counted against melee"
    );
    assert!(held(&reg, late_jab).is_empty());
}

/// B9 / P27: `melee` at 3, `heavy` at 1, `ranged` at 2. A slam (`melee` ×2,
/// `heavy`) waits longest while a rocket (`ranged`, `heavy`) holds `heavy`
/// and a jab holds `melee`. Later jabs fit only beside the slam's two
/// reserved `melee`, no later rocket takes `heavy`, pellets keep drawing
/// `ranged`, and the slam is granted the tick the rocket's `heavy` frees.
#[test]
fn declared_multi_token_waiter_is_granted_among_cycling_traffic() {
    let graph = marked_graph(vec![
        (
            "slam",
            held_attack(vec![costing("melee", 2), token("heavy")]),
        ),
        ("rocket", held_attack(vec![token("ranged"), token("heavy")])),
        ("jab", held_attack(vec![token("melee")])),
        ("pellet", held_attack(vec![token("ranged")])),
    ]);
    let mut reg = EntityRegistry::new();
    install_tokens(&mut reg, &[("melee", 3), ("heavy", 1), ("ranged", 2)]);
    let mut runtime = AiRuntime::new();
    spawn_player(&mut reg, player_spot());
    let rocket = spawn_at(&mut reg, 0, &graph, "rocket_act");
    let jab = spawn_at(&mut reg, 1, &graph, "jab_act");
    let slam = spawn_at(&mut reg, 2, &graph, "idle");
    let pellet = spawn_at(&mut reg, 3, &graph, "idle");
    let late_jab = spawn_at(&mut reg, 4, &graph, "idle");
    let late_rocket = spawn_at(&mut reg, 5, &graph, "idle");

    tick(&mut reg, &mut runtime);
    assert_eq!(held(&reg, rocket), vec!["rocket"]);
    assert_eq!(held(&reg, jab), vec!["jab"]);
    enter(&mut reg, slam, "slam_act");
    tick(&mut reg, &mut runtime);
    assert_eq!(wait_since(&reg, slam), Some(2));

    enter(&mut reg, pellet, "pellet_act");
    enter(&mut reg, late_jab, "jab_act");
    tick(&mut reg, &mut runtime);
    assert_eq!(held(&reg, pellet), vec!["pellet"], "ranged keeps flowing");
    assert!(
        held(&reg, late_jab).is_empty(),
        "one held jab plus the slam's two reserved melee fill the token"
    );
    enter(&mut reg, late_rocket, "rocket_act");
    for _ in 0..5 {
        tick(&mut reg, &mut runtime);
        assert!(
            held(&reg, late_rocket).is_empty(),
            "no rocket takes heavy past the slam"
        );
        assert!(held(&reg, slam).is_empty());
    }

    enter(&mut reg, rocket, "idle");
    tick(&mut reg, &mut runtime);
    assert_eq!(
        held(&reg, slam),
        vec!["slam"],
        "granted the tick the holding rocket's heavy frees"
    );
    assert!(held(&reg, late_rocket).is_empty());
}

fn tick_with_factions(
    reg: &mut EntityRegistry,
    runtime: &mut AiRuntime,
    factions: &FactionRegistry,
) {
    run_ai_tick_with_navigation_and_impact(
        reg,
        runtime,
        DT,
        AiTickInputs {
            mover_nav: crate::nav::MoverNav::none(),
            nav_graph: None,
            collision_world: None,
            descriptors: &[],
            descriptor_generation: 0,
            factions,
            faction_sentiment: &RefCell::new(FactionSentimentState::default()),
        },
        |_| {},
    );
}

fn set_faction(reg: &mut EntityRegistry, entity: EntityId, faction: f32) {
    reg.entity_state_mut(entity)
        .expect("a live fixture entity")
        .set(FACTION_STATE_FIELD, faction);
}

/// B18: every target has its own allowance. Under `melee` at 1, one enemy on
/// each of two co-op players, on an NPC ally and on an infighting monster
/// holds a turn at once, and a second attacker on each waits. On the rival
/// monster the second attacker is an ally: an ally and a hostile monster
/// attacking one monster share its allowance.
#[test]
fn each_target_has_its_own_allowance() {
    // Declared factions take indexes from 2: `allies` 2, `rivals` 3. Every
    // pair across factions is hostile by default.
    const ALLIES: f32 = 2.0;
    const RIVALS: f32 = 3.0;
    let factions = FactionRegistry::from_descriptors(vec![
        FactionDescriptor {
            name: "allies".to_string(),
        },
        FactionDescriptor {
            name: "rivals".to_string(),
        },
    ])
    .expect("valid factions");
    let claw = graph(
        "strike",
        vec![("strike", striking("claw"))],
        vec![],
        vec![("claw", held_attack(vec![token("melee")]))],
    );
    let bystander = graph("idle", vec![("idle", idle())], vec![], vec![]);
    let mut reg = EntityRegistry::new();
    install_tokens(&mut reg, &[("melee", 1)]);
    let mut runtime = AiRuntime::new();

    // Groups sit 30 m apart, so every attacker's nearest hostile is its own
    // group's target.
    let at = |x: f32, group: f32| Vec3::new(x, 0.0, 30.0 * group);
    let mut attackers = Vec::new();
    let pair = |reg: &mut EntityRegistry, group: f32| {
        let a = spawn_enemy(
            reg,
            at(0.0, group) - Vec3::Z * 0.1,
            authored_brain(&claw, "strike"),
            50.0,
        );
        let b = spawn_enemy(
            reg,
            at(0.0, group) + Vec3::Z * 0.1,
            authored_brain(&claw, "strike"),
            50.0,
        );
        [a, b]
    };
    spawn_player(&mut reg, at(1.5, 0.0));
    attackers.push(pair(&mut reg, 0.0));
    spawn_player(&mut reg, at(1.5, 1.0));
    attackers.push(pair(&mut reg, 1.0));
    let ally = spawn_enemy(
        &mut reg,
        at(1.5, 2.0),
        authored_brain(&bystander, "idle"),
        1_000.0,
    );
    set_faction(&mut reg, ally, ALLIES);
    attackers.push(pair(&mut reg, 2.0));
    // The rival stands between a monster on one side and an ally on the other.
    let rival = spawn_enemy(
        &mut reg,
        at(0.0, 3.0),
        authored_brain(&bystander, "idle"),
        1_000.0,
    );
    set_faction(&mut reg, rival, RIVALS);
    let monster = spawn_enemy(
        &mut reg,
        at(-1.5, 3.0),
        authored_brain(&claw, "strike"),
        50.0,
    );
    let helper = spawn_enemy(
        &mut reg,
        at(1.5, 3.0),
        authored_brain(&claw, "strike"),
        50.0,
    );
    set_faction(&mut reg, helper, ALLIES);
    set_enemy_yaw(&mut reg, helper, -std::f32::consts::FRAC_PI_2);
    attackers.push([monster, helper]);

    let mut held_on_rival = [false, false];
    for tick_index in 1..=60 {
        tick_with_factions(&mut reg, &mut runtime, &factions);
        for (group, pair) in attackers.iter().enumerate() {
            assert!(
                holders(&reg, pair) <= 1,
                "tick {tick_index}: two turns on target {group} at limit 1"
            );
        }
        for (index, &enemy) in [monster, helper].iter().enumerate() {
            held_on_rival[index] |= turns(&reg, enemy).holds_any();
        }
    }
    for (group, pair) in attackers.iter().enumerate() {
        assert_eq!(
            holders(&reg, pair),
            1,
            "target {group} has its own allowance, independent of the others"
        );
    }
    assert!(
        held_on_rival.iter().any(|&held| held),
        "the rival's allowance is in use"
    );
    assert_eq!(
        enemy_acquired_target(&reg, helper),
        Some(rival),
        "fixture: the ally attacks the rival"
    );
    assert_eq!(
        enemy_acquired_target(&reg, monster),
        Some(rival),
        "fixture: the monster attacks the rival"
    );
    for &enemy in &attackers[2] {
        assert_eq!(
            enemy_acquired_target(&reg, enemy),
            Some(ally),
            "fixture: monsters attack the ally"
        );
    }
}

/// B19: `defaultToken` needs no entry in `attackTokens`. A mod registering
/// only `melee` at 1 loads attacks without `draws`, which draw `defaultToken`
/// at its SDK limit of 3, beside the `melee` attacks that take turns one at a
/// time.
#[test]
fn default_token_needs_no_registration_and_draws_beside_mod_tokens() {
    let unauthored = striker(100_000.0);
    let clawing = graph(
        "strike",
        vec![("strike", striking("claw"))],
        vec![],
        vec![("claw", held_attack(vec![token("melee")]))],
    );
    let mut reg = EntityRegistry::new();
    install_tokens(&mut reg, &[("melee", 1)]);
    let mut runtime = AiRuntime::new();
    spawn_player(&mut reg, player_spot());
    let defaults: Vec<EntityId> = (0..4)
        .map(|index| spawn_at(&mut reg, index, &unauthored, "strike"))
        .collect();
    let claws: Vec<EntityId> = (4..6)
        .map(|index| spawn_at(&mut reg, index, &clawing, "strike"))
        .collect();

    for _ in 0..5 {
        tick(&mut reg, &mut runtime);
    }
    assert_eq!(holders(&reg, &defaults), 3, "defaultToken at its SDK limit");
    assert_eq!(holders(&reg, &claws), 1, "melee at its declared limit");
}

/// B20 / P19: `lobber` at 1, `melee` at 3. A's grenade holds; B's grenade
/// waits longest; B's graph then asks for its claw and is granted it, and its
/// dropped grenade request stops blocking `lobber`, so C's later grenade is
/// granted the tick A's releases. Meanwhile `attackTurnFree("grenade")` reads
/// false and `attackTurnFree("claw")` true.
#[test]
fn declared_waiter_switching_attack_frees_its_dropped_token() {
    let free = |attack: &str| brain_input(&format!("@attackTurnFree.{attack}"));
    let graph = graph(
        "idle",
        vec![
            ("idle", idle()),
            ("grenade_act", marked(striking("grenade"), "grenade")),
            ("claw_act", marked(striking("claw"), "claw")),
            ("probe_grenade", idle()),
            ("probe_claw", idle()),
            ("grenade_free", idle()),
            ("claw_free", idle()),
        ],
        vec![
            ("probe_grenade", vec![edge("grenade_free", free("grenade"))]),
            ("probe_claw", vec![edge("claw_free", free("claw"))]),
        ],
        vec![
            ("grenade", held_attack(vec![token("lobber")])),
            ("claw", held_attack(vec![token("melee")])),
        ],
    );
    let mut reg = EntityRegistry::new();
    install_tokens(&mut reg, &[("lobber", 1), ("melee", 3)]);
    let mut runtime = AiRuntime::new();
    spawn_player(&mut reg, player_spot());
    let a = spawn_at(&mut reg, 0, &graph, "grenade_act");
    let b = spawn_at(&mut reg, 1, &graph, "grenade_act");
    let c = spawn_at(&mut reg, 2, &graph, "idle");
    // The probes start reading once the line has formed (facts lag a tick).
    let grenade_probe = spawn_at(&mut reg, 3, &graph, "idle");
    let claw_probe = spawn_at(&mut reg, 4, &graph, "idle");

    tick(&mut reg, &mut runtime);
    assert_eq!(held(&reg, a), vec!["grenade"]);
    assert_eq!(wait_since(&reg, b), Some(1));

    enter(&mut reg, b, "claw_act");
    tick(&mut reg, &mut runtime);
    assert_eq!(held(&reg, b), vec!["claw"], "B is granted its claw");
    enter(&mut reg, c, "grenade_act");
    tick(&mut reg, &mut runtime);
    assert!(held(&reg, c).is_empty(), "A still holds lobber");
    enter(&mut reg, grenade_probe, "probe_grenade");
    enter(&mut reg, claw_probe, "probe_claw");
    tick(&mut reg, &mut runtime);
    assert_eq!(
        state(&reg, grenade_probe),
        "probe_grenade",
        "lobber is full"
    );
    assert_eq!(state(&reg, claw_probe), "claw_free", "melee has room");

    enter(&mut reg, a, "idle");
    tick(&mut reg, &mut runtime);
    assert_eq!(
        held(&reg, c),
        vec!["grenade"],
        "B's dropped grenade request no longer blocks lobber"
    );
}

/// B21 / P24: `melee` at 3. X holds cost 1; B's cost-3 slam has waited
/// longest and reserves the whole token; D asks for cost 1 after it. When B's
/// graph switches to its cost-1 jab, B's slam stops blocking, and B's jab and
/// D are both granted that tick: 1 + 1 + 1 fits.
#[test]
fn declared_waiter_switching_to_a_fitting_draw_unblocks_token_same_tick() {
    let graph = marked_graph(vec![
        ("slam", held_attack(vec![costing("melee", 3)])),
        ("jab", held_attack(vec![token("melee")])),
    ]);
    let mut reg = EntityRegistry::new();
    install_tokens(&mut reg, &[("melee", 3)]);
    let mut runtime = AiRuntime::new();
    spawn_player(&mut reg, player_spot());
    let x = spawn_at(&mut reg, 0, &graph, "jab_act");
    let b = spawn_at(&mut reg, 1, &graph, "slam_act");
    let d = spawn_at(&mut reg, 2, &graph, "idle");

    tick(&mut reg, &mut runtime);
    assert_eq!(held(&reg, x), vec!["jab"]);
    assert_eq!(wait_since(&reg, b), Some(1));
    enter(&mut reg, d, "jab_act");
    tick(&mut reg, &mut runtime);
    assert!(
        held(&reg, d).is_empty(),
        "B's cost-3 slam reserves all of melee"
    );

    enter(&mut reg, b, "jab_act");
    tick(&mut reg, &mut runtime);
    assert_eq!(
        held(&reg, b),
        vec!["jab"],
        "B's jab keeps B's place and fits"
    );
    assert_eq!(held(&reg, d), vec!["jab"], "D is granted on the same tick");
}

/// A committed vocabulary reaches live brains on the next tick: budgets
/// rebuild and draws re-resolve by name, so a reload that renumbers tokens
/// never leaves an attack drawing another token's id.
#[test]
fn committed_vocabulary_re_resolves_live_draws_by_name() {
    let graph = marked_graph(vec![("claw", held_attack(vec![token("melee")]))]);
    let mut reg = EntityRegistry::new();
    install_tokens(&mut reg, &[("melee", 3)]);
    let mut runtime = AiRuntime::new();
    spawn_player(&mut reg, player_spot());
    let claws: Vec<EntityId> = (0..3)
        .map(|index| spawn_at(&mut reg, index, &graph, "claw_act"))
        .collect();
    tick(&mut reg, &mut runtime);
    assert_eq!(holders(&reg, &claws), 3);

    // `heavy` now takes `melee`'s old id.
    install_tokens(&mut reg, &[("heavy", 1), ("melee", 3)]);
    for &claw in &claws {
        enter(&mut reg, claw, "idle");
    }
    tick(&mut reg, &mut runtime);
    assert_eq!(holders(&reg, &claws), 0);
    for &claw in &claws {
        enter(&mut reg, claw, "claw_act");
    }
    tick(&mut reg, &mut runtime);
    assert_eq!(
        holders(&reg, &claws),
        3,
        "claws draw `melee` at 3, not `heavy` at 1"
    );
}
