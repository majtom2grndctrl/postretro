// egui texture deltas carried until a presented frame applies them.
// See: context/lib/rendering_pipeline.md §12

/// Texture deltas egui produced that no presented frame has applied yet.
///
/// egui emits each delta once: the font atlas arrives as a single whole-texture
/// set, and every later glyph update is a partial write into it. A frame that
/// builds the UI but never reaches `Renderer::render_debug_ui` (no surface
/// this frame, an error exit) would otherwise lose its deltas, and egui-wgpu
/// rejects the next partial update against a texture it never allocated.
/// Merging with `TexturesDelta::append` keeps per-texture order and lets a
/// newer whole set or free supersede older entries.
#[derive(Default)]
pub struct PendingTextures {
    delta: egui::TexturesDelta,
}

impl PendingTextures {
    /// Queue one frame's deltas behind any still unapplied.
    pub fn push(&mut self, newer: egui::TexturesDelta) {
        self.delta.append(newer);
    }

    /// The merged delta, for `Renderer::render_debug_ui` to drain.
    pub fn delta_mut(&mut self) -> &mut egui::TexturesDelta {
        &mut self.delta
    }

    pub fn is_empty(&self) -> bool {
        self.delta.is_empty()
    }
}

impl Drop for PendingTextures {
    /// Discards what is still queued. This only drops with its `DebugUi`, on
    /// suspend or shutdown, when the egui context and its GPU textures go too.
    /// epaint debug-asserts that a dropped `TexturesDelta` is empty.
    fn drop(&mut self) {
        self.delta.clear();
    }
}

#[cfg(test)]
mod tests {
    use egui::epaint::{ColorImage, ImageDelta};
    use egui::{TextureId, TextureOptions};

    use super::*;

    fn whole(side: usize) -> ImageDelta {
        ImageDelta::full(
            ColorImage::filled([side, side], egui::Color32::WHITE),
            TextureOptions::LINEAR,
        )
    }

    fn partial(x: usize) -> ImageDelta {
        ImageDelta::partial(
            [x, 0],
            ColorImage::filled([1, 1], egui::Color32::BLACK),
            TextureOptions::LINEAR,
        )
    }

    fn frame(id: TextureId, delta: ImageDelta) -> egui::TexturesDelta {
        let mut out = egui::TexturesDelta::default();
        out.push(id, delta);
        out
    }

    #[test]
    fn skipped_frame_carries_whole_atlas_ahead_of_later_partial_updates() {
        let atlas = TextureId::Managed(0);
        let mut pending = PendingTextures::default();

        // Frame 1 is never presented; frame 2 writes glyphs into the atlas.
        pending.push(frame(atlas, whole(8)));
        pending.push(frame(atlas, partial(3)));

        let deltas = &pending.delta_mut().set[&atlas];
        assert_eq!(deltas.len(), 2);
        assert!(deltas[0].is_whole(), "the allocation must apply first");
        assert_eq!(deltas[1].pos, Some([3, 0]));
    }

    #[test]
    fn newer_free_drops_carried_sets_for_that_texture() {
        let user = TextureId::User(7);
        let mut pending = PendingTextures::default();
        pending.push(frame(user, whole(4)));

        let mut freed = egui::TexturesDelta::default();
        freed.free(user);
        pending.push(freed);

        let delta = pending.delta_mut();
        assert!(!delta.set.contains_key(&user));
        assert!(delta.free.contains(&user));
    }

    #[test]
    fn dropping_with_unapplied_deltas_does_not_trip_the_epaint_assert() {
        let mut pending = PendingTextures::default();
        pending.push(frame(TextureId::Managed(0), whole(8)));
        assert!(!pending.is_empty());

        drop(pending);
    }
}
