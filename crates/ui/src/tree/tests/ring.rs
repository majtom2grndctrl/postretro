// Static ring/arc collection: geometry projection, full-circle seam data, and
// paint-stream order. Dynamic scalar binding/tween behavior belongs to Task 2.

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
