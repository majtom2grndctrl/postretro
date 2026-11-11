// The three turn facts through the tick, read by guards (pins P7, P23).

use super::*;

fn reads(name: &str) -> IrNode {
    brain_input(name)
}

fn attack_turn_held(attack: &str) -> IrNode {
    reads(&format!("@attackTurnHeld.{attack}"))
}

fn attack_turn_free(attack: &str) -> IrNode {
    reads(&format!("@attackTurnFree.{attack}"))
}

fn not(node: IrNode) -> IrNode {
    IrNode::Not { x: Box::new(node) }
}

// B37 / P7: `hasAttackTurn` first reads true the tick after the grant, and
// stays true while any turn is held, whatever the path's current action.
#[test]
fn has_attack_turn_reads_the_previous_ticks_turns() {
    let graph = graph(
        "strike",
        vec![
            ("strike", striking("slam")),
            ("holding", idle()),
            ("lost", idle()),
        ],
        vec![
            (
                "strike",
                vec![edge("holding", reads(BRAIN_HAS_ATTACK_TURN_INPUT))],
            ),
            (
                "holding",
                vec![edge("lost", not(reads(BRAIN_HAS_ATTACK_TURN_INPUT)))],
            ),
        ],
        vec![("slam", contact(1.0, 100_000.0))],
    );
    let mut reg = EntityRegistry::new();
    let mut runtime = AiRuntime::new();
    spawn_player(&mut reg, player_spot());
    let enemy = spawn_at(&mut reg, 0, &graph, "strike");

    tick(&mut reg, &mut runtime);
    assert_eq!(held(&reg, enemy), vec!["slam"]);
    assert_eq!(
        state(&reg, enemy),
        "strike",
        "the grant tick still reads false"
    );
    tick(&mut reg, &mut runtime);
    assert_eq!(
        state(&reg, enemy),
        "holding",
        "true from the tick after the grant"
    );
    for _ in 0..5 {
        tick(&mut reg, &mut runtime);
    }
    assert_eq!(
        state(&reg, enemy),
        "holding",
        "true while held, though the path has no action"
    );
}

// B37: `attackTurnHeld("slam")` is false while the enemy holds only a
// `ranged` turn; `attackTurnHeld` of the held attack is true.
#[test]
fn attack_turn_held_names_one_attack() {
    let graph = graph(
        "shoot",
        vec![
            ("shoot", striking("rifle")),
            ("wrong", idle()),
            ("right", idle()),
        ],
        vec![(
            "shoot",
            vec![
                edge("wrong", attack_turn_held("slam")),
                edge("right", attack_turn_held("rifle")),
            ],
        )],
        vec![
            ("rifle", contact(1.0, 100_000.0)),
            ("slam", contact(1.0, 100_000.0)),
        ],
    );
    let mut reg = EntityRegistry::new();
    let mut runtime = AiRuntime::new();
    set_limits(&mut runtime, &[3, 2]);
    spawn_player(&mut reg, player_spot());
    let enemy = spawn_at(&mut reg, 0, &graph, "shoot");
    set_draws(&reg, &mut runtime, &[], enemy, "rifle", &[(RANGED, 1)]);

    tick(&mut reg, &mut runtime);
    tick(&mut reg, &mut runtime);
    assert_eq!(state(&reg, enemy), "right");
}

// B37 / P23: `attackTurnFree` is false with no target, and false for an
// attack sharing a token with a turn the reader holds; true for one that
// shares none.
#[test]
fn attack_turn_free_reads_target_and_own_turns() {
    let attacks = || {
        vec![
            ("jab", contact(1.0, 100_000.0)),
            ("slam", contact(1.0, 100_000.0)),
            ("rifle", contact(1.0, 100_000.0)),
        ]
    };
    // No target: nothing is free.
    let probe = graph(
        "probe",
        vec![("probe", idle()), ("slam_free", idle())],
        vec![("probe", vec![edge("slam_free", attack_turn_free("slam"))])],
        attacks(),
    );
    let mut reg = EntityRegistry::new();
    let mut runtime = AiRuntime::new();
    let lonely = spawn_at(&mut reg, 0, &probe, "probe");
    for _ in 0..5 {
        tick(&mut reg, &mut runtime);
    }
    assert_eq!(state(&reg, lonely), "probe", "false with no target");

    // Holding a jab turn: the slam shares its token, the rifle draws another.
    let graph = graph(
        "strike",
        vec![
            ("strike", striking("jab")),
            ("jab_held", idle()),
            ("slam_free", idle()),
            ("rifle_free", idle()),
        ],
        vec![
            (
                "strike",
                vec![edge("jab_held", reads(BRAIN_HAS_ATTACK_TURN_INPUT))],
            ),
            (
                "jab_held",
                vec![
                    edge("slam_free", attack_turn_free("slam")),
                    edge("rifle_free", attack_turn_free("rifle")),
                ],
            ),
        ],
        attacks(),
    );
    let mut reg = EntityRegistry::new();
    let mut runtime = AiRuntime::new();
    set_limits(&mut runtime, &[3, 2]);
    spawn_player(&mut reg, player_spot());
    let enemy = spawn_at(&mut reg, 0, &graph, "strike");
    set_draws(&reg, &mut runtime, &[], enemy, "rifle", &[(RANGED, 1)]);
    tick(&mut reg, &mut runtime);
    assert_eq!(held(&reg, enemy), vec!["jab"]);
    tick(&mut reg, &mut runtime);
    assert_eq!(state(&reg, enemy), "jab_held");
    tick(&mut reg, &mut runtime);
    assert_eq!(state(&reg, enemy), "rifle_free");
}

// B37 / P23: limit 3; X holds cost 1; B's cost-3 slam has waited longest and
// reserves the token; C, a fresh cost-1 jab requester, waits behind it. B's
// own request is left out and its clock applies, so B reads the jab free; C
// reads it taken.
#[test]
fn attack_turn_free_leaves_out_the_readers_own_request() {
    let graph = graph(
        "slam_hold",
        vec![
            ("slam_hold", marked(striking("slam"), "slam")),
            ("slam_wait", marked(striking("slam"), "slam")),
            ("jab_hold", marked(striking("jab"), "jab")),
            ("jab_wait", marked(striking("jab"), "jab")),
            ("jab_free", idle()),
        ],
        vec![
            ("slam_wait", vec![edge("jab_free", attack_turn_free("jab"))]),
            ("jab_wait", vec![edge("jab_free", attack_turn_free("jab"))]),
        ],
        vec![
            ("slam", contact(1.0, 100_000.0)),
            ("jab", contact(1.0, 100_000.0)),
        ],
    );
    let mut reg = EntityRegistry::new();
    let mut runtime = AiRuntime::new();
    spawn_player(&mut reg, player_spot());
    let x = spawn_at(&mut reg, 0, &striker(100_000.0), "strike");
    // The line forms first; the readers' guards start on the next tick.
    let b = spawn_at(&mut reg, 1, &graph, "slam_hold");
    let c = spawn_at(&mut reg, 2, &graph, "jab_hold");
    set_draws(&reg, &mut runtime, &[], b, "slam", &[(MELEE, 3)]);

    tick(&mut reg, &mut runtime);
    assert_eq!(held(&reg, x), vec!["attack"]);
    assert_eq!(requested(&reg, b).as_deref(), Some("slam"));
    assert_eq!(requested(&reg, c).as_deref(), Some("jab"));
    enter(&mut reg, b, "slam_wait");
    enter(&mut reg, c, "jab_wait");
    tick(&mut reg, &mut runtime);
    assert_eq!(state(&reg, b), "jab_free", "B's own request is left out");
    assert_eq!(
        state(&reg, c),
        "jab_wait",
        "C queues behind B's reservation"
    );
}
