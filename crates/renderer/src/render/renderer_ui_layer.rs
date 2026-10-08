// Game UI recording into the native-res UI layer the resolve composites.
// See: context/lib/ui.md §5 · context/lib/rendering_pipeline.md §7.8

use super::*;

impl Renderer {
    /// Lay out the frame's UI snapshot (presentation, HUD and modal layers) and
    /// record it into the UI layer at the surface extent, with UI's private
    /// depth target. The layer is cleared transparent every frame, UI or not.
    pub(super) fn record_ui_layer(
        &mut self,
        encoder: &mut wgpu::CommandEncoder,
        font_system: &mut postretro_ui::text::FontSystem,
    ) {
        let surface = self.render_extents().surface;
        let ui_viewport = [surface.width, surface.height];
        let Self {
            device,
            queue,
            full,
            ..
        } = self;
        let full = full
            .as_mut()
            .expect("renderer full-init must complete before full-ready paths run");
        // Modal stack: lay out and record each layer bottom→top (`trees[0]` is the
        // bottom HUD, the last entry the top/active modal). Each layer keeps its
        // own retained tree + dirty gate, so a frozen lower layer recomputes
        // nothing while the top animates. Painter's order is the stack order: a
        // later layer's quads composite over the earlier ones in the one UI pass.
        // Empty/empty-laying-out layers early-out individually.
        let stack_len = full.ui_snapshot.trees.len();

        // Lay out EVERY layer first into owned draw data, THEN compose all layers
        // into a SINGLE `encode` call. The glyphon text half (`UiTextRenderer`) is
        // shared across layers and holds ONE vertex buffer it overwrites at offset
        // 0 on each `prepare`; `queue.write_buffer` resolves on the queue timeline
        // (last write wins) regardless of recording order, so issuing a separate
        // `encode` per layer makes EVERY layer's text draw read the LAST layer's
        // shaped glyphs, so a lower layer's text renders the top layer's glyphs. This mirrors the multi-batch quad-buffer clobber
        // already documented in `UiPass::encode`: one `prepare`/`render` per frame,
        // with all layers' glyphs concatenated in painter order, sidesteps it.
        let mut layer_draws: Vec<ui::tree::UiDrawData> = Vec::with_capacity(stack_len + 1);
        // Presentation is a passive world-facing layer, not a retained modal.
        // Lower it first so HUD and modal trees remain visually above it, while
        // focus export continues to inspect only the retained top tree below.
        let presentation_draw = full.ui.layout_presentation_inputs(
            font_system,
            &full.presentation_inputs,
            ui_viewport,
            full.ui_images.image_sizes(),
            full.ui_images.image_sizes_generation(),
            &full.ui_theme,
            full.ui_theme_generation,
            ui::tree::TweenClock {
                now: full.ui_snapshot.time_seconds,
                snap: full.ui_snapshot.reduce_motion,
            },
        );
        layer_draws.push(presentation_draw);
        for (layer, entry) in full.ui_snapshot.trees.iter().enumerate() {
            let is_top = layer + 1 == stack_len;
            // Only the top tree scrolls: its focused stop scrolls into view and
            // the captured wheel scrolls the container under the cursor. Lower
            // layers hold their offsets.
            let scroll_input = if is_top {
                ui::tree::ScrollInput {
                    focused_id: full.ui_snapshot.focused_id.as_deref(),
                    wheel: full.ui_snapshot.wheel,
                }
            } else {
                ui::tree::ScrollInput::default()
            };
            // Image widgets measure from the renderer-owned image registry. A
            // missing key still collapses, but the registry now warns once when
            // the draw path tries to bind it instead of failing silently.
            // Bound text/panel nodes resolve against the snapshot's slot values
            // (disjoint field borrow from `&mut full.ui`). The entry carries the
            // layer's owner, which the retained layer records for the focus
            // export.
            let mut draw = full.ui.layout_gameplay_tree(
                font_system,
                layer,
                entry,
                ui_viewport,
                full.ui_images.image_sizes(),
                full.ui_images.image_sizes_generation(),
                &full.ui_snapshot.slot_values,
                &full.ui_snapshot.cell_values,
                &full.ui_theme,
                full.ui_theme_generation,
                ui::tree::TweenClock {
                    now: full.ui_snapshot.time_seconds,
                    snap: full.ui_snapshot.reduce_motion,
                },
                scroll_input,
            );
            // Focus ring: only the TOP layer takes focus, so
            // draw the engine ring around the focused node's rect on it. The
            // focused id rode in on the snapshot (resolved app-side last frame, so
            // it may trail a focus change by one frame). The ring is a `focus.ring`
            // bordered frame inset by the `xs` spacing token; appended through
            // the layer's paint stream so it composites over the focused content.
            if is_top && let Some(focused) = full.ui_snapshot.focused_id.as_deref() {
                let focus_rects = full.ui.export_top_focus_rects(
                    ui_viewport,
                    &full.ui_snapshot.slot_values,
                    &full.ui_snapshot.cell_values,
                );
                if let Some(fr) = focus_rects.rects.iter().find(|r| r.id == focused) {
                    let inset = full.ui_theme.spacing("xs").unwrap_or(0.0)
                        * ui::layout::device_scale(ui_viewport);
                    let ring_color = full
                        .ui_theme
                        .color("focus.ring")
                        .unwrap_or([1.0, 0.0, 1.0, 1.0]);
                    // A stop inside a scroll viewport rings within the
                    // viewport grown by the ring's reach along each edge the
                    // stop reaches, so a stop scrolled flush with an edge (or
                    // a full-width row) keeps every bar; a stop scrolled out
                    // of view draws no ring.
                    let ring_clip = match fr.clip {
                        None => Some(None),
                        Some(clip) => ui::focus_ring_clip(fr.rect, clip, inset).map(Some),
                    };
                    if let Some(ring_clip) = ring_clip {
                        draw.set_clip(ring_clip);
                        ui::push_focus_ring(&mut draw, fr.rect, inset, ring_color);
                        draw.set_clip(None);
                    }
                }
            }
            layer_draws.push(draw);
        }

        // Fold every laid-out layer into ONE whole-frame composition (bottom→top
        // painter order) and record a SINGLE UI pass. The composition is the unit
        // of encoding — `encode` takes the whole composition, never one layer — so
        // the cross-layer glyphon clobber (every layer's text reading the last
        // layer's shaped glyphs) is unrepresentable. The white bind group is cloned
        // out first so the `&self.ui_images` borrow the fold takes can coexist with
        // the `&mut self.ui` encode call below.
        let white_bg = full.ui.white_bind_group().clone();
        let composition =
            ui::UiComposition::from_layer_draws(&layer_draws, &white_bg, &full.ui_images);
        // Always encode, even an empty composition: the pass opens and clears
        // the layer transparent, so a frame with no UI shows none of the last
        // frame's.
        full.ui.encode(
            font_system,
            device,
            queue,
            encoder,
            full.screen_effects.ui_layer_view(),
            ui_viewport,
            wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
            &composition,
        );
        // The composition's frame-scoped borrows end here. Reclaim the passive
        // layer's translated aggregate so its Vec/String storage stays warm for
        // the next frame instead of being dropped with `layer_draws`.
        drop(composition);
        let presentation_draw = layer_draws.remove(0);
        full.ui.recycle_presentation_draw_data(presentation_draw);
        // Drop retained state for any layers popped since last frame (stack
        // shrank), so freed modal trees release their layout cache.
        full.ui.truncate_gameplay_stack(stack_len);
    }
}
