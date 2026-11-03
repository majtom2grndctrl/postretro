// Numeric `compare` predicates: boundary evaluation and `visibleWhen` show/hide.

use super::common::*;
use crate::descriptor::{CompareOp, PredicateCompare};
use crate::tree::predicate::evaluate_predicate;

const SLOT: &str = "player.assistProgress";

fn above_zero() -> Predicate {
    Predicate {
        compare: Some(PredicateCompare {
            op: CompareOp::GreaterThan,
            value: 0.0,
        }),
        ..pred(SLOT, None)
    }
}

#[test]
fn greater_than_zero_hides_at_zero_and_shows_above() {
    let p = above_zero();
    let at = |value: f32| evaluate_predicate(&p, None, &number_slots(SLOT, value), &no_cells());
    assert_eq!(at(0.0), 0.0, "0 is not above 0");
    assert_eq!(at(-0.0), 0.0, "negative zero is not above 0");
    assert_eq!(
        at(f32::MIN_POSITIVE),
        1.0,
        "the smallest positive value shows"
    );
    assert_eq!(at(0.5), 1.0);
    assert_eq!(at(1.0), 1.0);
}

#[test]
fn compare_on_nan_absent_or_non_number_resolves_false() {
    let p = above_zero();
    assert_eq!(
        evaluate_predicate(&p, None, &number_slots(SLOT, f32::NAN), &no_cells()),
        0.0,
        "NaN satisfies no comparison"
    );
    assert_eq!(
        evaluate_predicate(&p, None, &no_slots(), &no_cells()),
        0.0,
        "an absent slot resolves false, as an absent `equals` source does"
    );
    let boolean = HashMap::from([(SLOT.to_string(), SlotValue::Boolean(true))]);
    assert_eq!(
        evaluate_predicate(&p, None, &boolean, &no_cells()),
        0.0,
        "a non-Number value is a type mismatch"
    );
}

#[test]
fn inclusive_and_upper_ops_hold_at_their_bounds() {
    let with = |op: CompareOp, value: f64| Predicate {
        compare: Some(PredicateCompare { op, value }),
        ..pred(SLOT, None)
    };
    let at = |p: &Predicate, value: f32| {
        evaluate_predicate(p, None, &number_slots(SLOT, value), &no_cells())
    };
    assert_eq!(at(&with(CompareOp::AtLeast, 0.5), 0.5), 1.0);
    assert_eq!(at(&with(CompareOp::AtLeast, 0.5), 0.25), 0.0);
    assert_eq!(at(&with(CompareOp::AtMost, 0.5), 0.5), 1.0);
    assert_eq!(at(&with(CompareOp::AtMost, 0.5), 0.75), 0.0);
    assert_eq!(at(&with(CompareOp::LessThan, 0.5), 0.5), 0.0);
    assert_eq!(at(&with(CompareOp::LessThan, 0.5), 0.25), 1.0);
}

#[test]
fn visible_when_compare_hides_the_label_at_zero_and_draws_it_above() {
    let mut label = text("REVIVING", 18.0);
    if let Widget::Text(text) = &mut label {
        text.visible_when = Some(above_zero());
    }
    let tree = anchored(vstack(0.0, 0.0, Align::Start, vec![label]));
    let mut ui = UiTree::from_descriptor(&tree, &theme());
    let mut fs = font_system();
    let mut draws = |value: f32| {
        ui.build_draw_data_retained(
            [1280, 720],
            &mut fs,
            &no_images(),
            &number_slots(SLOT, value),
            &no_cells(),
            0.0,
        )
        .texts
        .iter()
        .any(|text| text.content == "REVIVING")
    };
    assert!(!draws(0.0), "hidden while progress is 0");
    assert!(draws(0.25), "shown once progress rises above 0");
    assert!(!draws(0.0), "hidden again when progress resets");
}
