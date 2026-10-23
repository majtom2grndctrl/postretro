// Ghost flash window: shared and carried-on edge slots, red's own, the budget,
// the change that carries on over budget, and the six-slot bound it keeps.
// See: context/lib/rendering_pipeline.md §7.8 (Photosensitivity limiter)

use super::*;

const B: SlotClass = SlotClass::Budgeted;
const E: SlotClass = SlotClass::Exempt;

#[test]
fn one_changes_transitions_share_a_slot_across_frames_and_each_join_restarts_its_age() {
    let mut window = GhostWindow::default();
    assert!(window.count_edge(0, B, true));
    for edge in 1..4 {
        window.age(0.3);
        assert!(window.count_edge(edge, B, true));
        assert_eq!(window.len(), 1, "edge {edge} joins the change");
    }
    // Joined 0.3 s ago, so the slot outlives its first transition's second.
    window.age(0.8);
    assert_eq!(window.len(), 1);
    // A transition of the other class spends a slot of its own.
    assert!(window.count_edge(4, E, true));
    assert_eq!(window.len(), 2);
}

// Regression: a ghost's luminance parts merged only while none of them had
// moved back, so one opacity change counted two or three times; here any
// point's transition joins until a member counts again.
#[test]
fn a_join_needs_a_slot_whose_members_have_not_counted_since() {
    // The tracker is already in the latest slot of its class.
    let mut window = GhostWindow::default();
    window.count_edge(0, B, true);
    assert!(window.count_edge(0, B, true));
    assert_eq!(window.len(), 2, "a tracker counts in a slot once");

    // A member of the latest budgeted slot counted in a later slot.
    let mut window = GhostWindow::default();
    window.count_edge(0, B, true);
    window.count_edge(0, E, true);
    assert!(window.count_edge(1, B, true));
    assert_eq!(window.len(), 3, "the change it would join has turned back");

    // The tracker itself counted in a later slot.
    let mut window = GhostWindow::default();
    window.count_edge(0, B, true);
    window.count_edge(1, E, true);
    assert!(window.count_edge(1, B, true));
    assert_eq!(window.len(), 3);

    // A later slot holding neither joins fine.
    let mut window = GhostWindow::default();
    window.count_edge(0, B, true);
    window.count_edge(5, E, true);
    assert!(window.count_edge(1, B, true));
    assert_eq!(window.len(), 2);
}

// Regression: a luminance transition could join a red slot, though a red
// transition always spends a slot of its own.
#[test]
fn a_red_transition_never_shares_a_slot() {
    let mut window = GhostWindow::default();
    window.count_red(true);
    window.count_edge(0, B, true);
    assert_eq!(window.len(), 2, "an edge onset does not join the red one");
    window.count_red(false);
    window.count_edge(1, B, true);
    window.count_edge(2, E, true);
    assert_eq!(
        window.len(),
        4,
        "edge 1 joins edge 0 past the red return; nothing joins red"
    );
}

// A point between edge samples may reach the flash threshold only when a
// counted swing reaches further, so that frame keeps the count a full second;
// with its slot aged out, reaching further counts it again.
#[test]
fn a_swing_reaching_further_keeps_its_count_or_counts_again() {
    let mut window = GhostWindow::default();
    window.count_edge(3, B, true);
    window.count_edge(5, E, true);
    window.age(0.9);
    assert!(window.extend(3, E, false), "its slot still holds it");
    assert_eq!(window.len(), 2);
    window.age(0.5);
    assert_eq!(window.len(), 1, "the slot it restarted outlives the other");
    window.age(0.5);
    assert!(window.is_empty());
    assert!(
        !window.extend(3, B, false),
        "no slot holds it, and none may open"
    );
    assert!(window.extend(3, B, true));
    assert_eq!(window.len(), 1);
}

#[test]
fn a_new_budgeted_slot_needs_room_for_one_more_and_an_exempt_one_always_opens() {
    let mut window = GhostWindow::default();
    // One tracker turning back each time, so nothing joins.
    for class in [B, E, B, E] {
        assert!(window.count_edge(0, class, true));
    }
    assert_eq!(window.len(), 4);
    assert_eq!(window.budget(), Budget::Open);
    assert!(window.count_edge(0, B, true), "4 held + 1 + 1 fits in six");
    assert_eq!(window.len(), 5);
    assert_ne!(window.budget(), Budget::Open);
    assert!(
        !window.count_edge(0, E, false),
        "nothing opens unless allowed"
    );
    assert!(window.count_edge(0, E, true));
    assert_eq!(window.len(), 6);
    assert!(
        !window.count_edge(0, B, true),
        "no room for a budgeted slot"
    );
    assert_eq!(window.len(), 6);
    // Joining needs no room.
    assert!(window.count_edge(7, E, true));
    assert_eq!(window.len(), 6);
    window.age(1.0);
    assert!(window.is_empty());
}

#[test]
fn a_red_onset_waits_for_the_returns_earlier_red_onsets_still_owe() {
    let mut window = GhostWindow::default();
    // A return with no onset before it owes nothing.
    window.count_red(false);
    window.count_red(true);
    assert!(window.red_onset_fits(), "2 held + 1 owed + 2 fits in six");
    window.count_red(true);
    assert!(!window.red_onset_fits(), "3 held + 2 owed + 2 is over six");
    // Edge slots fill the same budget.
    let mut window = GhostWindow::default();
    for class in [B, E, B] {
        window.count_edge(0, class, true);
    }
    window.count_red(true);
    assert!(!window.red_onset_fits(), "4 held + 1 owed + 2 is over six");
}

#[test]
fn over_budget_the_latest_budgeted_change_carries_on_until_an_exempt_slot_follows() {
    let mut window = GhostWindow::default();
    for class in [B, E, B, E] {
        window.count_edge(0, class, true);
    }
    // Edge 4 cannot join the change edge 0 turned back from: it opens one.
    window.count_edge(4, B, true);
    assert_eq!(window.len(), 5);
    assert_eq!(window.budget(), Budget::Continue);
    assert_eq!(window.continuing(), 1 << 4, "the change edge 4 opened");
    // A red slot after it changes nothing.
    window.count_red(false);
    assert_eq!(window.budget(), Budget::Continue);
    assert_eq!(window.continuing(), 1 << 4);
    // An exempt slot after it ends it: edge 4 turning back opens one.
    window.count_edge(4, E, true);
    assert_eq!(window.edge_slots(), 6);
    assert_eq!(window.budget(), Budget::ExemptOnly);
    assert_eq!(window.continuing(), 0);
}

// The bound the clamp's look depends on: no edge point counts more than six
// transitions in a second, because the window never holds more than six edge
// slots (a debug assertion on every push). Random edge transitions under the
// clamp's discipline: budgeted ones only with room, nothing opened while a
// change carries on, and over budget only the exempt ray, which is monotone at
// every edge point, so each tracker counts at most once from the frame the
// ray alone started moving. Red transitions follow the red rule. Random
// frame times, hitches and zero-length frames included.
#[test]
fn random_transitions_under_the_clamps_discipline_never_give_an_edge_point_seven_in_a_second() {
    let mut state = 0x9E37_79B9_7F4A_7C15u64;
    let mut next = move || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        (state >> 40) as f32 / (1u64 << 24) as f32
    };
    let edges = 8;
    for _ in 0..200 {
        let mut window = GhostWindow::default();
        // Each tracker's transition times in the last second.
        let mut counted: Vec<Vec<f32>> = vec![Vec::new(); edges];
        // Trackers that counted since the exempt ray alone started moving.
        let mut once = 0u64;
        let mut last_frame_budgeted = true;
        let mut red_onset_next = true;
        for _ in 0..600 {
            let dt = match (next() * 5.0) as u32 {
                0 => 0.0,
                1 => 0.3,
                _ => 1.0 / 60.0,
            };
            window.age(dt);
            for times in &mut counted {
                times.iter_mut().for_each(|age| *age += dt);
                times.retain(|&age| age < FLASH_WINDOW_SECONDS);
            }
            let budget = window.budget();
            let exempt: Vec<usize> = (0..edges).filter(|_| next() < 0.3).collect();
            let budgeted: Vec<usize> = (0..edges).filter(|_| next() < 0.3).collect();
            // A frame's transitions, exempt first: `None` if one finds no slot.
            let attempt = |with_budgeted: bool, once: u64| {
                let mut frame = window;
                let mut frame_once = once;
                let mut frame_counted = Vec::new();
                for &edge in &exempt {
                    let bit = 1u64 << edge;
                    if !with_budgeted && frame_once & bit != 0 {
                        continue;
                    }
                    let opens = !with_budgeted || budget == Budget::Open;
                    if !frame.count_edge(edge, E, opens) {
                        return None;
                    }
                    frame_once |= bit;
                    frame_counted.push(edge);
                }
                if with_budgeted {
                    for &edge in &budgeted {
                        if !frame.count_edge(edge, B, budget == Budget::Open) {
                            return None;
                        }
                        frame_counted.push(edge);
                    }
                }
                Some((frame, frame_once, frame_counted))
            };
            let moved = if budget != Budget::ExemptOnly && next() < 0.6 {
                attempt(true, 0).map(|moved| (moved, true))
            } else {
                None
            };
            // Over budget, or refused: the exempt ray alone, which always
            // finds its slots.
            let ((frame, frame_once, frame_counted), budgeted_frame) = moved.unwrap_or_else(|| {
                let streak = if last_frame_budgeted { 0 } else { once };
                (
                    attempt(false, streak).expect("the exempt ray always opens its slot"),
                    false,
                )
            });
            window = frame;
            once = frame_once;
            last_frame_budgeted = budgeted_frame;
            for edge in frame_counted {
                counted[edge].push(0.0);
                assert!(
                    counted[edge].len() <= FLASH_MAX_TRANSITIONS,
                    "edge {edge} counted {} transitions in a second",
                    counted[edge].len()
                );
            }
            if next() < 0.1 && (!red_onset_next || window.red_onset_fits()) {
                window.count_red(red_onset_next);
                red_onset_next = !red_onset_next;
            }
            assert!(window.edge_slots() <= FLASH_MAX_TRANSITIONS);
            assert!(window.len() <= GHOST_WINDOW_SLOTS);
        }
    }
}
