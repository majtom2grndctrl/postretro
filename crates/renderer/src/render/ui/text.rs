// glyphon shaped-text half of the UI pass: the embedded font, the glyph
// atlas/renderer, and the shape→prepare→render→trim cycle. glyphon ships its OWN
// pipeline and atlas — none of this routes through the quad pipeline in `mod.rs`;
// prepared text spans record INTO the same render pass at their mixed
// paint-stream positions, with depth testing against the private UI target.
// See: context/lib/ui.md

use std::{collections::HashSet, ops::Range};

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
    /// One glyphon draw recorder per text span in the mixed paint stream. Each
    /// owns a distinct vertex buffer, so every span can be prepared before the
    /// render pass and then recorded at its actual painter-order position.
    text_renderers: Vec<TextRenderer>,
    depth_stencil: wgpu::DepthStencilState,
    /// Debug-only guard: counts coordinated prepare phases since the last
    /// submitted UI command buffer. A second phase would overwrite each retained
    /// span buffer before the first composition executes. Release builds carry
    /// no guard cost (the field and its uses are `cfg(debug_assertions)`).
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
            Some(depth_stencil.clone()),
        );

        Self {
            swash_cache,
            glyph_cache,
            viewport,
            text_atlas,
            text_renderers: vec![text_renderer],
            depth_stencil,
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

    /// Run glyphon's prepare phase for the contiguous text spans in one
    /// composition. Every span has its own retained `TextRenderer` vertex
    /// buffer, so preparing a later span cannot overwrite an earlier one.
    pub fn prepare_text_batches(
        &mut self,
        font_system: &mut FontSystem,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        input: TextPrepareInput<'_>,
        batches: &[Range<usize>],
    ) -> Vec<bool> {
        let TextPrepareInput {
            viewport,
            texts,
            buffers,
            depths,
        } = input;

        if texts.is_empty() || batches.is_empty() {
            return Vec::new();
        }

        debug_assert_eq!(
            texts.len(),
            depths.len(),
            "each shaped UI text run needs one painter depth",
        );

        // Once-per-submit guard: one coordinated preparation phase may fill
        // several disjoint span buffers, but a second composition encode before
        // submission would overwrite those same buffers.
        #[cfg(debug_assertions)]
        {
            self.prepare_count += 1;
            debug_assert!(
                self.prepare_count <= 1,
                "glyphon prepare reached {} times before submit — a second \
                 composition would overwrite the retained text-span buffers \
                 (one prepare phase per submitted UI composition)",
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

        while self.text_renderers.len() < batches.len() {
            self.text_renderers.push(TextRenderer::new(
                &mut self.text_atlas,
                device,
                wgpu::MultisampleState::default(),
                Some(self.depth_stencil.clone()),
            ));
        }

        batches
            .iter()
            .enumerate()
            .map(|(batch_index, range)| {
                debug_assert!(range.start <= range.end && range.end <= texts.len());
                let areas = texts[range.clone()]
                    .iter()
                    .zip(&buffers[range.clone()])
                    .map(|(t, buffer)| TextArea {
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
                        default_color: GlyphColor::rgba(
                            t.color[0], t.color[1], t.color[2], t.color[3],
                        ),
                        custom_glyphs: &[],
                    });

                match self.text_renderers[batch_index].prepare_with_depth(
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
            })
            .collect()
    }

    /// Record one prepared text span into an already-open render pass at its
    /// retained paint-stream position. A failed draw is logged, not propagated —
    /// `render` only fails if the atlas grew after prepare, so a panic here would
    /// needlessly crash the frame.
    pub fn render_batch<'pass>(
        &'pass self,
        batch_index: usize,
        pass: &mut wgpu::RenderPass<'pass>,
    ) {
        let Some(renderer) = self.text_renderers.get(batch_index) else {
            return;
        };
        if let Err(e) = renderer.render(&self.text_atlas, &self.viewport, pass) {
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
