// Refresh through the tick: a spent draw stays counted after its turn ends,
// however it ends, for its token's `refreshMs` (pins P7, P26, P28, P29, P30).

use postretro_entities::components::brain::AttackPoolId;
use serde_json::json;

use super::*;

/// Registered ids: `defaultToken` is 0, then `melee`, then `heavy`.
const HEAVY_ID: AttackTokenId = AttackTokenId(2);
/// Ticks in 4 s at `DT`.
const FOUR_SECONDS: usize = 250;

/// Register `melee` and `heavy` as (limit, refreshMs), with `budgets` as the
/// mod's `attackBudgets`.
pub(super) fn install_budgets(
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

pub(super) fn slam_and_jab() -> BehaviorGraphDescriptor {
    marked_graph(vec![
        ("slam", held_attack(vec![token("melee"), token("heavy")])),
        ("jab", held_attack(vec![token("melee")])),
    ])
}

pub(super) fn heavy_refreshing(runtime: &AiRuntime, target: EntityId) -> u32 {
    runtime
        .turns
        .refreshing_now((AttackPoolId::Default, target), HEAVY_ID)
}

// B12 / P26: under `heavy` at { limit 1, refreshMs 4000 }, a second slam is
// granted no sooner than 4 s after the first's turn ends, ahead of a later
// slam, while jabs keep being granted on `melee`.
#[test]
fn heavy_refresh_spaces_slams_while_jabs_draw_melee() {
    let graph = slam_and_jab();
    let mut reg = EntityRegistry::new();
    install_budgets(&mut reg, (3, 0.0), (1, 4000.0), json!({}));
    let mut runtime = AiRuntime::new();
    spawn_player(&mut reg, player_spot());
    let first = spawn_at(&mut reg, 0, &graph, "slam_act");
    let second = spawn_at(&mut reg, 1, &graph, "idle");
    let jabs = [
        spawn_at(&mut reg, 2, &graph, "idle"),
        spawn_at(&mut reg, 3, &graph, "idle"),
    ];
    let late = spawn_at(&mut reg, 4, &graph, "idle");

    tick(&mut reg, &mut runtime);
    assert_eq!(held(&reg, first), vec!["slam"]);
    enter(&mut reg, second, "slam_act");
    tick(&mut reg, &mut runtime);
    assert_eq!(requested(&reg, second).as_deref(), Some("slam"));

    enter(&mut reg, first, "idle");
    tick(&mut reg, &mut runtime);
    assert!(held(&reg, first).is_empty(), "the first slam's turn ends");
    for jab in jabs {
        enter(&mut reg, jab, "jab_act");
    }
    enter(&mut reg, late, "slam_act");

    let mut jab_grants = 0;
    for elapsed in 1..FOUR_SECONDS {
        // One jab leaves every 50 ticks and asks again on the next.
        if elapsed % 50 == 0 {
            enter(&mut reg, jabs[0], "idle");
        } else if elapsed % 50 == 1 && elapsed > 1 {
            enter(&mut reg, jabs[0], "jab_act");
        }
        let before = held(&reg, jabs[0]).is_empty();
        tick(&mut reg, &mut runtime);
        if before && !held(&reg, jabs[0]).is_empty() {
            jab_grants += 1;
        }
        assert!(
            held(&reg, second).is_empty() && held(&reg, late).is_empty(),
            "{elapsed} ticks after the turn ended, heavy is still refreshing"
        );
    }
    assert!(
        jab_grants >= 4,
        "jabs keep drawing melee during the refresh ({jab_grants})"
    );

    tick(&mut reg, &mut runtime);
    assert_eq!(
        held(&reg, second),
        vec!["slam"],
        "granted 4 s after the first slam's turn ended"
    );
    assert!(held(&reg, late).is_empty(), "ahead of the later slam");
}

// B13 / P28: under `heavy` at { limit 2, refreshMs 4000 }, a refreshing count
// counts for admission and for head of line. A cost-3 draw waiting behind it
// is granted the tick the refresh lapses, and no later cost-1 draw is granted
// past it in the meantime.
#[test]
fn refreshing_count_holds_admission_and_head_of_line() {
    let graph = marked_graph(vec![
        ("strike", held_attack(vec![token("heavy")])),
        ("crush", held_attack(vec![costing("heavy", 3)])),
    ]);
    let mut reg = EntityRegistry::new();
    install_budgets(&mut reg, (3, 0.0), (2, 4000.0), json!({}));
    let mut runtime = AiRuntime::new();
    spawn_player(&mut reg, player_spot());
    let holder = spawn_at(&mut reg, 0, &graph, "strike_act");
    let crusher = spawn_at(&mut reg, 1, &graph, "idle");
    let strikers = [
        spawn_at(&mut reg, 2, &graph, "idle"),
        spawn_at(&mut reg, 3, &graph, "idle"),
    ];

    tick(&mut reg, &mut runtime);
    assert_eq!(held(&reg, holder), vec!["strike"]);
    enter(&mut reg, crusher, "crush_act");
    tick(&mut reg, &mut runtime);
    for striker in strikers {
        enter(&mut reg, striker, "strike_act");
    }
    tick(&mut reg, &mut runtime);
    assert!(
        strikers
            .iter()
            .all(|&striker| held(&reg, striker).is_empty()),
        "the crusher heads heavy and reserves it"
    );

    enter(&mut reg, holder, "idle");
    for elapsed in 0..FOUR_SECONDS {
        tick(&mut reg, &mut runtime);
        assert!(
            [crusher, strikers[0], strikers[1]]
                .iter()
                .all(|&enemy| held(&reg, enemy).is_empty()),
            "{elapsed} ticks after the turn ended: the refreshing count holds the line"
        );
    }
    tick(&mut reg, &mut runtime);
    assert_eq!(
        held(&reg, crusher),
        vec!["crush"],
        "granted the tick nothing is counted against heavy"
    );
    assert!(
        strikers
            .iter()
            .all(|&striker| held(&reg, striker).is_empty())
    );
}

#[derive(Clone, Copy, Debug)]
enum Interruption {
    GrantNeverStarts,
    TargetSwitch,
    HolderKilled,
    ReloadChangesDraws,
}

/// The `brute` descriptor carrying `graph`, as the live data registry would.
fn brute_descriptor(graph: &BehaviorGraphDescriptor) -> EntityTypeDescriptor {
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

/// Mark `enemy` as materialized from the `brute` descriptor.
fn from_brute(reg: &mut EntityRegistry, enemy: EntityId) {
    use postretro_entities::provenance::{DescriptorProvenance, DescriptorSpawnPath};
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
}

/// One tick against `descriptors` at `generation`.
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

// B14 / P29: a heavy turn that ends before its attack lands still shuts
// `heavy` for 4 s from the tick it ends: a grant that never starts, a target
// switch, the holder killed, and a reload that changes its `draws`.
#[test]
fn interrupted_turn_spends_its_refresh() {
    for case in [
        Interruption::GrantNeverStarts,
        Interruption::TargetSwitch,
        Interruption::HolderKilled,
        Interruption::ReloadChangesDraws,
    ] {
        let mut reg = EntityRegistry::new();
        install_budgets(&mut reg, (3, 0.0), (1, 4000.0), json!({}));
        let mut runtime = AiRuntime::new();
        let pawn = spawn_player(&mut reg, player_spot());
        let mut descriptors = Vec::new();
        let mut generation = 0;

        let (holder, target) = match case {
            Interruption::GrantNeverStarts => {
                // An undrawn killer earlier in the pass leaves the heavy
                // grant nothing to land on.
                let killer = graph(
                    "strike",
                    vec![("strike", striking("attack"))],
                    vec![],
                    vec![("attack", drawing(contact(100.0, 100_000.0), vec![]))],
                );
                let later = graph(
                    "strike",
                    vec![("strike", striking("attack"))],
                    vec![],
                    vec![("attack", held_attack(vec![token("heavy")]))],
                );
                spawn_at(&mut reg, 0, &killer, "strike");
                let holder = spawn_at(&mut reg, 1, &later, "strike");
                tick_at(&mut reg, &mut runtime, &descriptors, generation);
                assert_eq!(player_hp(&reg, pawn), 0.0, "fixture: the target dies");
                (holder, pawn)
            }
            Interruption::TargetSwitch => {
                let far = spawn_player(&mut reg, Vec3::new(1.5, 0.0, 12.0));
                let holder = spawn_at(&mut reg, 0, &slam_and_jab(), "slam_act");
                tick_at(&mut reg, &mut runtime, &descriptors, generation);
                assert_eq!(turns(&reg, holder).held().next().unwrap().target, pawn);
                // The two players trade places; the holder takes the near one.
                let mut moved = *reg.get_component::<Transform>(pawn).unwrap();
                moved.position = Vec3::new(1.5, 0.0, 40.0);
                reg.set_component(pawn, moved).unwrap();
                let mut moved = *reg.get_component::<Transform>(far).unwrap();
                moved.position = player_spot();
                reg.set_component(far, moved).unwrap();
                let mut switched = false;
                for _ in 0..60 {
                    tick_at(&mut reg, &mut runtime, &descriptors, generation);
                    if turns(&reg, holder).held().all(|turn| turn.target != pawn) {
                        switched = true;
                        break;
                    }
                }
                assert!(switched, "fixture: the holder switches target");
                (holder, pawn)
            }
            Interruption::HolderKilled => {
                let holder = spawn_at(&mut reg, 0, &slam_and_jab(), "slam_act");
                tick_at(&mut reg, &mut runtime, &descriptors, generation);
                set_hp(&mut reg, holder, 0.0);
                tick_at(&mut reg, &mut runtime, &descriptors, generation);
                (holder, pawn)
            }
            Interruption::ReloadChangesDraws => {
                let original = slam_and_jab();
                let holder = spawn_at(&mut reg, 0, &original, "slam_act");
                from_brute(&mut reg, holder);
                descriptors.push(brute_descriptor(&original));
                tick_at(&mut reg, &mut runtime, &descriptors, generation);
                assert_eq!(heavy_refreshing(&runtime, pawn), 0);
                let mut reloaded = original.clone();
                reloaded.attacks.get_mut("slam").unwrap().draws = Some(vec![token("melee")]);
                descriptors[0] = brute_descriptor(&reloaded);
                generation = 1;
                tick_at(&mut reg, &mut runtime, &descriptors, generation);
                (holder, pawn)
            }
        };

        // A killed holder's component keeps its stale turns, uncounted,
        // until it returns to the AI set and drops them.
        if !matches!(case, Interruption::HolderKilled) {
            assert!(
                turns(&reg, holder).held().all(|turn| turn.target != target
                    || turn
                        .draws
                        .as_slice()
                        .iter()
                        .all(|draw| draw.token != HEAVY_ID)),
                "{case:?}: no heavy turn on the target stands"
            );
        }
        assert_eq!(
            heavy_refreshing(&runtime, target),
            1,
            "{case:?}: heavy refreshes from the tick the turn ended"
        );
        for _ in 1..FOUR_SECONDS {
            tick_at(&mut reg, &mut runtime, &descriptors, generation);
        }
        assert_eq!(
            heavy_refreshing(&runtime, target),
            1,
            "{case:?}: still counted just short of 4 s"
        );
        tick_at(&mut reg, &mut runtime, &descriptors, generation);
        assert_eq!(
            heavy_refreshing(&runtime, target),
            0,
            "{case:?}: lapsed 4 s after the turn ended"
        );
    }
}

// B15 / P30, through the tick: a level change drops every refreshing count,
// so the next level's first waiter is granted at once, and a target leaving
// the world drops its tallies. The pool half is
// `refresh_lands_in_pool_at_turn_end_and_drops_on_stop`.
#[test]
fn refresh_drops_on_level_change_and_target_loss() {
    let graph = slam_and_jab();
    let mut reg = EntityRegistry::new();
    install_budgets(&mut reg, (3, 0.0), (1, 4000.0), json!({}));
    let mut runtime = AiRuntime::new();
    let pawn = spawn_player(&mut reg, player_spot());
    let holder = spawn_at(&mut reg, 0, &graph, "slam_act");
    let waiter = spawn_at(&mut reg, 1, &graph, "slam_act");
    tick(&mut reg, &mut runtime);
    enter(&mut reg, holder, "idle");
    tick(&mut reg, &mut runtime);
    tick(&mut reg, &mut runtime);
    assert!(held(&reg, waiter).is_empty(), "heavy is refreshing");
    assert_eq!(heavy_refreshing(&runtime, pawn), 1);

    runtime.clear_level_state();
    assert_eq!(runtime.turns.refreshing_entries(), 0);
    tick(&mut reg, &mut runtime);
    assert_eq!(
        held(&reg, waiter),
        vec!["slam"],
        "nothing refreshing survives the level"
    );

    // The waiter's turn ends, then its target leaves the world.
    enter(&mut reg, waiter, "idle");
    tick(&mut reg, &mut runtime);
    assert_eq!(heavy_refreshing(&runtime, pawn), 1);
    reg.despawn(pawn).unwrap();
    tick(&mut reg, &mut runtime);
    assert_eq!(
        runtime.turns.refreshing_entries(),
        0,
        "a target leaving the world drops its tallies"
    );
}

fn attack_turn_free(attack: &str) -> IrNode {
    brain_input(&format!("@attackTurnFree.{attack}"))
}

// B16 / P7: `attackTurnFree("slam")` is false while heavy's refreshing count
// leaves no room, and true from the tick after the refresh lapses.
#[test]
fn attack_turn_free_reads_refresh_with_one_tick_lag() {
    let slam = held_attack(vec![token("melee"), token("heavy")]);
    let graph = graph(
        "idle",
        vec![
            ("idle", idle()),
            ("slam_act", marked(striking("slam"), "slam")),
            ("wait", idle()),
            ("free", idle()),
        ],
        vec![("wait", vec![edge("free", attack_turn_free("slam"))])],
        vec![("slam", slam)],
    );
    let mut reg = EntityRegistry::new();
    install_budgets(&mut reg, (3, 0.0), (1, 4000.0), json!({}));
    let mut runtime = AiRuntime::new();
    spawn_player(&mut reg, player_spot());
    let holder = spawn_at(&mut reg, 0, &graph, "slam_act");
    let reader = spawn_at(&mut reg, 1, &graph, "idle");

    tick(&mut reg, &mut runtime);
    assert_eq!(held(&reg, holder), vec!["slam"]);
    enter(&mut reg, reader, "wait");
    enter(&mut reg, holder, "idle");
    // The turn ends on this tick.
    tick(&mut reg, &mut runtime);
    assert!(held(&reg, holder).is_empty());
    for elapsed in 1..=FOUR_SECONDS {
        tick(&mut reg, &mut runtime);
        assert_eq!(
            state(&reg, reader),
            "wait",
            "{elapsed} ticks after: the fact reads the previous tick's refresh"
        );
    }
    // The refresh lapsed on the last tick; the fact sees it now.
    tick(&mut reg, &mut runtime);
    assert_eq!(state(&reg, reader), "free");
}
