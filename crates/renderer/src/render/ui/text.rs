// glyphon shaped-text half of the UI pass: the embedded font, the glyph
// atlas, and one retained renderer per text span. Only a span whose inputs
// changed shapes and prepares; an unchanged span draws from its retained
// vertices. glyphon ships its OWN pipeline and atlas — none of this routes
// through the quad pipeline in `mod.rs`; prepared spans record INTO the same
// render pass at their mixed paint-stream positions, with depth testing against
// the private UI target.
// See: context/lib/ui.md §5

use std::collections::HashSet;

use glyphon::{
    Attrs, Buffer as TextBuffer, Cache as GlyphCache, Color as GlyphColor, Family, Metrics,
    PrepareError, Resolution, Shaping, SwashCache, TextArea, TextAtlas, TextBounds, TextRenderer,
    Viewport,
};

use postretro_ui::UiText;
use postretro_ui::text::{FontSystem, LINE_HEIGHT_FACTOR};

/// Encodes with text between atlas reclaims: one CPU-timing window, so a
/// settled window holds at most one reclaim.
pub(crate) const TEXT_RECLAIM_CADENCE: u32 = 120;

/// glyphon shaped-text state for the UI pass: glyph raster cache, glyphon's own
/// GPU atlas, and the retained span slots. Owned by `UiPass`, which drives it
/// from `encode`. The CPU `FontSystem` is session-owned and threaded in
/// explicitly. All wgpu here is glyphon's own — the quad pipeline in `mod.rs`
/// never touches it, but both record into one render pass.
pub(crate) struct UiTextRenderer {
    /// Per-glyph rasterization cache (CPU). First-glyph rasterization happens on
    /// the first shaped frame via `prepare`, not pre-warmed here.
    swash_cache: SwashCache,
    /// glyphon's shared GPU bind-group/pipeline cache; backs `Viewport`/`Atlas`.
    /// Held to keep the cache alive for the `Viewport`/`TextAtlas` built from it.
    #[allow(dead_code)]
    glyph_cache: GlyphCache,
    /// Device-resolution uniform glyphon maps glyph positions against. glyphon
    /// writes it only when the resolution changes.
    viewport: Viewport,
    /// Built by `TextAtlas::new`, which uses glyphon's default Accurate colour
    /// mode: glyph colours convert to
    /// linear in the shader, and the sRGB UI layer blends in linear space.
    text_atlas: TextAtlas,
    /// Retained span slots, indexed `[layer][span within layer]`. Each owns a
    /// distinct vertex buffer, so every span can be prepared before the render
    /// pass and then recorded at its painter-order position, and an unchanged
    /// span keeps its vertices across frames.
    slots: Vec<Vec<TextSlot>>,
    depth_stencil: wgpu::DepthStencilState,
    /// Bumped by every `register_font`: a new face can change how any span
    /// shapes, so every span's key moves.
    font_generation: u64,
    /// Encodes with text since the last trim.
    encodes_since_reclaim: u32,
    /// Debug-only guard: counts UI encodes since the last submitted UI command
    /// buffer. A second encode could prepare a span another encode drew from
    /// before either executes. Release builds carry no guard cost (the field and
    /// its uses are `cfg(debug_assertions)`).
    #[cfg(debug_assertions)]
    encode_count: u32,
}

/// One retained text span: its glyphon renderer, its shaped buffers, and the
/// key — everything `prepare` read — its vertices were prepared from.
struct TextSlot {
    renderer: TextRenderer,
    buffers: Vec<TextBuffer>,
    key: SpanKey,
    /// The retained vertices match `key` and may be drawn. False after a failed
    /// prepare, before the first one, and once an encode does not draw the slot.
    key_valid: bool,
    /// The current encode draws this slot.
    drawn: bool,
    /// The current encode ran `prepare` on this slot.
    prepared: bool,
}

/// Everything a span's `prepare` reads. Floats compare by bit pattern, so a
/// settled tween compares equal and any change, however small, does not.
#[derive(Default)]
struct SpanKey {
    texts: Vec<UiText>,
    depth_bits: Vec<u32>,
    viewport: [u32; 2],
    font_generation: u64,
}

impl SpanKey {
    fn matches(&self, span: &TextSpan<'_>, viewport: [u32; 2], font_generation: u64) -> bool {
        self.viewport == viewport
            && self.font_generation == font_generation
            && self.texts.len() == span.texts.len()
            && self
                .texts
                .iter()
                .zip(span.texts)
                .all(|(a, b)| same_text(a, b))
            && self.depth_bits.len() == span.depths.len()
            && self
                .depth_bits
                .iter()
                .zip(span.depths)
                .all(|(&bits, depth)| bits == depth.to_bits())
    }

    fn store(&mut self, span: &TextSpan<'_>, viewport: [u32; 2], font_generation: u64) {
        self.texts.clear();
        self.texts.extend_from_slice(span.texts);
        self.depth_bits.clear();
        self.depth_bits
            .extend(span.depths.iter().map(|depth| depth.to_bits()));
        self.viewport = viewport;
        self.font_generation = font_generation;
    }
}

/// Exhaustive by destructuring: a field added to `UiText` fails to compile
/// here until the key covers it.
fn same_text(a: &UiText, b: &UiText) -> bool {
    let UiText {
        content,
        position,
        font_size,
        color,
        family,
    } = a;
    *content == b.content
        && *family == b.family
        && *color == b.color
        && font_size.to_bits() == b.font_size.to_bits()
        && position.map(f32::to_bits) == b.position.map(f32::to_bits)
}

/// One span of the composition, as the gate reads it. Depths are the span's
/// own, one per text; a text's glyph metadata is its index within the span.
pub(crate) struct TextSpan<'a> {
    pub(crate) layer: usize,
    pub(crate) span: usize,
    pub(crate) texts: &'a [UiText],
    pub(crate) depths: &'a [f32],
}

/// What one encode's prepare phase did.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(crate) struct TextPrepareStats {
    /// Distinct spans whose `prepare` ran. glyphon writes vertices and atlas
    /// texels only inside `prepare`, so zero means no text write.
    pub(crate) spans_prepared: u32,
    /// The atlas in-use set was cleared: a cadence reclaim or an atlas-full
    /// recovery.
    pub(crate) trimmed: bool,
    pub(crate) reclaimed: bool,
    pub(crate) atlas_full_recovered: bool,
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
        let text_atlas = TextAtlas::new(device, queue, &glyph_cache, color_format);

        Self {
            swash_cache,
            glyph_cache,
            viewport,
            text_atlas,
            slots: Vec::new(),
            depth_stencil,
            font_generation: 0,
            encodes_since_reclaim: 0,
            #[cfg(debug_assertions)]
            encode_count: 0,
        }
    }

    /// Count one UI encode against the once-per-submit guard. Called at encode
    /// entry, whether or not the encode prepares: a later encode could prepare a
    /// span this one draws from retained vertices, and the queue-timeline write
    /// would land before both draws.
    pub fn begin_encode(&mut self) {
        #[cfg(debug_assertions)]
        {
            self.encode_count += 1;
            debug_assert!(
                self.encode_count <= 1,
                "UI encoded {} times before submit — a second composition could \
                 overwrite retained text-span buffers another encode draws from \
                 (one UI encode per submit)",
                self.encode_count,
            );
        }
    }

    /// Reset the once-per-submit guard. Called after the command buffer
    /// containing the UI encode is submitted. No-op in release.
    pub fn reset_prepare_guard(&mut self) {
        #[cfg(debug_assertions)]
        {
            self.encode_count = 0;
        }
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
        // Every call moves the generation: a `false` return can still have
        // added faces under another family name.
        self.font_generation += 1;
        let before = font_face_ids_for_family(font_system, family);
        font_system.db_mut().load_font_data(ttf_bytes);
        font_family_gained_face(font_system, family, &before)
    }

    /// Run the composition's prepare phase. A span whose key matches its slot's
    /// skips shaping and `prepare` and draws its retained vertices. Every
    /// `TEXT_RECLAIM_CADENCE` encodes with text, and at once when glyphon reports
    /// the atlas full, the atlas is trimmed and then every live span prepares,
    /// all before the pass opens. Trimming first keeps every live span's glyphs
    /// in glyphon's in-use set, which is all that protects a skipped span's
    /// atlas texels from eviction until the next reclaim.
    pub fn prepare_spans(
        &mut self,
        font_system: &mut FontSystem,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        viewport: [u32; 2],
        spans: &[TextSpan<'_>],
    ) -> TextPrepareStats {
        for slot in self.slots.iter_mut().flatten() {
            slot.drawn = false;
            slot.prepared = false;
        }
        let mut stats = TextPrepareStats::default();
        if spans.is_empty() {
            // No reclaim on a zero-text frame: the cadence counts encodes with
            // text, so a reclaim that falls due moves to the next one.
            self.forget_undrawn_slots();
            return stats;
        }

        self.viewport.update(
            queue,
            Resolution {
                width: viewport[0],
                height: viewport[1],
            },
        );

        self.encodes_since_reclaim += 1;
        let reclaim = self.encodes_since_reclaim >= TEXT_RECLAIM_CADENCE;
        if reclaim {
            self.trim();
            stats.trimmed = true;
            stats.reclaimed = true;
        }

        let pass = PreparePass {
            viewport,
            force: reclaim,
            last_chance: false,
        };
        if self
            .prepare_pass(font_system, device, queue, spans, pass)
            .is_err()
        {
            // Atlas-full recovery, once per encode: trim, then every live span
            // prepares again — those already prepared this encode included — in
            // this same prepare phase.
            self.trim();
            stats.trimmed = true;
            stats.atlas_full_recovered = true;
            let pass = PreparePass {
                viewport,
                force: true,
                last_chance: true,
            };
            let _ = self.prepare_pass(font_system, device, queue, spans, pass);
        }

        self.forget_undrawn_slots();
        stats.spans_prepared = self
            .slots
            .iter()
            .flatten()
            .filter(|slot| slot.prepared)
            .count() as u32;
        stats
    }

    /// Prepare each span whose key moved, or every span when `force`. Stops at
    /// the first atlas-full report unless this is the recovery pass, which
    /// leaves a failing span undrawable and continues.
    fn prepare_pass(
        &mut self,
        font_system: &mut FontSystem,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        spans: &[TextSpan<'_>],
        pass: PreparePass,
    ) -> Result<(), PrepareError> {
        for span in spans {
            self.ensure_slot(device, span.layer, span.span);
            let slot = &mut self.slots[span.layer][span.span];
            slot.drawn = true;

            let unchanged =
                slot.key_valid && slot.key.matches(span, pass.viewport, self.font_generation);
            if unchanged && !pass.force {
                continue;
            }
            if !unchanged {
                shape_span(font_system, &mut slot.buffers, span.texts, pass.viewport);
            }

            // Invalid until this prepare succeeds: a failed prepare clears and
            // partly refills glyphon's vertex list without writing the buffer, so
            // drawing it would read stale vertices under a partial count.
            slot.key_valid = false;
            slot.prepared = true;
            let areas = span
                .texts
                .iter()
                .zip(&slot.buffers)
                .map(|(t, buffer)| TextArea {
                    buffer,
                    left: t.position[0],
                    top: t.position[1],
                    scale: 1.0,
                    bounds: TextBounds {
                        left: 0,
                        top: 0,
                        right: pass.viewport[0] as i32,
                        bottom: pass.viewport[1] as i32,
                    },
                    default_color: GlyphColor::rgba(t.color[0], t.color[1], t.color[2], t.color[3]),
                    custom_glyphs: &[],
                });
            let depths = span.depths;
            match slot.renderer.prepare_with_depth(
                device,
                queue,
                font_system,
                &mut self.text_atlas,
                &self.viewport,
                areas,
                &mut self.swash_cache,
                |metadata| depths.get(metadata).copied().unwrap_or(0.0),
            ) {
                Ok(()) => {
                    slot.key.store(span, pass.viewport, self.font_generation);
                    slot.key_valid = true;
                }
                Err(e) if pass.last_chance => {
                    log::warn!("[Renderer] UI text prepare failed after atlas recovery: {e}");
                }
                Err(e) => return Err(e),
            }
        }
        Ok(())
    }

    fn ensure_slot(&mut self, device: &wgpu::Device, layer: usize, span: usize) {
        if self.slots.len() <= layer {
            self.slots.resize_with(layer + 1, Vec::new);
        }
        let layer_slots = &mut self.slots[layer];
        while layer_slots.len() <= span {
            layer_slots.push(TextSlot {
                renderer: TextRenderer::new(
                    &mut self.text_atlas,
                    device,
                    wgpu::MultisampleState::default(),
                    Some(self.depth_stencil.clone()),
                ),
                buffers: Vec::new(),
                key: SpanKey::default(),
                key_valid: false,
                drawn: false,
                prepared: false,
            });
        }
    }

    /// A slot the encode does not draw forgets its key, so when it returns it
    /// prepares again even with identical text: the atlas may have reclaimed
    /// its glyphs while it was away.
    fn forget_undrawn_slots(&mut self) {
        for slot in self.slots.iter_mut().flatten() {
            if !slot.drawn {
                slot.key_valid = false;
            }
        }
    }

    fn trim(&mut self) {
        self.text_atlas.trim();
        self.encodes_since_reclaim = 0;
    }

    /// Record one span into an already-open render pass at its retained
    /// paint-stream position. A span this encode does not draw, or whose prepare
    /// failed, records nothing. A failed draw is logged, not propagated —
    /// `render` only fails if the atlas grew after prepare, so a panic here would
    /// needlessly crash the frame.
    pub fn render_span<'pass>(
        &'pass self,
        layer: usize,
        span: usize,
        pass: &mut wgpu::RenderPass<'pass>,
    ) {
        let Some(slot) = self.slots.get(layer).and_then(|layer| layer.get(span)) else {
            return;
        };
        if !(slot.drawn && slot.key_valid) {
            return;
        }
        if let Err(e) = slot.renderer.render(&self.text_atlas, &self.viewport, pass) {
            log::warn!("UI text render failed: {e}");
        }
    }
}

#[cfg(test)]
impl UiTextRenderer {
    /// `(layer, span)` of every slot the last encode prepared, in slot order.
    pub(crate) fn prepared_spans_for_test(&self) -> Vec<(usize, usize)> {
        self.slots
            .iter()
            .enumerate()
            .flat_map(|(layer, slots)| {
                slots
                    .iter()
                    .enumerate()
                    .filter(|(_, slot)| slot.prepared)
                    .map(move |(span, _)| (layer, span))
            })
            .collect()
    }
}

#[derive(Clone, Copy)]
struct PreparePass {
    viewport: [u32; 2],
    force: bool,
    last_chance: bool,
}

/// Shape each `UiText` of one span into a glyphon `Buffer`, selecting the line's
/// own `family` at its device-pixel font size. Glyph metadata is the text's
/// index within the span, which the depth lookup reads, so a span's prepare
/// never depends on texts outside it.
fn shape_span(
    font_system: &mut FontSystem,
    buffers: &mut Vec<TextBuffer>,
    texts: &[UiText],
    viewport: [u32; 2],
) {
    buffers.clear();
    for (i, t) in texts.iter().enumerate() {
        let metrics = Metrics::new(t.font_size, t.font_size * LINE_HEIGHT_FACTOR);
        let mut buffer = TextBuffer::new(font_system, metrics);
        // Bound the layout box to the UI layer (surface extent): glyphon needs a finite
        // layout size to resolve the run (an unbounded box has nothing to lay
        // glyphs against).
        buffer.set_size(Some(viewport[0] as f32), Some(viewport[1] as f32));
        buffer.set_text(
            &t.content,
            &Attrs::new().family(Family::Name(&t.family)).metadata(i),
            Shaping::Advanced,
            None,
        );
        buffer.shape_until_scroll(font_system, false);
        buffers.push(buffer);
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
