// Link-state history: the ceil(f) read rule, dedupe, clock restarts and bounded pruning.
// See: context/lib/entity_model.md §7d

use super::*;

const DISARMED: LinkState = LinkState {
    armed: false,
    members: GhostMembers::all(),
};
const ARMED: LinkState = LinkState {
    armed: true,
    members: GhostMembers::all(),
};

#[test]
fn history_reads_a_change_stamped_n_plus_1_once_presentation_passes_tick_n() {
    let mut history = LinkHistory::new(DISARMED);
    // Armed during the tick labelled 10: stamped 11.
    history.push(11, ARMED);
    assert_eq!(history.state_at(9.5), DISARMED);
    assert_eq!(
        history.state_at(10.0),
        DISARMED,
        "alpha 0 at tick 10 presents the end of tick 9 exactly"
    );
    assert_eq!(
        history.state_at(10.001),
        ARMED,
        "any motion toward stamp 11 presents with the state stamped 11"
    );
    assert_eq!(history.state_at(11.0), ARMED);
    assert_eq!(history.state_at(40.0), ARMED);
}

#[test]
fn history_with_no_change_presents_local_spawn_state() {
    let armed_on_spawn = LinkHistory::new(ARMED);
    assert_eq!(armed_on_spawn.state_at(0.0), ARMED);
    assert_eq!(armed_on_spawn.state_at(f64::NAN), ARMED);
    assert_eq!(armed_on_spawn.latest(), ARMED);
}

#[test]
fn history_records_only_changes_and_a_repeated_stamp_replaces_its_change() {
    let mut history = LinkHistory::new(DISARMED);
    history.push(5, DISARMED);
    assert_eq!(
        history.changes.len(),
        0,
        "an unchanged state records nothing"
    );
    history.push(6, ARMED);
    history.push(7, ARMED);
    assert_eq!(history.changes.len(), 1);
    let narrowed = LinkState {
        armed: true,
        members: GhostMembers::PLAYERS,
    };
    history.push(7, narrowed);
    history.push(7, DISARMED);
    assert_eq!(history.state_at(6.0), ARMED);
    assert_eq!(history.state_at(7.0), DISARMED, "latest at stamp 7 wins");
}

#[test]
fn history_rebases_when_the_stamping_clock_restarts() {
    let mut history = LinkHistory::new(DISARMED);
    history.push(5000, ARMED);
    // A lower stamp: another clock (single player became host). Its state
    // covers every time on the new clock.
    history.push(12, DISARMED);
    assert_eq!(history.state_at(3.0), DISARMED);
    assert_eq!(history.state_at(6000.0), DISARMED);
}

#[test]
fn history_prune_keeps_the_change_covering_the_oldest_clock() {
    let mut history = LinkHistory::new(DISARMED);
    history.push(10, ARMED);
    history.push(12, DISARMED);
    history.push(14, ARMED);
    let clocks = [12.5, 13.0, 13.2, 20.0];
    let before = clocks.map(|clock| history.state_at(clock));
    assert_eq!(before, [DISARMED, DISARMED, ARMED, ARMED]);
    history.prune_before(12.5);
    assert_eq!(
        clocks.map(|clock| history.state_at(clock)),
        before,
        "clocks at or after the oldest read exactly as before the prune"
    );
    assert_eq!(
        history.changes.len(),
        1,
        "older changes folded into the base"
    );
    history.prune_before(13.2);
    assert!(history.changes.is_empty());
    assert_eq!(history.state_at(13.2), ARMED);
}

#[test]
fn history_memory_stays_bounded_without_pruning() {
    let mut history = LinkHistory::new(DISARMED);
    for stamp in 1..=(MAX_CHANGES as u32 * 4) {
        history.push(stamp, if stamp % 2 == 0 { DISARMED } else { ARMED });
    }
    assert_eq!(history.changes.len(), MAX_CHANGES);
    assert_eq!(history.latest(), DISARMED);
}

#[test]
fn history_reset_to_spawn_drops_received_changes() {
    let mut history = LinkHistory::new(DISARMED);
    history.push(30, ARMED);
    history.reset_to_spawn();
    assert_eq!(history.state_at(100.0), DISARMED);
    assert_eq!(history.latest(), DISARMED);
}
