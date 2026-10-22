// Flash window: the budget an onset must fit, the returns parts still owe, how a
// ghost's luminance parts share slots across frames, and the window's bounds.
// See: context/lib/rendering_pipeline.md §7.8 (Photosensitivity limiter)

use super::*;

#[test]
fn an_onset_waits_for_the_returns_earlier_onsets_still_owe() {
    let mut window = FlashWindow::default();
    // A return with no onset before it owes nothing.
    window.push(Part::Level, false);
    window.push(Part::Level, true);
    assert!(window.onset_fits(), "2 held + 1 owed + 2 fits in six");
    window.push(Part::Red, true);
    // Two onsets now owe their returns: 3 + 2 + 2 is over six, though
    // counting held transitions alone would admit it.
    assert!(!window.onset_fits());
    window.push(Part::Level, false);
    window.push(Part::Red, false);
    assert!(!window.onset_fits(), "five held leaves no room for a pair");
}

#[test]
fn an_onset_of_any_part_waits_for_the_returns_other_parts_still_owe() {
    let mut window = FlashWindow::default();
    for _ in 0..2 {
        window.push(Part::Level, true);
        window.push(Part::Level, false);
    }
    assert!(window.onset_fits(), "4 held + 0 owed + 2 fits in six");
    window.push(Part::Level, true);
    // The opacity onset's return takes the last room: a brightness onset
    // admitted now would leave it none.
    assert!(!window.onset_fits());
    window.push(Part::Level, false);
    assert_eq!((window.count, window.owed_returns()), (6, 0));

    // Each part's onsets owe their own returns, summed across parts.
    let mut window = FlashWindow::default();
    window.push(Part::Brightness, true);
    window.push(Part::Red, true);
    assert_eq!(window.owed_returns(), 2);
    assert!(window.onset_fits(), "2 held + 2 owed + 2 fits in six");
    window.push(Part::Level, true);
    assert!(!window.onset_fits(), "3 held + 3 owed + 2 is over six");
    // A return of one part settles only that part's onset.
    window.push(Part::Brightness, false);
    assert_eq!(window.owed_returns(), 2);
}

#[test]
fn a_merging_window_counts_one_frames_opacity_and_brightness_change_once() {
    let mut window = FlashWindow::merging();
    window.age(0.1);
    window.push(Part::Level, true);
    window.push(Part::Brightness, true);
    assert_eq!(window.count, 1, "one frame's rise in both is one onset");
    assert_eq!(window.owed_returns(), 2, "each part owes its own return");
    window.push(Part::Red, true);
    assert_eq!(window.count, 2, "a red onset spends a slot of its own");

    // A return never joins an onset's slot. A red slot between is passed
    // over: the opacity return joins the brightness return.
    window.age(0.1);
    window.push(Part::Brightness, false);
    window.push(Part::Red, false);
    assert_eq!(window.count, 4);
    window.push(Part::Level, false);
    assert_eq!(
        window.count, 4,
        "the opacity return joins the brightness one"
    );
    assert_eq!(
        window.owed_returns(),
        0,
        "a joined return settles each part"
    );

    // A screen channel's window keeps every part's transitions apart.
    let mut window = FlashWindow::default();
    window.age(0.1);
    window.push(Part::Level, true);
    window.push(Part::Brightness, true);
    assert_eq!(window.count, 2);
}

// Regression: opacity, brightness and darkening of one opacity change cross the
// threshold on different frames unless the tint is near white, and only a
// same-frame transition merged, so one change counted two or three times.
#[test]
fn a_luminance_part_joins_the_change_it_crosses_the_threshold_on_frames_later() {
    let mut window = FlashWindow::merging();
    window.age(0.1);
    window.moving(Part::Level, 0.05);
    window.push(Part::Level, true);
    for part in [Part::Brightness, Part::Darkening] {
        window.age(0.1);
        window.moving(Part::Level, 0.05);
        window.moving(part, 0.05);
        assert!(window.onset_fits() && window.joinable(part, true) == Some(0));
        window.push(part, true);
        assert_eq!(window.count, 1, "{part:?} joins the opacity onset");
        assert_eq!(window.ages[0], 0.0, "a join restarts the slot's age");
    }
    assert_eq!(window.owed_returns(), 3, "each part owes its own return");
    // The returns merge the same way, frames apart.
    window.age(0.1);
    window.push(Part::Darkening, false);
    window.age(0.1);
    window.push(Part::Level, false);
    window.push(Part::Brightness, false);
    assert_eq!((window.count, window.owed_returns()), (2, 0));
}

#[test]
fn a_join_needs_a_slot_whose_parts_have_not_reversed_or_turned_back() {
    // Opacity counted its return since its onset: brightness spends its own.
    let mut window = FlashWindow::merging();
    window.push(Part::Level, true);
    window.age(0.1);
    window.push(Part::Level, false);
    window.age(0.1);
    assert_eq!(window.joinable(Part::Brightness, true), None);
    window.push(Part::Brightness, true);
    assert_eq!(window.count, 3);

    // Opacity is moving back down, though it has not counted that yet.
    let mut window = FlashWindow::merging();
    window.push(Part::Level, true);
    window.age(0.1);
    window.moving(Part::Level, -0.05);
    window.moving(Part::Brightness, 0.05);
    assert_eq!(window.joinable(Part::Brightness, true), None);
    // A part holding still does not turn back.
    window.moving(Part::Level, 0.0);
    assert_eq!(window.joinable(Part::Brightness, true), Some(0));

    // A part joins a slot once.
    let mut window = FlashWindow::merging();
    window.push(Part::Level, true);
    window.push(Part::Brightness, true);
    assert_eq!(window.joinable(Part::Brightness, true), None);
}

// Regression: a brightness onset could join a red slot, though a red
// transition always spends a slot of its own.
#[test]
fn a_red_transition_never_shares_a_slot() {
    let mut window = FlashWindow::merging();
    window.age(0.1);
    window.push(Part::Red, true);
    assert_eq!(window.joinable(Part::Brightness, true), None);
    window.push(Part::Brightness, true);
    assert_eq!(window.count, 2, "brightness does not join the red onset");
    window.push(Part::Red, false);
    window.push(Part::Level, false);
    assert_eq!(window.count, 4, "nothing joins a red return either");
    window.push(Part::Darkening, true);
    assert_eq!(window.count, 4, "darkening joins the brightness onset");
}

// The window holds at most six transitions once an onset is admitted and each
// part adds at most one more before the next admission, so pushes never meet
// a full window (a debug assertion), and no part counts more than seven
// transitions (three flashes) in any second. Random transitions of every
// part, with motions that keep opacity the sum of brightness and darkening,
// admitted by the window's own rules, at random frame times.
#[test]
fn random_transitions_never_overflow_the_window_or_give_a_part_four_flashes() {
    let mut state = 0x2545_F491_4F6C_DD1Du64;
    let mut next = move || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        (state >> 40) as f32 / (1u64 << 24) as f32
    };
    let parts = [Part::Level, Part::Red, Part::Brightness, Part::Darkening];
    let sign_of = |roll: f32| -> f32 {
        if roll < 0.4 {
            -1.0
        } else if roll < 0.8 {
            1.0
        } else {
            0.0
        }
    };
    for _ in 0..200 {
        let mut window = FlashWindow::merging();
        let mut last_sign = [0i8; PARTS];
        let mut ages: [Vec<f32>; PARTS] = Default::default();
        for _ in 0..600 {
            let dt = match (next() * 4.0) as u32 {
                0 => 0.0,
                1 => 0.3,
                _ => 1.0 / 60.0,
            };
            window.age(dt);
            for times in &mut ages {
                times.iter_mut().for_each(|age| *age += dt);
                times.retain(|&age| age < FLASH_WINDOW_SECONDS);
            }
            // Brightness and darkening move freely; opacity moves with their
            // sum, either way when they move apart.
            let brightness = sign_of(next());
            let darkening = sign_of(next());
            let level = if brightness == darkening || darkening == 0.0 {
                brightness
            } else if brightness == 0.0 {
                darkening
            } else {
                sign_of(next())
            };
            window.moving(Part::Level, level);
            window.moving(Part::Brightness, brightness);
            window.moving(Part::Darkening, darkening);
            let red = sign_of(next());
            for (index, part) in parts.into_iter().enumerate() {
                let direction = [level, red, brightness, darkening][index];
                let sign = direction as i8;
                if sign == 0 || sign == last_sign[index] || next() < 0.3 {
                    continue;
                }
                let onset = sign > 0;
                let joins = onset && window.joinable(part, true).is_some();
                if onset && !window.onset_fits() && !joins {
                    continue;
                }
                window.push(part, onset);
                last_sign[index] = sign;
                ages[index].push(0.0);
                assert!(
                    ages[index].len() <= 7,
                    "{part:?} counted {} transitions in a second",
                    ages[index].len()
                );
            }
            assert!(window.count <= WINDOW_SLOTS);
        }
    }
}
