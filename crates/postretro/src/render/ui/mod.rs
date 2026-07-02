// UI render pass: hand-rolled instanced quad / 9-slice pipeline for panels and
// images. One instance per panel/image carries (rect, UV rect, color, 9-slice
// margin); the vertex stage expands each instance into 9 regions. All wgpu lives
// here per renderer-owns-GPU. Shaped text is glyphon's own pipeline, owned by
// the `text` submodule and recorded into this same pass after the quads.
// See: context/lib/ui.md

use wgpu::util::DeviceExt;

use postretro_ui::UiTexture;
use postretro_ui::text::FontSystem;

use self::text::UiTextRenderer;

/// glyphon shaped-text half of the pass: embedded font, glyph atlas/renderer,
/// and the shape→prepare→render→trim cycle. glyphon owns its own pipeline; the
/// text draw records into this same render pass, after the quads.
pub(crate) mod text;

#[cfg_attr(not(test), allow(unused_imports))]
pub(crate) use postretro_ui::{
    UiDrawList, UiInstance, UiReadSnapshot, UiText, UiTreeEntry, UiUniform,
};
#[cfg_attr(not(test), allow(unused_imports))]
pub(crate) use postretro_ui::{demo, descriptor, layout, modal_stack, theme, tree};

/// Shared headless GPU harness for the UI offscreen golden tests: the
/// `pollster` device init (self-skip on no adapter) and the offscreen-texture
/// readback. Used by `multi_batch_test` and `multi_layer_text_golden_test`.
/// See `testing_guide.md` §3/§4.
#[cfg(test)]
mod gpu_test_harness;

/// Headless regression for the multi-batch instance-buffer clobber: encodes two
/// non-empty batches into disjoint screen regions and asserts each region keeps
/// its own batch's color. Self-skips when no GPU adapter is present.
#[cfg(test)]
mod multi_batch_test;

/// Headless safety net for the multi-LAYER text compositing path: renders two
/// stacked retained-tree layers (distinct text per layer at disjoint positions)
/// into one offscreen target through a SINGLE `UiComposition` encode and asserts
/// each layer keeps its own text. Proves the historical per-layer encode loop
/// (two glyphon `prepare`s on the shared vertex buffer) clobbered the lower
/// layer — coverage `cargo test` otherwise can't see. Self-skips with no GPU
/// adapter.
#[cfg(test)]
mod multi_layer_text_golden_test;

/// G1b cross-cutting lifecycle + render suite: the
/// register -> resolve-by-name -> render chain over the production path, the
/// always-on compose -> render path, a mod theme override reaching a rendered
/// widget, a runtime-registered font usable by a `text` token, and `localState`
/// on a mixed store-bound + local-bound tree. Pure CPU — no GPU adapter.
#[cfg(test)]
mod lifecycle_render_test;

const UI_QUAD_WGSL: &str = include_str!("../../shaders/ui_quad.wgsl");

/// 9 regions * 2 triangles * 3 vertices. The vertex shader keys off
/// `vertex_index` to expand one instance into the 9-slice geometry; total is
/// 9 regions × `VERTS_PER_REGION` (= 6u) in `ui_quad.wgsl` = 54.
const VERTS_PER_INSTANCE: u32 = 54;

/// Small key→bind-group registry for `image` widget assets. The descriptor's
/// `image` nodes reference a texture by string key; the renderer pre-registers
/// the known keys and resolves each image batch's key through this map to the
/// bind group the draw binds.
///
/// Only pre-registered keys resolve — dynamic asset streaming is out of scope.
/// Only the splash logo key is pre-registered; the current demo gameplay HUD has
/// no `image` nodes. An unknown key is skipped-with-warn at draw time — the
/// image batch simply does not draw, and a single warning names the missing key.
/// Each entry owns its texture so the bind group's view stays valid for the
/// registry's lifetime.
#[derive(Default)]
pub(crate) struct UiImageRegistry {
    entries: std::collections::HashMap<String, UiImageEntry>,
}

struct UiImageEntry {
    /// Kept alive so the bind group's texture view stays valid.
    _texture: wgpu::Texture,
    bind_group: wgpu::BindGroup,
}

impl UiImageRegistry {
    /// Resolve `key` to its bind group, or `None` if no such key is registered.
    /// The live read side: `UiComposition::from_layer_draws` resolves each
    /// gameplay image batch's asset key through here. No production writer exists
    /// yet — the boot splash owns its logo in `BootSplashPass`, and the demo HUD
    /// has no `image` nodes — so the registry currently resolves nothing; the
    /// writer lands with the first gameplay image node.
    pub fn resolve(&self, key: &str) -> Option<&wgpu::BindGroup> {
        self.entries.get(key).map(|e| &e.bind_group)
    }
}

/// Initial instance-buffer capacity (records). Grows on demand in `encode`.
const INITIAL_INSTANCE_CAPACITY: usize = 64;
const INSTANCE_SIZE: usize = std::mem::size_of::<UiInstance>();

/// Instanced quad / 9-slice pass for panels and images. Owns its pipeline, BGL,
/// sampler, uniform buffer, instance buffer, and a 1×1 white texture so solid
/// panels and textured images share one instanced batch. Designed for a single
/// color target with no depth attachment so glyphon's text draw can share the pass.
pub(crate) struct UiPass {
    pipeline: wgpu::RenderPipeline,
    uniform_buffer: wgpu::Buffer,
    instance_buffer: wgpu::Buffer,
    instance_capacity: usize,
    /// 1×1 white texel bound for solid panels (degenerate UV slice). An
    /// untextured panel and a textured image then share one instanced batch.
    /// Held to keep the view alive for `white_bind_group`, which references it.
    #[allow(dead_code)]
    white_view: wgpu::TextureView,
    /// Bind group for the white-texel batch (panels). Rebuilt only if the
    /// uniform buffer changes, which it never does after construction.
    white_bind_group: wgpu::BindGroup,

    /// glyphon shaped-text half of the pass. Owns its own pipeline/atlas; its
    /// draw records into this same render pass, after the quads. See `text`.
    text: UiTextRenderer,

    /// Per-stack-layer retained gameplay trees, held across frames so each
    /// layer's dirty-gate and bound-value diff pay off (a fresh tree is always
    /// dirty). One entry per modal-stack layer, indexed bottom→top to match the
    /// snapshot's `trees`; empty until the first gameplay frame installs a layer.
    /// The boot splash deliberately does NOT use this; it renders through
    /// `BootSplashPass`, outside gameplay UI and the retained tree stack.
    gameplay_trees: Vec<RetainedGameplayTree>,
}

/// One retained gameplay UI tree plus the descriptor it was built from. The
/// descriptor is kept so the next frame can detect a structural change (the
/// snapshot delivered a different tree) by `!=` comparison — `AnchoredTree`
/// derives `PartialEq` — and rebuild only then. The cached draw list lives inside
/// the `UiTree` itself (see `UiTree::cached_draw_data`).
struct RetainedGameplayTree {
    /// The descriptor the retained `tree` was built from. Compared against the
    /// incoming descriptor each frame; a difference forces a rebuild.
    descriptor: descriptor::AnchoredTree,
    /// The renderer's UI theme generation the retained `tree` was built (and so
    /// token-resolved) against. A bump (the engine installed an override theme)
    /// invalidates the resolved colors/spacing/fonts baked into the tree, so the
    /// gate rebuilds it even when the descriptor is byte-for-byte identical.
    theme_generation: u64,
    /// The retained taffy-backed tree, carrying its layout cache, last viewport,
    /// per-bound-node last-resolved values, and cached draw list across frames.
    tree: tree::UiTree,
}

/// One instanced draw: a draw list plus the bind group for its bound texture.
/// Panels use the pass's white-texel bind group; images bind their own.
pub(crate) struct UiBatch<'a> {
    pub list: &'a UiDrawList,
    pub bind_group: &'a wgpu::BindGroup,
}

/// The whole frame's UI composition: every modal-stack layer's quad batches and
/// shaped-text runs, concatenated in bottom→top painter order, as the single unit
/// `UiPass::encode` records. The encode boundary is the WHOLE composition, never
/// one layer — making the historical per-layer encode loop (which clobbered the
/// shared glyphon vertex buffer across layers; see `UiPass::encode`'s disjoint-
/// region comment for the sibling quad-path rule) unrepresentable on the
/// production surface.
///
/// **Invariant — one `prepare`/vertex-buffer fill per surface composition.** All
/// layers funnel through ONE `encode`, so glyphon's `prepare` (which overwrites
/// its single internal vertex buffer at offset 0) runs once per composed frame.
/// The text path obeys the same "one fill per composition" rule the quad path
/// already enforces by giving each batch a disjoint instance-buffer region.
///
/// Borrows the raw quad data (`batches` hold `UiBatch<'a>`, each borrowing a
/// `&UiDrawList` + bind group, zero quad copy); owns the concatenated text runs
/// (`texts: Vec<UiText>`). Built in the caller's frame scope so the borrows
/// coexist with the `&mut self.ui` encode call. Two constructors:
/// `from_layer_draws` (gameplay modal stack) and `from_batches` (the standalone
/// splash assembly).
pub(crate) struct UiComposition<'a> {
    batches: Vec<UiBatch<'a>>,
    texts: Vec<UiText>,
}

impl<'a> UiComposition<'a> {
    /// Gameplay constructor: fold the per-layer `UiDrawData` slice (bottom→top)
    /// into one composition. Each layer contributes, in order, its non-empty panel
    /// quads (bound to `white_bind_group`), then each non-empty image batch (its
    /// `asset` key resolved through `images` to a bind group; an unregistered key
    /// is skipped-with-debug-log), then its text runs. This is the painter order
    /// the prior per-layer loop produced, now in a single composed unit.
    ///
    /// `white_bind_group` and `images` outlive the returned composition (they are
    /// the pass's own resources); `layer_draws` is the caller's frame-scoped fold
    /// output. All three borrows back the `'a` lifetime.
    pub fn from_layer_draws(
        layer_draws: &'a [tree::UiDrawData],
        white_bind_group: &'a wgpu::BindGroup,
        images: &'a UiImageRegistry,
    ) -> Self {
        let mut batches: Vec<UiBatch<'a>> = Vec::new();
        let mut texts: Vec<UiText> = Vec::new();
        for draw in layer_draws {
            if !draw.quads.is_empty() {
                batches.push(UiBatch {
                    list: &draw.quads,
                    bind_group: white_bind_group,
                });
            }
            // Unknown key degrades by skipping just that batch. Logged at debug,
            // not warn: this gameplay path runs every frame with no dedup, so a
            // persistently-missing key would spam at warn (development_guide §6.1).
            for (asset, list) in &draw.images {
                if list.is_empty() {
                    continue;
                }
                match images.resolve(asset) {
                    Some(bind_group) => batches.push(UiBatch { list, bind_group }),
                    None => log::debug!(
                        "[Renderer] UI image asset key '{asset}' is not registered — skipping its draw"
                    ),
                }
            }
            texts.extend_from_slice(&draw.texts);
        }
        Self { batches, texts }
    }

    /// Constructor from already-assembled batches and text — a single-layer
    /// composition that does not fold a `UiDrawData` stack. Now only the
    /// multi-batch headless regression uses it directly (the boot splash moved
    /// off the UI pass); kept for that test's disjoint-region coverage.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn from_batches(batches: Vec<UiBatch<'a>>, texts: Vec<UiText>) -> Self {
        Self { batches, texts }
    }

    /// `true` when the composition records nothing — no quad batches and no text.
    /// The gameplay path early-outs the UI pass on this.
    pub fn is_empty(&self) -> bool {
        self.batches.is_empty() && self.texts.is_empty()
    }
}

impl UiPass {
    pub fn new(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        surface_format: wgpu::TextureFormat,
    ) -> Self {
        let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("UI Quad BGL"),
            entries: &[
                // 0: UiUniform (device viewport), read in the vertex stage.
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: wgpu::BufferSize::new(
                            std::mem::size_of::<UiUniform>() as u64
                        ),
                    },
                    count: None,
                },
                // 1: bound texture (white texel for panels, image for logos).
                // Float-filterable so the same BGL works for linear sampling.
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                // 2: filtering sampler. Must be Filtering to pair with the
                // Float { filterable: true } texture binding above.
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });

        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("UI Quad Shader"),
            source: wgpu::ShaderSource::Wgsl(UI_QUAD_WGSL.into()),
        });

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("UI Quad Pipeline Layout"),
            bind_group_layouts: &[Some(&bind_group_layout)],
            immediate_size: 0,
        });

        // Per-instance vertex buffer: the four vec4 attributes of `UiInstance`.
        // No per-vertex buffer — geometry is generated from `vertex_index`.
        let instance_layout = wgpu::VertexBufferLayout {
            array_stride: INSTANCE_SIZE as u64,
            step_mode: wgpu::VertexStepMode::Instance,
            attributes: &[
                wgpu::VertexAttribute {
                    format: wgpu::VertexFormat::Float32x4,
                    offset: 0,
                    shader_location: 0,
                },
                wgpu::VertexAttribute {
                    format: wgpu::VertexFormat::Float32x4,
                    offset: 16,
                    shader_location: 1,
                },
                wgpu::VertexAttribute {
                    format: wgpu::VertexFormat::Float32x4,
                    offset: 32,
                    shader_location: 2,
                },
                wgpu::VertexAttribute {
                    format: wgpu::VertexFormat::Float32x4,
                    offset: 48,
                    shader_location: 3,
                },
            ],
        };

        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("UI Quad Pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                buffers: &[instance_layout],
                compilation_options: wgpu::PipelineCompilationOptions::default(),
            },
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                ..Default::default()
            },
            // Depth disabled: the UI pass attaches no depth target, so glyphon's
            // text draw can share this single-color-target configuration.
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_main"),
                targets: &[Some(wgpu::ColorTargetState {
                    format: surface_format,
                    // Standard alpha blend over the existing surface contents.
                    blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: wgpu::PipelineCompilationOptions::default(),
            }),
            multiview_mask: None,
            cache: None,
        });

        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("UI Quad Sampler"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Nearest,
            ..Default::default()
        });

        let uniform_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("UI Quad Uniform"),
            contents: bytemuck::bytes_of(&UiUniform {
                viewport: [1.0, 1.0],
                _pad: [0.0, 0.0],
            }),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });

        let instance_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("UI Quad Instance Buffer"),
            size: (INITIAL_INSTANCE_CAPACITY * INSTANCE_SIZE) as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        // 1×1 white texel: solid panels sample this so they share the image
        // batch's pipeline. White encodes to white under sRGB, so the tint color
        // passes through untouched. Uploaded as a standard UI RGBA8 texture.
        let white = UiTexture {
            data: vec![255, 255, 255, 255],
            width: 1,
            height: 1,
        };
        let white_view = upload_ui_texture(device, queue, &white).create_view(&Default::default());

        let white_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("UI White Panel Bind Group"),
            layout: &bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: uniform_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&white_view),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::Sampler(&sampler),
                },
            ],
        });

        // glyphon shaped-text state — its own pipeline/atlas, constructed here
        // so `TextAtlas` builds in `Renderer::new` rather than on the first
        // shaped frame. The CPU `FontSystem` is session-owned.
        let text = UiTextRenderer::new(device, queue, surface_format);

        Self {
            pipeline,
            uniform_buffer,
            instance_buffer,
            instance_capacity: INITIAL_INSTANCE_CAPACITY,
            white_view,
            white_bind_group,
            text,
            gameplay_trees: Vec::new(),
        }
    }

    /// Bind group for solid-color panels — samples the 1×1 white texel. Pass
    /// this as a `UiBatch::bind_group` for the panel batch.
    pub fn white_bind_group(&self) -> &wgpu::BindGroup {
        &self.white_bind_group
    }

    /// Install a runtime font face into the session-owned shaped-text
    /// `FontSystem` (the net-new runtime path; the embedded primary/mono faces
    /// are registered once by `postretro_ui::text::build_font_system`). Delegates
    /// to `text::UiTextRenderer::register_font`; returns `false` if the bytes
    /// register no face under `family`, so the renderer caller can surface a
    /// load-time diagnostic and skip rather than leave a `font` token resolving
    /// to a system fallback.
    pub fn register_font(
        &mut self,
        font_system: &mut FontSystem,
        family: &str,
        ttf_bytes: Vec<u8>,
    ) -> bool {
        self.text.register_font(font_system, family, ttf_bytes)
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
    /// their retained state. The splash stays on `layout_tree` (fresh build per
    /// frame) — it is transient and carries no bindings.
    ///
    /// `time_seconds` is the deterministic, dt-accumulated frame time threaded
    /// down to the retained build for the tween runtime to ease bound values over
    /// time.
    // Wide by necessity: layer + viewport + image sizes + slot values + theme +
    // theme generation + frame time are all distinct retained-build inputs;
    // bundling them into a struct would only obscure the per-frame call site.
    #[allow(clippy::too_many_arguments)]
    pub fn layout_gameplay_tree(
        &mut self,
        font_system: &mut FontSystem,
        layer: usize,
        tree: &descriptor::AnchoredTree,
        viewport: [u32; 2],
        image_sizes: &tree::ImageSizes,
        slot_values: &std::collections::HashMap<String, postretro_entities::SlotValue>,
        cell_values: &tree::CellValues,
        theme: &theme::UiTheme,
        theme_generation: u64,
        time_seconds: f64,
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
        let needs_build = match self.gameplay_trees.get(layer) {
            Some(retained) => {
                retained.descriptor != *tree || retained.theme_generation != theme_generation
            }
            None => true,
        };
        if needs_build {
            let rebuilt = RetainedGameplayTree {
                descriptor: tree.clone(),
                theme_generation,
                tree: tree::UiTree::from_descriptor(tree, theme),
            };
            if layer < self.gameplay_trees.len() {
                self.gameplay_trees[layer] = rebuilt;
            } else {
                self.gameplay_trees.push(rebuilt);
            }
        }

        let retained = &mut self.gameplay_trees[layer];
        retained.tree.build_draw_data_retained(
            viewport,
            font_system,
            image_sizes,
            slot_values,
            cell_values,
            time_seconds,
        )
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
            Some(retained) => retained.tree.export_focus_rects(
                &retained.descriptor,
                viewport,
                slot_values,
                cell_values,
            ),
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

    /// Record a whole-frame `UiComposition` (every modal-stack layer's quad
    /// batches + text runs, in painter order) into `view`. The encode boundary is
    /// the COMPOSITION, not one layer — a caller cannot loop `encode` per layer, so
    /// the historical cross-layer glyphon vertex-buffer clobber is unrepresentable
    /// here. See `UiComposition` for the "one `prepare`/vertex-buffer fill per
    /// surface composition" invariant; its text-path sibling is the disjoint
    /// per-batch instance-buffer region the quad loop below documents.
    ///
    /// Single color target, no depth; the caller's `load` op controls whether the
    /// surface is cleared first. `load` rides alongside `&UiComposition` because
    /// clear-vs-load is a target concern, not a composition one.
    ///
    /// Order matters: quads first, then text. Quad instances upload to the
    /// instance buffer and draw one instanced batch each; then glyphon's
    /// `TextRenderer::render` records its own draw INTO THE SAME render pass,
    /// AFTER the quads, so text composites over the panels/images into the same
    /// surface view. glyphon's atlas upload + CPU layout (`prepare`) runs BEFORE
    /// the pass opens (it needs `device`/`queue`, not the pass). With no quads
    /// and no text the pass still opens so the caller's `load` op lands.
    // Wide by necessity: the GPU handles (device/queue/encoder/view), the
    // viewport, the target's `load` op, and the whole-frame `UiComposition` are
    // all distinct encode inputs; bundling them into a builder would obscure the
    // single-pass contract for gameplay UI composition.
    #[allow(clippy::too_many_arguments)]
    pub fn encode(
        &mut self,
        font_system: &mut FontSystem,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        view: &wgpu::TextureView,
        viewport: [u32; 2],
        load: wgpu::LoadOp<wgpu::Color>,
        composition: &UiComposition<'_>,
    ) {
        // Keep the `&[UiBatch]`/`&[UiText]` shape internal to the pass — the public
        // boundary takes the whole composition, the quad/text loops below the
        // slices it spans.
        let batches: &[UiBatch<'_>] = &composition.batches;
        let texts: &[UiText] = &composition.texts;

        // Reset the once-per-composition prepare guard at the single per-frame
        // call site both the splash and gameplay paths funnel through. The guard
        // fires if glyphon `prepare` is reached more than once within this encoded
        // composition (a future intra-composition regression).
        self.text.reset_prepare_guard();

        queue.write_buffer(
            &self.uniform_buffer,
            0,
            bytemuck::bytes_of(&UiUniform {
                viewport: [viewport[0] as f32, viewport[1] as f32],
                _pad: [0.0, 0.0],
            }),
        );

        // Give each batch its OWN region of the instance buffer, sized to the
        // SUM of all batch instance counts. `queue.write_buffer` is a
        // queue-timeline op: every staged write lands (last-wins per region)
        // BEFORE the single submitted command buffer executes. Writing each
        // batch to offset 0 would therefore have every draw read the LAST
        // batch's data — recording a draw between writes does not snapshot the
        // buffer, since the writes resolve on the queue timeline, not the
        // command-recording timeline. Disjoint per-batch regions sidestep this.
        let total_instances: usize = batches.iter().map(|b| b.list.len()).sum();
        if total_instances > self.instance_capacity {
            self.grow_instance_buffer(device, total_instances);
        }

        // --- Shape + prepare text BEFORE the pass opens --------------------
        // glyphon shapes each line into a `Buffer`, then `prepare` does CPU
        // layout + atlas upload. Both must complete before `begin_render_pass`;
        // the `render` call below only records draw commands. The buffers must
        // outlive `prepare` (the `TextArea`s borrow them), so they live in this
        // `Vec` for the duration of `encode`. Empty `texts` => no text work.
        let text_buffers = self.text.shape_text(font_system, texts, viewport);
        let prepared =
            self.text
                .prepare_text(font_system, device, queue, viewport, texts, &text_buffers);

        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("UI Pass"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load,
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            ..Default::default()
        });

        // Quads first. Each non-empty batch concatenates into its own region:
        // batch K starts at `offset_k = (sum of prior batch lens) * INSTANCE_SIZE`.
        // The draw binds the vertex buffer from `offset_k` and uses instance
        // range `0..count_k`, so it reads its own region without relying on a
        // non-zero `first_instance`. Per-batch byte offsets are multiples of the
        // 64-byte instance stride, satisfying write_buffer/vertex-offset
        // alignment. Empty batches are skipped without consuming a region.
        pass.set_pipeline(&self.pipeline);
        let mut offset = 0u64;
        for batch in batches {
            if batch.list.is_empty() {
                continue;
            }
            let bytes: &[u8] = bytemuck::cast_slice(&batch.list.instances);
            queue.write_buffer(&self.instance_buffer, offset, bytes);
            pass.set_bind_group(0, batch.bind_group, &[]);
            pass.set_vertex_buffer(0, self.instance_buffer.slice(offset..));
            pass.draw(0..VERTS_PER_INSTANCE, 0..batch.list.len() as u32);
            offset += bytes.len() as u64;
        }

        // Then glyphon's text draw, into the same pass, after the quads. Skipped
        // when `prepare` had nothing to record (no text this frame).
        if prepared {
            self.text.render(&mut pass);
        }

        // Drop the pass (ends its borrow of `self.text`) before trimming, since
        // `trim` needs `&mut self.text`.
        drop(pass);

        // Reclaim atlas space for glyphs the last `prepare` did not touch — one
        // trim per frame, after the draw is recorded, per glyphon's guidance.
        self.text.trim();
    }

    fn grow_instance_buffer(&mut self, device: &wgpu::Device, needed: usize) {
        let mut capacity = self.instance_capacity.max(1);
        while capacity < needed {
            capacity *= 2;
        }
        self.instance_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("UI Quad Instance Buffer"),
            size: (capacity * INSTANCE_SIZE) as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        self.instance_capacity = capacity;
    }
}

/// Ring thickness in device pixels (before viewport scale is folded into the
/// rect math). A thin 2px outline reads as a focus ring without obscuring content.
const FOCUS_RING_THICKNESS: f32 = 2.0;

/// Append a focus-ring outline (four thin bars) around `rect` (device px
/// `[x, y, w, h]`) to `quads`. The ring sits `inset` device px OUTSIDE the rect
/// (the `xs` spacing token, scaled), framing the focused node without overlapping
/// it. `color` is the resolved `focus.ring` token (linear RGBA). Drawn as four
/// solid `UiInstance::panel` bars (top, bottom, left, right) so it needs no new
/// pipeline — it rides the existing white-texel quad batch. The focused id rides
/// the snapshot, so the ring may trail a focus change by one frame.
pub(crate) fn push_focus_ring(quads: &mut UiDrawList, rect: [f32; 4], inset: f32, color: [f32; 4]) {
    let t = FOCUS_RING_THICKNESS;
    // Outer frame: the focused rect grown outward by the inset.
    let ox = rect[0] - inset;
    let oy = rect[1] - inset;
    let ow = rect[2] + inset * 2.0;
    let oh = rect[3] + inset * 2.0;
    if ow <= 0.0 || oh <= 0.0 {
        return;
    }
    let bar = |r: [f32; 4]| UiInstance::panel(r, color, [0.0; 4]);
    // Top, bottom (full width), then left/right (between the horizontal bars).
    quads.push(bar([ox, oy, ow, t]));
    quads.push(bar([ox, oy + oh - t, ow, t]));
    quads.push(bar([ox, oy + t, t, (oh - 2.0 * t).max(0.0)]));
    quads.push(bar([ox + ow - t, oy + t, t, (oh - 2.0 * t).max(0.0)]));
}

/// Upload a CPU RGBA8 `UiTexture` and return the GPU texture. sRGB format so
/// image content decodes on sample (white encodes to white, so the panel texel
/// stays neutral). Kept local so the UI pass owns its own upload path. Used here
/// for the 1×1 white texel; the boot splash logo uploads through its own
/// renderer-owned `BootSplashPass::install_logo`.
fn upload_ui_texture(device: &wgpu::Device, queue: &wgpu::Queue, tex: &UiTexture) -> wgpu::Texture {
    let size = wgpu::Extent3d {
        width: tex.width,
        height: tex.height,
        depth_or_array_layers: 1,
    };
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("UI Texture"),
        size,
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8UnormSrgb,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    queue.write_texture(
        wgpu::TexelCopyTextureInfo {
            texture: &texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        &tex.data,
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(4 * tex.width),
            rows_per_image: Some(tex.height),
        },
        size,
    );
    texture
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ui_quad_wgsl_parses_and_validates() {
        let module =
            naga::front::wgsl::parse_str(UI_QUAD_WGSL).expect("ui_quad.wgsl should parse as WGSL");
        let has_vs = module
            .entry_points
            .iter()
            .any(|ep| ep.name == "vs_main" && ep.stage == naga::ShaderStage::Vertex);
        let has_fs = module
            .entry_points
            .iter()
            .any(|ep| ep.name == "fs_main" && ep.stage == naga::ShaderStage::Fragment);
        assert!(has_vs, "ui_quad.wgsl must export @vertex vs_main");
        assert!(has_fs, "ui_quad.wgsl must export @fragment fs_main");
        naga::valid::Validator::new(
            naga::valid::ValidationFlags::all(),
            naga::valid::Capabilities::all(),
        )
        .validate(&module)
        .expect("ui_quad.wgsl must pass naga validation");
    }
}
