// Slider retained-layout and presentation-mapping tests.
// See: context/lib/ui.md

use super::common::*;

fn sensitivity_slider() -> Widget {
    Widget::Slider(SliderWidget {
        id: "sensitivity".into(),
        label: None,
        labelled_by: Some("sensitivityLabel".into()),
        bind: SliderBind {
            source: BindSource::Slot {
                slot: "options.mouseSensitivity".into(),
            },
            tween: None,
        },
        min: 0.0005,
        max: 0.01,
        step: 0.0005,
        value_display: Some(SliderValueDisplay {
            min: 1.0,
            max: 100.0,
            suffix: "%".into(),
            decimal_places: Some(0),
        }),
        captures_nav: vec!["nav.left".into(), "nav.right".into()],
        focus_neighbors: Default::default(),
        disabled: false,
        visible_when: None,
        role: None,
    })
}

#[test]
fn slider_composes_track_thumb_and_presentation_only_percentage() {
    let tree = anchored(sensitivity_slider());
    let mut slots = HashMap::from([(
        "options.mouseSensitivity".to_string(),
        SlotValue::Number(0.002),
    )]);
    let mut ui = UiTree::from_descriptor(&tree, &theme());
    let mut fs = font_system();
    let data =
        ui.build_draw_data_retained([1280, 720], &mut fs, &no_images(), &slots, &no_cells(), 0.0);

    assert_eq!(data.texts.len(), 1);
    assert_eq!(data.texts[0].content, "17%");
    assert_eq!(data.quads.instances.len(), 3, "track, fill, and thumb");
    let track = data.quads.instances[0];
    let fill = data.quads.instances[1];
    let expected_fraction = (0.002_f32 - 0.0005) / (0.01 - 0.0005);
    assert!(approx(
        fill.rect[2],
        (track.rect[2] * expected_fraction).round()
    ));
    let SlotValue::Number(raw_value) = slots["options.mouseSensitivity"] else {
        panic!("mouse sensitivity remains numeric");
    };
    assert!(
        approx(raw_value, 0.002),
        "presentation mapping never rewrites the authoritative value"
    );

    slots.insert("options.mouseSensitivity".into(), SlotValue::Number(0.01));
    let maximum =
        ui.build_draw_data_retained([1280, 720], &mut fs, &no_images(), &slots, &no_cells(), 0.1);
    assert_eq!(maximum.texts[0].content, "100%");

    slots.insert("options.mouseSensitivity".into(), SlotValue::Number(0.0005));
    let minimum =
        ui.build_draw_data_retained([1280, 720], &mut fs, &no_images(), &slots, &no_cells(), 0.2);
    assert_eq!(minimum.texts[0].content, "1%");
}
