// Turn lifetimes through the tick: marked chains and switches, unmarked holds
// through execution and cooldown, wait clocks, and target switches.

use super::*;

fn ge(input: &str, value: f32) -> IrNode {
    IrNode::Ge {
        a: Box::new(brain_input(input)),
        b: Box::new(IrNode::Const {
            value: IrValue::Number(value),
        }),
    }
}

fn fired_at_least_once() -> IrNode {
    ge(BRAIN_ATTACKS_FIRED_IN_ACTIVITY_INPUT, 1.0)
}

/// Marked `slam` and `jab` siblings, each firing its own attack.
fn slam_and_jab(slam: AttackParams) -> BehaviorGraphDescriptor {
    graph(
        "slam_act",
        vec![
            ("slam_act", marked(striking("slam"), "slam")),
            ("jab_act", marked(striking("jab"), "jab")),
        ],
        vec![],
        vec![("slam", slam), ("jab", contact(1.0, 100_000.0))],
    )
}

// B22 / P21: limit 2, cost 1. X holds; E holds a `slam` turn; W has waited
// longest. E entering a sibling marked for `jab` releases the slam turn
// before grants, and E's jab waits behind W, though E's slot is earlier.
#[test]
fn marked_sibling_switch_requests_behind_older_waiters() {
    let graph = slam_and_jab(contact(1.0, 100_000.0));
    let mut reg = EntityRegistry::new();
    let mut runtime = AiRuntime::new();
    set_limits(&mut runtime, &[2]);
    spawn_player(&mut reg, player_spot());
    let x = spawn_at(&mut reg, 0, &graph, "slam_act");
    let e = spawn_at(&mut reg, 1, &graph, "slam_act");
    let w = spawn_at(&mut reg, 2, &graph, "slam_act");
    for _ in 0..4 {
        tick(&mut reg, &mut runtime);
    }
    assert_eq!(held(&reg, x), vec!["slam"]);
    assert_eq!(held(&reg, e), vec!["slam"]);
    assert_eq!(wait_since(&reg, w), Some(1));

    enter(&mut reg, e, "jab_act");
    tick(&mut reg, &mut runtime);
    assert_eq!(
        held(&reg, w),
        vec!["slam"],
        "the older waiter is granted on N"
    );
    assert!(
        held(&reg, e).is_empty(),
        "E's slam turn ended with its mark"
    );
    assert_eq!(requested(&reg, e).as_deref(), Some("jab"));
    assert_eq!(wait_since(&reg, e), Some(5), "E's jab wait starts at N");
}

// B23 / P20: mid-execution of a multi-shot `slam`, entering a sibling marked
// for `jab` (same token) holds the slam turn until the execution clears. The
// jab request stands from N, its wait starting at N; it is granted no earlier
// than the slam turn frees, and no jab lands between slam shots.
#[test]
fn marked_switch_mid_execution_holds_turn_until_clear() {
    let descriptors = [burst_weapon("enemy.slam", 3, 64.0, 16.0)];
    let graph = slam_and_jab(weapon("enemy.slam"));
    let mut reg = EntityRegistry::new();
    let mut runtime = AiRuntime::new();
    let pawn = spawn_player(&mut reg, player_spot());
    let enemy = spawn_at(&mut reg, 0, &graph, "slam_act");

    let mut shots = tick_with(&mut reg, &mut runtime, &descriptors)
        .projectile_spawns
        .len();
    assert_eq!(shots, 1, "fixture: the burst starts on the grant tick");
    enter(&mut reg, enemy, "jab_act");

    let mut last_shot_at = 1;
    let mut jab_granted_at = None;
    for tick_index in 2..=30 {
        let spawned = tick_with(&mut reg, &mut runtime, &descriptors)
            .projectile_spawns
            .len();
        if spawned > 0 {
            shots += spawned;
            last_shot_at = tick_index;
            assert!(jab_granted_at.is_none(), "a slam shot under the jab turn");
        }
        if tick_index == 2 {
            assert_eq!(
                held(&reg, enemy),
                vec!["slam"],
                "the execution holds the turn"
            );
            assert_eq!(requested(&reg, enemy).as_deref(), Some("jab"));
            assert_eq!(wait_since(&reg, enemy), Some(2), "the jab waits from N");
        }
        if jab_granted_at.is_none() && held(&reg, enemy) == vec!["jab"] {
            jab_granted_at = Some(tick_index);
        }
        if jab_granted_at.is_none() {
            assert_eq!(
                player_hp_lost(&reg, pawn),
                0.0,
                "no jab lands before its turn"
            );
        }
    }
    assert_eq!(shots, 3, "the whole burst lands");
    assert_eq!(
        jab_granted_at,
        Some(last_shot_at + 1),
        "the jab is granted once the slam turn frees, the tick after its execution clears"
    );
}

// B24 / P25: an offense selector reads `attackCooldownMs`, so the cooldown
// seed resolves `jab` while the fire step resolves `slam`. The request names
// `slam`.
#[test]
fn request_names_fire_step_attack_not_cooldown_seed_attack() {
    let mut engage = idle();
    engage.layers = BTreeMap::from([(
        "offense".to_string(),
        BehaviorLayerDescriptor::Selector(vec![
            BehaviorSelectorEntry::Row(BehaviorSelectorRow {
                when: Some(IrNode::Le {
                    a: Box::new(brain_input(BRAIN_ATTACK_COOLDOWN_MS_INPUT)),
                    b: Box::new(IrNode::Const {
                        value: IrValue::Number(0.0),
                    }),
                }),
                motion: None,
                action: Some(ActionVerb::Attack("jab".to_string())),
            }),
            BehaviorSelectorEntry::Row(BehaviorSelectorRow {
                when: None,
                motion: None,
                action: Some(ActionVerb::Attack("slam".to_string())),
            }),
        ]),
    )]);
    let graph = graph(
        "engage",
        vec![("engage", engage)],
        vec![],
        vec![
            ("jab", contact(1.0, 100_000.0)),
            ("slam", contact(1.0, 100_000.0)),
        ],
    );
    let mut reg = EntityRegistry::new();
    let mut runtime = AiRuntime::new();
    set_limits(&mut runtime, &[1]);
    spawn_player(&mut reg, player_spot());
    let holder = spawn_at(&mut reg, 0, &striker(100_000.0), "strike");
    let mut brain = authored_brain(&graph, "engage");
    brain
        .attack_cooldown_remaining_ms
        .insert("jab".to_string(), 500.0);
    let enemy = spawn_enemy(&mut reg, attacker_spot(1), brain, 50.0);

    tick(&mut reg, &mut runtime);
    assert_eq!(held(&reg, holder), vec!["attack"]);
    assert_eq!(requested(&reg, enemy).as_deref(), Some("slam"));
}

// B25 / P22: a gap in requesting keeps the clock; a target switch restarts it
// behind the new target's older waiters.
#[test]
fn wait_clock_survives_gaps_and_restarts_on_target_switch() {
    let asker = graph(
        "ask",
        vec![
            ("ask", marked(striking("attack"), "attack")),
            ("away", idle()),
        ],
        vec![],
        vec![("attack", contact(1.0, 100_000.0))],
    );
    let mut reg = EntityRegistry::new();
    let mut runtime = AiRuntime::new();
    set_limits(&mut runtime, &[1]);
    let near = spawn_player(&mut reg, player_spot());
    let far = spawn_player(&mut reg, Vec3::new(1.5, 0.0, 6.0));
    let holder = spawn_at(&mut reg, 0, &striker(100_000.0), "strike");
    let waiter = spawn_at(&mut reg, 1, &asker, "ask");
    // The far player's own holder and older waiter.
    let far_holder = spawn_enemy(
        &mut reg,
        Vec3::new(0.0, 0.0, 6.0),
        authored_brain(&striker(100_000.0), "strike"),
        50.0,
    );
    let far_waiter = spawn_enemy(
        &mut reg,
        Vec3::new(0.0, 0.0, 6.2),
        authored_brain(&asker, "ask"),
        50.0,
    );

    for _ in 0..5 {
        tick(&mut reg, &mut runtime);
    }
    assert_eq!(held(&reg, holder), vec!["attack"]);
    assert_eq!(held(&reg, far_holder), vec!["attack"]);
    assert_eq!(wait_since(&reg, waiter), Some(1));
    assert_eq!(wait_since(&reg, far_waiter), Some(1));

    enter(&mut reg, waiter, "away");
    for _ in 0..4 {
        tick(&mut reg, &mut runtime);
        assert_eq!(requested(&reg, waiter), None, "a gap requests nothing");
        assert_eq!(wait_since(&reg, waiter), Some(1), "and keeps the clock");
    }
    enter(&mut reg, waiter, "ask");
    tick(&mut reg, &mut runtime);
    assert_eq!(requested(&reg, waiter).as_deref(), Some("attack"));
    assert_eq!(
        wait_since(&reg, waiter),
        Some(1),
        "requesting again keeps the clock"
    );

    reg.despawn(near).unwrap();
    let mut switched_at = None;
    for tick_index in 11..=40 {
        tick(&mut reg, &mut runtime);
        let request = turns(&reg, waiter).request;
        if switched_at.is_none() && request.is_some_and(|request| request.target == far) {
            switched_at = Some(tick_index);
        }
    }
    let switched = switched_at.expect("the waiter takes the far player");
    assert_eq!(
        wait_since(&reg, waiter),
        Some(switched),
        "a target switch restarts the clock"
    );
    assert_eq!(held(&reg, far_holder), vec!["attack"]);
    assert_eq!(
        wait_since(&reg, far_waiter),
        Some(1),
        "and queues it behind the new target's older waiter"
    );
}

// B30: an enemy alternating two unmarked attacks drawing one token never
// starts the second under the first's turn; the second requests once that
// turn ends.
#[test]
fn unmarked_attacks_sharing_a_token_never_overlap() {
    let graph = graph(
        "jab_act",
        vec![("jab_act", striking("jab")), ("slam_act", striking("slam"))],
        vec![
            ("jab_act", vec![edge("slam_act", fired_at_least_once())]),
            ("slam_act", vec![edge("jab_act", fired_at_least_once())]),
        ],
        vec![("jab", contact(1.0, 160.0)), ("slam", contact(1.0, 160.0))],
    );
    let mut reg = EntityRegistry::new();
    let mut runtime = AiRuntime::new();
    set_limits(&mut runtime, &[1]);
    let pawn = spawn_player(&mut reg, player_spot());
    let enemy = spawn_at(&mut reg, 0, &graph, "jab_act");

    let mut grants = Vec::new();
    let mut previous: Vec<String> = Vec::new();
    for tick_index in 1..=60 {
        tick(&mut reg, &mut runtime);
        let now = held(&reg, enemy);
        assert!(
            now.len() <= 1,
            "two turns on one token at tick {tick_index}"
        );
        if !now.is_empty() && now != previous {
            grants.push((tick_index, now[0].clone()));
        }
        if !now.is_empty() && state(&reg, enemy) != format!("{}_act", now[0]) {
            assert_eq!(
                requested(&reg, enemy),
                None,
                "the other attack never requests under a held turn"
            );
        }
        previous = now;
    }
    assert!(
        grants.len() >= 4,
        "the attacks keep alternating: {grants:?}"
    );
    for pair in grants.windows(2) {
        assert_ne!(pair[0].1, pair[1].1, "turns alternate: {grants:?}");
        assert!(
            pair[1].0 - pair[0].0 >= 10,
            "the second starts only after the first's cooldown: {grants:?}"
        );
    }
    assert_eq!(player_hp_lost(&reg, pawn), grants.len() as f32);
}

// B31 / P4: under limit 1, an unmarked three-shot weapon attack holds its
// turn through the execution and its cooldown; a ready neighbor never starts
// between shots, and is granted once that turn ends. The first enemy
// multi-shot burst through the AI tick.
#[test]
fn unmarked_burst_holds_turn_through_execution_and_cooldown() {
    let descriptors = [burst_weapon("enemy.burst", 3, 64.0, 160.0)];
    let burst = graph(
        "strike",
        vec![("strike", striking("attack"))],
        vec![],
        vec![("attack", weapon("enemy.burst"))],
    );
    let mut reg = EntityRegistry::new();
    let mut runtime = AiRuntime::new();
    set_limits(&mut runtime, &[1]);
    let pawn = spawn_player(&mut reg, player_spot());
    let shooter = spawn_at(&mut reg, 0, &burst, "strike");
    let neighbor = spawn_at(&mut reg, 1, &striker(100_000.0), "strike");

    let mut shot_ticks = Vec::new();
    let mut released_at = None;
    let mut neighbor_fired_at = None;
    for tick_index in 1..=60 {
        let result = tick_with(&mut reg, &mut runtime, &descriptors);
        shot_ticks.extend(result.projectile_spawns.iter().map(|_| tick_index));
        if released_at.is_none() && tick_index > 1 && !turns(&reg, shooter).holds_any() {
            released_at = Some(tick_index);
        }
        if neighbor_fired_at.is_none() && player_hp_lost(&reg, pawn) > 0.0 {
            neighbor_fired_at = Some(tick_index);
        }
    }
    assert_eq!(
        shot_ticks.len(),
        3,
        "the burst fires three shots: {shot_ticks:?}"
    );
    assert!(
        shot_ticks.windows(2).all(|pair| pair[0] < pair[1]),
        "over later ticks"
    );
    let last_shot = *shot_ticks.last().unwrap();
    let released = released_at.expect("the turn ends with the cooldown");
    assert!(
        released >= last_shot + 10,
        "the turn holds through the execution and the 160 ms cooldown"
    );
    assert_eq!(
        neighbor_fired_at,
        Some(released),
        "the neighbor starts only once the turn frees"
    );
    assert_eq!(held(&reg, neighbor), vec!["attack"]);
}

// B32 / P3: a granted unmarked attack that does not start that tick (an
// earlier outcome kills its target) releases its turn; nothing commits.
#[test]
fn granted_unmarked_attack_that_cannot_start_releases_same_tick() {
    let mut killing = striker(100_000.0);
    killing.attacks.get_mut("attack").unwrap().damage = Some(100.0);
    let mut reg = EntityRegistry::new();
    let mut runtime = AiRuntime::new();
    let pawn = spawn_player(&mut reg, player_spot());
    let killer = spawn_at(&mut reg, 0, &killing, "strike");
    let later = spawn_at(&mut reg, 1, &striker(100_000.0), "strike");

    tick(&mut reg, &mut runtime);
    assert_eq!(player_hp(&reg, pawn), 0.0);
    assert_eq!(held(&reg, killer), vec!["attack"]);
    assert!(
        held(&reg, later).is_empty(),
        "the unstarted grant is released that tick"
    );
    assert_eq!(fired(&reg, later), 0);
    assert!(
        !reg.get_component::<BrainComponent>(later)
            .unwrap()
            .attack_cooldown_remaining_ms
            .contains_key("attack")
    );
}

/// `engage` nests an offense chain marked for `slam` from windup through
/// recover, then rests unmarked.
fn marked_chain(slam: AttackParams) -> BehaviorGraphDescriptor {
    let offense = BehaviorLayerDescriptor::Graph(BehaviorGraphEnvelope {
        initial: "windup".to_string(),
        activities: BTreeMap::from([
            ("windup".to_string(), marked(idle(), "slam")),
            ("commit".to_string(), marked(striking("slam"), "slam")),
            ("recover".to_string(), marked(idle(), "slam")),
            ("rest".to_string(), idle()),
        ]),
        transitions: BTreeMap::from([
            (
                "windup".to_string(),
                vec![edge("commit", ge(BRAIN_TIME_IN_ACTIVITY_MS_INPUT, 32.0))],
            ),
            (
                "commit".to_string(),
                vec![edge("recover", fired_at_least_once())],
            ),
            (
                "recover".to_string(),
                vec![edge("rest", ge(BRAIN_TIME_IN_ACTIVITY_MS_INPUT, 16.0))],
            ),
        ]),
    });
    // Chasing keeps the target through the actionless phases.
    let mut engage = authored_state("walk", MotionVerb::ChaseTarget, None);
    engage.layers = BTreeMap::from([("offense".to_string(), offense)]);
    graph(
        "engage",
        vec![("engage", engage)],
        vec![],
        vec![("slam", slam)],
    )
}

// B34: a turn holds across marked siblings for one attack with no release
// and regrant gap, and holds after leaving the marked chain mid-burst until
// the execution clears.
#[test]
fn marked_chain_holds_one_turn_without_regrant_gap() {
    let descriptors = [burst_weapon("enemy.slam", 3, 64.0, 16.0)];
    let graph = marked_chain(weapon("enemy.slam"));
    let mut reg = EntityRegistry::new();
    let mut runtime = AiRuntime::new();
    spawn_player(&mut reg, player_spot());
    let enemy = spawn_at(&mut reg, 0, &graph, "engage");

    let mut states = Vec::new();
    let mut holding = Vec::new();
    let mut shot_ticks = Vec::new();
    for tick_index in 1..=40 {
        let result = tick_with(&mut reg, &mut runtime, &descriptors);
        shot_ticks.extend(result.projectile_spawns.iter().map(|_| tick_index));
        states.push(enemy_state_path(&reg, enemy));
        holding.push(turns(&reg, enemy).holds_any());
        if turns(&reg, enemy).holds_any() {
            assert_eq!(
                wait_since(&reg, enemy),
                None,
                "a held chain never re-requests"
            );
        }
    }
    assert!(holding[0], "granted on windup entry");
    let released = holding
        .iter()
        .position(|held| !held)
        .expect("the turn ends after the burst");
    assert!(
        holding[..released].iter().all(|held| *held),
        "held without a gap across windup, commit and recover"
    );
    assert!(
        holding[released..].iter().all(|held| !held),
        "and never granted again"
    );
    for phase in ["windup", "commit", "recover", "rest"] {
        assert!(
            states[..released]
                .iter()
                .any(|state| state.ends_with(phase)),
            "the turn spans {phase}: {states:?}"
        );
    }
    assert_eq!(shot_ticks.len(), 3, "{shot_ticks:?}");
    let tick_of = |index: usize| index + 1;
    let rest_at = tick_of(
        states
            .iter()
            .position(|state| state.ends_with("rest"))
            .unwrap(),
    );
    let last_shot = *shot_ticks.last().unwrap();
    assert!(rest_at < last_shot, "fixture: the chain is left mid-burst");
    assert_eq!(
        tick_of(released),
        last_shot + 1,
        "the turn frees the tick after the execution clears"
    );
}

// B35 / P6: a target switch mid-burst cancels the burst and releases the
// turn; no shot of that burst lands on the new target.
#[test]
fn target_switch_mid_burst_cancels_execution_and_releases() {
    let descriptors = [burst_weapon("enemy.burst", 3, 64.0, 160.0)];
    let burst = graph(
        "strike",
        vec![("strike", striking("attack"))],
        vec![],
        vec![("attack", weapon("enemy.burst"))],
    );
    let mut reg = EntityRegistry::new();
    let mut runtime = AiRuntime::new();
    let first = spawn_player(&mut reg, player_spot());
    let second = spawn_player(&mut reg, Vec3::new(1.5, 0.0, 0.4));
    let shooter = spawn_at(&mut reg, 0, &burst, "strike");

    let shots = tick_with(&mut reg, &mut runtime, &descriptors)
        .projectile_spawns
        .len();
    assert_eq!(shots, 1);
    assert_eq!(enemy_acquired_target(&reg, shooter), Some(first));
    assert_eq!(held(&reg, shooter), vec!["attack"]);

    reg.despawn(first).unwrap();
    let mut later_shots = 0;
    for tick_index in 2..=30 {
        later_shots += tick_with(&mut reg, &mut runtime, &descriptors)
            .projectile_spawns
            .len();
        if tick_index == 2 {
            assert_eq!(
                enemy_acquired_target(&reg, shooter),
                Some(second),
                "fixture: the switch lands on one tick"
            );
            assert!(
                held(&reg, shooter).is_empty(),
                "the turn on the old target frees"
            );
        }
    }
    assert_eq!(
        later_shots, 0,
        "the cancelled burst lands nothing on the new target"
    );
}

// P6 for every release cause: a reload that changes the attack's draws
// mid-burst ends its turn and cancels the burst, so no shot fires without a
// turn. Regression: only a target switch cancelled; the burst fired on.
#[test]
fn draws_change_mid_burst_cancels_execution() {
    let descriptors = [burst_weapon("enemy.burst", 3, 64.0, 160.0)];
    let burst = graph(
        "strike",
        vec![("strike", striking("attack"))],
        vec![],
        vec![("attack", weapon("enemy.burst"))],
    );
    let mut reg = EntityRegistry::new();
    let mut runtime = AiRuntime::new();
    spawn_player(&mut reg, player_spot());
    let shooter = spawn_at(&mut reg, 0, &burst, "strike");

    let shots = tick_with(&mut reg, &mut runtime, &descriptors)
        .projectile_spawns
        .len();
    assert_eq!(shots, 1);
    assert_eq!(held(&reg, shooter), vec!["attack"]);

    // The reload re-resolves the attack to `defaultToken` at cost 2.
    set_draws(
        &reg,
        &mut runtime,
        &descriptors,
        shooter,
        "attack",
        &[(0, 2)],
    );
    let mut later_shots = 0;
    for tick_index in 2..=30 {
        later_shots += tick_with(&mut reg, &mut runtime, &descriptors)
            .projectile_spawns
            .len();
        if tick_index == 2 {
            assert!(
                held(&reg, shooter).is_empty(),
                "the draws change ends the turn"
            );
        }
    }
    assert_eq!(later_shots, 0, "the burst ends with its turn");
}
