// Retained gameplay-tree layers and passive presentation layouts: per-frame
// layout, the focus-rect export, and stack truncation.
// See: context/lib/ui.md §1, §4

use super::*;

/// One retained gameplay UI tree plus the descriptor it was built from. The
/// descriptor is kept so the next frame can detect a structural change (the
/// snapshot delivered a different tree) by `!=` comparison — `AnchoredTree`
/// derives `PartialEq` — and rebuild only then. The cached draw list lives inside
/// the `UiTree` itself (see `UiTree::cached_draw_data`).
pub(super) struct RetainedGameplayTree {
    /// The descriptor the retained `tree` was built from. Compared against the
    /// incoming descriptor each frame; a difference forces a rebuild.
    pub(super) descriptor: descriptor::AnchoredTree,
    /// The renderer's UI theme generation the retained `tree` was built (and so
    /// token-resolved) against. A bump (the engine installed an override theme)
    /// invalidates the resolved colors/spacing/fonts baked into the tree, so the
    /// gate rebuilds it even when the descriptor is byte-for-byte identical.
    pub(super) theme_generation: u64,
    /// The retained taffy-backed tree, carrying its layout cache, last viewport,
    /// per-bound-node last-resolved values, and cached draw list across frames.
    pub(super) tree: tree::UiTree,
    /// The registry name and tier of the stack entry last laid out at this
    /// layer. The focus export is stamped from here, never from a later
    /// snapshot, so a frame that skipped layout cannot attribute these rects
    /// to a different tree.
    pub(super) owner: tree::FocusRectOwner,
}

/// Renderer-local state retained for one live passive presentation. Layout,
/// fact cells, and tween state stay warm across frames until the app drops the
/// instance from its bounded input set.
pub(super) struct PresentationLayout {
    pub(super) template: postretro_entities::PresentationTemplateHandle,
    pub(super) theme_generation: u64,
    pub(super) layout: tree::PresentationTemplateLayout,
    pub(super) fact_cells: tree::CellValues,
    pub(super) relative_draw: tree::UiDrawData,
    pub(super) active_generation: u64,
}

impl UiPass {
    /// Replace the whole manifest-owned passive-template snapshot. Existing
    /// instance layouts deliberately rebuild: an author can hot-reload a
    /// widget subtree while its current transient remains live.
    pub fn replace_presentation_templates(&mut self, templates: Vec<PresentationTemplate>) {
        self.presentation_templates.clear();
        self.presentation_templates.reserve(templates.len());
        self.warned_missing_presentation_templates.clear();
        self.presentation_layouts.clear();
        self.presentation_draw = tree::UiDrawData::default();
        for template in templates {
            let handle = postretro_entities::PresentationTemplateHandle::from(template.id.clone());
            if self
                .presentation_templates
                .insert(handle.clone(), template)
                .is_some()
            {
                log::warn!(
                    "[Renderer] duplicate presentation template `{}` replaced during registry install",
                    handle.0
                );
            }
        }
    }

    /// Lay out ONE modal-stack layer's descriptor tree through the RETAINED
    /// `UiTree` held for that layer, so layout and the draw list only rebuild when
    /// their inputs change across frames (the runtime perf win). `layer` is the
    /// bottom→top stack index; each layer keeps its own retained tree, dirty gate,
    /// and bound-value diff, so a frozen lower layer recomputes nothing while a
    /// top layer animates.
    ///
    /// Reuse vs rebuild (per layer): the retained tree is reused while the
    /// incoming `descriptor` equals the one it was built from AND the theme
    /// generation is unchanged. A different descriptor (a structurally new tree at
    /// this layer — including the stack growing into a fresh slot) rebuilds it via
    /// `UiTree::from_descriptor`. Once reused, `build_draw_data_retained` runs the
    /// subscriber-aware bound-value diff and the relayout/redraw split:
    /// - an appearance-only bound change (the panel flash color) rebuilds the
    ///   draw list WITHOUT a taffy relayout,
    /// - a bound text-content change re-measures and relays out,
    /// - a no-change frame returns the cached draw list and recomputes nothing.
    ///
    /// The caller drives layers `0..stack_len` in order and calls
    /// `truncate_gameplay_stack(stack_len)` once per frame so popped layers drop
    /// their retained state. The boot splash never calls this — it renders through
    /// `BootSplashPass`, outside gameplay UI and the retained tree stack.
    ///
    /// `clock` is the deterministic, dt-accumulated frame time (plus the
    /// reduce-motion switch) threaded
    /// down to the retained build for the tween runtime to ease bound values over
    /// time.
    ///
    /// `entry` also names the layer's owner (registry name and tier), which the
    /// focus export carries: the owner is recorded with the layout it names.
    ///
    /// `scroll_input` carries the focused id and pointer wheel to the TOP layer's
    /// scroll containers; lower layers pass the default and hold their offsets.
    // Wide by necessity: layer + viewport + image sizes + slot values + theme +
    // theme generation + frame time are all distinct retained-build inputs;
    // bundling them into a struct would only obscure the per-frame call site.
    #[allow(clippy::too_many_arguments)]
    pub fn layout_gameplay_tree(
        &mut self,
        font_system: &mut FontSystem,
        layer: usize,
        entry: &postretro_ui::UiTreeEntry,
        viewport: [u32; 2],
        image_sizes: &tree::ImageSizes,
        image_sizes_generation: u64,
        slot_values: &std::collections::HashMap<String, postretro_entities::SlotValue>,
        cell_values: &tree::CellValues,
        theme: &theme::UiTheme,
        theme_generation: u64,
        clock: tree::TweenClock,
        scroll_input: tree::ScrollInput<'_>,
    ) -> tree::UiDrawData {
        debug_assert!(
            layer <= self.gameplay_trees.len(),
            "layers must be driven in bottom→top order without gaps",
        );

        // Rebuild this layer's retained tree when there is none yet (the stack
        // grew into this slot), when the incoming descriptor differs from the one
        // it was built from (a structural change), OR when the theme generation
        // moved (override theme installed, so baked tokens are stale). A settled
        // frame (same descriptor + same generation) reuses the retained tree.
        let tree = &entry.descriptor;
        let needs_build = match self.gameplay_trees.get(layer) {
            Some(retained) => {
                retained.descriptor != *tree || retained.theme_generation != theme_generation
            }
            None => true,
        };
        if needs_build {
            let mut fresh = tree::UiTree::from_descriptor(tree, theme);
            // The same tree rebuilt in place (a rebound control, a glyph that
            // followed the device family, a theme swap) keeps its scroll
            // positions; a different tree in this slot starts at the top.
            if let Some(previous) = self.gameplay_trees.get(layer)
                && previous.owner.name == entry.name
                && previous.owner.tier == entry.tier
            {
                fresh.carry_scroll_from(&previous.tree);
            }
            let rebuilt = RetainedGameplayTree {
                descriptor: tree.clone(),
                theme_generation,
                tree: fresh,
                owner: tree::FocusRectOwner {
                    name: entry.name.clone(),
                    tier: entry.tier,
                },
            };
            if layer < self.gameplay_trees.len() {
                self.gameplay_trees[layer] = rebuilt;
            } else {
                self.gameplay_trees.push(rebuilt);
            }
        }

        let retained = &mut self.gameplay_trees[layer];
        // An identical descriptor under another name reuses the layout; the
        // owner still follows the entry. Written only on change, so a settled
        // frame allocates nothing.
        if retained.owner.name != entry.name || retained.owner.tier != entry.tier {
            retained.owner = tree::FocusRectOwner {
                name: entry.name.clone(),
                tier: entry.tier,
            };
        }
        retained
            .tree
            .build_draw_data_retained_with_image_generation(
                viewport,
                font_system,
                image_sizes,
                image_sizes_generation,
                slot_values,
                cell_values,
                clock,
                scroll_input,
            )
    }

    /// Lower app-projected passive presentation instances into one draw list.
    /// Each instance owns its fact snapshot; no value is read from the gameplay
    /// slot table, and this path has no retained-tree/focus/input interaction.
    ///
    #[allow(clippy::too_many_arguments)]
    pub fn layout_presentation_inputs(
        &mut self,
        font_system: &mut FontSystem,
        inputs: &[super::super::PresentationDrawInput],
        viewport: [u32; 2],
        image_sizes: &tree::ImageSizes,
        image_sizes_generation: u64,
        theme: &theme::UiTheme,
        theme_generation: u64,
        clock: tree::TweenClock,
    ) -> tree::UiDrawData {
        self.presentation_layout_generation = self.presentation_layout_generation.wrapping_add(1);
        if self.presentation_layout_generation == 0 {
            self.presentation_layouts.clear();
            self.presentation_layout_generation = 1;
        }
        let active_generation = self.presentation_layout_generation;
        let additional_layout_capacity =
            inputs.len().saturating_sub(self.presentation_layouts.len());
        self.presentation_layouts
            .reserve(additional_layout_capacity);
        let mut draw = std::mem::take(&mut self.presentation_draw);
        if draw.paint_order.capacity() == 0 && !inputs.is_empty() {
            draw = tree::UiDrawData::with_estimated_presentation_capacity(inputs.len());
        } else {
            draw.clear_preserving_capacity();
        }

        for input in inputs {
            let Some(template) = self.presentation_templates.get(&input.template) else {
                if self
                    .warned_missing_presentation_templates
                    .insert(input.template.clone())
                {
                    log::warn!(
                        "[Renderer] passive presentation template `{}` is not registered; skipping draw",
                        input.template.0
                    );
                }
                self.presentation_layouts.remove(&input.instance_id);
                continue;
            };
            let rebuild = match self.presentation_layouts.get(&input.instance_id) {
                Some(cached) => {
                    cached.template != input.template || cached.theme_generation != theme_generation
                }
                None => true,
            };
            if rebuild {
                self.presentation_layouts.insert(
                    input.instance_id,
                    PresentationLayout {
                        template: input.template.clone(),
                        theme_generation,
                        layout: tree::PresentationTemplateLayout::from_widget(
                            &template.root,
                            theme,
                        ),
                        fact_cells: tree::CellValues::with_capacity(input.facts.len()),
                        relative_draw: tree::UiDrawData::default(),
                        active_generation,
                    },
                );
            }

            let cached = self
                .presentation_layouts
                .get_mut(&input.instance_id)
                .expect("presentation layout inserted or retained above");
            cached.active_generation = active_generation;
            tree::PresentationTemplateLayout::update_fact_cell_values(
                &input.facts,
                &mut cached.fact_cells,
            );
            cached.layout.build_draw_data_into(
                viewport,
                font_system,
                image_sizes,
                image_sizes_generation,
                &cached.fact_cells,
                clock,
                &mut cached.relative_draw,
            );
            if input.visible {
                draw.append_translated(&cached.relative_draw, input.anchor, input.opacity);
            }
        }
        self.presentation_layouts
            .retain(|_, layout| layout.active_generation == active_generation);
        draw
    }

    /// Return the presentation aggregate after the composition has finished
    /// borrowing it. This preserves all bounded frame-output allocations.
    pub fn recycle_presentation_draw_data(&mut self, draw: tree::UiDrawData) {
        self.presentation_draw = draw;
    }

    /// Export the flat hit-test / focus rect list for the TOP stack layer (the
    /// only one that takes focus), against the descriptor it was built from and the
    /// current `viewport` projection. Returns an empty list when there is no layer.
    /// The renderer publishes this back to the app (the reverse twin of the
    /// app→renderer snapshot); the app's focus engine consumes it the NEXT frame.
    ///
    /// `slot_values`/`cell_values` are the frame's read snapshot: the export resolves
    /// each focusable button's `selected`/`checked` predicate (M13 G2) against them
    /// for the a11y readback. Pass the same snapshot the draw build used.
    ///
    /// Must be called after `layout_gameplay_tree` has laid out every layer this
    /// frame, so the top layer's taffy layout is current for `viewport`.
    pub fn export_top_focus_rects(
        &self,
        viewport: [u32; 2],
        slot_values: &std::collections::HashMap<String, postretro_entities::SlotValue>,
        cell_values: &tree::CellValues,
    ) -> tree::FocusRectList {
        match self.gameplay_trees.last() {
            Some(retained) => {
                let mut rects = retained.tree.export_focus_rects(
                    &retained.descriptor,
                    viewport,
                    slot_values,
                    cell_values,
                );
                // Stamped with the owner laid out beside these rects, so a
                // press attributes to the tree that owned them (`ui.md` §4.1).
                rects.owner = Some(retained.owner.clone());
                rects
            }
            None => tree::FocusRectList::default(),
        }
    }

    /// Drop retained state for stack layers at or above `len` — called once per
    /// frame after laying out `0..len`, so popped modal trees release their
    /// retained `UiTree` (layout cache, bound-value subscriptions) rather than
    /// lingering. A stack that shrank to zero (HUD-only frame back to no UI)
    /// clears every layer.
    pub fn truncate_gameplay_stack(&mut self, len: usize) {
        if self.gameplay_trees.len() > len {
            self.gameplay_trees.truncate(len);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::render::gpu_test_harness::try_init_gpu;
    use postretro_ui::modal_stack::ScopeTier;

    fn entry(name: &str, tier: ScopeTier) -> postretro_ui::UiTreeEntry {
        let descriptor = descriptor::AnchoredTree {
            anchor: layout::Anchor::TopLeft,
            offset: [0.0, 0.0],
            root: descriptor::Widget::Text(descriptor::TextWidget {
                content: "MENU".into(),
                font_size: 24.0,
                color: descriptor::ColorValue::Literal([1.0; 4]),
                font: None,
                bind: None,
                style_ranges: None,
                id: None,
                focus_neighbors: Default::default(),
                visible_when: None,
                role: None,
            }),
            capture_mode: descriptor::CaptureMode::Capture,
            initial_focus: None,
            text_entry_target: None,
            accessible_name: None,
            role: None,
            restore_on_return: None,
        };
        postretro_ui::UiTreeEntry {
            name: name.into(),
            tier,
            capture_mode: descriptor.capture_mode,
            descriptor,
            on_commit: None,
        }
    }

    fn owner(name: &str, tier: ScopeTier) -> Option<tree::FocusRectOwner> {
        Some(tree::FocusRectOwner {
            name: name.into(),
            tier,
        })
    }

    // The focus export names the tree whose layout produced its rects. A frame
    // that skips layout (its acquire failed) after the stack changed exports
    // the retained layer, so it must still name that layer's tree, whatever
    // the newer snapshot's top is. An identical descriptor under another name
    // reuses the layout but takes the new owner.
    #[test]
    fn the_focus_export_names_the_tree_its_rects_were_laid_out_from() {
        let Some(ctx) = try_init_gpu() else {
            eprintln!("retained layer owner test: no adapter, skipping (not a pass)");
            return;
        };
        let mut pass = UiPass::new(&ctx.device, &ctx.queue, wgpu::TextureFormat::Rgba8UnormSrgb);
        let mut font_system = postretro_ui::text::build_font_system();
        let viewport = [1280, 720];
        let images = tree::ImageSizes::new();
        let slots = std::collections::HashMap::new();
        let cells = tree::CellValues::new();
        let theme = theme::UiTheme::engine_default();
        let mut lay_out = |pass: &mut UiPass, entry: &postretro_ui::UiTreeEntry| {
            pass.layout_gameplay_tree(
                &mut font_system,
                0,
                entry,
                viewport,
                &images,
                0,
                &slots,
                &cells,
                &theme,
                0,
                tree::TweenClock::easing(0.0),
                tree::ScrollInput::default(),
            );
        };

        let mod_menu = entry("modMenu", ScopeTier::Mod);
        lay_out(&mut pass, &mod_menu);
        let exported = pass.export_top_focus_rects(viewport, &slots, &cells);
        assert_eq!(exported.owner, owner("modMenu", ScopeTier::Mod));

        // The snapshot's top becomes the engine panel, but this frame lays
        // nothing out: the export still names the mod tree.
        let exported = pass.export_top_focus_rects(viewport, &slots, &cells);
        assert_eq!(exported.owner, owner("modMenu", ScopeTier::Mod));

        let panel = entry("accessibilityPanel", ScopeTier::Engine);
        lay_out(&mut pass, &panel);
        let exported = pass.export_top_focus_rects(viewport, &slots, &cells);
        assert_eq!(
            exported.owner,
            owner("accessibilityPanel", ScopeTier::Engine)
        );

        pass.truncate_gameplay_stack(0);
        assert_eq!(
            pass.export_top_focus_rects(viewport, &slots, &cells).owner,
            None
        );
    }
}
