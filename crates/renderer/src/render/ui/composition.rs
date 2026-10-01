// Whole-frame UI composition: every modal-stack layer's shape batches and text
// runs in one painter-ordered command stream for a single encode.
// See: context/lib/ui.md §5

use super::*;

/// One instanced draw: a draw list plus the bind group for its bound texture.
/// Panels use the pass's white-texel bind group; images bind their own.
pub(crate) struct UiBatch<'a> {
    pub list: &'a UiDrawList,
    pub bind_group: &'a wgpu::BindGroup,
}

pub(super) struct OrderedUiBatch<'a> {
    pub(super) instances: Vec<UiInstance>,
    pub(super) order: usize,
    pub(super) bind_group: &'a wgpu::BindGroup,
    pub(super) writes_depth: bool,
}

/// An SDF ring batch with a unique painter-depth order. Ring instances use a
/// dedicated vertex buffer and pipeline, so they cannot share quad batches.
pub(super) struct OrderedRingBatch {
    pub(super) instances: Vec<UiRingInstance>,
    pub(super) order: usize,
}

pub(super) struct OrderedTextBatch {
    pub(super) range: std::ops::Range<usize>,
}

#[derive(Clone, Copy)]
pub(super) enum UiDrawCommand {
    Quad(usize),
    Ring(usize),
    Text(usize),
}

/// The whole frame's UI composition: every modal-stack layer's quad batches and
/// shaped-text runs in bottom→top painter order, as the single unit
/// `UiPass::encode` records. The encode boundary is the WHOLE composition, never
/// one layer — making the historical per-layer encode loop (which clobbered the
/// shared glyphon vertex buffer across layers) unrepresentable on the production
/// surface.
///
/// **Invariant — one coordinated prepare phase per surface composition.** All
/// layers funnel through ONE `encode`. Each text span in that phase owns a
/// disjoint glyphon vertex buffer, just as each shape batch owns a disjoint
/// instance-buffer region, so no queued upload can clobber another command.
///
/// Owns renderer-local quad/ring batches and concatenated text runs plus the
/// mixed command stream that records them. Consecutive text runs share one
/// glyphon batch; a shape between text runs starts another batch with independent
/// prepared storage so source-over painter order remains representable. Built in
/// the caller's frame scope so the bind-group borrows coexist with the
/// `&mut self.ui` encode call.
/// Two constructors: `from_layer_draws` (gameplay modal stack) and `from_batches`
/// (test assembly).
pub(crate) struct UiComposition<'a> {
    pub(super) batches: Vec<OrderedUiBatch<'a>>,
    pub(super) ring_batches: Vec<OrderedRingBatch>,
    pub(super) texts: Vec<UiText>,
    pub(super) text_orders: Vec<usize>,
    pub(super) text_batches: Vec<OrderedTextBatch>,
    pub(super) commands: Vec<UiDrawCommand>,
    pub(super) order_count: usize,
}

impl<'a> UiComposition<'a> {
    /// Gameplay constructor: fold the per-layer `UiDrawData` slice (bottom→top)
    /// into one composition. Production draw data carries a per-item paint stream,
    /// so A/B/A image nodes stay A/B/A instead of collapsing into one asset batch,
    /// and renderer-added focus rings can sit above the focused content. Hand-built
    /// tests that mutate the legacy lists directly fall back to the old coarse
    /// order.
    ///
    /// `white_bind_group` and `images` outlive the returned composition (they are
    /// the pass's own resources); `layer_draws` is the caller's frame-scoped fold
    /// output. All three borrows back the `'a` lifetime.
    pub fn from_layer_draws(
        layer_draws: &'a [tree::UiDrawData],
        white_bind_group: &'a wgpu::BindGroup,
        images: &'a UiImageRegistry,
    ) -> Self {
        let mut batches: Vec<OrderedUiBatch<'a>> = Vec::new();
        let mut ring_batches: Vec<OrderedRingBatch> = Vec::new();
        let mut texts: Vec<UiText> = Vec::new();
        let mut text_orders: Vec<usize> = Vec::new();
        let mut text_batches: Vec<OrderedTextBatch> = Vec::new();
        let mut commands: Vec<UiDrawCommand> = Vec::new();
        let mut order = 0usize;
        for draw in layer_draws {
            if draw.paint_order.is_empty() {
                LegacyDrawAppend {
                    white_bind_group,
                    images,
                    batches: &mut batches,
                    ring_batches: &mut ring_batches,
                    texts: &mut texts,
                    text_orders: &mut text_orders,
                    text_batches: &mut text_batches,
                    commands: &mut commands,
                    order: &mut order,
                }
                .append(draw);
                continue;
            }

            // Invariant: once `paint_order` is non-empty, it is the complete
            // record of every item in `quads`/`images`/`texts` — production
            // collection routes exclusively through `push_quad`/`push_image`/
            // `push_text`, which append to both in lockstep. A partially
            // populated `paint_order` (some items pushed, some added directly
            // to the grouped lists) would silently drop the directly-added
            // items below, since only ops in the stream get drawn.
            #[cfg(debug_assertions)]
            {
                let grouped_len = draw.quads.len()
                    + draw
                        .images
                        .iter()
                        .map(|(_, list)| list.len())
                        .sum::<usize>()
                    + draw.rings.len()
                    + draw.texts.len();
                debug_assert_eq!(
                    draw.paint_order.len(),
                    grouped_len,
                    "UiDrawData.paint_order is non-empty but incomplete: {} ops vs {} grouped items \
                     (quads + images + rings + texts). A non-empty-but-incomplete paint_order silently drops \
                     whichever grouped items were added directly instead of through push_quad/push_image/push_text. \
                     All production collection must route through those push_* helpers to keep the stream complete.",
                    draw.paint_order.len(),
                    grouped_len,
                );
            }

            for op in &draw.paint_order {
                match *op {
                    tree::UiPaintOp::Quad { index } => {
                        if let Some(instance) = draw.quads.instances.get(index).copied() {
                            append_ordered_quad_batch(
                                &mut batches,
                                white_bind_group,
                                instance,
                                order,
                                true,
                            );
                            commands.push(UiDrawCommand::Quad(batches.len() - 1));
                            order += 1;
                        }
                    }
                    tree::UiPaintOp::Image { batch, index } => {
                        let Some((asset, list)) = draw.images.get(batch) else {
                            continue;
                        };
                        let Some(instance) = list.instances.get(index).copied() else {
                            continue;
                        };
                        // Unknown key degrades by skipping just that image. The
                        // registry emits one warning per missing key, not per frame.
                        if let Some(bind_group) = images.resolve(asset) {
                            append_ordered_quad_batch(
                                &mut batches,
                                bind_group,
                                instance,
                                order,
                                false,
                            );
                            commands.push(UiDrawCommand::Quad(batches.len() - 1));
                            order += 1;
                        }
                    }
                    tree::UiPaintOp::Ring { index } => {
                        if let Some(instance) = draw.rings.get(index).copied() {
                            ring_batches.push(OrderedRingBatch {
                                instances: vec![instance],
                                order,
                            });
                            commands.push(UiDrawCommand::Ring(ring_batches.len() - 1));
                            order += 1;
                        }
                    }
                    tree::UiPaintOp::Text { index } => {
                        if let Some(text) = draw.texts.get(index) {
                            text_orders.push(order);
                            texts.push(text.clone());
                            append_ordered_text_batch(
                                &mut text_batches,
                                &mut commands,
                                texts.len() - 1,
                            );
                            order += 1;
                        }
                    }
                }
            }
        }
        Self {
            batches,
            ring_batches,
            texts,
            text_orders,
            text_batches,
            commands,
            order_count: order,
        }
    }

    /// Constructor from already-assembled batches and text — a single-layer
    /// composition that does not fold a `UiDrawData` stack. Now only the
    /// multi-batch headless regression uses it directly (the boot splash moved
    /// off the UI pass); kept for that test's disjoint-region coverage.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn from_batches(batches: Vec<UiBatch<'a>>, texts: Vec<UiText>) -> Self {
        let order_count = batches.len() + usize::from(!texts.is_empty());
        let text_order = batches.len();
        let batches: Vec<OrderedUiBatch<'a>> = batches
            .into_iter()
            .enumerate()
            .map(|(order, batch)| OrderedUiBatch {
                instances: batch.list.instances.clone(),
                order,
                bind_group: batch.bind_group,
                writes_depth: batch.list.instances.iter().all(instance_writes_depth),
            })
            .collect();
        let mut commands: Vec<UiDrawCommand> =
            (0..batches.len()).map(UiDrawCommand::Quad).collect();
        let text_batches = if texts.is_empty() {
            Vec::new()
        } else {
            commands.push(UiDrawCommand::Text(0));
            vec![OrderedTextBatch {
                range: 0..texts.len(),
            }]
        };
        Self {
            batches,
            ring_batches: Vec::new(),
            text_orders: std::iter::repeat_n(text_order, texts.len()).collect(),
            texts,
            text_batches,
            commands,
            order_count,
        }
    }
}

pub(super) fn append_ordered_text_batch(
    batches: &mut Vec<OrderedTextBatch>,
    commands: &mut Vec<UiDrawCommand>,
    text_index: usize,
) {
    if let Some(UiDrawCommand::Text(batch_index)) = commands.last().copied()
        && batches[batch_index].range.end == text_index
    {
        batches[batch_index].range.end += 1;
        return;
    }

    let batch_index = batches.len();
    batches.push(OrderedTextBatch {
        range: text_index..text_index + 1,
    });
    commands.push(UiDrawCommand::Text(batch_index));
}

pub(super) fn append_ordered_quad_batch<'a>(
    batches: &mut Vec<OrderedUiBatch<'a>>,
    bind_group: &'a wgpu::BindGroup,
    instance: UiInstance,
    order: usize,
    allow_depth_write: bool,
) {
    batches.push(OrderedUiBatch {
        instances: vec![instance],
        order,
        bind_group,
        writes_depth: allow_depth_write && instance_writes_depth(&instance),
    });
}

/// Mutable accumulation context for the coarse legacy draw-list fallback.
/// Keeping these outputs together makes the historical grouped order explicit
/// without widening the append operation into an eight-argument helper.
pub(super) struct LegacyDrawAppend<'a, 'out> {
    pub(super) white_bind_group: &'a wgpu::BindGroup,
    pub(super) images: &'a UiImageRegistry,
    pub(super) batches: &'out mut Vec<OrderedUiBatch<'a>>,
    pub(super) ring_batches: &'out mut Vec<OrderedRingBatch>,
    pub(super) texts: &'out mut Vec<UiText>,
    pub(super) text_orders: &'out mut Vec<usize>,
    pub(super) text_batches: &'out mut Vec<OrderedTextBatch>,
    pub(super) commands: &'out mut Vec<UiDrawCommand>,
    pub(super) order: &'out mut usize,
}

impl<'a, 'out> LegacyDrawAppend<'a, 'out> {
    /// Preserve the legacy coarse order: quads, images, rings, then text.
    fn append(&mut self, draw: &tree::UiDrawData) {
        if !draw.quads.is_empty() {
            self.batches.push(OrderedUiBatch {
                instances: draw.quads.instances.clone(),
                order: *self.order,
                bind_group: self.white_bind_group,
                writes_depth: draw.quads.instances.iter().all(instance_writes_depth),
            });
            self.commands
                .push(UiDrawCommand::Quad(self.batches.len() - 1));
            *self.order += 1;
        }
        for (asset, list) in &draw.images {
            if list.is_empty() {
                continue;
            }
            if let Some(bind_group) = self.images.resolve(asset) {
                self.batches.push(OrderedUiBatch {
                    instances: list.instances.clone(),
                    order: *self.order,
                    bind_group,
                    writes_depth: false,
                });
                self.commands
                    .push(UiDrawCommand::Quad(self.batches.len() - 1));
                *self.order += 1;
            }
        }
        if !draw.rings.is_empty() {
            self.ring_batches.push(OrderedRingBatch {
                instances: draw.rings.clone(),
                order: *self.order,
            });
            self.commands
                .push(UiDrawCommand::Ring(self.ring_batches.len() - 1));
            *self.order += 1;
        }
        if !draw.texts.is_empty() {
            self.text_orders
                .extend(std::iter::repeat_n(*self.order, draw.texts.len()));
            self.texts.extend_from_slice(&draw.texts);
            let batch_index = self.text_batches.len();
            self.text_batches.push(OrderedTextBatch {
                range: self.texts.len() - draw.texts.len()..self.texts.len(),
            });
            self.commands.push(UiDrawCommand::Text(batch_index));
            *self.order += 1;
        }
    }
}

pub(super) fn instance_writes_depth(instance: &UiInstance) -> bool {
    instance.color[3] >= 1.0
}
