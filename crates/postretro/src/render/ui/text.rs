// glyphon shaped-text half of the UI pass: the embedded font, the glyph
// atlas/renderer, and the shape→prepare→render→trim cycle. glyphon ships its OWN
// pipeline and atlas — none of this routes through the quad pipeline in `mod.rs`;
// the text draw records INTO the same render pass, after the quads.
// See: context/plans/in-progress/M13--descriptor-tree-layout

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
const UI_FONT_FAMILY: &str = "Inter";

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
}

impl UiText {
    /// Convenience constructor for a single device-positioned line.
    pub fn new(
        content: impl Into<String>,
        position: [f32; 2],
        font_size: f32,
        color: [u8; 4],
    ) -> Self {
        Self {
            content: content.into(),
            position,
            font_size,
            color,
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
        }
    }

    /// Shape each `UiText` into a glyphon `Buffer`, selecting the embedded Inter
    /// family at the line's device-pixel font size. Returns the owned buffers so
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
                &Attrs::new().family(Family::Name(UI_FONT_FAMILY)),
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

/// Build a `FontSystem` with the embedded Inter face registered. Pure CPU — no
/// GPU device needed, so the layout-measure path can be exercised headless. The
/// embedded slice is compile-time data (`load_font_data` takes ownership of the
/// bytes) with no runtime file I/O.
pub(crate) fn build_font_system() -> FontSystem {
    let mut font_system = FontSystem::new();
    font_system.db_mut().load_font_data(UI_FONT_TTF.to_vec());
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
/// GPU state: shaping is pure cosmic-text and runs without a device.
pub(crate) fn measure_run(
    font_system: &mut FontSystem,
    content: &str,
    font_size: f32,
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
        &Attrs::new().family(Family::Name(UI_FONT_FAMILY)),
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
        );
        assert_eq!(t.font_size, 72.0);
        assert_eq!(t.position, [40.0, 600.0]);
        assert_eq!(t.color, [220, 230, 240, 255]);
        // Line height tracks font size by the single-line factor.
        let line_height = t.font_size * LINE_HEIGHT_FACTOR;
        assert_eq!(line_height, 90.0);
    }
}
