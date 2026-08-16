// UI render pass: hand-rolled instanced quad / 9-slice pipeline for panels and
// images. One instance per panel/image carries (rect, UV rect, color, 9-slice
// margin, painter depth); the vertex stage expands each instance into 9 regions.
// All wgpu lives here per renderer-owns-GPU. Shaped text is glyphon's own
// pipeline, owned by the `text` submodule and recorded into this same pass after
// the quads with matching painter depths.
// See: context/lib/ui.md

use bytemuck::{Pod, Zeroable};
use wgpu::util::DeviceExt;

use postretro_scripting_core::data_descriptors::PresentationTemplate;
use postretro_ui::UiTexture;
use postretro_ui::text::FontSystem;

use self::text::{TextPrepareInput, UiTextRenderer};

/// glyphon shaped-text half of the pass: embedded font, glyph atlas/renderer,
/// and the shape→prepare→render→trim cycle. glyphon owns its own pipeline; the
/// text draw records into this same render pass, after the quads. The UI depth
/// target keeps later-layer quads in front of earlier-layer text.
pub(crate) mod text;

pub(crate) use postretro_ui::{
    UiDrawList, UiInstance, UiReadSnapshot, UiText, UiUniform, descriptor, layout, theme, tree,
};
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

const UI_QUAD_WGSL: &str = include_str!("../../shaders/ui_quad.wgsl");

/// 9 regions * 2 triangles * 3 vertices. The vertex shader keys off
/// `vertex_index` to expand one instance into the 9-slice geometry; total is
/// 9 regions × `VERTS_PER_REGION` (= 6u) in `ui_quad.wgsl` = 54.
const VERTS_PER_INSTANCE: u32 = 54;

/// Small key→bind-group registry for `image` widget assets. The descriptor's
/// `image` nodes reference a texture by string key; the renderer pre-registers
/// the known keys and resolves each image batch's key through this map to the
/// bind group the draw binds. The same entries also expose natural image sizes
/// to the CPU layout pass so image nodes measure from uploaded asset dimensions.
///
/// Only pre-registered keys resolve — dynamic asset streaming is out of scope.
/// An unknown key is skipped-with-warn at draw time — the image batch simply
/// does not draw, and a single warning names the missing key. Each entry owns
/// its texture so the bind group's view stays valid for the registry's lifetime.
///
/// E19's current UI manifest surface carries trees, theme tokens, and font
/// assets, but no authored UI image asset list/path contract. `register_uploaded`
/// is the renderer-owned seam that future producer will call; until then
/// production may legitimately run with an empty registry.
#[derive(Default)]
pub(crate) struct UiImageRegistry {
    entries: std::collections::HashMap<String, UiImageEntry>,
    image_sizes: tree::ImageSizes,
    image_sizes_generation: u64,
    warned_missing: std::cell::RefCell<std::collections::HashSet<String>>,
}

struct UiImageEntry {
    /// Kept alive so the bind group's texture view stays valid.
    _texture: wgpu::Texture,
    bind_group: wgpu::BindGroup,
}

impl UiImageRegistry {
    /// Install an already-uploaded UI texture and its bind group. Renderer code
    /// creates the GPU objects; the registry keeps the texture alive, resolves
    /// the key at draw time, and exposes the same texture's natural size to
    /// layout.
    #[allow(dead_code)]
    pub fn register_uploaded(
        &mut self,
        key: impl Into<String>,
        texture: wgpu::Texture,
        bind_group: wgpu::BindGroup,
        size: [u32; 2],
    ) {
        let key = key.into();
        let natural_size = [size[0] as f32, size[1] as f32];
        if self.image_sizes.get(&key).copied() != Some(natural_size) {
            self.image_sizes_generation = self.image_sizes_generation.wrapping_add(1);
        }
        self.image_sizes.insert(key.clone(), natural_size);
        self.warned_missing.borrow_mut().remove(&key);
        self.entries.insert(
            key,
            UiImageEntry {
                _texture: texture,
                bind_group,
            },
        );
    }

    /// Natural reference sizes for registered UI image assets. Passed directly
    /// into `UiTree` layout; this is the production counterpart to CPU tests that
    /// build a non-empty `ImageSizes` fixture.
    pub fn image_sizes(&self) -> &tree::ImageSizes {
        &self.image_sizes
    }

    /// Monotonic generation for natural-size availability. Retained UI layout
    /// uses this as an external measure input: a late image upload can change an
    /// image node from zero-sized to naturally sized even when the descriptor,
    /// slots, viewport, and theme are unchanged.
    pub fn image_sizes_generation(&self) -> u64 {
        self.image_sizes_generation
    }

    /// Resolve `key` to its bind group, or `None` if no such key is registered.
    /// The live read side: `UiComposition::from_layer_draws` resolves each
    /// gameplay image batch's asset key through here.
    pub fn resolve(&self, key: &str) -> Option<&wgpu::BindGroup> {
        if let Some(entry) = self.entries.get(key) {
            return Some(&entry.bind_group);
        }
        if self.warned_missing.borrow_mut().insert(key.to_string()) {
            log::warn!(
                "[Renderer] UI image asset key '{key}' is not registered; skipping its draw"
            );
        }
        None
    }

    #[cfg(test)]
    fn register_size_for_test(&mut self, key: &str, size: [u32; 2]) {
        let natural_size = [size[0] as f32, size[1] as f32];
        if self.image_sizes.get(key).copied() != Some(natural_size) {
            self.image_sizes_generation = self.image_sizes_generation.wrapping_add(1);
        }
        self.image_sizes.insert(key.to_string(), natural_size);
    }
}

/// Initial instance-buffer capacity (records). Grows on demand in `encode`.
const INITIAL_INSTANCE_CAPACITY: usize = 64;
const INSTANCE_SIZE: usize = std::mem::size_of::<GpuUiInstance>();
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

/// Instanced quad / 9-slice pass for panels and images. Owns its pipeline, BGL,
/// sampler, uniform buffer, instance buffer, and a 1×1 white texture so solid
/// panels and textured images share one instanced path. Uses a private UI depth
/// target so glyphon's text draw can share the pass while opaque upper-layer
/// quads still hard-occlude lower-layer text.
pub(crate) struct UiPass {
    opaque_pipeline: wgpu::RenderPipeline,
    translucent_pipeline: wgpu::RenderPipeline,
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

    /// Private depth target for the UI pass. It is cleared every encode and only
    /// exists to preserve painter order across the quad/text draw split.
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

/// Renderer-local state retained for one live passive presentation. Layout,
/// fact cells, and tween state stay warm across frames until the app drops the
/// instance from its bounded input set.
struct PresentationLayout {
    template: postretro_entities::PresentationTemplateHandle,
    theme_generation: u64,
    layout: tree::PresentationTemplateLayout,
    fact_cells: tree::CellValues,
    relative_draw: tree::UiDrawData,
    active_generation: u64,
}

/// One instanced draw: a draw list plus the bind group for its bound texture.
/// Panels use the pass's white-texel bind group; images bind their own.
pub(crate) struct UiBatch<'a> {
    pub list: &'a UiDrawList,
    pub bind_group: &'a wgpu::BindGroup,
}

struct OrderedUiBatch<'a> {
    instances: Vec<UiInstance>,
    order: usize,
    bind_group: &'a wgpu::BindGroup,
    writes_depth: bool,
}

/// The whole frame's UI composition: every modal-stack layer's quad batches and
/// shaped-text runs in bottom→top painter order, as the single unit
/// `UiPass::encode` records. The encode boundary is the WHOLE composition, never
/// one layer — making the historical per-layer encode loop (which clobbered the
/// shared glyphon vertex buffer across layers) unrepresentable on the production
/// surface.
///
/// **Invariant — one `prepare`/vertex-buffer fill per surface composition.** All
/// layers funnel through ONE `encode`, so glyphon's `prepare` (which overwrites
/// its single internal vertex buffer at offset 0) runs once per composed frame.
/// The text path obeys the same "one fill per composition" rule the quad path
/// already enforces by giving each batch a disjoint instance-buffer region.
///
/// Owns renderer-local quad batches and concatenated text runs. Each batch and
/// text run also carries a composition order. `encode` still draws quads before
/// glyphon text, but the private UI depth target makes later opaque commands
/// occlude earlier commands across that split. Translucent/image batches
/// depth-test without writing depth so they do not hard-erase lower text before
/// glyphon renders; exact source-over ordering between translucent quads and text
/// is still limited by glyphon's single render call. Built in the caller's frame
/// scope so the bind-group borrows coexist with the `&mut self.ui` encode call.
/// Two constructors: `from_layer_draws` (gameplay modal stack) and `from_batches`
/// (test assembly).
pub(crate) struct UiComposition<'a> {
    batches: Vec<OrderedUiBatch<'a>>,
    texts: Vec<UiText>,
    text_orders: Vec<usize>,
    order_count: usize,
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
        let mut texts: Vec<UiText> = Vec::new();
        let mut text_orders: Vec<usize> = Vec::new();
        let mut order = 0usize;
        for draw in layer_draws {
            if draw.paint_order.is_empty() {
                append_legacy_draw_order(
                    draw,
                    white_bind_group,
                    images,
                    &mut batches,
                    &mut texts,
                    &mut text_orders,
                    &mut order,
                );
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
                    + draw.texts.len();
                debug_assert_eq!(
                    draw.paint_order.len(),
                    grouped_len,
                    "UiDrawData.paint_order is non-empty but incomplete: {} ops vs {} grouped items \
                     (quads + images + texts). A non-empty-but-incomplete paint_order silently drops \
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
                            order += 1;
                        }
                    }
                    tree::UiPaintOp::Text { index } => {
                        if let Some(text) = draw.texts.get(index) {
                            text_orders.push(order);
                            texts.push(text.clone());
                            order += 1;
                        }
                    }
                }
            }
        }
        Self {
            batches,
            texts,
            text_orders,
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
        Self {
            batches: batches
                .into_iter()
                .enumerate()
                .map(|(order, batch)| OrderedUiBatch {
                    instances: batch.list.instances.clone(),
                    order,
                    bind_group: batch.bind_group,
                    writes_depth: batch.list.instances.iter().all(instance_writes_depth),
                })
                .collect(),
            text_orders: std::iter::repeat_n(text_order, texts.len()).collect(),
            texts,
            order_count,
        }
    }

    /// `true` when the composition records nothing — no quad batches and no text.
    /// The gameplay path early-outs the UI pass on this.
    pub fn is_empty(&self) -> bool {
        self.batches.is_empty() && self.texts.is_empty()
    }
}

fn append_ordered_quad_batch<'a>(
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

fn append_legacy_draw_order<'a>(
    draw: &'a tree::UiDrawData,
    white_bind_group: &'a wgpu::BindGroup,
    images: &'a UiImageRegistry,
    batches: &mut Vec<OrderedUiBatch<'a>>,
    texts: &mut Vec<UiText>,
    text_orders: &mut Vec<usize>,
    order: &mut usize,
) {
    if !draw.quads.is_empty() {
        batches.push(OrderedUiBatch {
            instances: draw.quads.instances.clone(),
            order: *order,
            bind_group: white_bind_group,
            writes_depth: draw.quads.instances.iter().all(instance_writes_depth),
        });
        *order += 1;
    }
    for (asset, list) in &draw.images {
        if list.is_empty() {
            continue;
        }
        if let Some(bind_group) = images.resolve(asset) {
            batches.push(OrderedUiBatch {
                instances: list.instances.clone(),
                order: *order,
                bind_group,
                writes_depth: false,
            });
            *order += 1;
        }
    }
    if !draw.texts.is_empty() {
        text_orders.extend(std::iter::repeat_n(*order, draw.texts.len()));
        texts.extend_from_slice(&draw.texts);
        *order += 1;
    }
}

fn instance_writes_depth(instance: &UiInstance) -> bool {
    instance.color[3] >= 1.0
}

impl UiPass {
    pub fn new(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        color_format: wgpu::TextureFormat,
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

        // Per-instance vertex buffer: the four vec4 attributes from `UiInstance`
        // plus one renderer-local painter depth. No per-vertex buffer — geometry
        // is generated from `vertex_index`.
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
                wgpu::VertexAttribute {
                    format: wgpu::VertexFormat::Float32,
                    offset: 64,
                    shader_location: 4,
                },
            ],
        };

        let opaque_pipeline = create_ui_quad_pipeline(
            device,
            &pipeline_layout,
            &shader,
            &instance_layout,
            color_format,
            true,
            "UI Quad Pipeline",
        );
        let translucent_pipeline = create_ui_quad_pipeline(
            device,
            &pipeline_layout,
            &shader,
            &instance_layout,
            color_format,
            false,
            "UI Quad Translucent Pipeline",
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
        let text = UiTextRenderer::new(device, queue, color_format, ui_depth_stencil_state(false));

        Self {
            opaque_pipeline,
            translucent_pipeline,
            uniform_buffer,
            instance_buffer,
            instance_capacity: INITIAL_INSTANCE_CAPACITY,
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
        image_sizes_generation: u64,
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
        retained
            .tree
            .build_draw_data_retained_with_image_generation(
                viewport,
                font_system,
                image_sizes,
                image_sizes_generation,
                slot_values,
                cell_values,
                time_seconds,
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
        inputs: &[super::PresentationDrawInput],
        viewport: [u32; 2],
        image_sizes: &tree::ImageSizes,
        image_sizes_generation: u64,
        theme: &theme::UiTheme,
        theme_generation: u64,
        time_seconds: f64,
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
                time_seconds,
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

    /// Mark the command buffer containing the UI encode as submitted. The debug
    /// text guard resets here, not at `encode` entry, so two UI encodes recorded
    /// before one submit still count as two glyphon prepares and trip the guard.
    pub fn mark_submitted(&mut self) {
        self.text.reset_prepare_guard();
    }

    /// Record a whole-frame `UiComposition` (every modal-stack layer's quad
    /// batches + text runs, in painter order) into `view`. The encode boundary is
    /// the COMPOSITION, not one layer — a caller cannot loop `encode` per layer, so
    /// the historical cross-layer glyphon vertex-buffer clobber is unrepresentable
    /// here. See `UiComposition` for the "one `prepare`/vertex-buffer fill per
    /// surface composition" invariant; its text-path sibling is the disjoint
    /// per-batch instance-buffer region the quad loop below documents.
    ///
    /// Single color target plus a private UI depth target; the caller's `load` op
    /// controls whether the color surface is cleared first. The depth target is
    /// always cleared. `load` rides alongside `&UiComposition` because
    /// clear-vs-load is a target concern, not a composition one.
    ///
    /// Record order is quads first, then one glyphon text draw. Painter order is
    /// preserved for opaque occlusion by depth: later composition commands use
    /// smaller depth, so an opaque top-layer panel/backdrop rejects lower-layer
    /// text even though text records after quads. Translucent/image batches test
    /// depth but do not write it, preventing hard erasure of lower text; exact
    /// alpha source-over interleaving with text would require splitting glyphon's
    /// single render call. glyphon's atlas upload + CPU layout (`prepare`) runs
    /// BEFORE the pass opens (it needs `device`/`queue`, not the pass). With no
    /// quads and no text the pass still opens so the caller's `load` op lands.
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
        // Keep ordered batch/text slices internal to the pass. The public
        // boundary takes the whole composition so caller-side per-layer encode
        // loops stay unrepresentable.
        let batches: &[OrderedUiBatch<'_>] = &composition.batches;
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
        // SUM of all batch instance counts. `queue.write_buffer` is a
        // queue-timeline op: every staged write lands (last-wins per region)
        // BEFORE the single submitted command buffer executes. Writing each
        // batch to offset 0 would therefore have every draw read the LAST
        // batch's data — recording a draw between writes does not snapshot the
        // buffer, since the writes resolve on the queue timeline, not the
        // command-recording timeline. Disjoint per-batch regions sidestep this.
        let total_instances: usize = batches.iter().map(|b| b.instances.len()).sum();
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
        let text_depths: Vec<f32> = composition
            .text_orders
            .iter()
            .map(|&order| painter_depth(order, composition.order_count))
            .collect();
        let prepared = self.text.prepare_text(
            font_system,
            device,
            queue,
            TextPrepareInput {
                viewport,
                texts,
                buffers: &text_buffers,
                depths: &text_depths,
            },
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

        // Quads first. Each non-empty batch concatenates into its own region:
        // batch K starts at `offset_k = (sum of prior batch lens) * INSTANCE_SIZE`.
        // The draw binds the vertex buffer from `offset_k` and uses instance
        // range `0..count_k`, so it reads its own region without relying on a
        // non-zero `first_instance`. Empty batches are skipped without consuming
        // a region.
        let mut offset = 0u64;
        for ordered in batches {
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
}

fn create_ui_quad_pipeline(
    device: &wgpu::Device,
    pipeline_layout: &wgpu::PipelineLayout,
    shader: &wgpu::ShaderModule,
    instance_layout: &wgpu::VertexBufferLayout<'_>,
    color_format: wgpu::TextureFormat,
    depth_write_enabled: bool,
    label: &'static str,
) -> wgpu::RenderPipeline {
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some(label),
        layout: Some(pipeline_layout),
        vertex: wgpu::VertexState {
            module: shader,
            entry_point: Some("vs_main"),
            buffers: std::slice::from_ref(instance_layout),
            compilation_options: wgpu::PipelineCompilationOptions::default(),
        },
        primitive: wgpu::PrimitiveState {
            topology: wgpu::PrimitiveTopology::TriangleList,
            ..Default::default()
        },
        // Private UI depth preserves painter order across the quad/text split.
        // Opaque quads write it for hard occlusion; translucent/image batches
        // only test so they do not erase lower text before glyphon renders.
        depth_stencil: Some(ui_depth_stencil_state(depth_write_enabled)),
        multisample: wgpu::MultisampleState::default(),
        fragment: Some(wgpu::FragmentState {
            module: shader,
            entry_point: Some("fs_main"),
            targets: &[Some(wgpu::ColorTargetState {
                format: color_format,
                // Standard alpha blend over the existing surface contents.
                blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                write_mask: wgpu::ColorWrites::ALL,
            })],
            compilation_options: wgpu::PipelineCompilationOptions::default(),
        }),
        multiview_mask: None,
        cache: None,
    })
}

fn ui_depth_stencil_state(depth_write_enabled: bool) -> wgpu::DepthStencilState {
    wgpu::DepthStencilState {
        format: UI_DEPTH_FORMAT,
        depth_write_enabled: Some(depth_write_enabled),
        depth_compare: Some(wgpu::CompareFunction::LessEqual),
        stencil: wgpu::StencilState::default(),
        bias: wgpu::DepthBiasState::default(),
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
