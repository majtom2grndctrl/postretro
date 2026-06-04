// Hard-gate CPU draw-list / layout assertion for the boot splash (Task 6a).
//
// Feeds `build_splash_descriptor()` + the fullscreen background fill through
// `layout::project` at known backbuffer sizes and pins the produced device-pixel
// quad rects: the panel anchor (centered), the logical-reference→device scale,
// the integer-pixel snap, and the 9-slice corner rects (via
// `UiInstance::corner_rects()`). This test FAILS if the splash anchor, the
// 9-slice corner rects, or the scale math regresses — i.e. if the produced quad
// rects move. Pure CPU: no GPU adapter, no wgpu call (the draw list is built by
// `layout::project`, which holds no GPU handles).
//
// Intentionally mirrors `Renderer::record_splash_ui`'s draw-list assembly: the
// background fill is the first quad, then the framed panel quads (border, fill)
// in one batch, and the logo in its own batch. If that assembly changes, this
// fixture must change with it — that coupling is the point.
//
// See: context/plans/in-progress/M13--ui-render-pass-slice (Task 6, Acceptance
// criteria: "fails if the splash anchor, 9-slice corner rects, or
// logical-reference→device scale math regresses").

use super::layout::{self, device_scale};
use super::splash::{SplashDescriptor, build_splash_descriptor};
use super::{UiDrawList, UiInstance};
use crate::render::splash::splash_bg_rgba;

/// Device-pixel comparison epsilon. Rects are snapped to whole pixels, but float
/// rounding can leave a sub-ulp residue, so an explicit epsilon is required (per
/// the testing guide's floating-point rule) rather than exact equality.
const EPS: f32 = 1e-3;

fn assert_rect(label: &str, got: [f32; 4], want: [f32; 4]) {
    for i in 0..4 {
        assert!(
            (got[i] - want[i]).abs() <= EPS,
            "{label} rect[{i}] = {} != {} (got {got:?}, want {want:?})",
            got[i],
            want[i],
        );
    }
}

/// Reproduce the renderer's splash draw-list assembly (`record_splash_ui`): the
/// oversized background fill first, then the framed panel quads, projected into
/// one device-pixel list. The logo (its own textured batch) is returned
/// separately. Keeps the test fixture honest against the real draw path.
fn project_splash(desc: &SplashDescriptor, device_size: [u32; 2]) -> (UiDrawList, UiDrawList) {
    let bg = SplashDescriptor::background_element(splash_bg_rgba());
    let mut panel_elems = vec![bg];
    panel_elems.extend_from_slice(&desc.panel_elements());
    let panels = layout::project(&panel_elems, device_size);
    let logo = layout::project(&[desc.logo_element()], device_size);
    (panels, logo)
}

/// Indices into the panel draw list assembled by `project_splash`.
const BG: usize = 0;
const BORDER: usize = 1;
const FILL: usize = 2;

#[test]
fn splash_panel_quads_anchor_centered_at_reference_resolution() {
    // Regression guard: at the exact 1280x720 reference (device scale 1.0) the
    // framed panel must land dead-center and the background must cover the
    // canvas. If the Center anchor math or the panel size regresses, these rects
    // move and the test fails.
    let desc = build_splash_descriptor();
    let (panels, logo) = project_splash(&desc, [1280, 720]);
    assert_eq!(panels.len(), 3, "background + border + fill");

    // Background: oversized fill centered on the canvas, so its top-left is
    // negative and it overhangs the backbuffer on every edge.
    let bg = panels.instances[BG].rect;
    assert!(
        bg[0] <= 0.0 && bg[1] <= 0.0 && bg[0] + bg[2] >= 1280.0 && bg[1] + bg[3] >= 720.0,
        "background must cover the full backbuffer, got {bg:?}",
    );

    // Border: 560x360 centered -> ((1280-560)/2, (720-360)/2) = (360, 180).
    assert_rect(
        "border",
        panels.instances[BORDER].rect,
        [360.0, 180.0, 560.0, 360.0],
    );
    // Fill: 552x352 (inset by 4px each edge) centered -> (364, 184).
    assert_rect(
        "fill",
        panels.instances[FILL].rect,
        [364.0, 184.0, 552.0, 352.0],
    );

    // Logo: 220x220, centered with a -28 vertical nudge -> top-left
    // (640-110, (360-28)-110) = (530, 222).
    assert_rect("logo", logo.instances[0].rect, [530.0, 222.0, 220.0, 220.0]);
}

#[test]
fn splash_panel_quads_scale_uniformly_at_4k() {
    // Regression guard for the logical-reference->device scale math. 3840x2160 is
    // exactly 3x the 1280x720 reference, so every reference rect must triple in
    // position and size with no re-layout artifact between the two resolutions.
    let desc = build_splash_descriptor();
    assert!((device_scale([3840, 2160]) - 3.0).abs() <= EPS);
    let (panels, logo) = project_splash(&desc, [3840, 2160]);

    // Border (360,180,560,360) * 3 -> (1080,540,1680,1080).
    assert_rect(
        "border@4k",
        panels.instances[BORDER].rect,
        [1080.0, 540.0, 1680.0, 1080.0],
    );
    // Fill (364,184,552,352) * 3 -> (1092,552,1656,1056).
    assert_rect(
        "fill@4k",
        panels.instances[FILL].rect,
        [1092.0, 552.0, 1656.0, 1056.0],
    );
    // Logo (530,222,220,220) * 3 -> (1590,666,660,660).
    assert_rect(
        "logo@4k",
        logo.instances[0].rect,
        [1590.0, 666.0, 660.0, 660.0],
    );

    // 9-slice margin scales: 12px logical -> 36px device.
    let m = panels.instances[BORDER].margin;
    assert_rect("border margin@4k", m, [36.0, 36.0, 36.0, 36.0]);
}

#[test]
fn splash_panel_anchor_centers_against_letterbox_on_non_16_9() {
    // Regression guard for the uniform-scale + letterbox-centering rule on a
    // non-16:9 backbuffer. 1920x720: x ratio 1.5, y ratio 1.0 -> uniform scale
    // takes min = 1.0, and the 1280-wide canvas is centered horizontally with a
    // (1920-1280)/2 = 320px left margin. The panel must shift right by that
    // margin, NOT stretch — proving the canvas letterboxes rather than scaling
    // each axis independently.
    let desc = build_splash_descriptor();
    assert!((device_scale([1920, 720]) - 1.0).abs() <= EPS);
    let (panels, _logo) = project_splash(&desc, [1920, 720]);

    // Border centered in the letterboxed canvas: reference (360,180) + (320,0)
    // origin -> (680,180); size unchanged at scale 1.0.
    assert_rect(
        "border letterbox",
        panels.instances[BORDER].rect,
        [680.0, 180.0, 560.0, 360.0],
    );
}

#[test]
fn splash_panel_rects_snap_to_integer_device_pixels() {
    // Regression guard for the device-pixel snap. At a non-integer scale the
    // projected edges are fractional pre-snap; every produced rect component must
    // round to a whole device pixel so panels show no subpixel edge blur. 1281x721
    // gives scale 1281/1280 ~ 1.00078, which moves edges off integer boundaries.
    let desc = build_splash_descriptor();
    let (panels, logo) = project_splash(&desc, [1281, 721]);
    for inst in panels.instances.iter().chain(logo.instances.iter()) {
        for v in inst.rect {
            assert!(
                (v - v.round()).abs() <= EPS,
                "rect component {v} not snapped to a whole device pixel (inst {:?})",
                inst.rect,
            );
        }
        for m in inst.margin {
            assert!(
                (m - m.round()).abs() <= EPS,
                "margin component {m} not snapped to a whole device pixel",
            );
        }
    }
}

#[test]
fn splash_border_9slice_corner_rects_are_fixed_size_and_pinned() {
    // Regression guard for the 9-slice corner rects. The border's corners must
    // keep their (scaled) fixed size and stay anchored to the four rect corners
    // at every resolution — if the corner derivation or the margin scale
    // regresses, the corners move/stretch and these pinned rects fail.
    let desc = build_splash_descriptor();

    // Reference: 12px corners on the (360,180,560,360) border.
    let panels = project_splash(&desc, [1280, 720]).0;
    let border = panels.instances[BORDER];
    assert_corner_rects(border, 12.0);

    // 4K: corners scale to 36px and stay pinned to the (1080,540,1680,1080) rect.
    let panels4k = project_splash(&desc, [3840, 2160]).0;
    assert_corner_rects(panels4k.instances[BORDER], 36.0);
}

/// Assert the four 9-slice corners of `inst` are `corner`-sized squares pinned to
/// the rect's corners.
fn assert_corner_rects(inst: UiInstance, corner: f32) {
    let [x, y, w, h] = inst.rect;
    let [tl, tr, bl, br] = inst.corner_rects();
    assert_rect("corner TL", tl, [x, y, corner, corner]);
    assert_rect("corner TR", tr, [x + w - corner, y, corner, corner]);
    assert_rect("corner BL", bl, [x, y + h - corner, corner, corner]);
    assert_rect(
        "corner BR",
        br,
        [x + w - corner, y + h - corner, corner, corner],
    );
}

#[test]
fn splash_logo_preserves_aspect_across_resolutions() {
    // Regression guard: the logo must scale by the uniform device factor only —
    // never an independent x/y stretch — so its aspect is identical at 720p, 4K,
    // and a non-16:9 size. "without stretching the logo" in the acceptance
    // criteria.
    let desc = build_splash_descriptor();
    let sizes = [[1280u32, 720], [3840, 2160], [1920, 720]];
    let mut aspects = Vec::new();
    for size in sizes {
        let r = project_splash(&desc, size).1.instances[0].rect;
        aspects.push(r[2] / r[3]);
    }
    for a in &aspects[1..] {
        assert!(
            (a - aspects[0]).abs() <= EPS,
            "logo aspect drifted across resolutions: {aspects:?}",
        );
    }
}
