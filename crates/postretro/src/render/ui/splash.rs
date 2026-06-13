// Splash content descriptor: the hardcoded Rust description of the boot splash,
// authored in 1280x720 logical-reference space and drawn through the UI pass via
// the retained descriptor tree (`UiTree`). This is the ONE named seam
// (`build_splash_descriptor`) built on `AnchoredTree`; script ingestion (G1) will
// replace the body while keeping the `SplashDescriptor` shape and call sites stable.
// Fixed Rust content behind this builder — no script ingestion yet.
// See: context/lib/boot_sequence.md §3a · context/lib/ui.md

use crate::input::UiCaptureMode;

use super::descriptor::{
    Align, AnchoredTree, Border, ColorValue, ContainerWidget, ImageWidget, SpacingValue,
    TextWidget, Widget,
};
use super::layout::{Anchor, REFERENCE_HEIGHT, REFERENCE_WIDTH};

/// Linear-space sRGB(21, 27, 35) — the framed panel fill behind the logo. Kept a
/// touch lighter than the fullscreen background so the framed panel reads as a
/// distinct surface against the letterbox fill.
const PANEL_COLOR: [f32; 4] = [0.018, 0.026, 0.039, 1.0];

/// Panel border (linear RGBA) — a brighter rim drawn as a 9-slice frame around
/// the fill so the 9-slice corners are genuinely exercised.
const PANEL_BORDER_COLOR: [f32; 4] = [0.10, 0.55, 0.62, 1.0];

/// 9-slice corner margin for the bordered frame, logical-reference px.
const PANEL_BORDER_MARGIN: f32 = 12.0;

/// Logo width in logical-reference px — about half the 1280-wide canvas. Height
/// is derived from the decoded source aspect (see `splash_logo_reference_size`).
const LOGO_REFERENCE_WIDTH: f32 = 600.0;

/// The border rim thickness (logical-reference px): the outer container's padding.
/// The outer (border-colored) container draws its backdrop, then pads its single
/// child — the inner (panel-colored) container — in by this on every edge, so a
/// 4px rim of the border color frames the inner fill. This replaces the old
/// absolute-inset overlap: the rim now falls out of parent-backdrop + padding.
const PANEL_BORDER_THICKNESS: f32 = 4.0;

/// Inner-panel padding (logical-reference px): breathing room between the framed
/// panel's inner fill edge and the flowed logo/text content. The panel
/// content-sizes to logo + this padding + the gap + the text line, so the panel
/// is no longer a hardcoded 740x360 — its size falls out of its content.
const PANEL_CONTENT_PADDING: f32 = 36.0;

/// Vertical gap (logical-reference px) between the logo and the version line as
/// the inner container flows them top-to-bottom.
const LOGO_TEXT_GAP: f32 = 28.0;

/// Shaped-text line: device-independent font size in logical-reference px.
const TEXT_LOGICAL_FONT_SIZE: f32 = 22.0;

/// Shaped-text color, linear RGBA — a soft off-white that reads against the dark
/// panel. These are the exact linear values that round-trip to the hand-built
/// path's sRGB (196, 214, 224) (the tree converts linear→sRGB at draw time), so
/// the version text color is byte-identical to the pre-tree splash.
const TEXT_COLOR: [f32; 4] = [0.552011, 0.672443, 0.745404, 1.0];

/// The image-registry key the splash logo resolves through. `install_splash_from_loaded`
/// registers the uploaded PNG's bind group under this key, and the descriptor's
/// logo `image` node references it; the renderer's image registry maps the two.
/// The single named splash asset — only known keys are pre-registered.
pub(crate) const SPLASH_LOGO_ASSET: &str = "splash/logo";

/// The splash's input capture mode, for the dispatch seam. The splash is
/// non-interactive, so it is always `Passthrough` (events flow to gameplay). A
/// free function so the renderer can report it without building a descriptor.
pub(crate) fn splash_capture_mode() -> UiCaptureMode {
    UiCaptureMode::Passthrough
}

/// The logo's natural reference size (logical-reference px, `[width, height]`):
/// a fixed on-screen width with the height derived from the decoded asset's
/// natural pixel dimensions (width / height), so the logo is always shaped to the
/// real asset and never stretched. The renderer threads this into the measure
/// seam (`tree::ImageSizes`) keyed by `SPLASH_LOGO_ASSET`, so the logo `image`
/// node sizes from its content like a text run — no wire-level size, no absolute
/// placement. `natural_dims` is the uploaded texture's pixel size, which the
/// renderer knows from `install_splash_from_loaded`.
pub(crate) fn splash_logo_reference_size(natural_dims: [u32; 2]) -> [f32; 2] {
    let aspect = natural_dims[0] as f32 / natural_dims[1] as f32;
    [LOGO_REFERENCE_WIDTH, LOGO_REFERENCE_WIDTH / aspect]
}

/// Hardcoded splash content. Carries the descriptor `AnchoredTree` the renderer
/// lays out through `UiTree`, the fullscreen background color the caller draws as
/// the first quad, and the capture/passthrough flag for the input-dispatch seam.
///
/// This is the seam: built on `AnchoredTree`; script ingestion (G1) will replace
/// the body while keeping the `SplashDescriptor` shape and call sites stable.
/// The renderer stores one of these as the active splash and re-lays-out its
/// tree each frame against the live backbuffer size.
pub(crate) struct SplashDescriptor {
    /// The descriptor tree: a center-anchored outer container (border-colored
    /// backdrop + padding) wrapping an inner container (panel-colored backdrop)
    /// that flows the logo `image` above the version `text`. The framed-panel rim
    /// is the outer backdrop showing through the outer padding; the logo and text
    /// flow inside the inner fill — no absolute overlap. The splash's capture mode
    /// is non-interactive (`splash_capture_mode`), independent of this content.
    tree: AnchoredTree,
}

/// The single named builder seam. Returns the active splash content as a
/// descriptor `AnchoredTree`. `version_line` is the per-frame version/tagline
/// string from the read snapshot — it becomes the splash's `text` node content.
///
/// The logo `image` node carries only its asset key; its size is content-driven,
/// resolved from the asset's natural dimensions through the measure seam (the
/// renderer supplies `splash_logo_reference_size` keyed by `SPLASH_LOGO_ASSET`).
/// So this builder no longer takes the logo aspect — sizing moved to the
/// content-driven measure path, matching how text nodes size from shaped glyphs.
pub(crate) fn build_splash_descriptor(version_line: &str) -> SplashDescriptor {
    // Inner container: the panel-colored fill that the logo + version text flow
    // inside. It content-sizes to its children (logo, gap, text) plus its own
    // padding, and centers them on the cross axis (so the version line centers on
    // the logo column from its real measured run width — measured-width centering,
    // now expressed as flex `align: center`).
    let logo = Widget::Image(ImageWidget {
        asset: SPLASH_LOGO_ASSET.to_string(),
    });
    let text = Widget::Text(TextWidget {
        content: version_line.to_string(),
        font_size: TEXT_LOGICAL_FONT_SIZE,
        color: ColorValue::Literal(TEXT_COLOR),
        font: None,
        bind: None,
        style_ranges: None,
    });
    let inner = Widget::VStack(ContainerWidget {
        gap: SpacingValue::Literal(LOGO_TEXT_GAP),
        padding: SpacingValue::Literal(PANEL_CONTENT_PADDING),
        align: Align::Center,
        fill: Some(ColorValue::Literal(PANEL_COLOR)),
        border: None,
        children: vec![logo, text],
    });

    // Outer container: the border-colored backdrop + 9-slice frame. Its single
    // child is the inner panel, padded in by the rim thickness on every edge, so
    // the outer backdrop shows through as a 4px border-colored rim. The outer
    // content-sizes to the inner panel + 2*rim — the whole framed panel sizes to
    // its content (no hardcoded 740x360).
    let outer = Widget::VStack(ContainerWidget {
        gap: SpacingValue::Literal(0.0),
        padding: SpacingValue::Literal(PANEL_BORDER_THICKNESS),
        align: Align::Stretch,
        fill: Some(ColorValue::Literal(PANEL_BORDER_COLOR)),
        border: Some(Border {
            texture: String::new(),
            slice: [
                PANEL_BORDER_MARGIN,
                PANEL_BORDER_MARGIN,
                PANEL_BORDER_MARGIN,
                PANEL_BORDER_MARGIN,
            ],
            tint: ColorValue::Literal(PANEL_BORDER_COLOR),
        }),
        children: vec![inner],
    });

    let tree = AnchoredTree {
        anchor: Anchor::Center,
        offset: [0.0, 0.0],
        root: outer,
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
    use crate::render::ui::theme::UiTheme;
    use crate::render::ui::tree::{ImageSizes, UiTree};

    /// The real committed logo asset is 2028x582, a wide banner (aspect ~3.485).
    const ASSET_LOGO_DIMS: [u32; 2] = [2028, 582];

    fn font_system() -> glyphon::FontSystem {
        crate::render::ui::text::build_font_system()
    }

    /// The `ImageSizes` map the renderer threads into the measure seam: the logo
    /// asset's natural reference size keyed by `SPLASH_LOGO_ASSET`.
    fn logo_image_sizes() -> ImageSizes {
        let mut sizes = ImageSizes::new();
        sizes.insert(
            SPLASH_LOGO_ASSET.to_string(),
            splash_logo_reference_size(ASSET_LOGO_DIMS),
        );
        sizes
    }

    #[test]
    fn descriptor_is_passthrough() {
        // The splash is non-interactive — the dispatch seam must stay passthrough.
        // (Built once to confirm the descriptor still constructs cleanly.)
        let _ = build_splash_descriptor("v0.1.0");
        assert_eq!(splash_capture_mode(), UiCaptureMode::Passthrough);
    }

    #[test]
    fn splash_tree_lays_out_panel_logo_and_text() {
        // The descriptor tree must produce the splash's visual layers: the outer
        // border backdrop + inner fill backdrop (quads), the logo (its own image
        // batch keyed to the logo asset), and the version text run.
        let desc = build_splash_descriptor("postretro v0.1.0");
        let mut ui = UiTree::from_descriptor(desc.tree(), &UiTheme::engine_default());
        let mut fs = font_system();
        let slots = std::collections::HashMap::new();
        let data = ui.build_draw_data([1280, 720], &mut fs, &logo_image_sizes(), &slots);

        // Outer border + inner fill containers are backdrop quads (white-texel
        // batch).
        assert!(
            data.quads.len() >= 2,
            "splash draws the outer border + inner fill backdrop quads, got {}",
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
