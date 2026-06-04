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

/// The real committed logo asset is 2028x582 (a wide banner, aspect ~3.485). The
/// descriptor derives the logo's height from this decoded aspect, so the fixture
/// builds the descriptor with the real value and pins rects shaped to the asset.
const ASSET_LOGO_ASPECT: f32 = 2028.0 / 582.0;

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
    let desc = build_splash_descriptor(ASSET_LOGO_ASPECT);
    let (panels, logo) = project_splash(&desc, [1280, 720]);
    assert_eq!(panels.len(), 3, "background + border + fill");

    // Background: oversized fill centered on the canvas, so its top-left is
    // negative and it overhangs the backbuffer on every edge.
    let bg = panels.instances[BG].rect;
    assert!(
        bg[0] <= 0.0 && bg[1] <= 0.0 && bg[0] + bg[2] >= 1280.0 && bg[1] + bg[3] >= 720.0,
        "background must cover the full backbuffer, got {bg:?}",
    );

    // Border: 740x360 centered -> ((1280-740)/2, (720-360)/2) = (270, 180).
    assert_rect(
        "border",
        panels.instances[BORDER].rect,
        [270.0, 180.0, 740.0, 360.0],
    );
    // Fill: 732x352 (inset by 4px each edge) centered -> (274, 184).
    assert_rect(
        "fill",
        panels.instances[FILL].rect,
        [274.0, 184.0, 732.0, 352.0],
    );

    // Logo: 600 wide, height 600/3.485 ~ 172 (rounded), centered with the -40
    // vertical nudge -> top-left (640-300, (360-40)-86) = (340, 234).
    assert_rect("logo", logo.instances[0].rect, [340.0, 234.0, 600.0, 172.0]);
}

#[test]
fn splash_panel_quads_scale_uniformly_at_4k() {
    // Regression guard for the logical-reference->device scale math. 3840x2160 is
    // exactly 3x the 1280x720 reference, so every reference rect must triple in
    // position and size with no re-layout artifact between the two resolutions.
    let desc = build_splash_descriptor(ASSET_LOGO_ASPECT);
    assert!((device_scale([3840, 2160]) - 3.0).abs() <= EPS);
    let (panels, logo) = project_splash(&desc, [3840, 2160]);

    // Border (270,180,740,360) * 3 -> (810,540,2220,1080).
    assert_rect(
        "border@4k",
        panels.instances[BORDER].rect,
        [810.0, 540.0, 2220.0, 1080.0],
    );
    // Fill (274,184,732,352) * 3 -> (822,552,2196,1056).
    assert_rect(
        "fill@4k",
        panels.instances[FILL].rect,
        [822.0, 552.0, 2196.0, 1056.0],
    );
    // Logo: at 3x the 600x172.19 logical rect projects+snaps to (1020,702,1800,516).
    assert_rect(
        "logo@4k",
        logo.instances[0].rect,
        [1020.0, 702.0, 1800.0, 516.0],
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
    let desc = build_splash_descriptor(ASSET_LOGO_ASPECT);
    assert!((device_scale([1920, 720]) - 1.0).abs() <= EPS);
    let (panels, _logo) = project_splash(&desc, [1920, 720]);

    // Border centered in the letterboxed canvas: reference (270,180) + (320,0)
    // origin -> (590,180); size unchanged at scale 1.0.
    assert_rect(
        "border letterbox",
        panels.instances[BORDER].rect,
        [590.0, 180.0, 740.0, 360.0],
    );
}

#[test]
fn splash_panel_rects_snap_to_integer_device_pixels() {
    // Regression guard for the device-pixel snap. At a non-integer scale the
    // projected edges are fractional pre-snap; every produced rect component must
    // round to a whole device pixel so panels show no subpixel edge blur. 1281x721
    // gives scale 1281/1280 ~ 1.00078, which moves edges off integer boundaries.
    let desc = build_splash_descriptor(ASSET_LOGO_ASPECT);
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
    let desc = build_splash_descriptor(ASSET_LOGO_ASPECT);

    // Reference: 12px corners on the (270,180,740,360) border.
    let panels = project_splash(&desc, [1280, 720]).0;
    let border = panels.instances[BORDER];
    assert_corner_rects(border, 12.0);

    // 4K: corners scale to 36px and stay pinned to the (810,540,2220,1080) rect.
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
    // Regression: the descriptor forced the logo square (LOGO_ASPECT = 1.0),
    // crushing the wide banner asset. The logo rect must (a) match the SOURCE
    // image aspect — not whatever the code emits — and (b) scale by the uniform
    // device factor only, so the aspect is identical at 720p, 4K, and a non-16:9
    // size. "without stretching the logo" in the acceptance criteria.
    let desc = build_splash_descriptor(ASSET_LOGO_ASPECT);
    let sizes = [[1280u32, 720], [3840, 2160], [1920, 720]];
    let mut aspects = Vec::new();
    for size in sizes {
        let r = project_splash(&desc, size).1.instances[0].rect;
        aspects.push(r[2] / r[3]);
    }
    // Each projected rect aspect matches the source banner aspect. The integer
    // device-pixel snap perturbs it slightly, so the epsilon is looser than the
    // exact-pixel EPS — but tight enough to catch a forced-square regression.
    for a in &aspects {
        assert!(
            (a - ASSET_LOGO_ASPECT).abs() <= 1e-2,
            "logo rect aspect {a} must match source aspect {ASSET_LOGO_ASPECT}: {aspects:?}",
        );
    }
    for a in &aspects[1..] {
        assert!(
            (a - aspects[0]).abs() <= 1e-2,
            "logo aspect drifted across resolutions: {aspects:?}",
        );
    }
}
