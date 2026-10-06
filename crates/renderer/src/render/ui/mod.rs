// UI render pass: hand-rolled instanced quad / 9-slice pipeline for panels and
// images. One instance per panel/image carries (rect, UV rect, color, 9-slice
// margin, painter depth); the vertex stage expands each instance into 9 regions.
// All wgpu lives here per renderer-owns-GPU. Shaped text is glyphon's own
// pipeline, owned by the `text` submodule and recorded into this same pass after
// the quads with matching painter depths.
// See: context/lib/ui.md

use crate::render::uploads::UploadQueue;

use bytemuck::{Pod, Zeroable};
use wgpu::util::DeviceExt;

use postretro_scripting_core::data_descriptors::PresentationTemplate;
use postretro_ui::UiTexture;
use postretro_ui::text::FontSystem;

use self::text::{TextPrepareInput, UiTextRenderer};

/// glyphon shaped-text half of the pass: embedded font, glyph atlas/renderers,
/// and the shape→prepare→render→trim cycle. Text spans record into this same
/// render pass at their retained paint-stream positions.
pub(crate) mod text;

mod composition;
mod image_registry;
mod pipelines;
mod retained_layers;

pub(crate) use composition::UiComposition;
use composition::*;
pub(crate) use image_registry::UiImageRegistry;
use retained_layers::{PresentationLayout, RetainedGameplayTree};

pub(crate) use postretro_ui::{
    UiDrawList, UiInstance, UiReadSnapshot, UiRingInstance, UiText, UiUniform, descriptor, layout,
    theme, tree,
};
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

#[cfg(test)]
mod ring_composition_test;

const UI_QUAD_WGSL: &str = include_str!("../../shaders/ui_quad.wgsl");
const UI_RING_WGSL: &str = include_str!("../../shaders/ui_ring.wgsl");

/// 9 regions * 2 triangles * 3 vertices. The vertex shader keys off
/// `vertex_index` to expand one instance into the 9-slice geometry; total is
/// 9 regions × `VERTS_PER_REGION` (= 6u) in `ui_quad.wgsl` = 54.
const VERTS_PER_INSTANCE: u32 = 54;
/// One bounding quad (two triangles) per SDF ring instance.
const RING_VERTS_PER_INSTANCE: u32 = 6;

/// Initial instance-buffer capacity (records). Grows on demand in `encode`.
const INITIAL_INSTANCE_CAPACITY: usize = 64;
const INSTANCE_SIZE: usize = std::mem::size_of::<GpuUiInstance>();
const RING_INSTANCE_SIZE: usize = std::mem::size_of::<GpuUiRingInstance>();
const UI_DEPTH_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Depth24Plus;

/// Renderer-local instance layout. CPU UI draw lists stay GPU-free and carry no
/// painter depth; composition assigns depth as it uploads each batch.
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct GpuUiInstance {
    rect: [f32; 4],
    uv_rect: [f32; 4],
    color: [f32; 4],
    margin: [f32; 4],
    depth: f32,
}

impl GpuUiInstance {
    fn from_ui(instance: &UiInstance, depth: f32) -> Self {
        Self {
            rect: instance.rect,
            uv_rect: instance.uv_rect,
            color: instance.color,
            margin: instance.margin,
            depth,
        }
    }
}

/// Renderer-local upload mirror for [`UiRingInstance`]. CPU records retain their
/// 48-byte shape-only ABI; this mirror appends the renderer-owned painter depth.
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct GpuUiRingInstance {
    rect: [f32; 4],
    color: [f32; 4],
    radius: f32,
    thickness: f32,
    start_angle: f32,
    sweep: f32,
    depth: f32,
}

impl GpuUiRingInstance {
    fn from_ui(instance: &UiRingInstance, depth: f32) -> Self {
        Self {
            rect: instance.rect,
            color: instance.color,
            radius: instance.radius,
            thickness: instance.thickness,
            start_angle: instance.start_angle,
            sweep: instance.sweep,
            depth,
        }
    }
}

/// Instanced quad / 9-slice pass for panels and images. Owns its pipeline, BGL,
/// sampler, uniform buffer, instance buffer, and a 1×1 white texture so solid
/// panels and textured images share one instanced path. Uses a private UI depth
/// target so glyphon's text draw can share the pass while opaque upper-layer
/// quads still hard-occlude lower-layer text.
pub(crate) struct UiPass {
    opaque_pipeline: wgpu::RenderPipeline,
    translucent_pipeline: wgpu::RenderPipeline,
    ring_pipeline: wgpu::RenderPipeline,
    uniform_buffer: wgpu::Buffer,
    instance_buffer: wgpu::Buffer,
    instance_capacity: usize,
    ring_instance_buffer: wgpu::Buffer,
    ring_instance_capacity: usize,
    ring_bind_group: wgpu::BindGroup,
    /// 1×1 white texel bound for solid panels (degenerate UV slice). An
    /// untextured panel and a textured image then share one instanced batch.
    /// Held to keep the view alive for `white_bind_group`, which references it.
    #[allow(dead_code)]
    white_view: wgpu::TextureView,
    /// Bind group for the white-texel batch (panels). Rebuilt only if the
    /// uniform buffer changes, which it never does after construction.
    white_bind_group: wgpu::BindGroup,

    /// glyphon shaped-text half of the pass. Owns its pipeline, atlas, and
    /// per-span draw recorders. See `text`.
    text: UiTextRenderer,

    /// Private depth target for the UI pass. It is cleared every encode and only
    /// exists to preserve painter depth across mixed UI commands.
    depth_texture: Option<wgpu::Texture>,
    depth_view: Option<wgpu::TextureView>,
    depth_size: [u32; 2],

    /// Per-stack-layer retained gameplay trees, held across frames so each
    /// layer's dirty-gate and bound-value diff pay off (a fresh tree is always
    /// dirty). One entry per modal-stack layer, indexed bottom→top to match the
    /// snapshot's `trees`; empty until the first gameplay frame installs a layer.
    /// The boot splash deliberately does NOT use this; it renders through
    /// `BootSplashPass`, outside gameplay UI and the retained tree stack.
    gameplay_trees: Vec<RetainedGameplayTree>,

    /// Per-live-instance layout state for passive world presentations. This is
    /// intentionally separate from `gameplay_trees`: it retains only taffy
    /// measurement/tween state, never a modal tree, focus list, or input state.
    presentation_layouts: std::collections::HashMap<u64, PresentationLayout>,
    /// Reusable translated aggregate swapped into the frame's layer list and
    /// returned after the single composition encode.
    presentation_draw: tree::UiDrawData,
    /// Monotonic mark used to prune layouts absent from the current input set
    /// without allocating a second set of active instance ids each frame.
    presentation_layout_generation: u64,
    /// Registered manifest data keyed by its stable handle. This is intentionally
    /// a renderer-side layout registry, not a UI tree/modal registry.
    presentation_templates: std::collections::HashMap<
        postretro_entities::PresentationTemplateHandle,
        PresentationTemplate,
    >,
    /// Unknown handles can arrive from future producers. Warn once and leave
    /// them invisible instead of failing a render frame.
    warned_missing_presentation_templates:
        std::collections::HashSet<postretro_entities::PresentationTemplateHandle>,
}

impl UiPass {
    pub fn new(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        color_format: wgpu::TextureFormat,
    ) -> Self {
        let bind_group_layout = pipelines::create_quad_bind_group_layout(device);
        let ring_bind_group_layout = pipelines::create_ring_bind_group_layout(device);

        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("UI Quad Shader"),
            source: wgpu::ShaderSource::Wgsl(UI_QUAD_WGSL.into()),
        });

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("UI Quad Pipeline Layout"),
            bind_group_layouts: &[Some(&bind_group_layout)],
            immediate_size: 0,
        });

        let opaque_pipeline = pipelines::create_ui_quad_pipeline(
            device,
            &pipeline_layout,
            &shader,
            color_format,
            true,
            "UI Quad Pipeline",
        );
        let translucent_pipeline = pipelines::create_ui_quad_pipeline(
            device,
            &pipeline_layout,
            &shader,
            color_format,
            false,
            "UI Quad Translucent Pipeline",
        );

        let ring_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("UI Ring Shader"),
            source: wgpu::ShaderSource::Wgsl(UI_RING_WGSL.into()),
        });
        let ring_pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("UI Ring Pipeline Layout"),
            bind_group_layouts: &[Some(&ring_bind_group_layout)],
            immediate_size: 0,
        });
        let ring_pipeline = pipelines::create_ui_ring_pipeline(
            device,
            &ring_pipeline_layout,
            &ring_shader,
            color_format,
        );

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
        let ring_instance_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("UI Ring Instance Buffer"),
            size: (INITIAL_INSTANCE_CAPACITY * RING_INSTANCE_SIZE) as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let ring_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("UI Ring Bind Group"),
            layout: &ring_bind_group_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: uniform_buffer.as_entire_binding(),
            }],
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
        let text = UiTextRenderer::new(
            device,
            queue,
            color_format,
            pipelines::ui_depth_stencil_state(false),
        );

        Self {
            opaque_pipeline,
            translucent_pipeline,
            ring_pipeline,
            uniform_buffer,
            instance_buffer,
            instance_capacity: INITIAL_INSTANCE_CAPACITY,
            ring_instance_buffer,
            ring_instance_capacity: INITIAL_INSTANCE_CAPACITY,
            ring_bind_group,
            white_view,
            white_bind_group,
            text,
            depth_texture: None,
            depth_view: None,
            depth_size: [0, 0],
            gameplay_trees: Vec::new(),
            presentation_layouts: std::collections::HashMap::new(),
            presentation_draw: tree::UiDrawData::default(),
            presentation_layout_generation: 0,
            presentation_templates: std::collections::HashMap::new(),
            warned_missing_presentation_templates: std::collections::HashSet::new(),
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

    /// Mark the command buffer containing the UI encode as submitted. The debug
    /// text guard resets here, not at `encode` entry, so two UI encodes recorded
    /// before one submit still count as two prepare phases and trip the guard.
    pub fn mark_submitted(&mut self) {
        self.text.reset_prepare_guard();
    }

    /// Record a whole-frame `UiComposition` (every modal-stack layer's quad
    /// batches + text runs, in painter order) into `view`. The encode boundary is
    /// the COMPOSITION, not one layer — a caller cannot loop `encode` per layer, so
    /// the historical cross-layer glyphon vertex-buffer clobber is unrepresentable
    /// here. See `UiComposition` for the one coordinated prepare phase and
    /// disjoint per-command GPU-buffer invariant.
    ///
    /// Single color target plus a private UI depth target; the caller's `load` op
    /// controls whether the color target (the UI layer) is cleared first. The depth target is
    /// always cleared. `load` rides alongside `&UiComposition` because
    /// clear-vs-load is a target concern, not a composition one.
    ///
    /// Quad, image, ring, and text batches record in their mixed paint-stream
    /// order. Consecutive text runs remain batched, while shapes split text into
    /// independently prepared spans so translucent source-over blending follows
    /// the authored order. Every glyphon atlas upload + CPU layout runs BEFORE
    /// the pass opens (it needs `device`/`queue`, not the pass). With no UI draws
    /// the pass still opens so the caller's `load` op lands.
    // Wide by necessity: the GPU handles (device/queue/encoder/view), the
    // viewport, the target's `load` op, and the whole-frame `UiComposition` are
    // all distinct encode inputs; bundling them into a builder would obscure the
    // single-pass contract for gameplay UI composition.
    #[allow(clippy::too_many_arguments)]
    pub fn encode(
        &mut self,
        font_system: &mut FontSystem,
        device: &wgpu::Device,
        queue: &UploadQueue,
        encoder: &mut wgpu::CommandEncoder,
        view: &wgpu::TextureView,
        viewport: [u32; 2],
        load: wgpu::LoadOp<wgpu::Color>,
        composition: &UiComposition<'_>,
    ) {
        // Keep ordered batch/text slices internal to the pass. The public
        // boundary takes the whole composition so caller-side per-layer encode
        // loops stay unrepresentable.
        let batches: &[OrderedUiBatch<'_>] = &composition.batches;
        let ring_batches: &[OrderedRingBatch] = &composition.ring_batches;
        let texts: &[UiText] = &composition.texts;

        queue.write_buffer(
            &self.uniform_buffer,
            0,
            bytemuck::bytes_of(&UiUniform {
                viewport: [viewport[0] as f32, viewport[1] as f32],
                _pad: [0.0, 0.0],
            }),
        );

        // Give each batch its OWN region of the instance buffer, sized to the
        // SUM of all batch instance counts. The deferred upload batch lands
        // every write (last-wins per region) before the submitted draws execute.
        // Writing each batch to offset 0 would have every draw read the LAST
        // batch's data — recording a draw between writes does not snapshot the
        // buffer, since the writes resolve on the queue timeline, not the
        // command-recording timeline. Disjoint per-batch regions sidestep this.
        let total_instances: usize = batches.iter().map(|b| b.instances.len()).sum();
        if total_instances > self.instance_capacity {
            self.grow_instance_buffer(device, total_instances);
        }
        let total_ring_instances: usize = ring_batches.iter().map(|b| b.instances.len()).sum();
        if total_ring_instances > self.ring_instance_capacity {
            self.grow_ring_instance_buffer(device, total_ring_instances);
        }

        // --- Shape + prepare text BEFORE the pass opens --------------------
        // glyphon shapes each line into a `Buffer`, then `prepare` does CPU
        // layout + atlas upload. Both must complete before `begin_render_pass`;
        // the `render` call below only records draw commands. The buffers must
        // outlive `prepare` (the `TextArea`s borrow them), so they live in this
        // `Vec` for the duration of `encode`. Empty `texts` => no text work.
        let text_buffers = self.text.shape_text(font_system, texts, viewport);
        let text_depths: Vec<f32> = composition
            .text_orders
            .iter()
            .map(|&order| painter_depth(order, composition.order_count))
            .collect();
        let text_ranges: Vec<std::ops::Range<usize>> = composition
            .text_batches
            .iter()
            .map(|batch| batch.range.clone())
            .collect();
        let prepared_text_batches = self.text.prepare_text_batches(
            font_system,
            device,
            queue.raw(),
            TextPrepareInput {
                viewport,
                texts,
                buffers: &text_buffers,
                depths: &text_depths,
            },
            &text_ranges,
        );

        self.ensure_depth_target(device, viewport);
        let depth_view = self
            .depth_view
            .as_ref()
            .expect("UI depth target created before render pass");

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
            depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                view: depth_view,
                depth_ops: Some(wgpu::Operations {
                    load: wgpu::LoadOp::Clear(1.0),
                    store: wgpu::StoreOp::Discard,
                }),
                stencil_ops: None,
            }),
            timestamp_writes: None,
            ..Default::default()
        });

        // Each shape batch concatenates into its own type-specific buffer region.
        // Commands then bind those disjoint regions in the retained mixed paint
        // order, without relying on a non-zero `first_instance`.
        let mut offset = 0u64;
        let mut ring_offset = 0u64;
        for command in &composition.commands {
            match *command {
                UiDrawCommand::Quad(batch_index) => {
                    let ordered = &batches[batch_index];
                    if ordered.instances.is_empty() {
                        continue;
                    }
                    let depth = painter_depth(ordered.order, composition.order_count);
                    let upload: Vec<GpuUiInstance> = ordered
                        .instances
                        .iter()
                        .map(|instance| GpuUiInstance::from_ui(instance, depth))
                        .collect();
                    let bytes: &[u8] = bytemuck::cast_slice(&upload);
                    queue.write_buffer(&self.instance_buffer, offset, bytes);
                    let pipeline = if ordered.writes_depth {
                        &self.opaque_pipeline
                    } else {
                        &self.translucent_pipeline
                    };
                    pass.set_pipeline(pipeline);
                    pass.set_bind_group(0, ordered.bind_group, &[]);
                    pass.set_vertex_buffer(0, self.instance_buffer.slice(offset..));
                    pass.draw(0..VERTS_PER_INSTANCE, 0..ordered.instances.len() as u32);
                    offset += bytes.len() as u64;
                }
                UiDrawCommand::Ring(batch_index) => {
                    let ordered = &ring_batches[batch_index];
                    if ordered.instances.is_empty() {
                        continue;
                    }
                    let depth = painter_depth(ordered.order, composition.order_count);
                    let upload: Vec<GpuUiRingInstance> = ordered
                        .instances
                        .iter()
                        .map(|instance| GpuUiRingInstance::from_ui(instance, depth))
                        .collect();
                    let bytes: &[u8] = bytemuck::cast_slice(&upload);
                    queue.write_buffer(&self.ring_instance_buffer, ring_offset, bytes);
                    pass.set_pipeline(&self.ring_pipeline);
                    pass.set_bind_group(0, &self.ring_bind_group, &[]);
                    pass.set_vertex_buffer(0, self.ring_instance_buffer.slice(ring_offset..));
                    pass.draw(
                        0..RING_VERTS_PER_INSTANCE,
                        0..ordered.instances.len() as u32,
                    );
                    ring_offset += bytes.len() as u64;
                }
                UiDrawCommand::Text(batch_index) => {
                    if prepared_text_batches
                        .get(batch_index)
                        .copied()
                        .unwrap_or(false)
                    {
                        self.text.render_batch(batch_index, &mut pass);
                    }
                }
            }
        }

        // Drop the pass (ends its borrow of `self.text`) before trimming, since
        // `trim` needs `&mut self.text`.
        drop(pass);

        // Reclaim atlas space for glyphs the last `prepare` did not touch — one
        // trim per frame, after the draw is recorded, per glyphon's guidance.
        self.text.trim();
    }

    fn ensure_depth_target(&mut self, device: &wgpu::Device, viewport: [u32; 2]) {
        let size = [viewport[0].max(1), viewport[1].max(1)];
        if self.depth_view.is_some() && self.depth_size == size {
            return;
        }

        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("UI Depth Texture"),
            size: wgpu::Extent3d {
                width: size[0],
                height: size[1],
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: UI_DEPTH_FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        });
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        self.depth_texture = Some(texture);
        self.depth_view = Some(view);
        self.depth_size = size;
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

    fn grow_ring_instance_buffer(&mut self, device: &wgpu::Device, needed: usize) {
        let mut capacity = self.ring_instance_capacity.max(1);
        while capacity < needed {
            capacity *= 2;
        }
        self.ring_instance_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("UI Ring Instance Buffer"),
            size: (capacity * RING_INSTANCE_SIZE) as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        self.ring_instance_capacity = capacity;
    }
}

fn painter_depth(order: usize, order_count: usize) -> f32 {
    if order_count == 0 {
        return 0.0;
    }
    1.0 - ((order as f32 + 1.0) / (order_count as f32 + 1.0))
}

/// Ring thickness in device pixels (before viewport scale is folded into the
/// rect math). A thin 2px outline reads as a focus ring without obscuring content.
const FOCUS_RING_THICKNESS: f32 = 2.0;

/// Append a focus-ring outline (four thin bars) around `rect` (device px
/// `[x, y, w, h]`) to `draw`. The ring sits `inset` device px OUTSIDE the rect
/// (the `xs` spacing token, scaled), framing the focused node without overlapping
/// it. `color` is the resolved `focus.ring` token (linear RGBA). Drawn as four
/// solid `UiInstance::panel` bars (top, bottom, left, right) so it needs no new
/// pipeline. The focused id rides the snapshot, so the ring may trail a focus
/// change by one frame.
pub(crate) fn push_focus_ring(
    draw: &mut tree::UiDrawData,
    rect: [f32; 4],
    inset: f32,
    color: [f32; 4],
) {
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
    draw.push_quad(bar([ox, oy, ow, t]));
    draw.push_quad(bar([ox, oy + oh - t, ow, t]));
    draw.push_quad(bar([ox, oy + t, t, (oh - 2.0 * t).max(0.0)]));
    draw.push_quad(bar([ox + ow - t, oy + t, t, (oh - 2.0 * t).max(0.0)]));
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

    #[test]
    fn ui_ring_wgsl_parses_and_validates() {
        let module =
            naga::front::wgsl::parse_str(UI_RING_WGSL).expect("ui_ring.wgsl should parse as WGSL");
        let has_vs = module
            .entry_points
            .iter()
            .any(|ep| ep.name == "vs_main" && ep.stage == naga::ShaderStage::Vertex);
        let has_fs = module
            .entry_points
            .iter()
            .any(|ep| ep.name == "fs_main" && ep.stage == naga::ShaderStage::Fragment);
        assert!(has_vs, "ui_ring.wgsl must export @vertex vs_main");
        assert!(has_fs, "ui_ring.wgsl must export @fragment fs_main");
        naga::valid::Validator::new(
            naga::valid::ValidationFlags::all(),
            naga::valid::Capabilities::all(),
        )
        .validate(&module)
        .expect("ui_ring.wgsl must pass naga validation");
    }

    #[test]
    fn image_registry_exposes_registered_natural_sizes_to_layout() {
        let mut registry = UiImageRegistry::default();
        assert_eq!(registry.image_sizes_generation(), 0);

        registry.register_size_for_test("ui/icon", [32, 16]);

        assert_eq!(
            registry.image_sizes().get("ui/icon").copied(),
            Some([32.0, 16.0]),
            "layout must receive natural image sizes from the renderer registry"
        );
        assert_eq!(registry.image_sizes_generation(), 1);

        registry.register_size_for_test("ui/icon", [32, 16]);
        assert_eq!(
            registry.image_sizes_generation(),
            1,
            "re-registering the same size must not invalidate retained layout"
        );
    }

    #[test]
    fn focus_ring_appends_after_existing_content_in_paint_order() {
        let mut draw = tree::UiDrawData::default();
        draw.push_image("ui/icon", UiInstance::image([10.0, 10.0, 16.0, 16.0]));

        push_focus_ring(
            &mut draw,
            [10.0, 10.0, 16.0, 16.0],
            2.0,
            [1.0, 1.0, 0.0, 1.0],
        );

        assert_eq!(draw.quads.len(), 4, "focus ring emits four quad bars");
        assert_eq!(draw.paint_order.len(), 5);
        assert!(
            matches!(draw.paint_order[0], tree::UiPaintOp::Image { .. }),
            "focused image remains first in painter order",
        );
        assert!(
            draw.paint_order[1..]
                .iter()
                .all(|op| matches!(op, tree::UiPaintOp::Quad { .. })),
            "focus ring quads append after focused content",
        );
    }
}
