// Ring/arc collection: static geometry, per-property binding/tween behavior,
// draw-only clamping, and paint-stream order.

use super::common::*;

fn ring(
    fill: [f32; 4],
    track: Option<[f32; 4]>,
    start_angle: Option<f32>,
    sweep: Option<f32>,
) -> Widget {
    Widget::Ring(RingWidget {
        diameter: 120.0,
        radius: ScalarValue::Literal(48.0),
        thickness: ScalarValue::Literal(3.0),
        start_angle: start_angle.map(ScalarValue::Literal),
        sweep: sweep.map(ScalarValue::Literal),
        fill: ColorValue::Literal(fill),
        track: track.map(ColorValue::Literal),
        id: None,
        visible_when: None,
        role: None,
    })
}

fn draw(root: Widget, viewport: [u32; 2]) -> UiDrawData {
    let mut tree = UiTree::from_descriptor(&anchored(root), &theme());
    let mut fonts = font_system();
    tree.build_draw_data_retained(
        viewport,
        &mut fonts,
        &no_images(),
        &no_slots(),
        &no_cells(),
        0.0,
    )
}

fn bound(slot: &str, tween: Option<TextTween>) -> ScalarValue {
    ScalarValue::Bound(BoundScalar {
        source: BindSource::Slot {
            slot: slot.to_string(),
        },
        tween,
    })
}

fn local_bound(name: &str, tween: Option<TextTween>) -> ScalarValue {
    ScalarValue::Bound(BoundScalar {
        source: BindSource::Local {
            local: name.to_string(),
        },
        tween,
    })
}

fn tween(duration_ms: f32, from: Option<f32>) -> TextTween {
    TextTween {
        duration_ms,
        easing: Easing::Linear,
        from,
    }
}

fn ring_with_scalars(
    radius: ScalarValue,
    thickness: ScalarValue,
    start_angle: ScalarValue,
    sweep: ScalarValue,
) -> Widget {
    Widget::Ring(RingWidget {
        diameter: 100.0,
        radius,
        thickness,
        start_angle: Some(start_angle),
        sweep: Some(sweep),
        fill: ColorValue::Literal([1.0, 0.2, 0.3, 1.0]),
        track: None,
        id: None,
        visible_when: None,
        role: None,
    })
}

fn ring_slots(values: &[(&str, f32)]) -> HashMap<String, SlotValue> {
    values
        .iter()
        .map(|(name, value)| ((*name).to_string(), SlotValue::Number(*value)))
        .collect()
}

fn retained(
    ui: &mut UiTree,
    fonts: &mut cosmic_text::FontSystem,
    slots: &HashMap<String, SlotValue>,
    cells: &CellValues,
    now: f64,
) -> UiDrawData {
    ui.build_draw_data_retained([1280, 720], fonts, &no_images(), slots, cells, now)
}

#[test]
fn literal_ring_projects_track_and_shape_to_device_geometry() {
    let fill = [1.0, 0.2, 0.3, 1.0];
    let track = [0.1, 0.1, 0.1, 0.5];
    // 2560x1440 is exactly 2x the logical reference canvas.
    let data = draw(
        ring(fill, Some(track), Some(30.0), Some(90.0)),
        [2560, 1440],
    );

    assert_eq!(data.rings.len(), 2, "track then shape");
    let track_instance = data.rings[0];
    let shape_instance = data.rings[1];
    assert_eq!(track_instance.rect, [0.0, 0.0, 240.0, 240.0]);
    assert!(approx(track_instance.radius, 96.0));
    assert!(approx(track_instance.thickness, 6.0));
    assert!(approx(track_instance.start_angle, 0.0));
    assert!(approx(track_instance.sweep, std::f32::consts::TAU));
    assert_eq!(track_instance.color, track);
    assert_eq!(shape_instance.rect, track_instance.rect);
    assert!(approx(shape_instance.radius, track_instance.radius));
    assert!(approx(shape_instance.thickness, track_instance.thickness));
    assert!(approx(shape_instance.start_angle, 30.0f32.to_radians()));
    assert!(approx(shape_instance.sweep, 90.0f32.to_radians()));
    assert_eq!(shape_instance.color, fill);
    assert_eq!(
        data.paint_order,
        vec![UiPaintOp::Ring { index: 0 }, UiPaintOp::Ring { index: 1 }],
        "track must paint behind its shape"
    );
}

#[test]
fn full_circle_is_exact_and_open_arc_uses_up_clockwise_radians() {
    let full = draw(ring([1.0; 4], None, None, Some(360.0)), [1280, 720]);
    assert_eq!(full.rings.len(), 1);
    assert!(approx(full.rings[0].sweep, std::f32::consts::TAU));

    let open = draw(ring([1.0; 4], None, Some(0.0), Some(90.0)), [1280, 720]);
    assert_eq!(open.rings.len(), 1);
    let arc = open.rings[0];
    assert!(approx(arc.start_angle, 0.0));
    assert!(approx(arc.sweep, std::f32::consts::FRAC_PI_2));
    // The shader's angle basis is authored from these radians: 0 -> up and a
    // positive quarter turn -> right in Y-down UI device space.
    let up = [arc.start_angle.sin(), -arc.start_angle.cos()];
    let right = [arc.sweep.sin(), -arc.sweep.cos()];
    assert!(approx(up[0], 0.0) && approx(up[1], -1.0));
    assert!(approx(right[0], 1.0) && approx(right[1], 0.0));
}

#[test]
fn two_rings_keep_monotonic_painter_order() {
    let root = vstack(
        0.0,
        0.0,
        Align::Start,
        vec![
            ring([1.0, 0.0, 0.0, 1.0], None, None, None),
            ring([0.0, 1.0, 0.0, 1.0], None, None, None),
        ],
    );
    let data = draw(root, [1280, 720]);
    assert_eq!(data.rings.len(), 2);
    assert_eq!(
        data.paint_order,
        vec![UiPaintOp::Ring { index: 0 }, UiPaintOp::Ring { index: 1 }]
    );
    assert_eq!(data.rings[0].color, [1.0, 0.0, 0.0, 1.0]);
    assert_eq!(data.rings[1].color, [0.0, 1.0, 0.0, 1.0]);
}

#[test]
fn bound_ring_scalars_follow_raw_sources_without_relayout() {
    let root = ring_with_scalars(
        bound("hud.radius", None),
        ScalarValue::Literal(4.0),
        ScalarValue::Literal(0.0),
        bound("hud.sweep", None),
    );
    let mut ui = UiTree::from_descriptor(&anchored(root), &theme());
    let mut fonts = font_system();

    let first = retained(
        &mut ui,
        &mut fonts,
        &ring_slots(&[("hud.radius", 10.0), ("hud.sweep", 90.0)]),
        &no_cells(),
        0.0,
    );
    assert!(approx(first.rings[0].radius, 10.0));
    assert!(approx(first.rings[0].sweep, std::f32::consts::FRAC_PI_2));
    let layouts = ui.recompute_count();
    let rebuilds = ui.draw_rebuild_count();

    let changed = retained(
        &mut ui,
        &mut fonts,
        &ring_slots(&[("hud.radius", 20.0), ("hud.sweep", 180.0)]),
        &no_cells(),
        0.1,
    );
    assert!(approx(changed.rings[0].radius, 20.0));
    assert!(approx(changed.rings[0].sweep, std::f32::consts::PI));
    assert_eq!(
        ui.recompute_count(),
        layouts,
        "scalar changes never relayout"
    );
    assert_eq!(ui.draw_rebuild_count(), rebuilds + 1);

    retained(
        &mut ui,
        &mut fonts,
        &ring_slots(&[("hud.radius", 20.0), ("hud.sweep", 180.0)]),
        &no_cells(),
        0.2,
    );
    assert_eq!(
        ui.draw_rebuild_count(),
        rebuilds + 1,
        "settled frame is cached"
    );
}

#[test]
fn two_bound_ring_scalars_tween_without_stalling_each_other() {
    let root = ring_with_scalars(
        bound("hud.radius", Some(tween(100.0, Some(0.0)))),
        ScalarValue::Literal(4.0),
        ScalarValue::Literal(0.0),
        bound("hud.sweep", Some(tween(100.0, Some(0.0)))),
    );
    let mut ui = UiTree::from_descriptor(&anchored(root), &theme());
    let mut fonts = font_system();
    let targets = ring_slots(&[("hud.radius", 20.0), ("hud.sweep", 180.0)]);

    retained(&mut ui, &mut fonts, &targets, &no_cells(), 0.0);
    let midway = retained(&mut ui, &mut fonts, &targets, &no_cells(), 0.05);
    assert_eq!(
        midway.rings.len(),
        1,
        "both properties advanced beyond zero"
    );
    assert!(approx(midway.rings[0].radius, 10.0));
    assert!(approx(midway.rings[0].sweep, std::f32::consts::FRAC_PI_2));
}

#[test]
fn radius_tween_clamps_only_at_draw_and_recovers_from_an_overrange_target() {
    let root = ring_with_scalars(
        bound("hud.radius", Some(tween(100.0, None))),
        ScalarValue::Literal(4.0),
        ScalarValue::Literal(0.0),
        ScalarValue::Literal(360.0),
    );
    let mut ui = UiTree::from_descriptor(&anchored(root), &theme());
    let mut fonts = font_system();

    retained(
        &mut ui,
        &mut fonts,
        &ring_slots(&[("hud.radius", 10.0)]),
        &no_cells(),
        0.0,
    );
    retained(
        &mut ui,
        &mut fonts,
        &ring_slots(&[("hud.radius", 200.0)]),
        &no_cells(),
        0.1,
    );
    let clamped = retained(
        &mut ui,
        &mut fonts,
        &ring_slots(&[("hud.radius", 200.0)]),
        &no_cells(),
        0.2,
    );
    assert!(approx(clamped.rings[0].radius, 50.0));

    retained(
        &mut ui,
        &mut fonts,
        &ring_slots(&[("hud.radius", 40.0)]),
        &no_cells(),
        0.25,
    );
    let still_clamped = retained(
        &mut ui,
        &mut fonts,
        &ring_slots(&[("hud.radius", 40.0)]),
        &no_cells(),
        0.30,
    );
    assert!(approx(still_clamped.rings[0].radius, 50.0));
    let recovered = retained(
        &mut ui,
        &mut fonts,
        &ring_slots(&[("hud.radius", 40.0)]),
        &no_cells(),
        0.35,
    );
    assert!(approx(recovered.rings[0].radius, 40.0));
}

#[test]
fn ring_clamps_thickness_after_clamping_radius() {
    let root = ring_with_scalars(
        bound("hud.radius", None),
        bound("hud.thickness", None),
        ScalarValue::Literal(0.0),
        ScalarValue::Literal(360.0),
    );
    let mut ui = UiTree::from_descriptor(&anchored(root), &theme());
    let mut fonts = font_system();
    let data = retained(
        &mut ui,
        &mut fonts,
        &ring_slots(&[("hud.radius", 200.0), ("hud.thickness", 60.0)]),
        &no_cells(),
        0.0,
    );
    assert!(approx(data.rings[0].radius, 50.0));
    assert!(approx(data.rings[0].thickness, 50.0));
}

#[test]
fn nonpositive_bound_sweep_skips_shape_but_keeps_track() {
    let mut root = ring_with_scalars(
        ScalarValue::Literal(40.0),
        ScalarValue::Literal(4.0),
        ScalarValue::Literal(0.0),
        bound("hud.sweep", None),
    );
    let Widget::Ring(ring) = &mut root else {
        unreachable!("ring helper returns a Ring");
    };
    ring.track = Some(ColorValue::Literal([0.1, 0.1, 0.1, 1.0]));
    let mut ui = UiTree::from_descriptor(&anchored(root), &theme());
    let mut fonts = font_system();
    let data = retained(
        &mut ui,
        &mut fonts,
        &ring_slots(&[("hud.sweep", 0.0)]),
        &no_cells(),
        0.0,
    );
    assert_eq!(data.rings.len(), 1, "only the full-circle track remains");
    assert_eq!(data.rings[0].color, [0.1, 0.1, 0.1, 1.0]);
}

#[test]
fn tween_to_360_rebuilds_the_settle_frame_as_a_seamless_ring() {
    let root = ring_with_scalars(
        ScalarValue::Literal(40.0),
        ScalarValue::Literal(4.0),
        ScalarValue::Literal(0.0),
        bound("hud.sweep", Some(tween(100.0, Some(0.0)))),
    );
    let mut ui = UiTree::from_descriptor(&anchored(root), &theme());
    let mut fonts = font_system();
    let targets = ring_slots(&[("hud.sweep", 360.0)]);

    retained(&mut ui, &mut fonts, &targets, &no_cells(), 0.0);
    let open = retained(&mut ui, &mut fonts, &targets, &no_cells(), 0.05);
    assert!(open.rings[0].sweep < std::f32::consts::TAU);
    let before_settle = ui.draw_rebuild_count();
    let settled = retained(&mut ui, &mut fonts, &targets, &no_cells(), 0.1);
    assert!(approx(settled.rings[0].sweep, std::f32::consts::TAU));
    assert_eq!(ui.draw_rebuild_count(), before_settle + 1);
}

#[test]
fn visible_when_relayouts_but_bound_ring_values_only_redraw() {
    let mut root = ring_with_scalars(
        bound("hud.radius", None),
        ScalarValue::Literal(4.0),
        ScalarValue::Literal(0.0),
        ScalarValue::Literal(360.0),
    );
    let Widget::Ring(ring) = &mut root else {
        unreachable!("ring helper returns a Ring");
    };
    ring.visible_when = Some(pred("hud.visible", None));
    let mut ui = UiTree::from_descriptor(&anchored(root), &theme());
    let mut fonts = font_system();

    let shown = HashMap::from([
        ("hud.radius".to_string(), SlotValue::Number(10.0)),
        ("hud.visible".to_string(), SlotValue::Boolean(true)),
    ]);
    retained(&mut ui, &mut fonts, &shown, &no_cells(), 0.0);
    let layouts = ui.recompute_count();
    let redraws = ui.draw_rebuild_count();

    let changed_radius = HashMap::from([
        ("hud.radius".to_string(), SlotValue::Number(20.0)),
        ("hud.visible".to_string(), SlotValue::Boolean(true)),
    ]);
    retained(&mut ui, &mut fonts, &changed_radius, &no_cells(), 0.1);
    assert_eq!(ui.recompute_count(), layouts);
    assert_eq!(ui.draw_rebuild_count(), redraws + 1);

    let hidden = HashMap::from([
        ("hud.radius".to_string(), SlotValue::Number(20.0)),
        ("hud.visible".to_string(), SlotValue::Boolean(false)),
    ]);
    retained(&mut ui, &mut fonts, &hidden, &no_cells(), 0.2);
    assert_eq!(ui.recompute_count(), layouts + 1);

    retained(&mut ui, &mut fonts, &changed_radius, &no_cells(), 0.3);
    assert_eq!(ui.recompute_count(), layouts + 2);
}

#[test]
fn hidden_ring_tween_advances_and_retargets_in_the_background() {
    let mut root = ring_with_scalars(
        bound("hud.radius", Some(tween(100.0, None))),
        ScalarValue::Literal(4.0),
        ScalarValue::Literal(0.0),
        ScalarValue::Literal(360.0),
    );
    let Widget::Ring(ring) = &mut root else {
        unreachable!("ring helper returns a Ring");
    };
    ring.visible_when = Some(pred("hud.visible", None));
    let mut ui = UiTree::from_descriptor(&anchored(root), &theme());
    let mut fonts = font_system();

    let shown_at_10 = HashMap::from([
        ("hud.radius".to_string(), SlotValue::Number(10.0)),
        ("hud.visible".to_string(), SlotValue::Boolean(true)),
    ]);
    retained(&mut ui, &mut fonts, &shown_at_10, &no_cells(), 0.0);
    let hidden_at_20 = HashMap::from([
        ("hud.radius".to_string(), SlotValue::Number(20.0)),
        ("hud.visible".to_string(), SlotValue::Boolean(false)),
    ]);
    retained(&mut ui, &mut fonts, &hidden_at_20, &no_cells(), 0.02);
    let hidden_at_40 = HashMap::from([
        ("hud.radius".to_string(), SlotValue::Number(40.0)),
        ("hud.visible".to_string(), SlotValue::Boolean(false)),
    ]);
    retained(&mut ui, &mut fonts, &hidden_at_40, &no_cells(), 0.12);
    let shown = retained(
        &mut ui,
        &mut fonts,
        &HashMap::from([
            ("hud.radius".to_string(), SlotValue::Number(40.0)),
            ("hud.visible".to_string(), SlotValue::Boolean(true)),
        ]),
        &no_cells(),
        0.22,
    );
    assert_eq!(shown.rings.len(), 1);
    assert!(approx(shown.rings[0].radius, 40.0));
}

#[test]
fn fresh_bound_ring_uses_its_source_directly_and_overrange_sweep_is_full_circle() {
    let root = ring_with_scalars(
        bound("hud.radius", Some(tween(100.0, Some(0.0)))),
        ScalarValue::Literal(4.0),
        ScalarValue::Literal(0.0),
        bound("hud.sweep", None),
    );
    let mut ui = UiTree::from_descriptor(&anchored(root), &theme());
    let mut fonts = font_system();
    let data = ui.build_draw_data(
        [1280, 720],
        &mut fonts,
        &no_images(),
        &ring_slots(&[("hud.radius", 25.0), ("hud.sweep", 400.0)]),
    );
    assert_eq!(data.rings.len(), 1);
    assert!(approx(data.rings[0].radius, 25.0));
    assert!(approx(data.rings[0].sweep, std::f32::consts::TAU));
}

#[test]
fn track_follows_the_eased_radius() {
    let mut root = ring_with_scalars(
        bound("hud.radius", Some(tween(100.0, Some(20.0)))),
        ScalarValue::Literal(4.0),
        ScalarValue::Literal(0.0),
        ScalarValue::Literal(90.0),
    );
    let Widget::Ring(ring) = &mut root else {
        unreachable!("ring helper returns a Ring");
    };
    ring.track = Some(ColorValue::Literal([0.1, 0.1, 0.1, 1.0]));
    let mut ui = UiTree::from_descriptor(&anchored(root), &theme());
    let mut fonts = font_system();
    let slots = ring_slots(&[("hud.radius", 40.0)]);

    retained(&mut ui, &mut fonts, &slots, &no_cells(), 0.0);
    let halfway = retained(&mut ui, &mut fonts, &slots, &no_cells(), 0.05);
    assert_eq!(halfway.rings.len(), 2);
    assert!(approx(halfway.rings[0].radius, 30.0));
    assert!(approx(halfway.rings[1].radius, 30.0));
    assert!(approx(halfway.rings[0].sweep, std::f32::consts::TAU));
}

#[test]
fn local_bound_ring_scalar_resolves_in_its_declaring_scope() {
    let ring = ring_with_scalars(
        local_bound("radius", None),
        ScalarValue::Literal(4.0),
        ScalarValue::Literal(0.0),
        ScalarValue::Literal(360.0),
    );
    let root = Widget::VStack(ContainerWidget {
        gap: SpacingValue::Literal(0.0),
        padding: SpacingValue::Literal(0.0),
        align: Align::Start,
        fill: None,
        border: None,
        id: None,
        focus_neighbors: Default::default(),
        focus: None,
        restore_on_return: false,
        local_state: Some(LocalState {
            scope: "ring-scope".to_string(),
            cells: Default::default(),
        }),
        visible_when: None,
        role: None,
        children: vec![ring],
    });
    let mut ui = UiTree::from_descriptor(&anchored(root), &theme());
    let mut fonts = font_system();
    let cells = CellValues::from([(
        ("ring-scope".to_string(), "radius".to_string()),
        SlotValue::Number(25.0),
    )]);
    let data = retained(&mut ui, &mut fonts, &no_slots(), &cells, 0.0);
    assert_eq!(data.rings.len(), 1);
    assert!(approx(data.rings[0].radius, 25.0));
}
