// The respawn drop on turns, at production cadence: a player respawning
// while NPCs fight them ends every turn on them (recording its refresh),
// stops a burst aimed at them, frees the slot, and clears the wait clock, on
// the respawn tick; noticing them again starts a new wait.
// See: context/plans/done/E10--ai-tick-scheduling/index.md (Acquiring,
//      losing and resets; P47)
//      context/lib/entity_model.md §7c (Attack budget)

use super::*;
use crate::decide::DECIDE_INTERVAL_TICKS;
use postretro_entities::components::brain::AttackPoolId;

/// The default token, limited to `limit`, refreshing for a second.
fn refreshing_limit(runtime: &mut AiRuntime, limit: u32) {
    runtime.turns.budgets = AttackBudgets::new(BudgetTable::new(vec![TokenBudget {
        limit,
        refresh_ms: 1_000.0,
    }]));
}

/// A marked activity that asks for `attack`'s turn and waits in line.
fn asker() -> BehaviorGraphDescriptor {
    graph(
        "ask",
        vec![("ask", marked(striking("attack"), "attack"))],
        vec![],
        vec![("attack", contact(1.0, 100_000.0))],
    )
}

fn spawn_engaged_at(
    reg: &mut EntityRegistry,
    index: usize,
    graph: &BehaviorGraphDescriptor,
    state: &str,
    target: EntityId,
    life: u32,
) -> EntityId {
    spawn_enemy(
        reg,
        attacker_spot(index),
        engaged_on(authored_brain(graph, state), target, life),
        50.0,
    )
}

// AC 11 / P47: one NPC mid-burst on the player, one holding a turn on them,
// one waiting in line on them. On the respawn tick the burst stops, both
// turns end and record their refresh, the slot frees and the waiter's clock
// clears; nobody re-takes the player on sight.
#[test]
fn respawn_drop_stops_the_burst_and_frees_turns_slot_and_line_on_the_respawn_tick() {
    let descriptors = [burst_weapon("enemy.burst", 3, 64.0, 160.0)];
    let burst = graph(
        "strike",
        vec![("strike", striking("attack"))],
        vec![],
        vec![("attack", weapon("enemy.burst"))],
    );
    let mut reg = EntityRegistry::new();
    let mut runtime = AiRuntime::new();
    refreshing_limit(&mut runtime, 2);
    let player = spawn_player(&mut reg, player_spot());
    let shooter = spawn_engaged_at(&mut reg, 0, &burst, "strike", player, 0);
    let holder = spawn_engaged_at(&mut reg, 1, &striker(100_000.0), "strike", player, 0);
    let waiter = spawn_engaged_at(&mut reg, 2, &asker(), "ask", player, 0);
    let shots = tick_with(&mut reg, &mut runtime, &descriptors)
        .projectile_spawns
        .len();
    assert_eq!(shots, 1, "the burst's first shot");
    assert_eq!(held(&reg, shooter), vec!["attack"]);
    assert_eq!(held(&reg, holder), vec!["attack"]);
    assert_eq!(wait_since(&reg, waiter), Some(1), "the line is full");
    let group = (AttackPoolId::Default, player);
    let refreshing_before = runtime.turns.refreshing_now(group, AttackTokenId(0));

    respawn_in_place(&mut reg, player);
    let shots = tick_with(&mut reg, &mut runtime, &descriptors)
        .projectile_spawns
        .len();
    assert_eq!(shots, 0);
    for npc in [shooter, holder, waiter] {
        let brain = reg.get_component::<BrainComponent>(npc).unwrap();
        assert_eq!(brain.acquired_target, None, "the respawn drop");
        assert_eq!(brain.combat_slot, None, "its slot frees");
        assert_eq!(brain.retaliation_acquired_target, None);
        assert!(held(&reg, npc).is_empty(), "every turn on the player ends");
        assert_eq!(wait_since(&reg, npc), None, "no clock on the player");
        assert_eq!(requested(&reg, npc), None);
    }
    assert_eq!(
        runtime.turns.refreshing_now(group, AttackTokenId(0)),
        refreshing_before + 2,
        "both ended turns record their refresh"
    );
    let mut later = 0;
    for _ in 0..8 * DECIDE_INTERVAL_TICKS {
        later += tick_with(&mut reg, &mut runtime, &descriptors)
            .projectile_spawns
            .len();
    }
    assert_eq!(
        later, 0,
        "the burst stopped; no one re-took the player on sight"
    );
}

// AC 11 / P47: a waiter dropped by the respawn that later notices the player
// again starts a new wait, behind NPCs already waiting on them.
#[test]
fn respawn_drop_requeues_a_waiter_behind_those_already_waiting() {
    let mut reg = EntityRegistry::new();
    let mut runtime = AiRuntime::new();
    refreshing_limit(&mut runtime, 1);
    let player = spawn_player(&mut reg, player_spot());
    let blocker = spawn_engaged_at(&mut reg, 0, &striker(100_000.0), "strike", player, 0);
    let waiter = spawn_engaged_at(&mut reg, 1, &asker(), "ask", player, 0);
    tick(&mut reg, &mut runtime);
    assert_eq!(held(&reg, blocker), vec!["attack"]);
    assert_eq!(wait_since(&reg, waiter), Some(1));
    for _ in 0..10 {
        tick(&mut reg, &mut runtime);
    }

    // The respawn; two NPCs engage the new life at once: one takes the
    // player's turn, one waits behind it.
    respawn_in_place(&mut reg, player);
    let new_holder = spawn_engaged_at(&mut reg, 3, &striker(100_000.0), "strike", player, 1);
    let early = spawn_engaged_at(&mut reg, 4, &asker(), "ask", player, 1);
    let respawn_tick = 12;
    tick(&mut reg, &mut runtime);
    assert_eq!(wait_since(&reg, waiter), None, "the drop clears the clock");
    let mut early_since = None;
    let mut waiter_since = None;
    for _ in 0..200 {
        tick(&mut reg, &mut runtime);
        early_since = early_since.or(wait_since(&reg, early));
        if let Some(since) = wait_since(&reg, waiter) {
            waiter_since = Some(since);
            break;
        }
    }
    let waiter_since = waiter_since.expect("the waiter notices the player again");
    let early_since = early_since.expect("the early asker waits");
    assert!(held(&reg, new_holder).len() + held(&reg, early).len() <= 1);
    assert!(waiter_since > respawn_tick, "a new wait");
    assert!(
        waiter_since > early_since,
        "behind the NPCs already waiting: {waiter_since} vs {early_since}"
    );
}
