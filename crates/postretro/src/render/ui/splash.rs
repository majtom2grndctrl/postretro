// Splash content descriptor: the hardcoded Rust description of the boot splash,
// authored in 1280x720 logical-reference space and drawn through the UI pass via
// the retained descriptor tree (`UiTree`). This is the ONE named seam
// (`build_splash_descriptor`) that Goal B builds on `AnchoredTree` and G1 will
// replace with script ingestion. Goal B ingests no script — the content is fixed
// Rust behind this builder.
// See: context/lib/boot_sequence.md §3a · context/plans/in-progress/M13--descriptor-tree-layout

use crate::input::UiCaptureMode;

use super::descriptor::{
    Align, AnchoredTree, Border, ContainerWidget, ImageWidget, PanelWidget, Place, TextWidget,
    Widget,
};
use super::layout::{Anchor, REFERENCE_HEIGHT, REFERENCE_WIDTH};

/// Linear-space sRGB(21, 27, 35) — the framed panel fill behind the logo. Kept a
/// touch lighter than the fullscreen background so the framed 9-slice panel reads
/// as a distinct surface and its corners are visible on screen.
const PANEL_COLOR: [f32; 4] = [0.018, 0.026, 0.039, 1.0];

/// Panel border (linear RGBA) — a brighter rim drawn as a 9-slice frame behind
/// the fill so the 9-slice corners are genuinely exercised.
const PANEL_BORDER_COLOR: [f32; 4] = [0.10, 0.55, 0.62, 1.0];

/// 9-slice corner margin for the bordered frame, logical-reference px.
const PANEL_BORDER_MARGIN: f32 = 12.0;

/// Logo width in logical-reference px — about half the 1280-wide canvas. Height
/// is derived from the decoded source aspect (see `logo_reference_size`).
const LOGO_REFERENCE_WIDTH: f32 = 600.0;

/// Horizontal padding between the logo and the framed panel's inner edge
/// (logical-reference px, each side).
const PANEL_H_PADDING: f32 = 70.0;

/// Framed panel size in logical-reference px. Width hugs the logo (+ horizontal
/// padding); height is fixed tall enough to clear the (nudged-up) logo on top and
/// the shaped-text line below.
const PANEL_SIZE: [f32; 2] = [LOGO_REFERENCE_WIDTH + 2.0 * PANEL_H_PADDING, 360.0];

/// Inset of the fill panel inside the border frame (logical-reference px). The
/// border draws at `PANEL_SIZE`; the fill draws inset by this on every edge so a
/// rim of the border color frames the fill.
const PANEL_BORDER_THICKNESS: f32 = 4.0;

/// Vertical nudge of the logo above the panel center (logical-reference px), so
/// the shaped-text line fits below it within the framed panel.
const LOGO_OFFSET_Y: f32 = -40.0;

/// Shaped-text line: device-independent font size in logical-reference px, and
/// its baseline-box offset below the panel center so it sits under the logo.
const TEXT_LOGICAL_FONT_SIZE: f32 = 22.0;
const TEXT_OFFSET_Y: f32 = 90.0;

/// Shaped-text color, linear RGBA — a soft off-white that reads against the dark
/// panel. These are the exact linear values that round-trip to the hand-built
/// path's sRGB (196, 214, 224) (the tree converts linear→sRGB at draw time), so
/// the version text color is byte-identical to the pre-tree splash.
const TEXT_COLOR: [f32; 4] = [0.552011, 0.672443, 0.745404, 1.0];

/// The image-registry key the splash logo resolves through. `install_splash_from_loaded`
/// registers the uploaded PNG's bind group under this key, and the descriptor's
/// logo `image` node references it; the renderer's image registry maps the two.
/// The single named splash asset — Goal B pre-registers only known keys.
pub(crate) const SPLASH_LOGO_ASSET: &str = "splash/logo";

/// The splash's input capture mode, for the dispatch seam. The splash is
/// non-interactive, so it is always `Passthrough` (events flow to gameplay). A
/// free function so the renderer can report it without building a descriptor.
pub(crate) fn splash_capture_mode() -> UiCaptureMode {
    UiCaptureMode::Passthrough
}

/// The logo size in logical-reference px: a fixed width with the height derived
/// from the decoded source aspect (width / height), so the on-screen logo is
/// always shaped to the real asset.
fn logo_reference_size(logo_aspect: f32) -> [f32; 2] {
    [LOGO_REFERENCE_WIDTH, LOGO_REFERENCE_WIDTH / logo_aspect]
}

/// Hardcoded splash content. Carries the descriptor `AnchoredTree` the renderer
/// lays out through `UiTree`, the fullscreen background color the caller draws as
/// the first quad, and the capture/passthrough flag for the input-dispatch seam.
///
/// This is the seam: Goal B builds it on `AnchoredTree`; G1 replaces the body
/// with script ingestion while keeping the `SplashDescriptor` shape and call
/// sites stable. The renderer stores one of these as the active splash and
/// re-lays-out its tree each frame against the live backbuffer size.
pub(crate) struct SplashDescriptor {
    /// The descriptor tree: a center-anchored backing panel with the fill, logo
    /// `image`, and version `text` overlaid (absolute placement). The splash's
    /// capture mode is non-interactive (`splash_capture_mode`), independent of
    /// this content.
    tree: AnchoredTree,
}

/// The single named builder seam. Returns the active splash content as a
/// descriptor `AnchoredTree`. `logo_aspect` is the decoded logo image's
/// width/height (the caller passes the real dimensions so the logo is shaped to
/// the asset). `version_line` is the per-frame version/tagline string from the
/// read snapshot — it becomes the splash's `text` node content.
pub(crate) fn build_splash_descriptor(logo_aspect: f32, version_line: &str) -> SplashDescriptor {
    let logo_size = logo_reference_size(logo_aspect);

    // The anchored root is a zero-gap container fixed to the panel box (PANEL_SIZE),
    // centered on the canvas. Every visual layer is an absolutely-placed child
    // measured from the container's content-box top-left — the container has no
    // padding, so insets land at exact panel-relative coordinates. This reproduces
    // the hand-built path's overlapping composition (border, fill, logo, text).

    // Backing 9-slice border panel — fills the whole panel box. Only `slice`
    // drives the shader's corner margins; the white-texel quad path ignores the
    // border texture/tint (panels sample the 1×1 white texel), mirroring the old
    // `panel_9slice`.
    let border = Widget::Panel(PanelWidget {
        fill: PANEL_BORDER_COLOR,
        border: Some(Border {
            texture: String::new(),
            slice: [
                PANEL_BORDER_MARGIN,
                PANEL_BORDER_MARGIN,
                PANEL_BORDER_MARGIN,
                PANEL_BORDER_MARGIN,
            ],
            tint: PANEL_BORDER_COLOR,
        }),
        place: Some(Place::at([0.0, 0.0], PANEL_SIZE)),
    });

    // Fill panel inset by the border thickness on every edge, so a rim of the
    // border color frames it.
    let fill = Widget::Panel(PanelWidget {
        fill: PANEL_COLOR,
        border: None,
        place: Some(Place::at(
            [PANEL_BORDER_THICKNESS, PANEL_BORDER_THICKNESS],
            [
                PANEL_SIZE[0] - 2.0 * PANEL_BORDER_THICKNESS,
                PANEL_SIZE[1] - 2.0 * PANEL_BORDER_THICKNESS,
            ],
        )),
    });

    // Logo centered over the panel at the fixed logical-reference size, nudged up
    // by LOGO_OFFSET_Y.
    let logo_inset = [
        (PANEL_SIZE[0] - logo_size[0]) * 0.5,
        (PANEL_SIZE[1] - logo_size[1]) * 0.5 + LOGO_OFFSET_Y,
    ];
    let logo = Widget::Image(ImageWidget {
        asset: SPLASH_LOGO_ASSET.to_string(),
        place: Some(Place::at(logo_inset, logo_size)),
    });

    // Version line: centered horizontally on the panel center, placed below the
    // panel center by TEXT_OFFSET_Y. `center_x` backs the run off half its real
    // shaped width at draw time, so centering uses the measured run width.
    let text_center = [PANEL_SIZE[0] * 0.5, PANEL_SIZE[1] * 0.5 + TEXT_OFFSET_Y];
    let text = Widget::Text(TextWidget {
        content: version_line.to_string(),
        font_size: TEXT_LOGICAL_FONT_SIZE,
        color: TEXT_COLOR,
        place: Some(Place::centered_x(text_center)),
    });

    let root = Widget::VStack(ContainerWidget {
        gap: 0.0,
        padding: 0.0,
        align: Align::Start,
        children: vec![border, fill, logo, text],
        // Pin the container to the panel box so the anchored-root size is exactly
        // PANEL_SIZE regardless of the (all-absolute) children.
        place: Some(Place::sized(PANEL_SIZE)),
    });

    let tree = AnchoredTree {
        anchor: Anchor::Center,
        offset: [0.0, 0.0],
        root,
    };

    SplashDescriptor { tree }
}

impl SplashDescriptor {
    /// The descriptor tree the renderer lays out through `UiTree`. Borrowed so
    /// the renderer can rebuild its `UiTree` from it without cloning each frame.
    pub(crate) fn tree(&self) -> &AnchoredTree {
        &self.tree
    }

    /// A fullscreen-background `UiElement` covering the whole logical-reference
    /// canvas with the given linear-RGBA color. Drawn as the first quad so it
    /// fills the letterbox region and sits behind the framed panel. The
    /// background stays a single oversized quad outside the tree — it is a plain
    /// letterbox fill, not part of the panel composition.
    pub(crate) fn background_element(color: [f32; 4]) -> super::layout::UiElement {
        super::layout::UiElement::panel(
            Anchor::Center,
            [0.0, 0.0],
            // Oversize so the scaled fill always covers the backbuffer even when
            // the canvas letterboxes (uniform scale leaves margins on one axis).
            [REFERENCE_WIDTH * 4.0, REFERENCE_HEIGHT * 4.0],
            color,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::render::ui::tree::UiTree;

    /// The real committed logo asset is 2028x582, a wide banner (aspect ~3.485).
    const ASSET_LOGO_ASPECT: f32 = 2028.0 / 582.0;

    fn font_system() -> glyphon::FontSystem {
        crate::render::ui::text::build_font_system()
    }

    #[test]
    fn descriptor_is_passthrough() {
        // The splash is non-interactive — the dispatch seam must stay passthrough.
        // (Built once to confirm the descriptor still constructs cleanly.)
        let _ = build_splash_descriptor(ASSET_LOGO_ASPECT, "v0.1.0");
        assert_eq!(splash_capture_mode(), UiCaptureMode::Passthrough);
    }

    #[test]
    fn splash_tree_lays_out_panel_logo_and_text() {
        // The descriptor tree must produce the splash's three visual layers: a
        // backing panel + fill (quads), the logo (its own image batch keyed to
        // the logo asset), and the version text run.
        let desc = build_splash_descriptor(ASSET_LOGO_ASPECT, "postretro v0.1.0");
        let mut ui = UiTree::from_descriptor(desc.tree());
        let mut fs = font_system();
        let data = ui.build_draw_data([1280, 720], &mut fs);

        // Border + fill panels are quads (white-texel batch).
        assert!(
            data.quads.len() >= 2,
            "splash draws the border + fill panel quads, got {}",
            data.quads.len()
        );
        // The logo resolves to an image batch keyed to the logo asset.
        assert_eq!(data.images.len(), 1, "one image batch (the logo)");
        assert_eq!(data.images[0].0, SPLASH_LOGO_ASSET);
        assert_eq!(data.images[0].1.len(), 1, "one logo quad");
        // The version line is one shaped-text run.
        assert_eq!(data.texts.len(), 1, "the version line");
        assert_eq!(data.texts[0].content, "postretro v0.1.0");
    }
}
