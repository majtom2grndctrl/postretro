// A holder the AI tick schedule skips stays in the AI set: its turns keep
// their capacity, end only on its next run, and a queued grant opens there.
// See: context/plans/done/E10--ai-tick-scheduling/index.md (P1, P2)
//      context/lib/entity_model.md §7c (Attack budget)

use super::*;

/// Ticks a dormant brain sits out between runs: 2 Hz.
const SKIPPED: usize = 29;

// P1: under limit 1, the holder's turn still counts on every tick the
// schedule skips it: the waiter is never granted, and no refresh starts.
#[test]
fn a_skipped_holder_keeps_capacity_and_starts_no_refresh() {
    let graph = striker(100_000.0);
    let mut reg = EntityRegistry::new();
    let mut runtime = AiRuntime::noticing_on_sight();
    set_limits(&mut runtime, &[1]);
    spawn_player(&mut reg, player_spot());
    let enemies: Vec<EntityId> = (0..2)
        .map(|index| spawn_at(&mut reg, index, &graph, "strike"))
        .collect();
    tick(&mut reg, &mut runtime);
    let holder = *enemies
        .iter()
        .find(|&&enemy| turns(&reg, enemy).holds_any())
        .expect("one holds");
    let waiter = *enemies.iter().find(|&&enemy| enemy != holder).unwrap();
    let refreshing = runtime.turns.refreshing_entries();

    for tick_index in 0..SKIPPED {
        tick_skipping(&mut reg, &mut runtime, &[holder]);
        assert!(
            held(&reg, waiter).is_empty(),
            "tick {tick_index}: the waiter took a skipped holder's capacity"
        );
        assert_eq!(
            runtime.turns.refreshing_entries(),
            refreshing,
            "tick {tick_index}: a skip started a refresh"
        );
    }
    tick(&mut reg, &mut runtime);
    assert_eq!(held(&reg, holder), vec!["attack"], "it ran, still holding");
    assert!(held(&reg, waiter).is_empty());
}

// P1: a turn whose cooldown ends while its holder is skipped holds until the
// holder's next run, which releases it; the waiter is granted on that tick.
#[test]
fn a_turn_ending_while_skipped_releases_on_next_run() {
    let graph = striker(160.0);
    let mut reg = EntityRegistry::new();
    let mut runtime = AiRuntime::noticing_on_sight();
    set_limits(&mut runtime, &[1]);
    let pawn = spawn_player(&mut reg, player_spot());
    let enemies: Vec<EntityId> = (0..2)
        .map(|index| spawn_at(&mut reg, index, &graph, "strike"))
        .collect();
    tick(&mut reg, &mut runtime);
    let holder = *enemies
        .iter()
        .find(|&&enemy| turns(&reg, enemy).holds_any())
        .expect("one holds");
    let waiter = *enemies.iter().find(|&&enemy| enemy != holder).unwrap();
    let hp_lost = player_hp_lost(&reg, pawn);

    // Its 160 ms cooldown ends ten ticks in; skipped, it holds on.
    for tick_index in 0..SKIPPED {
        tick_skipping(&mut reg, &mut runtime, &[holder]);
        assert_eq!(
            held(&reg, holder),
            vec!["attack"],
            "tick {tick_index}: a skipped holder's turn ended"
        );
        assert!(held(&reg, waiter).is_empty(), "tick {tick_index}");
    }
    assert_eq!(player_hp_lost(&reg, pawn), hp_lost, "nobody struck");

    tick(&mut reg, &mut runtime);
    assert_eq!(
        held(&reg, waiter),
        vec!["attack"],
        "the holder's run released its turn, and the waiter was granted"
    );
}

// A skipped brain's run spans every tick since its last run, up to one
// dormant interval; one that left the AI set returns spanning one tick,
// holding nothing.
#[test]
fn admit_spans_a_dormant_skip_and_reports_a_return() {
    use crate::attack_turns::{admit, returns_to_set};
    use postretro_sim::npc_tiers::DORMANT_INTERVAL_TICKS;

    let mut turns = AttackTurnState::default();
    turns.seen_tick = 10;
    let tick = 10 + DORMANT_INTERVAL_TICKS;
    assert!(!returns_to_set(&turns, tick, true), "skipped, so present");
    assert_eq!(admit(&mut turns, tick, true), DORMANT_INTERVAL_TICKS as u32);
    assert_eq!(turns.seen_tick, tick);

    // Gone for a tick: it returns, its clocks crediting one tick.
    turns.wait = Some(postretro_entities::components::brain::WaitClock {
        since_tick: 3,
        target: EntityId::from_raw(1),
        target_life: 0,
    });
    assert!(returns_to_set(&turns, tick + 5, false));
    assert_eq!(admit(&mut turns, tick + 5, false), 1);
    assert_eq!(turns.wait, None, "a return holds nothing");
}
