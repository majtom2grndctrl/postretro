// A masked Text: one `*` per grapheme of whatever it would draw, on the
// fresh and retained paths, bound or not.

use super::common::*;

fn masked_text(content: &str, slot: Option<&str>) -> AnchoredTree {
    let Widget::Text(mut text) = (match slot {
        Some(slot) => bound_text(content, slot, None),
        None => bound_text(content, "unused.slot", None),
    }) else {
        unreachable!("bound_text builds a Text")
    };
    if slot.is_none() {
        text.bind = None;
    }
    text.mask = true;
    AnchoredTree {
        anchor: Anchor::TopLeft,
        offset: [0.0, 0.0],
        root: Widget::Text(text),
        capture_mode: CaptureMode::Passthrough,
        initial_focus: None,
        text_entry_target: None,
        accessible_name: None,
        role: None,
        restore_on_return: None,
        background: None,
    }
}

fn string_slots(slot: &str, value: &str) -> HashMap<String, SlotValue> {
    HashMap::from([(slot.to_string(), SlotValue::String(value.to_string()))])
}

// AC U1: one `*` per grapheme, so a combining mark or an emoji sequence is
// one star; empty content draws an empty string.
#[test]
fn mask_draws_one_star_per_grapheme_and_nothing_for_nothing() {
    use crate::tree::mask_text;
    assert_eq!(mask_text(""), "");
    assert_eq!(mask_text("hunter2"), "*******");
    assert_eq!(mask_text("e\u{301}"), "*", "e plus a combining acute");
    assert_eq!(
        mask_text("\u{1F469}\u{200D}\u{1F680}"),
        "*",
        "a ZWJ sequence"
    );
}

// AC U1: a masked Text draws its literal content, a bound value and an
// empty value masked, on the fresh path and on the retained path.
#[test]
fn masked_text_never_draws_its_content() {
    let mut fs = font_system();
    let literal = masked_text("abc", None);
    let mut ui = UiTree::from_descriptor(&literal, &theme());
    let data = ui.build_draw_data([1280, 720], &mut fs, &no_images(), &HashMap::new());
    assert_eq!(data.texts[0].content, "***");

    let bound = masked_text("", Some("session.joinPassword"));
    for (value, drawn) in [("p4ss", "****"), ("", "")] {
        let slots = string_slots("session.joinPassword", value);
        let mut fresh = UiTree::from_descriptor(&bound, &theme());
        let data = fresh.build_draw_data([1280, 720], &mut fs, &no_images(), &slots);
        let content = data.texts.first().map_or("", |text| text.content.as_str());
        assert_eq!(content, drawn, "fresh path, `{value}`");

        let mut retained = UiTree::from_descriptor(&bound, &theme());
        let data = retained.build_draw_data_retained_with_image_generation(
            [1280, 720],
            &mut fs,
            &no_images(),
            0,
            &slots,
            &no_cells(),
            crate::tree::TweenClock::easing(0.0),
            crate::tree::ScrollInput::default(),
        );
        let content = data.texts.first().map_or("", |text| text.content.as_str());
        assert_eq!(content, drawn, "retained path, `{value}`");
    }
}
