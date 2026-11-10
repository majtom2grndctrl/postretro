// The budget through the tick: how many attack at once, the hard gate, and
// who requests.

use super::*;

// B1: with no budgets and no draws, every attack draws `defaultToken` at
// limit 3, so a fourth ready attacker on one target waits.
#[test]
fn default_token_admits_three_and_queues_fourth() {
    let graph = striker(100_000.0);
    let mut reg = EntityRegistry::new();
    let mut runtime = AiRuntime::new();
    let pawn = spawn_player(&mut reg, player_spot());
    let enemies: Vec<EntityId> = (0..4)
        .map(|index| spawn_at(&mut reg, index, &graph, "strike"))
        .collect();

    for _ in 0..10 {
        tick(&mut reg, &mut runtime);
        assert_eq!(player_hp_lost(&reg, pawn), 3.0, "three attack at once");
        assert_eq!(holders(&reg, &enemies), 3);
    }
    let fourth = enemies[3];
    assert!(held(&reg, fourth).is_empty());
    assert_eq!(requested(&reg, fourth).as_deref(), Some("attack"));
    assert_eq!(
        wait_since(&reg, fourth),
        Some(1),
        "it has waited since its first request"
    );
    assert_eq!(fired(&reg, fourth), 0);
}

// B2 / P1: under limit 2, a third requester never starts until a holder
// releases, and is granted (and fires) on the tick the release is known.
#[test]
fn third_requester_waits_until_a_holder_releases() {
    let graph = striker(160.0);
    let mut reg = EntityRegistry::new();
    let mut runtime = AiRuntime::new();
    set_limits(&mut runtime, &[2]);
    spawn_player(&mut reg, player_spot());
    let enemies: Vec<EntityId> = (0..3)
        .map(|index| spawn_at(&mut reg, index, &graph, "strike"))
        .collect();
    let third = enemies[2];

    let mut third_fired_at = None;
    let mut first_release_at = None;
    for tick_index in 1..=20 {
        tick(&mut reg, &mut runtime);
        assert!(holders(&reg, &enemies) <= 2, "tick {tick_index}");
        if first_release_at.is_none()
            && enemies[..2]
                .iter()
                .any(|&enemy| !turns(&reg, enemy).holds_any())
        {
            first_release_at = Some(tick_index);
        }
        if third_fired_at.is_none() && fired(&reg, third) == 1 {
            third_fired_at = Some(tick_index);
        }
    }
    let released = first_release_at.expect("a holder's cooldown ends");
    assert!(
        released > 1,
        "holders keep their turns through their cooldown"
    );
    assert_eq!(
        third_fired_at,
        Some(released),
        "the waiter is granted and fires on the release tick"
    );
}

// B11: one enemy's turns on separate tokens overlap; its second melee attack
// waits for its melee turn to end, and its wait starts only then.
#[test]
fn one_enemy_holds_melee_and_ranged_turns_at_once() {
    let graph = graph(
        "claw_act",
        vec![
            ("claw_act", striking("claw")),
            ("rifle_act", striking("rifle")),
            ("claw2_act", striking("claw2")),
        ],
        vec![],
        vec![
            ("claw", contact(1.0, 160.0)),
            ("rifle", contact(1.0, 100_000.0)),
            ("claw2", contact(1.0, 100_000.0)),
        ],
    );
    let mut reg = EntityRegistry::new();
    let mut runtime = AiRuntime::new();
    set_limits(&mut runtime, &[1, 1]);
    spawn_player(&mut reg, player_spot());
    let enemy = spawn_at(&mut reg, 0, &graph, "claw_act");
    // Another melee attacker waits on the same target from the first tick.
    let rival = spawn_at(&mut reg, 1, &striker(100_000.0), "strike");
    set_draws(&reg, &mut runtime, &[], enemy, "claw", &[(MELEE, 1)]);
    set_draws(&reg, &mut runtime, &[], enemy, "rifle", &[(RANGED, 1)]);
    set_draws(&reg, &mut runtime, &[], enemy, "claw2", &[(MELEE, 1)]);

    tick(&mut reg, &mut runtime);
    assert_eq!(held(&reg, enemy), vec!["claw"]);
    assert_eq!(wait_since(&reg, rival), Some(1));

    enter(&mut reg, enemy, "rifle_act");
    tick(&mut reg, &mut runtime);
    assert_eq!(
        held(&reg, enemy),
        vec!["claw", "rifle"],
        "melee and ranged overlap"
    );

    enter(&mut reg, enemy, "claw2_act");
    let mut claw_ended_at = None;
    for tick_index in 3..=30 {
        tick(&mut reg, &mut runtime);
        if held(&reg, enemy).contains(&"claw".to_string()) {
            assert_eq!(
                requested(&reg, enemy),
                None,
                "claw2 never rides the claw turn"
            );
            assert_eq!(
                wait_since(&reg, enemy),
                None,
                "and its wait has not started"
            );
        } else if claw_ended_at.is_none() {
            claw_ended_at = Some(tick_index);
        }
    }
    let ended = claw_ended_at.expect("the claw turn ends with its cooldown");
    assert_eq!(
        held(&reg, rival),
        vec!["attack"],
        "the older waiter takes melee"
    );
    assert_eq!(requested(&reg, enemy).as_deref(), Some("claw2"));
    assert_eq!(
        wait_since(&reg, enemy),
        Some(ended),
        "claw2's wait starts when the claw turn ends"
    );
    assert_eq!(held(&reg, enemy), vec!["rifle"]);
}

// B28 / P2: a refused attack commits nothing (no cooldown, count, event or
// damage) and fires on its first granted tick in the same activity.
#[test]
fn refused_attack_commits_nothing_and_fires_when_granted() {
    let graph = striker(160.0);
    let mut reg = EntityRegistry::new();
    let mut runtime = AiRuntime::new();
    set_limits(&mut runtime, &[1]);
    let pawn = spawn_player(&mut reg, player_spot());
    let holder = spawn_at(&mut reg, 0, &graph, "strike");
    let refused = spawn_at(&mut reg, 1, &graph, "strike");

    let events = run_ai_tick(&mut reg, &mut runtime, DT);
    assert_eq!(
        events,
        vec![ENEMY_ATTACK_EVENT],
        "one attack event, the holder's"
    );
    assert_eq!(player_hp_lost(&reg, pawn), 1.0);
    let brain = reg.get_component::<BrainComponent>(refused).unwrap();
    assert!(
        !brain.attack_cooldown_remaining_ms.contains_key("attack"),
        "no cooldown"
    );
    assert_eq!(
        fired(&reg, refused),
        0,
        "no count; the once-per-entry fire stays open"
    );
    assert_eq!(fired(&reg, holder), 1);

    let mut granted_at = None;
    for tick_index in 2..=20 {
        tick(&mut reg, &mut runtime);
        if granted_at.is_none() && held(&reg, refused) == vec!["attack"] {
            granted_at = Some(tick_index);
            assert_eq!(
                fired(&reg, refused),
                1,
                "it fires on its first granted tick"
            );
            assert_eq!(state(&reg, refused), "strike", "in the same activity");
            assert_eq!(player_hp_lost(&reg, pawn), 2.0);
        }
    }
    assert!(granted_at.is_some(), "the refused attacker is granted");
}

// B29: under limit 1, three contact attackers ready together. At most one
// holds a turn on any tick; each holds until its own cooldown ends, even
// after leaving its attacking activity; each attacks within a bounded window.
#[test]
fn contact_attackers_take_bounded_turns_under_limit_one() {
    const COOLDOWN_TICKS: usize = 10;
    let graph = cycling_striker(COOLDOWN_TICKS as f32 * 16.0);
    let mut reg = EntityRegistry::new();
    let mut runtime = AiRuntime::new();
    set_limits(&mut runtime, &[1]);
    spawn_player(&mut reg, player_spot());
    let enemies: Vec<EntityId> = (0..3)
        .map(|index| spawn_at(&mut reg, index, &graph, "strike"))
        .collect();

    let mut grants: Vec<Vec<usize>> = vec![Vec::new(); 3];
    let mut was_holding = [false; 3];
    let mut held_while_resting = false;
    for tick_index in 1..=150 {
        tick(&mut reg, &mut runtime);
        assert!(holders(&reg, &enemies) <= 1, "tick {tick_index}");
        for (index, &enemy) in enemies.iter().enumerate() {
            let holding = turns(&reg, enemy).holds_any();
            if holding && !was_holding[index] {
                grants[index].push(tick_index);
            }
            held_while_resting |= holding && state(&reg, enemy) == "rest";
            was_holding[index] = holding;
        }
    }
    assert!(
        held_while_resting,
        "a holder keeps its turn after leaving `strike` until its cooldown ends"
    );
    let window = 3 * (COOLDOWN_TICKS + 2);
    for (index, ticks) in grants.iter().enumerate() {
        assert!(ticks.len() >= 3, "enemy {index} keeps attacking: {ticks:?}");
        assert!(
            ticks[0] <= window,
            "enemy {index} first attacks late: {ticks:?}"
        );
        assert!(
            ticks.windows(2).all(|pair| pair[1] - pair[0] <= window),
            "enemy {index} waits past the bounded window: {ticks:?}"
        );
    }
}

// B33: an unmarked attack requests no turn while out of range or line of
// sight, so a ready neighbor under limit 1 is granted at once.
#[test]
fn unmarked_attack_requests_only_when_fire_gates_pass() {
    let graph = striker(100_000.0);
    let mut reg = EntityRegistry::new();
    let mut runtime = AiRuntime::new();
    set_limits(&mut runtime, &[1]);
    let pawn = spawn_player(&mut reg, player_spot());
    // Earlier slots than the ready attacker, so either would win a tie.
    let out_of_range = spawn_enemy(
        &mut reg,
        Vec3::new(-3.0, 0.0, 0.0),
        authored_brain(&graph, "strike"),
        50.0,
    );
    let behind_wall = spawn_enemy(
        &mut reg,
        Vec3::new(0.0, 0.0, 0.0),
        authored_brain(&graph, "strike"),
        50.0,
    );
    let ready = spawn_enemy(
        &mut reg,
        Vec3::new(1.5, 0.0, 1.6),
        authored_brain(&graph, "strike"),
        50.0,
    );
    set_enemy_yaw(&mut reg, ready, std::f32::consts::PI);
    let wall = wall_world(0.75, 0.0, 3.0);

    run_ai_tick_with_navigation(&mut reg, &mut runtime, DT, None, Some(&wall));
    for enemy in [out_of_range, behind_wall] {
        assert_eq!(requested(&reg, enemy), None);
        assert_eq!(wait_since(&reg, enemy), None);
    }
    assert_eq!(held(&reg, ready), vec!["attack"], "granted at once");
    assert_eq!(player_hp_lost(&reg, pawn), 1.0);
}
