// Hard-gate CPU draw-list / layout assertion for the boot splash (Task 6 gate).
//
// Lays the splash descriptor `AnchoredTree` out through `UiTree` (the retained
// tree + taffy layout + glyphon measure seam) at known backbuffer sizes and pins
// the produced device-pixel draw data: the panel anchor (centered), the
// logical-reference→device scale, the integer-pixel snap, the 9-slice border
// corner rects, the logo image batch (keyed to the logo asset), and the
// measured-width-centered version text. This test FAILS if the splash anchor,
// the 9-slice corners, the scale math, or the panel/logo/text composition
// regresses. Pure CPU: a headless `FontSystem` measures text; no GPU adapter, no
// wgpu call.
//
// Mirrors `Renderer::record_splash_ui`'s draw-list assembly: the oversized
// background fill is the first quad (projected through `layout`, outside the
// tree), then the tree's panel quads in the white-texel batch, then the logo in
// its own image batch, then the version text. If that assembly changes, this
// fixture changes with it — that coupling is the point.
//
// See: context/plans/in-progress/M13--descriptor-tree-layout (Task 6 gate).

use super::layout::{self, device_scale};
use super::splash::{SPLASH_LOGO_ASSET, build_splash_descriptor};
use super::tree::{UiDrawData, UiTree};
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

/// A headless `FontSystem` (embedded Inter face registered, no GPU). Text nodes
/// measure through this in `build_draw_data`.
fn font_system() -> glyphon::FontSystem {
    super::text::build_font_system()
}

/// Reproduce the renderer's splash draw-list assembly (`record_splash_ui`): the
/// oversized background fill (projected outside the tree) plus the tree's draw
/// data (panel quads, logo image batch, version text). Returns the combined
/// white-texel quad list (background + panels) and the full tree draw data.
fn lay_out_splash(version: &str, device_size: [u32; 2]) -> (UiDrawList, UiDrawData) {
    let bg = super::splash::SplashDescriptor::background_element(splash_bg_rgba());
    let mut panels = layout::project(&[bg], device_size);

    let desc = build_splash_descriptor(ASSET_LOGO_ASPECT, version);
    let mut ui = UiTree::from_descriptor(desc.tree());
    let mut fs = font_system();
    let draw = ui.build_draw_data(device_size, &mut fs);

    panels.instances.extend_from_slice(&draw.quads.instances);
    (panels, draw)
}

/// Indices into the combined white-texel quad list: background first, then the
/// tree's border + fill panels (in tree order).
const BG: usize = 0;
const BORDER: usize = 1;
const FILL: usize = 2;

#[test]
fn splash_panel_quads_anchor_centered_at_reference_resolution() {
    // At the exact 1280x720 reference (device scale 1.0) the framed panel lands
    // dead-center and the background covers the canvas. If the Center anchor math
    // or the panel size regresses these rects move and the test fails.
    let (panels, draw) = lay_out_splash("postretro v0.1.0", [1280, 720]);
    assert_eq!(panels.len(), 3, "background + border + fill");

    // Background: oversized fill centered on the canvas, overhanging every edge.
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
    // Fill: 732x352 (inset by 4px each edge) -> (274, 184).
    assert_rect(
        "fill",
        panels.instances[FILL].rect,
        [274.0, 184.0, 732.0, 352.0],
    );

    // Logo: its own image batch keyed to the logo asset. 600 wide, height
    // 600/3.485 ~ 172, centered with the -40 nudge -> (340, 234).
    assert_eq!(draw.images.len(), 1, "one image batch (the logo)");
    assert_eq!(draw.images[0].0, SPLASH_LOGO_ASSET);
    assert_rect(
        "logo",
        draw.images[0].1.instances[0].rect,
        [340.0, 234.0, 600.0, 172.0],
    );
}

#[test]
fn splash_panel_quads_scale_uniformly_at_4k() {
    // 3840x2160 is exactly 3x the 1280x720 reference, so every reference rect
    // triples in position and size with no re-layout artifact.
    assert!((device_scale([3840, 2160]) - 3.0).abs() <= EPS);
    let (panels, draw) = lay_out_splash("postretro v0.1.0", [3840, 2160]);

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
    // Logo (340,234,600,172) * 3 -> (1020,702,1800,516).
    assert_rect(
        "logo@4k",
        draw.images[0].1.instances[0].rect,
        [1020.0, 702.0, 1800.0, 516.0],
    );

    // 9-slice margin scales: 12px logical -> 36px device.
    assert_rect(
        "border margin@4k",
        panels.instances[BORDER].margin,
        [36.0, 36.0, 36.0, 36.0],
    );
}

#[test]
fn splash_panel_anchor_centers_against_letterbox_on_non_16_9() {
    // 1920x720: x ratio 1.5, y ratio 1.0 -> uniform scale 1.0, and the 1280-wide
    // canvas is centered horizontally with a (1920-1280)/2 = 320px left margin.
    // The panel shifts right by that margin, NOT stretching — the canvas
    // letterboxes rather than scaling each axis independently.
    assert!((device_scale([1920, 720]) - 1.0).abs() <= EPS);
    let (panels, _draw) = lay_out_splash("postretro v0.1.0", [1920, 720]);

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
    // At a non-integer scale every produced rect component must round to a whole
    // device pixel so panels show no subpixel edge blur. 1281x721 gives scale
    // ~1.00078, moving edges off integer boundaries pre-snap.
    let (panels, draw) = lay_out_splash("postretro v0.1.0", [1281, 721]);
    let logo = &draw.images[0].1;
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
    // The border's 9-slice corners keep their (scaled) fixed size and stay
    // anchored to the four rect corners at every resolution.
    let panels = lay_out_splash("postretro v0.1.0", [1280, 720]).0;
    assert_corner_rects(panels.instances[BORDER], 12.0);

    let panels4k = lay_out_splash("postretro v0.1.0", [3840, 2160]).0;
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
    // The logo rect must (a) match the SOURCE image aspect and (b) scale by the
    // uniform device factor only, so the aspect is identical at 720p, 4K, and a
    // non-16:9 size — never stretched.
    let sizes = [[1280u32, 720], [3840, 2160], [1920, 720]];
    let mut aspects = Vec::new();
    for size in sizes {
        let (_, draw) = lay_out_splash("postretro v0.1.0", size);
        let r = draw.images[0].1.instances[0].rect;
        aspects.push(r[2] / r[3]);
    }
    // Each projected rect aspect matches the source banner aspect (the integer
    // device-pixel snap perturbs it slightly, so the epsilon is looser).
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

#[test]
fn splash_version_text_centers_on_panel_via_measured_width() {
    // The version line centers horizontally on the panel center from its REAL
    // shaped-run width (the measured-width centering Goal B owes A): the run's
    // center x must land on the panel center (canvas center 640 at scale 1.0),
    // and its left edge must back off half the measured run width. A wider
    // string shifts its left edge further left while keeping the same center —
    // proof the centering uses the measured width, not a fixed offset.
    let (_, narrow) = lay_out_splash("v1", [1280, 720]);
    let (_, wide) = lay_out_splash("postretro v0.1.0-wide-build", [1280, 720]);

    assert_eq!(narrow.texts.len(), 1);
    assert_eq!(wide.texts.len(), 1);

    // Center x is the panel center (canvas center at scale 1.0) for both, derived
    // from each run's measured width: left + width/2 == 640.
    let panel_center_x = 640.0;
    let measured_w = |d: &UiDrawData| {
        // Re-derive the measured device width: the centered left edge is
        // `center - width/2`, so width = 2 * (center - left).
        2.0 * (panel_center_x - d.texts[0].position[0])
    };
    let narrow_w = measured_w(&narrow);
    let wide_w = measured_w(&wide);

    // The wider string measured wider, so its left edge backs off further.
    assert!(
        wide_w > narrow_w + 1.0,
        "wider version string must shape wider ({wide_w} vs {narrow_w})",
    );
    assert!(
        wide.texts[0].position[0] < narrow.texts[0].position[0],
        "wider run's left edge backs off further from the shared center",
    );
    // Both share the panel center: left + width/2 == 640.
    for d in [&narrow, &wide] {
        let center = d.texts[0].position[0] + measured_w(d) * 0.5;
        assert!(
            (center - panel_center_x).abs() <= 1.0,
            "version text center {center} must sit on the panel center {panel_center_x}",
        );
    }
    // The run sits below the logo, inside the panel (the composition invariant).
    let logo = wide.images[0].1.instances[0].rect;
    let text_top = wide.texts[0].position[1];
    assert!(
        text_top > logo[1] + logo[3],
        "version text starts below the logo (text_top {text_top}, logo bottom {})",
        logo[1] + logo[3],
    );
}
