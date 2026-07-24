// glyphon shaped-text half of the UI pass: the embedded font, the glyph
// atlas/renderer, and the shape→prepare→render→trim cycle. glyphon ships its OWN
// pipeline and atlas — none of this routes through the quad pipeline in `mod.rs`;
// the text draw records INTO the same render pass, after the quads, with depth
// testing against the UI pass's private depth target.
// See: context/lib/ui.md

use std::collections::HashSet;

use glyphon::{
    Attrs, Buffer as TextBuffer, Cache as GlyphCache, Color as GlyphColor, Family, Metrics,
    Resolution, Shaping, SwashCache, TextArea, TextAtlas, TextBounds, TextRenderer, Viewport,
};

use postretro_ui::UiText;
use postretro_ui::text::{FontSystem, LINE_HEIGHT_FACTOR};

/// glyphon shaped-text state for the UI pass: glyph raster cache and glyphon's
/// own GPU atlas/renderer. Owned by `UiPass`, which drives it from `encode`.
/// The CPU `FontSystem` is session-owned and threaded in explicitly. All wgpu
/// here is glyphon's own — the quad pipeline in `mod.rs` never touches it, but
/// both record into one render pass.
pub(crate) struct UiTextRenderer {
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
    /// submitted UI command buffer. The shared vertex buffer `prepare` fills is
    /// overwritten at offset 0, so a SECOND `prepare` before submit would clobber
    /// the first composition's glyphs. A `debug_assert!` in `prepare_text` fires
    /// if this exceeds one. Release builds carry no guard cost (the field and its
    /// uses are `cfg(debug_assertions)`).
    #[cfg(debug_assertions)]
    prepare_count: u32,
}

pub(crate) struct TextPrepareInput<'a> {
    pub(crate) viewport: [u32; 2],
    pub(crate) texts: &'a [UiText],
    pub(crate) buffers: &'a [TextBuffer],
    pub(crate) depths: &'a [f32],
}

impl UiTextRenderer {
    pub fn new(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        color_format: wgpu::TextureFormat,
        depth_stencil: wgpu::DepthStencilState,
    ) -> Self {
        // Build glyphon's own GPU/cache state here so `TextAtlas` construction
        // happens in `Renderer::new` (not on the first shaped frame). We do NOT
        // pre-rasterize glyphs — the first-glyph rasterization lands on the first
        // `prepare` (first shaped frame).
        let swash_cache = SwashCache::new();
        let glyph_cache = GlyphCache::new(device);
        let viewport = Viewport::new(device, &glyph_cache);

        // Text draws into the UI target alongside its matching quad pipelines.
        let mut text_atlas = TextAtlas::new(device, queue, &glyph_cache, color_format);
        let text_renderer = TextRenderer::new(
            &mut text_atlas,
            device,
            wgpu::MultisampleState::default(),
            Some(depth_stencil),
        );

        Self {
            swash_cache,
            glyph_cache,
            viewport,
            text_atlas,
            text_renderer,
            #[cfg(debug_assertions)]
            prepare_count: 0,
        }
    }

    /// Reset the once-per-submit `prepare` guard. Called after the command buffer
    /// containing the UI encode is submitted. No-op in release.
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
    pub fn shape_text(
        &mut self,
        font_system: &mut FontSystem,
        texts: &[UiText],
        viewport: [u32; 2],
    ) -> Vec<TextBuffer> {
        let mut buffers = Vec::with_capacity(texts.len());
        for (i, t) in texts.iter().enumerate() {
            let metrics = Metrics::new(t.font_size, t.font_size * LINE_HEIGHT_FACTOR);
            let mut buffer = TextBuffer::new(font_system, metrics);
            // Bound the layout box to the backbuffer: glyphon needs a finite
            // layout size to resolve the run (an unbounded box has nothing to lay
            // glyphs against).
            buffer.set_size(
                font_system,
                Some(viewport[0] as f32),
                Some(viewport[1] as f32),
            );
            buffer.set_text(
                font_system,
                &t.content,
                &Attrs::new().family(Family::Name(&t.family)).metadata(i),
                Shaping::Advanced,
                None,
            );
            buffer.shape_until_scroll(font_system, false);
            buffers.push(buffer);
        }
        buffers
    }

    /// Register a font face at runtime from owned TTF/OTF bytes (the net-new
    /// runtime counterpart to `build_font_system`'s compile-time `include_bytes!`
    /// faces). Hands the bytes to cosmic-text's font database — `load_font_data`
    /// takes ownership, the same call the embedded faces use — so a subsequent
    /// `Family::Name(family)` shape resolves to this face. `family` is the family
    /// name the asset declares in its TTF `name` table; it is logged for diagnosis
    /// but the database keys faces by their own embedded name table, so the
    /// caller's declared family must match what the file actually contains for a
    /// `font` token to resolve to it. Returns `false` if the bytes register no
    /// face under `family` (a malformed/empty file or a family-name mismatch), so
    /// the caller can surface a load-time diagnostic and skip rather than leave a
    /// `font` token silently resolving to a system fallback.
    pub fn register_font(
        &mut self,
        font_system: &mut FontSystem,
        family: &str,
        ttf_bytes: Vec<u8>,
    ) -> bool {
        let before = font_face_ids_for_family(font_system, family);
        font_system.db_mut().load_font_data(ttf_bytes);
        font_family_gained_face(font_system, family, &before)
    }

    /// Run glyphon's `prepare` (CPU layout + atlas upload) for the shaped lines.
    /// Sets the `Viewport` resolution from the device backbuffer size first.
    /// Returns `true` if any text was prepared (so `encode` knows whether to
    /// record the text draw). First-glyph rasterization lands here, on the first
    /// shaped frame.
    pub fn prepare_text(
        &mut self,
        font_system: &mut FontSystem,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        input: TextPrepareInput<'_>,
    ) -> bool {
        let TextPrepareInput {
            viewport,
            texts,
            buffers,
            depths,
        } = input;

        if texts.is_empty() {
            return false;
        }

        debug_assert_eq!(
            texts.len(),
            depths.len(),
            "each shaped UI text run needs one painter depth",
        );

        // Once-per-submit guard: this is placed AFTER the empty-text
        // early-return so empty-text frames never count. The shared vertex buffer
        // `prepare` fills is overwritten at offset 0, so a SECOND `prepare`
        // before submit would clobber the first composition's glyphs. The guard
        // resets after submit, so two `UiPass::encode` calls recorded into one
        // command buffer are caught here; release builds carry no cost.
        #[cfg(debug_assertions)]
        {
            self.prepare_count += 1;
            debug_assert!(
                self.prepare_count <= 1,
                "glyphon prepare reached {} times before submit — the shared \
                 vertex buffer is overwritten at offset 0, so a second prepare \
                 clobbers earlier glyphs (one prepare per submitted UI composition)",
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

        match self.text_renderer.prepare_with_depth(
            device,
            queue,
            font_system,
            &mut self.text_atlas,
            &self.viewport,
            areas,
            &mut self.swash_cache,
            |metadata| depths.get(metadata).copied().unwrap_or(0.0),
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

fn font_face_ids_for_family(font_system: &FontSystem, family: &str) -> HashSet<String> {
    font_system
        .db()
        .faces()
        .filter(|face| face.families.iter().any(|(name, _)| name == family))
        .map(|face| face.id.to_string())
        .collect()
}

fn font_family_gained_face(
    font_system: &FontSystem,
    family: &str,
    before: &HashSet<String>,
) -> bool {
    font_face_ids_for_family(font_system, family)
        .difference(before)
        .next()
        .is_some()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn font_delta_validation_rejects_malformed_bytes_when_family_already_exists() {
        let mut font_system = postretro_ui::text::build_font_system();
        let family = postretro_ui::text::UI_FONT_FAMILY;
        let before = font_face_ids_for_family(&font_system, family);
        assert!(!before.is_empty(), "engine family should be preloaded");

        font_system.db_mut().load_font_data(b"not a font".to_vec());

        assert!(!font_family_gained_face(&font_system, family, &before));
    }
}
