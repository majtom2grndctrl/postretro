// Splash content descriptor for the boot splash, authored in 1280x720
// logical-reference space and drawn through the UI pass via the retained
// descriptor tree (`UiTree`). Loads `content/base/ui/splash.json` once via a
// `LazyLock`, then each call clones the cached tree and substitutes the live
// `{version}` string; a minimal in-code tree is the fallback when the JSON is
// absent or malformed. This is the ONE named seam (`build_splash_descriptor`)
// built on `AnchoredTree`; script ingestion (G1) will replace the body while
// keeping the `SplashDescriptor` shape and call sites stable.
// See: context/lib/boot_sequence.md §1 (Splash state machine) · context/lib/ui.md

use std::path::PathBuf;
use std::sync::LazyLock;

use crate::input::UiCaptureMode;

use super::descriptor::{
    Align, AnchoredTree, ColorValue, ContainerWidget, ImageWidget, SpacingValue, TextWidget, Widget,
};
use super::layout::{Anchor, REFERENCE_HEIGHT, REFERENCE_WIDTH};

/// Linear-space sRGB(21, 27, 35) — the framed panel fill behind the logo. Kept a
/// touch lighter than the fullscreen background so the framed panel reads as a
/// distinct surface against the letterbox fill.
const PANEL_COLOR: [f32; 4] = [0.018, 0.026, 0.039, 1.0];

/// Logo width in logical-reference px — about half the 1280-wide canvas. Height
/// is derived from the decoded source aspect (see `splash_logo_reference_size`).
const LOGO_REFERENCE_WIDTH: f32 = 600.0;

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

/// Templated placeholder the splash JSON authors in the version `text` node's
/// content. `build_splash_descriptor` finds that node by this exact sentinel
/// content and substitutes the per-frame `version_line` into it. The sentinel is
/// the join point (the wire model has no node `id`), and it stays intact in the
/// cached tree so each frame's clone substitutes against a fresh sentinel.
const VERSION_SENTINEL: &str = "{version}";

/// Engine-shipped splash descriptor path, relative to the working directory — the
/// same `content/base/...` convention the splash PNG and keyboard JSON use. The
/// engine runs from the workspace root, so `content/base/ui/splash.json` resolves
/// directly; absent, the loader degrades to `fallback_splash_tree`. The splash
/// ships with the engine, so it loads from `base` regardless of which mod is
/// active.
///
/// In test builds the path is additionally anchored off `CARGO_MANIFEST_DIR`,
/// because `cargo test` runs from the crate dir where the cwd-relative path does
/// not resolve — without it the `SPLASH_TREE` LazyLock would load the fallback
/// under test and the layout assertions would fail. Production never embeds this
/// build-machine path; the sibling asset loaders gate their manifest anchor the
/// same way.
#[cfg(not(test))]
fn splash_asset_path() -> PathBuf {
    PathBuf::from("content/base/ui/splash.json")
}

#[cfg(test)]
fn splash_asset_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join("content/base/ui/splash.json")
}

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

/// JSON-loaded splash content: carries the descriptor `AnchoredTree` the renderer
/// lays out through `UiTree`. The fullscreen letterbox fill is drawn separately by
/// the caller (`background_element`), and the capture/passthrough flag is the free
/// `splash_capture_mode` — neither lives on this struct.
///
/// This is the seam: built on `AnchoredTree`; script ingestion (G1) will replace
/// the body while keeping the `SplashDescriptor` shape and call sites stable.
/// The renderer does not store one of these: it tracks splash-installed via the
/// logo-size field and builds a fresh `SplashDescriptor` each frame from the
/// `LazyLock`-cached tree clone.
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
/// renderer supplies `splash_logo_reference_size` keyed by `SPLASH_LOGO_ASSET`),
/// so the logo sizes from its content like a text node sizes from shaped glyphs.
pub(crate) fn build_splash_descriptor(version_line: &str) -> SplashDescriptor {
    // Clone the once-loaded cached tree (logo image + version `text` carrying the
    // `{version}` sentinel) and substitute the per-frame version into it. The disk
    // read + parse happen exactly once (in `SPLASH_TREE`'s initializer); per-call
    // work is a clone + one string swap, so the per-frame `record_splash_ui` site
    // never touches disk.
    let mut tree = SPLASH_TREE.clone();
    substitute_version(&mut tree.root, version_line);
    SplashDescriptor { tree }
}

/// The splash descriptor tree, loaded + parsed exactly once. Holds the JSON-
/// authored tree (logo image + version text with the `{version}` sentinel intact)
/// on success, or the minimal in-code fallback on a missing/malformed
/// `splash.json`. Each `build_splash_descriptor` call clones this and substitutes
/// the live version, so the cached sentinel is never consumed.
static SPLASH_TREE: LazyLock<AnchoredTree> = LazyLock::new(load_splash_tree);

/// Force the splash tree's one-time load + parse now, rather than lazily on the
/// first `build_splash_descriptor` call (which lands inside the first splash
/// frame's render). The renderer calls this at splash install so the disk read +
/// deserialize happen at boot-time init, keeping the per-frame path off disk and
/// boot init deterministic — matching how HUD/pause/keyboard trees load eagerly.
pub(crate) fn force_splash_tree_init() {
    LazyLock::force(&SPLASH_TREE);
}

/// Load + deserialize the splash descriptor from `content/base/ui/splash.json`,
/// degrading to the minimal in-code fallback on a missing or malformed file. On
/// failure it `warn!`s once (this runs once, inside `SPLASH_TREE`'s initializer)
/// and returns the fallback so the boot path never panics — mirroring
/// `tree_asset::load_named_tree`'s graceful degradation and `[UI]` log tag.
fn load_splash_tree() -> AnchoredTree {
    let path = splash_asset_path();
    let bytes = match std::fs::read_to_string(&path) {
        Ok(bytes) => bytes,
        Err(err) => {
            log::warn!(
                "[UI] splash asset '{}' could not be read ({err}); using minimal fallback splash",
                path.display()
            );
            return fallback_splash_tree();
        }
    };
    match serde_json::from_str::<AnchoredTree>(&bytes) {
        Ok(tree) => tree,
        Err(err) => {
            log::warn!(
                "[UI] splash asset '{}' failed to deserialize ({err}); using minimal fallback splash",
                path.display()
            );
            fallback_splash_tree()
        }
    }
}

/// The one sanctioned in-code splash tree: a minimal center-anchored panel with
/// the logo `image` above the version `text` (sentinel content), used ONLY when
/// `splash.json` is absent or broken. It keeps the same logo asset + `{version}`
/// sentinel so substitution and the logo measure seam behave identically to the
/// JSON path — the splash still shows, just without the framed border treatment.
fn fallback_splash_tree() -> AnchoredTree {
    let logo = Widget::Image(ImageWidget {
        asset: SPLASH_LOGO_ASSET.to_string(),
        id: None,
        focus_neighbors: Default::default(),
        // The splash logo is presentational chrome — decorative, no a11y name.
        label: None,
        decorative: true,
        visible_when: None,
        role: None,
    });
    let version = Widget::Text(TextWidget {
        content: VERSION_SENTINEL.to_string(),
        font_size: TEXT_LOGICAL_FONT_SIZE,
        color: ColorValue::Literal(TEXT_COLOR),
        font: None,
        bind: None,
        style_ranges: None,
        id: None,
        focus_neighbors: Default::default(),
        visible_when: None,
        role: None,
    });
    let panel = Widget::VStack(ContainerWidget {
        gap: SpacingValue::Literal(LOGO_TEXT_GAP),
        padding: SpacingValue::Literal(PANEL_CONTENT_PADDING),
        align: Align::Center,
        fill: Some(ColorValue::Literal(PANEL_COLOR)),
        border: None,
        id: None,
        focus_neighbors: Default::default(),
        focus: None,
        restore_on_return: false,
        local_state: None,
        visible_when: None,
        role: None,
        children: vec![logo, version],
    });
    AnchoredTree::passthrough(Anchor::Center, [0.0, 0.0], panel)
}

/// Walk the descriptor tree and replace the `{version}` sentinel in the version
/// `text` node's content with `version_line`. The node is located by its sentinel
/// content (the wire model has no node `id`), so any `text` whose content is the
/// sentinel is templated. Recurses through container children.
fn substitute_version(widget: &mut Widget, version_line: &str) {
    match widget {
        Widget::Text(text) => {
            if text.content == VERSION_SENTINEL {
                text.content = version_line.to_string();
            }
        }
        Widget::VStack(container) | Widget::HStack(container) => {
            for child in &mut container.children {
                substitute_version(child, version_line);
            }
        }
        Widget::Image(_)
        | Widget::Panel(_)
        | Widget::Grid(_)
        | Widget::Spacer(_)
        | Widget::Button(_)
        | Widget::Slider(_)
        | Widget::Bar(_)
        | Widget::Announce(_) => {}
    }
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

    /// The authored `content/base/ui/splash.json` carries the structural facts the
    /// renderer and substitution path depend on: the logo `image` node's asset key,
    /// the version `text` node's `{version}` sentinel, and the tree's center-anchor
    /// + passthrough envelope. Deserialized directly through the wire path (not via
    /// the builder) so the assertions pin the authored layout independently of the
    /// loader — not a tautology against a tree the builder itself loaded from this
    /// same file. Path anchored to the repo root off `CARGO_MANIFEST_DIR` (mirrors
    /// the `keyboard_asset` test precedent), NOT runtime cwd, so it passes under
    /// `cargo test`.
    #[test]
    fn splash_json_carries_logo_asset_version_sentinel_and_envelope() {
        use crate::render::ui::descriptor::CaptureMode;

        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .join("content/base/ui/splash.json");
        let bytes = std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("fixture '{}' exists: {e}", path.display()));
        let tree: AnchoredTree = serde_json::from_str(&bytes)
            .unwrap_or_else(|e| panic!("fixture '{}' deserializes: {e}", path.display()));

        // Envelope: the splash centers in the canvas and passes input through (it is
        // non-interactive), so it never sits as a capturing modal.
        assert_eq!(
            tree.anchor,
            Anchor::Center,
            "the splash anchors to the canvas center",
        );
        assert_eq!(
            tree.capture_mode,
            CaptureMode::Passthrough,
            "the splash passes input through (non-interactive)",
        );

        // The logo image and version text are the two content leaves. Walk the tree
        // so the assertions hold regardless of how many container layers the authored
        // layout nests them under.
        assert!(
            tree_has_image_asset(&tree.root, SPLASH_LOGO_ASSET),
            "the logo image node carries the {SPLASH_LOGO_ASSET} asset key",
        );
        assert!(
            tree_has_exact_text(&tree.root, VERSION_SENTINEL),
            "the version text node carries the {VERSION_SENTINEL} sentinel for substitution",
        );
    }

    /// Recursively check the tree for an `image` node with the given asset key.
    fn tree_has_image_asset(widget: &Widget, asset: &str) -> bool {
        match widget {
            Widget::Image(img) => img.asset == asset,
            Widget::VStack(c) | Widget::HStack(c) => {
                c.children.iter().any(|w| tree_has_image_asset(w, asset))
            }
            _ => false,
        }
    }

    /// Recursively check the tree for a `text` node whose content is exactly `text`.
    fn tree_has_exact_text(widget: &Widget, text: &str) -> bool {
        match widget {
            Widget::Text(t) => t.content == text,
            Widget::VStack(c) | Widget::HStack(c) => {
                c.children.iter().any(|w| tree_has_exact_text(w, text))
            }
            _ => false,
        }
    }

    #[test]
    fn fallback_splash_tree_is_non_empty_and_substitutes_version() {
        // Degradation path (mirrors the keyboard's degradation test): when
        // `splash.json` is absent/malformed the loader returns the in-code
        // fallback. The fallback must be a non-empty tree (logo + version text)
        // and must not panic; the substitution path replaces the sentinel.
        let mut tree = fallback_splash_tree();
        // The fallback is a panel flowing the logo image above the version text.
        let Widget::VStack(panel) = &tree.root else {
            panic!("fallback root is a vstack panel");
        };
        assert!(
            !panel.children.is_empty(),
            "fallback splash tree is non-empty",
        );
        assert!(
            panel
                .children
                .iter()
                .any(|w| matches!(w, Widget::Image(img) if img.asset == SPLASH_LOGO_ASSET)),
            "fallback carries the logo image",
        );
        // The fallback's version text still carries the sentinel, so substitution
        // works identically to the JSON path.
        substitute_version(&mut tree.root, "postretro v9.9.9");
        let Widget::VStack(panel) = &tree.root else {
            panic!("fallback root is a vstack panel");
        };
        let versioned = panel
            .children
            .iter()
            .any(|w| matches!(w, Widget::Text(t) if t.content == "postretro v9.9.9"));
        assert!(versioned, "the sentinel is replaced with the version line");
    }

    #[test]
    fn malformed_splash_json_degrades_without_panic() {
        // A malformed payload through the wire path degrades to the fallback
        // rather than panicking — the boot path must never panic on a broken
        // engine-shipped asset.
        let parsed = serde_json::from_str::<AnchoredTree>("{ not valid json ");
        assert!(parsed.is_err(), "the malformed payload fails to parse");
        // The loader swallows that error into the fallback; assert the fallback is
        // usable directly (the loader's disk read is exercised at runtime).
        let tree = fallback_splash_tree();
        assert!(
            matches!(&tree.root, Widget::VStack(p) if !p.children.is_empty()),
            "degradation yields a non-empty tree",
        );
    }

    #[test]
    fn builder_substitutes_version_into_sentinel_node() {
        // The migrated builder loads the cached tree and substitutes the live
        // version into the `{version}` sentinel node.
        let desc = build_splash_descriptor("postretro v1.2.3");
        let found = find_version_text(desc.tree(), "postretro v1.2.3");
        assert!(
            found,
            "the version line replaces the sentinel in the built tree"
        );
    }

    /// Recursively check the tree for a `text` node with the given content.
    fn find_version_text(tree: &AnchoredTree, expected: &str) -> bool {
        fn walk(widget: &Widget, expected: &str) -> bool {
            match widget {
                Widget::Text(t) => t.content == expected,
                Widget::VStack(c) | Widget::HStack(c) => {
                    c.children.iter().any(|w| walk(w, expected))
                }
                _ => false,
            }
        }
        walk(&tree.root, expected)
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
