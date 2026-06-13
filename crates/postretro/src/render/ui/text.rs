// glyphon shaped-text half of the UI pass: the embedded font, the glyph
// atlas/renderer, and the shape→prepare→render→trim cycle. glyphon ships its OWN
// pipeline and atlas — none of this routes through the quad pipeline in `mod.rs`;
// the text draw records INTO the same render pass, after the quads.
// See: context/lib/ui.md

use glyphon::{
    Attrs, Buffer as TextBuffer, Cache as GlyphCache, Color as GlyphColor, Family, FontSystem,
    Metrics, Resolution, Shaping, SwashCache, TextArea, TextAtlas, TextBounds, TextRenderer,
    Viewport,
};

/// Engine default UI typeface: Inter (SIL Open Font License 1.1). Embedded at
/// compile time so the engine has no main-thread runtime font file I/O — the
/// bytes are registered once into glyphon's `FontSystem` in `UiTextRenderer::new`.
/// The license travels alongside the asset at `content/base/fonts/Inter-OFL.txt`.
const UI_FONT_TTF: &[u8] = include_bytes!("../../../../../content/base/fonts/Inter-Regular.ttf");

/// Font family name inside `UI_FONT_TTF` (the TTF `name` table family record).
/// `TextArea`s select it by family so glyphon resolves to the embedded face
/// rather than a system fallback.
pub(crate) const UI_FONT_FAMILY: &str = "Inter";

/// Engine default UI monospace typeface: JetBrains Mono (SIL Open Font License
/// 1.1). Embedded at compile time alongside Inter and registered into the same
/// `FontSystem` in `UiTextRenderer::new`, so the `mono` theme token resolves to
/// the embedded face with no runtime font file I/O. The license travels with the
/// asset at `content/base/fonts/JetBrainsMono-OFL.txt`.
const UI_MONO_FONT_TTF: &[u8] =
    include_bytes!("../../../../../content/base/fonts/JetBrainsMono-Regular.ttf");

/// Font family name inside `UI_MONO_FONT_TTF` (the TTF `name` table family
/// record). Must match the `mono` font token in `theme::UiTheme::engine_default`
/// exactly, or token resolution selects a family glyphon never registered.
/// Referenced by the family-registration/measure tests and the theme contract;
/// the production `mono` family string lives in `engine_default`, so this is
/// test-only on a release build.
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) const UI_MONO_FONT_FAMILY: &str = "JetBrains Mono";

/// glyphon shapes against a `Metrics { font_size, line_height }`. UI text here is
/// single-line, so line height tracks the font size with a small factor for the
/// ascent/descent the face needs to render uncropped.
const LINE_HEIGHT_FACTOR: f32 = 1.25;

/// One shaped text line for glyphon to lay out and draw. Positions and font size
/// arrive already in **device pixels** (device-scaled by the caller, not in
/// logical-reference units), so glyphon and the quad pipeline share one
/// coordinate space and text tracks resolution the same way panels do. The
/// position is NOT integer-snapped — glyphon keeps sub-pixel AA.
#[derive(Debug, Clone)]
pub(crate) struct UiText {
    /// The string to shape and render.
    pub content: String,
    /// Top-left baseline-box position in device pixels (`[left, top]`). Not
    /// snapped — glyphon positions glyphs with sub-pixel precision.
    pub position: [f32; 2],
    /// Font size in device pixels (already device-scaled by the caller).
    pub font_size: f32,
    /// Glyph color, linear-ish sRGB 0..=255 per channel + alpha. glyphon's
    /// `TextAtlas` is built with the sRGB surface format so coverage blends in
    /// the surface color space (see `UiTextRenderer::new`).
    pub color: [u8; 4],
    /// Registered font family name to shape this line with. Selected per line in
    /// `shape_text` via `Family::Name`, so it must match a family registered in
    /// `build_font_system` (e.g. `UI_FONT_FAMILY`/`UI_MONO_FONT_FAMILY`); an
    /// unregistered name falls back to a system face.
    pub family: String,
}

impl UiText {
    /// Convenience constructor for a single device-positioned line.
    pub fn new(
        content: impl Into<String>,
        position: [f32; 2],
        font_size: f32,
        color: [u8; 4],
        family: impl Into<String>,
    ) -> Self {
        Self {
            content: content.into(),
            position,
            font_size,
            color,
            family: family.into(),
        }
    }
}

/// glyphon shaped-text state for the UI pass: CPU font database/shaper, glyph
/// raster cache, and glyphon's own GPU atlas/renderer. Owned by `UiPass`, which
/// drives it from `encode`. All wgpu here is glyphon's own — the quad pipeline
/// in `mod.rs` never touches it, but both record into one render pass.
pub(crate) struct UiTextRenderer {
    /// CPU font database + shaper. The embedded Inter face is registered into it
    /// once in `new`. `&mut` is needed for shaping, hence stored owned.
    font_system: FontSystem,
    /// Per-glyph rasterization cache (CPU). First-glyph rasterization happens on
    /// the first shaped frame via `prepare`, not pre-warmed here.
    swash_cache: SwashCache,
    /// glyphon's shared GPU bind-group/pipeline cache; backs `Viewport`/`Atlas`.
    /// Held to keep the cache alive for the `Viewport`/`TextAtlas` built from it.
    #[allow(dead_code)]
    glyph_cache: GlyphCache,
    /// Device-resolution uniform glyphon maps glyph positions against. Set from
    /// the backbuffer size each frame in `prepare`.
    viewport: Viewport,
    /// glyphon's glyph atlas, built with the sRGB surface format so coverage
    /// blends correctly against the sRGB swapchain (see `new`).
    text_atlas: TextAtlas,
    /// glyphon's text pipeline/draw recorder.
    text_renderer: TextRenderer,
    /// Debug-only guard: counts glyphon `prepare` invocations since the last
    /// `reset_prepare_guard` (called at `UiPass::encode` entry). The shared
    /// vertex buffer `prepare` fills is overwritten at offset 0, so a SECOND
    /// `prepare` within one encoded composition would clobber the first layer's
    /// glyphs — the invariant is one `prepare` per composed frame. A
    /// `debug_assert!` in `prepare_text` fires if this exceeds one. Release builds
    /// carry no guard cost (the field and its uses are `cfg(debug_assertions)`).
    #[cfg(debug_assertions)]
    prepare_count: u32,
}

impl UiTextRenderer {
    pub fn new(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        surface_format: wgpu::TextureFormat,
    ) -> Self {
        // Build glyphon's own state here so `FontSystem`/`TextAtlas` construction
        // happens in `Renderer::new` (not on the first shaped frame). We do NOT
        // pre-rasterize glyphs — the first-glyph rasterization lands on the first
        // `prepare` (first shaped frame), so frame 1 of the boot splash does not
        // absorb font-system construction.
        let font_system = build_font_system();

        let swash_cache = SwashCache::new();
        let glyph_cache = GlyphCache::new(device);
        let viewport = Viewport::new(device, &glyph_cache);

        // Color space: build the atlas with the sRGB *surface* format. glyphon's
        // default `ColorMode::Accurate` then stores colored glyphs in an sRGB
        // atlas and blends coverage in the surface color space, keeping glyph
        // coverage physically correct against the sRGB swapchain — edges neither
        // over- nor under-darkened.
        let mut text_atlas = TextAtlas::new(device, queue, &glyph_cache, surface_format);
        let text_renderer = TextRenderer::new(
            &mut text_atlas,
            device,
            wgpu::MultisampleState::default(),
            None,
        );

        Self {
            font_system,
            swash_cache,
            glyph_cache,
            viewport,
            text_atlas,
            text_renderer,
            #[cfg(debug_assertions)]
            prepare_count: 0,
        }
    }

    /// Reset the once-per-composition `prepare` guard. Called at `UiPass::encode`
    /// entry — the single per-frame site both splash and gameplay funnel through —
    /// so each encoded composition starts the count fresh. No-op in release.
    pub fn reset_prepare_guard(&mut self) {
        #[cfg(debug_assertions)]
        {
            self.prepare_count = 0;
        }
    }

    /// Shape each `UiText` into a glyphon `Buffer`, selecting the line's own
    /// `family` at its device-pixel font size. Returns the owned buffers so
    /// they outlive `prepare`/`render`. Empty input yields an empty `Vec` and no
    /// shaping work.
    pub fn shape_text(&mut self, texts: &[UiText], viewport: [u32; 2]) -> Vec<TextBuffer> {
        let mut buffers = Vec::with_capacity(texts.len());
        for t in texts {
            let metrics = Metrics::new(t.font_size, t.font_size * LINE_HEIGHT_FACTOR);
            let mut buffer = TextBuffer::new(&mut self.font_system, metrics);
            // Bound the layout box to the backbuffer: glyphon needs a finite
            // layout size to resolve the run (an unbounded box has nothing to lay
            // glyphs against).
            buffer.set_size(
                &mut self.font_system,
                Some(viewport[0] as f32),
                Some(viewport[1] as f32),
            );
            buffer.set_text(
                &mut self.font_system,
                &t.content,
                &Attrs::new().family(Family::Name(&t.family)),
                Shaping::Advanced,
                None,
            );
            buffer.shape_until_scroll(&mut self.font_system, false);
            buffers.push(buffer);
        }
        buffers
    }

    /// Borrow the CPU `FontSystem` for measurement — the GPU-free bridge into the
    /// taffy measure closure. `UiPass::layout_tree` threads this into
    /// `tree::UiTree::build_draw_data`, which hands it to taffy's
    /// `compute_layout_with_measure` so text nodes shape through cosmic-text
    /// CPU-side and size from real shaped metrics. Only the `FontSystem` crosses —
    /// glyphon's GPU atlas/renderer never leave this type, keeping the renderer
    /// the sole GPU owner while text measurement stays a pure-CPU seam.
    pub fn font_system_mut(&mut self) -> &mut FontSystem {
        &mut self.font_system
    }

    /// Run glyphon's `prepare` (CPU layout + atlas upload) for the shaped lines.
    /// Sets the `Viewport` resolution from the device backbuffer size first.
    /// Returns `true` if any text was prepared (so `encode` knows whether to
    /// record the text draw). First-glyph rasterization lands here, on the first
    /// shaped frame.
    pub fn prepare_text(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        viewport: [u32; 2],
        texts: &[UiText],
        buffers: &[TextBuffer],
    ) -> bool {
        if texts.is_empty() {
            return false;
        }

        // Once-per-composition guard: this is placed AFTER the empty-text
        // early-return so empty-text frames never count. The shared vertex buffer
        // `prepare` fills is overwritten at offset 0, so a SECOND `prepare` within
        // one encoded composition would clobber the first layer's glyphs. The
        // guard fires if more than one `prepare` is reached per `UiPass::encode`
        // (reset there); release builds carry no cost. The historical
        // per-encode-loop clobber resets the guard between encodes, so it is NOT
        // caught here — a separate test covers that.
        #[cfg(debug_assertions)]
        {
            self.prepare_count += 1;
            debug_assert!(
                self.prepare_count <= 1,
                "glyphon prepare reached {} times in one composition — the shared \
                 vertex buffer is overwritten at offset 0, so a second prepare \
                 clobbers earlier layers' glyphs (one prepare per composition)",
                self.prepare_count,
            );
        }

        self.viewport.update(
            queue,
            Resolution {
                width: viewport[0],
                height: viewport[1],
            },
        );

        let areas = texts.iter().zip(buffers).map(|(t, buffer)| TextArea {
            buffer,
            left: t.position[0],
            top: t.position[1],
            scale: 1.0,
            bounds: TextBounds {
                left: 0,
                top: 0,
                right: viewport[0] as i32,
                bottom: viewport[1] as i32,
            },
            default_color: GlyphColor::rgba(t.color[0], t.color[1], t.color[2], t.color[3]),
            custom_glyphs: &[],
        });

        match self.text_renderer.prepare(
            device,
            queue,
            &mut self.font_system,
            &mut self.text_atlas,
            &self.viewport,
            areas,
            &mut self.swash_cache,
        ) {
            Ok(()) => true,
            Err(e) => {
                log::warn!("UI text prepare failed: {e}");
                false
            }
        }
    }

    /// Record glyphon's text draw into an already-open render pass, after the
    /// quad draws. Only called when `prepare_text` returned `true` (text this
    /// frame). A failed draw is logged, not propagated — `render` only fails if
    /// the atlas grew past `prepare` (it didn't, we just prepared into it), so a
    /// panic here would needlessly crash the frame.
    pub fn render<'pass>(&'pass self, pass: &mut wgpu::RenderPass<'pass>) {
        if let Err(e) = self
            .text_renderer
            .render(&self.text_atlas, &self.viewport, pass)
        {
            log::warn!("UI text render failed: {e}");
        }
    }

    /// Reclaim atlas space for glyphs not used by the last `prepare`. glyphon's
    /// docs prescribe one `trim` per frame after rendering: shaping keeps every
    /// touched glyph resident in the atlas, so without a periodic trim the atlas
    /// grows monotonically as text content changes (e.g. a counting version line).
    pub fn trim(&mut self) {
        self.text_atlas.trim();
    }
}

/// Build a `FontSystem` with the embedded Inter (body) and JetBrains Mono (mono)
/// faces registered. Pure CPU — no GPU device needed, so the layout-measure path
/// can be exercised headless. Each embedded slice is compile-time data
/// (`load_font_data` takes ownership of the bytes) with no runtime file I/O;
/// cosmic-text's DB takes one `load_font_data` call per face.
pub(crate) fn build_font_system() -> FontSystem {
    let mut font_system = FontSystem::new();
    font_system.db_mut().load_font_data(UI_FONT_TTF.to_vec());
    font_system
        .db_mut()
        .load_font_data(UI_MONO_FONT_TTF.to_vec());
    font_system
}

/// Measure a single text run's intrinsic size from real shaped-glyph metrics, in
/// the SAME units as `font_size` (logical-reference px at layout time — the
/// caller passes the un-device-scaled size). Shapes `content` at `font_size`
/// through cosmic-text with no width constraint, then takes the widest laid-out
/// run for width and the summed line heights for height. This is the taffy
/// measure seam: the layout tree sizes text nodes from this, not from a
/// glyph-count estimate. Empty content measures to a zero-width box one line
/// tall, so an empty label still reserves its line.
///
/// Takes `&mut FontSystem` (not `&mut UiTextRenderer`) so measurement carries no
/// GPU state: shaping is pure cosmic-text and runs without a device. Shapes with
/// the given `family` so a node measures against the same face it will draw with
/// (a monospace run sizes wider/narrower than the proportional body face).
pub(crate) fn measure_run(
    font_system: &mut FontSystem,
    content: &str,
    font_size: f32,
    family: &str,
) -> (f32, f32) {
    let line_height = font_size * LINE_HEIGHT_FACTOR;
    let metrics = Metrics::new(font_size, line_height);
    let mut buffer = TextBuffer::new(font_system, metrics);
    // No width bound: intrinsic measurement wants the run's natural width, not a
    // wrapped-to-viewport width. `None` width lets cosmic-text lay the whole run
    // on one line so `line_w` is the true shaped advance.
    buffer.set_size(font_system, None, None);
    buffer.set_text(
        font_system,
        content,
        &Attrs::new().family(Family::Name(family)),
        Shaping::Advanced,
        None,
    );
    buffer.shape_until_scroll(font_system, false);

    let mut width = 0.0_f32;
    let mut height = 0.0_f32;
    for run in buffer.layout_runs() {
        width = width.max(run.line_w);
        height += run.line_height;
    }
    // No runs (empty string) => zero width, one line tall so the node still
    // claims a line box rather than collapsing to nothing.
    if height == 0.0 {
        height = line_height;
    }
    (width, height)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embedded_font_bytes_are_present_and_a_truetype() {
        // The font is embedded via `include_bytes!`; a missing/empty asset must
        // fail the build-test, not just produce blank text at runtime. The
        // sfnt/TrueType magic is `0x00010000` (or `OTTO`/`true`/`ttcf`).
        assert!(
            UI_FONT_TTF.len() > 1024,
            "embedded TTF looks truncated ({} bytes)",
            UI_FONT_TTF.len(),
        );
        let magic = &UI_FONT_TTF[0..4];
        assert!(
            magic == [0x00, 0x01, 0x00, 0x00]
                || magic == *b"OTTO"
                || magic == *b"true"
                || magic == *b"ttcf",
            "embedded font is not a recognized sfnt/TrueType (magic {magic:?})",
        );
    }

    #[test]
    fn embedded_font_registers_and_resolves_family() {
        // CPU-only (no GPU): `FontSystem` is pure cosmic-text. Registering the
        // embedded bytes must make the `Inter` family queryable, so the
        // `Family::Name(UI_FONT_FAMILY)` selection in `shape_text` resolves to
        // the embedded face rather than a system fallback.
        let mut fs = FontSystem::new();
        fs.db_mut().load_font_data(UI_FONT_TTF.to_vec());
        let has_family = fs
            .db()
            .faces()
            .any(|face| face.families.iter().any(|(name, _)| name == UI_FONT_FAMILY));
        assert!(
            has_family,
            "embedded font did not register family {UI_FONT_FAMILY:?}",
        );
    }

    #[test]
    fn embedded_mono_font_bytes_are_present_and_a_truetype() {
        // Mirror of the Inter magic check for the mono face: a missing/empty
        // asset must fail the build-test, not just produce blank monospace text.
        assert!(
            UI_MONO_FONT_TTF.len() > 1024,
            "embedded mono TTF looks truncated ({} bytes)",
            UI_MONO_FONT_TTF.len(),
        );
        let magic = &UI_MONO_FONT_TTF[0..4];
        assert!(
            magic == [0x00, 0x01, 0x00, 0x00]
                || magic == *b"OTTO"
                || magic == *b"true"
                || magic == *b"ttcf",
            "embedded mono font is not a recognized sfnt/TrueType (magic {magic:?})",
        );
    }

    #[test]
    fn embedded_mono_font_registers_and_resolves_family() {
        // CPU-only: registering the embedded mono bytes must make the
        // `UI_MONO_FONT_FAMILY` family queryable so the `mono` theme token (which
        // names the identical string) resolves to the embedded face. If this
        // fails because the real family name differs, update both
        // `UI_MONO_FONT_FAMILY` here and the `mono` entry in `theme.rs`.
        let mut fs = FontSystem::new();
        fs.db_mut().load_font_data(UI_MONO_FONT_TTF.to_vec());
        let has_family = fs.db().faces().any(|face| {
            face.families
                .iter()
                .any(|(name, _)| name == UI_MONO_FONT_FAMILY)
        });
        assert!(
            has_family,
            "embedded mono font did not register family {UI_MONO_FONT_FAMILY:?}",
        );
    }

    #[test]
    fn build_font_system_registers_both_body_and_mono_families() {
        // `build_font_system` registers both faces, so a single FontSystem
        // resolves both the body and mono token families — the shaping seam both
        // `shape_text` and `measure_run` select against.
        let fs = build_font_system();
        let has_family = |target: &str| {
            fs.db()
                .faces()
                .any(|face| face.families.iter().any(|(name, _)| name == target))
        };
        assert!(
            has_family(UI_FONT_FAMILY),
            "body family {UI_FONT_FAMILY:?} not registered",
        );
        assert!(
            has_family(UI_MONO_FONT_FAMILY),
            "mono family {UI_MONO_FONT_FAMILY:?} not registered",
        );
    }

    #[test]
    fn mono_and_body_families_measure_to_different_widths() {
        // The same content shaped against the proportional body face and the
        // monospace face produces different advances — proof that `measure_run`
        // honors the family and that the embedded mono face is actually selected
        // (not silently falling back to the body face).
        let mut fs = build_font_system();
        let content = "iiiiWWWW mmmm";
        let font_size = 24.0_f32;
        let (body_w, _) = measure_run(&mut fs, content, font_size, UI_FONT_FAMILY);
        let (mono_w, _) = measure_run(&mut fs, content, font_size, UI_MONO_FONT_FAMILY);
        // Approximate comparison: assert the widths DIFFER beyond an epsilon,
        // rather than equal-within-epsilon (testing guide §3 — floats).
        const EPS: f32 = 1.0;
        assert!(
            (body_w - mono_w).abs() > EPS,
            "mono ({mono_w}) and body ({body_w}) widths should differ \
             beyond {EPS}px; mono may have fallen back to the body face",
        );
    }

    #[test]
    fn ui_text_carries_device_scaled_inputs() {
        // UiText carries device-pixel position + a device-scaled font size +
        // color, no logical-reference coords. Font size scales by the same
        // `device_scale` as panels, so text tracks resolution (e.g. a 24px
        // logical line at 3x is a 72px device line).
        let logical_size = 24.0_f32;
        let scale = 3.0_f32;
        let t = UiText::new(
            "v0.1.0",
            [40.0, 600.0],
            logical_size * scale,
            [220, 230, 240, 255],
            UI_FONT_FAMILY,
        );
        assert_eq!(t.family, UI_FONT_FAMILY);
        assert_eq!(t.font_size, 72.0);
        assert_eq!(t.position, [40.0, 600.0]);
        assert_eq!(t.color, [220, 230, 240, 255]);
        // Line height tracks font size by the single-line factor.
        let line_height = t.font_size * LINE_HEIGHT_FACTOR;
        assert_eq!(line_height, 90.0);
    }
}
