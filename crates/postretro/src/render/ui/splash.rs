// Splash content descriptor: the hardcoded Rust description of the boot splash,
// authored in 1280x720 logical-reference space and drawn through the UI pass.
// This is the ONE named seam (`build_splash_descriptor`) that Goal B replaces
// with a parsed descriptor model and G1 with script ingestion. Goal A ingests no
// script — the content is fixed Rust behind this builder.
// See: context/lib/boot_sequence.md §3a · context/plans/in-progress/M13--ui-render-pass-slice

use crate::input::UiCaptureMode;

use super::UiText;
use super::layout::{Anchor, REFERENCE_HEIGHT, REFERENCE_WIDTH, UiElement};

/// Linear-space sRGB(21, 27, 35) — the framed panel fill behind the logo. Kept a
/// touch lighter than the fullscreen background so the framed 9-slice panel reads
/// as a distinct surface and its corners are visible on screen. The fullscreen
/// background fill itself uses `render::splash::SPLASH_BG_COLOR` (the retired
/// pipeline's clear color), passed in by the caller as the first quad.
const PANEL_COLOR: [f32; 4] = [0.018, 0.026, 0.039, 1.0];

/// Panel border (linear RGBA) — a brighter rim drawn as a 9-slice frame behind
/// the fill so the 9-slice corners are genuinely exercised. The fill panel sits
/// inset over this so only the frame's border shows.
const PANEL_BORDER_COLOR: [f32; 4] = [0.10, 0.55, 0.62, 1.0];

/// 9-slice corner margin for the bordered frame, logical-reference px. Corners
/// keep this size when the panel scales; edges/center stretch (the 9-slice rule).
const PANEL_BORDER_MARGIN: f32 = 12.0;

/// Logo width in logical-reference px — about half the 1280-wide canvas, so the
/// banner reads prominently. Height is derived from the decoded source aspect (see
/// `logo_reference_size`), never a constant, so the logo is always shaped to the
/// real asset and only the uniform device scale applies — never an x/y stretch.
const LOGO_REFERENCE_WIDTH: f32 = 600.0;

/// Horizontal padding between the logo and the framed panel's inner edge
/// (logical-reference px, each side). The panel width is the logo width plus this
/// on both sides, so the frame visibly contains the banner with breathing room.
const PANEL_H_PADDING: f32 = 70.0;

/// Framed panel size in logical-reference px. Non-fullscreen and centered, so the
/// fullscreen background fill shows around it and the framed corners are on screen.
/// Width hugs the logo (+ horizontal padding); height is fixed tall enough to clear
/// the (nudged-up) logo on top and the shaped-text line below, both with padding.
/// Stays within the 1280x720 canvas when centered.
const PANEL_SIZE: [f32; 2] = [LOGO_REFERENCE_WIDTH + 2.0 * PANEL_H_PADDING, 360.0];

/// Inset of the fill panel inside the border frame (logical-reference px). The
/// border draws at `PANEL_SIZE`; the fill draws inset by this on every edge so a
/// rim of the border color frames the fill.
const PANEL_BORDER_THICKNESS: f32 = 4.0;

/// Vertical nudge of the logo above the panel center (logical-reference px), so
/// the shaped-text line fits below it within the framed panel.
const LOGO_OFFSET_Y: f32 = -40.0;

/// Shaped-text line: device-independent font size in logical-reference px (the
/// caller multiplies by `device_scale`), and its baseline-box offset below the
/// panel center so it sits under the logo inside the frame.
const TEXT_LOGICAL_FONT_SIZE: f32 = 22.0;
const TEXT_OFFSET_Y: f32 = 90.0;

/// Shaped-text color, sRGB 0..=255 + alpha — a soft off-white that reads against
/// the dark panel.
const TEXT_COLOR: [u8; 4] = [196, 214, 224, 255];

/// The logo size in logical-reference px: a fixed width with the height derived
/// from the decoded source aspect (width / height), so the on-screen logo is
/// always shaped to the real asset. Projection then applies only the uniform
/// device scale to both axes — never an independent x/y stretch.
fn logo_reference_size(logo_aspect: f32) -> [f32; 2] {
    [LOGO_REFERENCE_WIDTH, LOGO_REFERENCE_WIDTH / logo_aspect]
}

/// Hardcoded splash content in logical-reference (1280x720) space. Carries the
/// quad elements (background fill is supplied by the caller, then border frame,
/// fill panel, logo) plus the shaped-text layout parameters and the
/// capture/passthrough mode flag that feeds the input-dispatch seam.
///
/// This is the seam: Goal B replaces `build_splash_descriptor` with a parsed
/// descriptor; G1 with script ingestion. The renderer stores one of these as the
/// active splash and re-projects it each frame against the live backbuffer size.
pub(crate) struct SplashDescriptor {
    /// Bordered 9-slice frame (drawn first of the panel quads, behind the fill).
    border: UiElement,
    /// Fill panel inset over the border so a rim of the border shows.
    fill: UiElement,
    /// Centered logo image at a fixed logical-reference size.
    logo: UiElement,
    /// Input capture mode the descriptor drives at the dispatch seam. The splash
    /// is non-interactive, so this is `Passthrough` (events flow to gameplay).
    capture_mode: UiCaptureMode,
}

/// The single named builder seam. Returns the active splash content; Goal B/G1
/// replace this body with descriptor parsing / script ingestion while keeping the
/// `SplashDescriptor` shape and call sites stable. `logo_aspect` is the decoded
/// logo image's width/height — the caller passes the real dimensions so the logo
/// is shaped to the asset rather than a hardcoded constant.
pub(crate) fn build_splash_descriptor(logo_aspect: f32) -> SplashDescriptor {
    let logo_size = logo_reference_size(logo_aspect);

    // Bordered 9-slice frame, centered. Margins exercise the 9-slice corners.
    let border = UiElement::panel_9slice(
        Anchor::Center,
        [0.0, 0.0],
        PANEL_SIZE,
        PANEL_BORDER_COLOR,
        [
            PANEL_BORDER_MARGIN,
            PANEL_BORDER_MARGIN,
            PANEL_BORDER_MARGIN,
            PANEL_BORDER_MARGIN,
        ],
    );

    // Fill panel inset over the border so only the frame's rim shows.
    let fill = UiElement::panel(
        Anchor::Center,
        [0.0, 0.0],
        [
            PANEL_SIZE[0] - 2.0 * PANEL_BORDER_THICKNESS,
            PANEL_SIZE[1] - 2.0 * PANEL_BORDER_THICKNESS,
        ],
        PANEL_COLOR,
    );

    // Logo centered over the panel at the fixed logical-reference size.
    let logo = UiElement::image(Anchor::Center, [0.0, LOGO_OFFSET_Y], logo_size);

    SplashDescriptor {
        border,
        fill,
        logo,
        // Splash is non-interactive: events pass through to gameplay (which is
        // inert pre-`Running` anyway). Drives the Task 5 dispatch seam.
        capture_mode: UiCaptureMode::Passthrough,
    }
}

impl SplashDescriptor {
    /// The capture/passthrough mode the App feeds into the input-dispatch seam.
    pub(crate) fn capture_mode(&self) -> UiCaptureMode {
        self.capture_mode
    }

    /// The solid-panel quad elements (border frame + fill), in draw order. The
    /// caller prepends the fullscreen background fill and appends the logo as a
    /// separate textured batch, so panels and the logo land in distinct batches.
    pub(crate) fn panel_elements(&self) -> [UiElement; 2] {
        [self.border, self.fill]
    }

    /// The logo image element (separate textured batch — its own bound texture).
    pub(crate) fn logo_element(&self) -> UiElement {
        self.logo
    }

    /// A fullscreen-background `UiElement` covering the whole logical-reference
    /// canvas with the given linear-RGBA color. Drawn as the first quad so it
    /// fills the letterbox region and sits behind the framed panel.
    pub(crate) fn background_element(color: [f32; 4]) -> UiElement {
        UiElement::panel(
            Anchor::Center,
            [0.0, 0.0],
            // Oversize so the scaled fill always covers the backbuffer even when
            // the canvas letterboxes (uniform scale leaves margins on one axis).
            [REFERENCE_WIDTH * 4.0, REFERENCE_HEIGHT * 4.0],
            color,
        )
    }

    /// Build the shaped-text line for the given version/tagline string and device
    /// scale. Position is in device pixels (centered horizontally below the logo);
    /// font size is device-scaled. The string comes from the read-handle snapshot
    /// so the once-per-frame contract carries a real value.
    pub(crate) fn text_line(&self, content: &str, device_size: [u32; 2], scale: f32) -> UiText {
        // Canvas origin so the text anchors to the same letterboxed canvas the
        // quads project against (mirrors `layout::project`'s centering).
        let scaled_w = REFERENCE_WIDTH * scale;
        let scaled_h = REFERENCE_HEIGHT * scale;
        let origin_x = (device_size[0] as f32 - scaled_w) * 0.5;
        let origin_y = (device_size[1] as f32 - scaled_h) * 0.5;

        // Baseline-box top-left in device pixels. glyphon positions the run from
        // `left`; we approximate-center by backing off half an estimated run
        // width. Single-line UI text needs no exact centering in A — the layout
        // box clips to the backbuffer — so a left position under the panel center
        // is sufficient and keeps the path device-scaled and unstretched.
        let center_x = origin_x + (REFERENCE_WIDTH * 0.5) * scale;
        let top = origin_y + (REFERENCE_HEIGHT * 0.5 + TEXT_OFFSET_Y) * scale;
        // Rough centering: shift left by half the string's estimated device width
        // (an average glyph advance ~ 0.5em). Not exact — glyphon owns shaping —
        // but keeps the line under the logo at every resolution.
        let est_advance = TEXT_LOGICAL_FONT_SIZE * scale * 0.5;
        let left = center_x - (content.chars().count() as f32 * est_advance) * 0.5;

        UiText::new(
            content,
            [left, top],
            TEXT_LOGICAL_FONT_SIZE * scale,
            TEXT_COLOR,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::super::layout::{device_scale, project};
    use super::*;

    /// The real committed logo asset is 2028x582, a wide banner (aspect ~3.485).
    /// The descriptor takes this from the decoded image; tests pass the real value
    /// so they pin the same shaping the engine applies.
    const ASSET_LOGO_ASPECT: f32 = 2028.0 / 582.0;

    #[test]
    fn descriptor_is_passthrough() {
        // The splash is non-interactive — the dispatch seam must stay passthrough.
        assert_eq!(
            build_splash_descriptor(ASSET_LOGO_ASPECT).capture_mode(),
            UiCaptureMode::Passthrough
        );
    }

    #[test]
    fn panel_is_framed_and_centered_at_reference_res() {
        // The bordered frame is non-fullscreen and centered, so the fullscreen
        // background shows around it and the 9-slice corners are on screen.
        let desc = build_splash_descriptor(ASSET_LOGO_ASPECT);
        let list = project(&desc.panel_elements(), [1280, 720]);
        let border = list.instances[0];
        // Centered 740x360 panel: top-left at ((1280-740)/2, (720-360)/2).
        assert_eq!(border.rect, [270.0, 180.0, 740.0, 360.0]);
        // Non-zero 9-slice margin so the shader expands real corners.
        assert!(border.margin.iter().all(|&m| m > 0.0));
        // Fill panel is inset inside the border.
        let fill = list.instances[1];
        assert!(fill.rect[0] > border.rect[0], "fill inset from border left");
        assert!(fill.rect[2] < border.rect[2], "fill narrower than border");
    }

    #[test]
    fn logo_keeps_aspect_under_scale() {
        // Regression: the descriptor hardcoded a square logo (LOGO_ASPECT = 1.0),
        // crushing the wide banner asset horizontally. The logo rect's aspect must
        // now equal the SOURCE image aspect, not just stay self-consistent across
        // resolutions — that consistency held even while the logo was wrongly square.
        let desc = build_splash_descriptor(ASSET_LOGO_ASPECT);
        let r1 = project(&[desc.logo_element()], [1280, 720]).instances[0].rect;
        let r3 = project(&[desc.logo_element()], [3840, 2160]).instances[0].rect;
        let aspect1 = r1[2] / r1[3];
        let aspect3 = r3[2] / r3[3];
        // Both match the source aspect (within the device-pixel snap tolerance).
        assert!(
            (aspect1 - ASSET_LOGO_ASPECT).abs() < 1e-2,
            "logo rect aspect {aspect1} must match source aspect {ASSET_LOGO_ASPECT}"
        );
        assert!(
            (aspect1 - aspect3).abs() < 1e-2,
            "logo aspect preserved at 4K"
        );
        // And the size tripled (uniform device scale only).
        assert!((r3[2] - r1[2] * 3.0).abs() < 2.0);
        assert!((r3[3] - r1[3] * 3.0).abs() < 2.0);
    }

    #[test]
    fn logo_fits_within_framed_panel() {
        // Composition invariant: the widened panel must fully contain the logo
        // (including its LOGO_OFFSET_Y nudge), leaving room below for the text line.
        // Guards against the logo overflowing the frame after the banner widening.
        let desc = build_splash_descriptor(ASSET_LOGO_ASPECT);
        let panels = project(&desc.panel_elements(), [1280, 720]);
        let border = panels.instances[0].rect;
        let logo = project(&[desc.logo_element()], [1280, 720]).instances[0].rect;

        let (bx, by, bw, bh) = (border[0], border[1], border[2], border[3]);
        let (lx, ly, lw, lh) = (logo[0], logo[1], logo[2], logo[3]);
        assert!(lx >= bx, "logo left {lx} inside panel left {bx}");
        assert!(lx + lw <= bx + bw, "logo right inside panel right");
        assert!(ly >= by, "logo top {ly} inside panel top {by}");
        assert!(ly + lh <= by + bh, "logo bottom inside panel bottom");

        // The shaped-text line sits below the logo, still inside the panel.
        let text = desc.text_line("v0.1.0", [1280, 720], device_scale([1280, 720]));
        let text_top = text.position[1];
        assert!(text_top > ly + lh, "text starts below the logo");
        assert!(text_top < by + bh, "text top stays inside the panel");
    }

    #[test]
    fn background_covers_backbuffer() {
        // The fullscreen fill must cover the whole backbuffer at any resolution,
        // including the letterboxed margins.
        let elem = SplashDescriptor::background_element([0.0, 0.0, 0.0, 1.0]);
        let list = project(&[elem], [1280, 1440]); // tall, letterboxed
        let r = list.instances[0].rect;
        assert!(
            r[0] <= 0.0 && r[1] <= 0.0,
            "background origin covers top-left"
        );
        assert!(r[0] + r[2] >= 1280.0, "background covers width");
        assert!(r[1] + r[3] >= 1440.0, "background covers height");
    }

    #[test]
    fn text_line_is_device_scaled() {
        let desc = build_splash_descriptor(ASSET_LOGO_ASPECT);
        let scale = device_scale([3840, 2160]);
        let text = desc.text_line("v0.1.0", [3840, 2160], scale);
        // Font size is the logical size times the device scale.
        assert!((text.font_size - TEXT_LOGICAL_FONT_SIZE * scale).abs() < 1e-3);
    }
}
