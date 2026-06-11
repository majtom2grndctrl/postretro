// UI render pass: hand-rolled instanced quad / 9-slice pipeline for panels and
// images. One instance per panel/image carries (rect, UV rect, color, 9-slice
// margin); the vertex stage expands each instance into 9 regions. All wgpu lives
// here per renderer-owns-GPU. Shaped text is glyphon's own pipeline, owned by
// the `text` submodule and recorded into this same pass after the quads.

use bytemuck::{Pod, Zeroable};
use wgpu::util::DeviceExt;

use crate::ui_texture::UiTexture;

use self::text::UiTextRenderer;

/// Logical-reference scaling model + device-pixel projection/snap. Pure CPU,
/// no GPU handles — produces a `UiDrawList` the pass uploads at encode time.
pub(crate) mod layout;

/// Serde descriptor model for the UI widget tree: the `Widget` enum, its field
/// structs, and the `AnchoredTree` placement envelope. Pure data — the retained
/// tree (`tree`) maps it onto taffy and the renderer lays it out.
pub(crate) mod descriptor;

/// glyphon shaped-text half of the pass: embedded font, glyph atlas/renderer,
/// and the shape→prepare→render→trim cycle. glyphon owns its own pipeline; the
/// text draw records into this same render pass, after the quads.
pub(crate) mod text;

/// Retained widget tree: maps the `descriptor` model into a `taffy::TaffyTree`,
/// computes flex/grid layout, and reads laid-out rects back into the device-pixel
/// draw list + shaped-text entries through the `layout` projection path. taffy
/// lives entirely here (renderer-owns-GPU). The renderer lays out the splash and
/// gameplay descriptor trees through it.
pub(crate) mod tree;

/// UI theme-token table: named color/font/spacing tokens widgets resolve against,
/// the engine default theme, and the `ThemeDescriptor` override wire form. Pure
/// data — widgets (later tasks) reference tokens by name; the merge is per-token.
pub(crate) mod theme;

pub(crate) use self::text::UiText;

/// Hardcoded splash content descriptor behind the one named builder seam
/// (`splash::build_splash_descriptor`). The builder returns an `AnchoredTree` the
/// renderer lays out via `UiTree`; G1 later swaps the body for script ingestion.
pub(crate) mod splash;

/// Hardcoded demo gameplay HUD descriptor (M13 Goal C state-binding demo) behind
/// the `demo::build_demo_descriptor` seam. The FIRST gameplay UI producer:
/// `main.rs` publishes its `AnchoredTree` on the per-frame read snapshot and the
/// renderer drives it through the retained gameplay path. Two `text` nodes bind
/// `player.health`/`player.ammo`; one `panel` binds `intro.flashColor`.
pub(crate) mod demo;

/// Hard-gate CPU draw-list / layout assertion for the splash: pins the
/// device-pixel quad rects (anchor, scale, snap, 9-slice corners) the splash
/// projects to. Pure CPU — no GPU adapter.
#[cfg(test)]
mod splash_layout_test;

/// Optional headless golden: renders the splash UI pass into an offscreen target
/// and asserts tolerance-scoped structural properties of the readback. Self-skips
/// when no GPU adapter is present — never the hard gate.
#[cfg(test)]
mod splash_golden_test;

/// Hard-gate CPU assertion for the gameplay UI path: the renderer builds a
/// non-empty draw list from a fixture descriptor tree, and an empty tree
/// early-outs the UI pass (no `begin_render_pass`). Pure CPU — asserts the
/// layout + early-out decision the renderer's gameplay path makes, without a GPU.
#[cfg(test)]
mod gameplay_ui_gate_test;

/// Hard-gate CPU assertion for the demo gameplay HUD (`demo::build_demo_descriptor`):
/// drives the real demo descriptor through the retained gameplay tree and asserts
/// bind resolution, the appearance-only-vs-content-change relayout split, the
/// post-settle no-recompute frame, and the subscriber-aware unbound-slot no-op.
/// Pure CPU — no GPU adapter.
#[cfg(test)]
mod demo_ui_gate_test;

/// Hard-gate CPU assertions for M13 fonts+theming Task 4: the theme-generation
/// rebuild gate (reproduced CPU-side, mirroring `UiPass::layout_gameplay_tree`)
/// and the exactly-one-warning-per-build fallback contract for unknown tokens.
/// Pure CPU — no GPU adapter.
#[cfg(test)]
mod theme_gate_test;

/// Headless regression for the multi-batch instance-buffer clobber: encodes two
/// non-empty batches into disjoint screen regions and asserts each region keeps
/// its own batch's color. Self-skips when no GPU adapter is present.
#[cfg(test)]
mod multi_batch_test;

const UI_QUAD_WGSL: &str = include_str!("../../shaders/ui_quad.wgsl");

/// 9 regions * 2 triangles * 3 vertices. The vertex shader keys off
/// `vertex_index` to expand one instance into the 9-slice geometry; total is
/// 9 regions × `VERTS_PER_REGION` (= 6u) in `ui_quad.wgsl` = 54.
const VERTS_PER_INSTANCE: u32 = 54;

/// Per-instance draw record. Layout mirrors `UiInstance` in `ui_quad.wgsl`:
/// four `vec4<f32>` attributes, tightly packed, no padding. Byte-for-byte
/// stable so `bytemuck` can cast a slice straight into the instance buffer.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Pod, Zeroable)]
pub(crate) struct UiInstance {
    /// Device-pixel rect: `[x, y, width, height]`, top-left origin.
    pub rect: [f32; 4],
    /// UV rect into the bound texture: `[u0, v0, u_width, v_height]`.
    pub uv_rect: [f32; 4],
    /// Linear RGBA tint multiplied into the sampled texel.
    pub color: [f32; 4],
    /// 9-slice margin in device pixels: `[left, top, right, bottom]`. All zero
    /// renders a plain stretched quad (the degenerate case).
    pub margin: [f32; 4],
}

impl UiInstance {
    /// Solid-color panel: full UV slice over the bound 1×1 white texel, with an
    /// optional 9-slice margin. Color is linear RGBA. Production paths build
    /// instances via `layout::project`; this ctor backs the corner-rect tests.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn panel(rect: [f32; 4], color: [f32; 4], margin: [f32; 4]) -> Self {
        Self {
            rect,
            uv_rect: [0.0, 0.0, 1.0, 1.0],
            color,
            margin,
        }
    }

    /// Textured image: samples the full bound texture, untinted (white).
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn image(rect: [f32; 4]) -> Self {
        Self {
            rect,
            uv_rect: [0.0, 0.0, 1.0, 1.0],
            color: [1.0, 1.0, 1.0, 1.0],
            margin: [0.0; 4],
        }
    }

    /// CPU-side derivation of the 9-slice corner rects (device pixels) for this
    /// instance — the four fixed-size corners as `[x, y, w, h]` in order
    /// top-left, top-right, bottom-left, bottom-right. Mirrors the shader's
    /// `axis` margin clamp so layout assertions match what the GPU draws.
    /// Used by tests (and future layout assertions); not consumed by the draw.
    #[cfg(test)]
    pub fn corner_rects(&self) -> [[f32; 4]; 4] {
        let (x, y, w, h) = (self.rect[0], self.rect[1], self.rect[2], self.rect[3]);
        let (ml, mt, mr, mb) = (
            self.margin[0],
            self.margin[1],
            self.margin[2],
            self.margin[3],
        );
        // Clamp margins so opposing corners never overrun the rect — matches
        // `axis` in ui_quad.wgsl.
        let clamp_axis = |full: f32, lo: f32, hi: f32| -> (f32, f32) {
            let avail = full.max(0.0);
            let lo_c = lo.clamp(0.0, avail);
            let hi_c = hi.clamp(0.0, (avail - lo_c).max(0.0));
            (lo_c, hi_c)
        };
        let (cl, cr) = clamp_axis(w, ml, mr);
        let (ct, cb) = clamp_axis(h, mt, mb);
        [
            [x, y, cl, ct],                   // top-left
            [x + w - cr, y, cr, ct],          // top-right
            [x, y + h - cb, cl, cb],          // bottom-left
            [x + w - cr, y + h - cb, cr, cb], // bottom-right
        ]
    }
}

/// Pure CPU draw list — a flat batch of instances sharing one bound texture.
/// Built with no wgpu call so layout/scaling logic stays GPU-independent: the
/// `layout` projection path populates it and the CPU layout tests assert against
/// it. The pass uploads it to the instance buffer at encode time.
#[derive(Debug, Default, Clone)]
pub(crate) struct UiDrawList {
    pub instances: Vec<UiInstance>,
}

impl UiDrawList {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn push(&mut self, instance: UiInstance) {
        self.instances.push(instance);
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub fn clear(&mut self) {
        self.instances.clear();
    }

    pub fn is_empty(&self) -> bool {
        self.instances.is_empty()
    }

    pub fn len(&self) -> usize {
        self.instances.len()
    }
}

/// Once-per-frame published read-only snapshot the UI pass reads when it records.
/// Stored on the `Renderer` via a setter the `App` calls just before each render
/// call — NOT threaded as a render parameter, so both render signatures stay
/// stable.
///
/// Carries three things: the splash version/tagline line, the gameplay descriptor
/// tree for the current frame, and the frame's resolved slot values. The renderer
/// reads `version_line` on the splash path and `gameplay_tree`/`slot_values` on
/// the gameplay path. The descriptor is carried here — never laid-out rects;
/// taffy/glyphon live in the renderer per renderer-owns-GPU. Slot values are
/// cloned out of the live `SlotTable` once per frame so the renderer never borrows
/// the live store — preserving the renderer/game-logic boundary. Value-less slots
/// are omitted; a present key always carries a resolved value.
#[derive(Debug, Clone, Default)]
pub(crate) struct UiReadSnapshot {
    /// Version/tagline string the splash's shaped-text line renders. Read only on
    /// the splash path; empty on the gameplay path.
    pub version_line: String,
    /// The gameplay-path descriptor tree to lay out and draw this frame. `None`
    /// (the default) on the splash path and whenever gameplay publishes no UI —
    /// the renderer's UI pass then early-outs with no `begin_render_pass`. The
    /// current gameplay UI producer is `demo::build_demo_descriptor`, published
    /// by `main.rs` each frame; the test gate feeds a fixture tree.
    pub gameplay_tree: Option<descriptor::AnchoredTree>,
    /// Resolved state-store values for this frame, keyed by dotted slot name.
    /// Cloned out of the live `SlotTable` once per frame (see the type doc).
    /// Only slots that currently hold a value appear; value-less slots are
    /// skipped. Empty on the splash path.
    pub slot_values: std::collections::HashMap<String, crate::scripting::slot_table::SlotValue>,
    /// Deterministic frame time in seconds, accumulated from per-frame `dt`
    /// (`App::script_time`) — NEVER wall-clock. Stays `f64` end-to-end to match
    /// the App's accumulator. The retained gameplay build threads it down so the
    /// tween runtime can ease bound display values over time. `0.0` (the default)
    /// on the splash/fresh path, where inertness is structural — that path takes
    /// no time at all.
    pub time_seconds: f64,
}

impl UiReadSnapshot {
    /// Snapshot carrying the splash version/tagline line (splash path). The
    /// slot-value map stays empty — the splash has no store-bound widgets.
    pub fn with_version_line(version_line: impl Into<String>) -> Self {
        Self {
            version_line: version_line.into(),
            gameplay_tree: None,
            slot_values: std::collections::HashMap::new(),
            // Splash/fresh path takes no time — inertness is structural.
            time_seconds: 0.0,
        }
    }

    /// Snapshot carrying a gameplay-path descriptor tree (the content side) plus
    /// the frame's resolved slot-value snapshot and the deterministic frame time.
    /// The renderer lays the tree out into the UI draw list, resolves `bind` slots
    /// against `slot_values`, and threads `time_seconds` into the retained build so
    /// the tween runtime can ease bound display values over time.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn with_gameplay_tree(
        tree: descriptor::AnchoredTree,
        slot_values: std::collections::HashMap<String, crate::scripting::slot_table::SlotValue>,
        time_seconds: f64,
    ) -> Self {
        Self {
            version_line: String::new(),
            gameplay_tree: Some(tree),
            slot_values,
            time_seconds,
        }
    }
}

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
    /// Register (or replace) the bind group + owning texture for `key`. The
    /// texture is held so its view outlives every draw that binds the group.
    pub fn register(
        &mut self,
        key: impl Into<String>,
        texture: wgpu::Texture,
        bind_group: wgpu::BindGroup,
    ) {
        let key = key.into();
        self.entries.insert(
            key,
            UiImageEntry {
                _texture: texture,
                bind_group,
            },
        );
    }

    /// Resolve `key` to its bind group, or `None` if no such key is registered.
    pub fn resolve(&self, key: &str) -> Option<&wgpu::BindGroup> {
        self.entries.get(key).map(|e| &e.bind_group)
    }

    /// Drop all registered images (splash teardown).
    pub fn clear(&mut self) {
        self.entries.clear();
    }
}

/// UI uniform: device viewport in pixels. 16 bytes (vec2 + vec2 pad) to match
/// `UiUniform` in `ui_quad.wgsl` and satisfy uniform alignment.
#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
struct UiUniform {
    viewport: [f32; 2],
    _pad: [f32; 2],
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
    bind_group_layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
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

    /// The retained gameplay tree, held across frames so the dirty-gate and the
    /// bound-value diff actually pay off (a fresh tree is always dirty). `None`
    /// until the first gameplay frame installs one. The splash deliberately does
    /// NOT use this — it stays on the stateless `layout_tree` fresh-build path.
    gameplay_tree: Option<RetainedGameplayTree>,
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
        // so `FontSystem`/`TextAtlas` build in `Renderer::new` rather than on the
        // first shaped frame. See `text::UiTextRenderer::new`.
        let text = UiTextRenderer::new(device, queue, surface_format);

        Self {
            pipeline,
            bind_group_layout,
            sampler,
            uniform_buffer,
            instance_buffer,
            instance_capacity: INITIAL_INSTANCE_CAPACITY,
            white_view,
            white_bind_group,
            text,
            gameplay_tree: None,
        }
    }

    /// Bind group for solid-color panels — samples the 1×1 white texel. Pass
    /// this as a `UiBatch::bind_group` for the panel batch.
    pub fn white_bind_group(&self) -> &wgpu::BindGroup {
        &self.white_bind_group
    }

    /// Build a bind group for a caller-bound image texture (e.g. the logo). One
    /// batch per bound texture, so the image draws as a separate instanced draw.
    pub fn make_texture_bind_group(
        &self,
        device: &wgpu::Device,
        view: &wgpu::TextureView,
    ) -> wgpu::BindGroup {
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("UI Image Bind Group"),
            layout: &self.bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: self.uniform_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(view),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::Sampler(&self.sampler),
                },
            ],
        })
    }

    /// Lay a descriptor tree out into device-pixel draw data, threading the
    /// pass's glyphon `FontSystem` so text nodes size from real shaped-run
    /// metrics (the taffy↔glyphon measure seam). SPLASH path only — the gameplay
    /// path uses the retained `layout_gameplay_tree`. Turns the draw data into
    /// `UiBatch`es. Layout (taffy) runs on the CPU here — no `wgpu` call — so the
    /// GPU stays untouched until `encode`.
    ///
    /// Builds a FRESH `UiTree` every call: the splash re-derives its descriptor
    /// each frame, is transient, and carries no bindings — so the dirty-gate never
    /// fires on this path and retaining it would only add bookkeeping. The
    /// gameplay path retains its tree across frames via `layout_gameplay_tree`,
    /// where the dirty-gate and bound-value diff pay off.
    ///
    /// `image_sizes` maps each referenced `image` asset key to its natural
    /// reference size; the measure seam sizes image nodes from it (content-driven,
    /// like text). The splash passes the logo's size; the gameplay path passes an
    /// empty map (the demo HUD has no image nodes).
    ///
    /// `slot_values` is the frame's resolved state-store read snapshot, keyed by
    /// dotted slot name. Bound text/panel nodes resolve their drawn string/color
    /// against it at draw-data build time; an absent slot falls back to the literal
    /// descriptor value. The splash passes an empty map (its tree has no binds);
    /// gameplay passes the snapshot's `slot_values`.
    pub fn layout_tree(
        &mut self,
        tree: &descriptor::AnchoredTree,
        viewport: [u32; 2],
        image_sizes: &tree::ImageSizes,
        slot_values: &std::collections::HashMap<String, crate::scripting::slot_table::SlotValue>,
        theme: &theme::UiTheme,
    ) -> tree::UiDrawData {
        let mut ui_tree = tree::UiTree::from_descriptor(tree, theme);
        ui_tree.build_draw_data(
            viewport,
            self.text.font_system_mut(),
            image_sizes,
            slot_values,
        )
    }

    /// Lay out the gameplay descriptor tree through the RETAINED `UiTree` held on
    /// the pass, so layout and the draw list only rebuild when their inputs change
    /// across frames (the runtime perf win this task delivers).
    ///
    /// Reuse vs rebuild: the retained tree is reused while the incoming
    /// `descriptor` equals the one it was built from. A different descriptor (the
    /// snapshot delivered a structurally new tree) rebuilds it from scratch via
    /// `UiTree::from_descriptor`. Once reused, `build_draw_data_retained` runs the
    /// subscriber-aware bound-value diff and the relayout/redraw split:
    /// - an appearance-only bound change (the panel flash color) rebuilds the
    ///   draw list WITHOUT a taffy relayout,
    /// - a bound text-content change re-measures and relays out,
    /// - a no-change frame returns the cached draw list and recomputes nothing.
    ///
    /// The splash stays on `layout_tree` (fresh build per frame) — it is transient
    /// and carries no bindings, so retaining it would only add bookkeeping.
    ///
    /// `time_seconds` is the deterministic, dt-accumulated frame time threaded
    /// down to the retained build for the tween runtime to ease bound values over
    /// time. The splash/fresh `layout_tree` takes no such time — its inertness is
    /// structural.
    // Wide by necessity: viewport + image sizes + slot values + theme + theme
    // generation + frame time are all distinct retained-build inputs; bundling
    // them into a struct would only obscure the per-frame call site.
    #[allow(clippy::too_many_arguments)]
    pub fn layout_gameplay_tree(
        &mut self,
        tree: &descriptor::AnchoredTree,
        viewport: [u32; 2],
        image_sizes: &tree::ImageSizes,
        slot_values: &std::collections::HashMap<String, crate::scripting::slot_table::SlotValue>,
        theme: &theme::UiTheme,
        theme_generation: u64,
        time_seconds: f64,
    ) -> tree::UiDrawData {
        // Rebuild the retained tree when there is none yet, when the incoming
        // descriptor differs from the one it was built from (a structural change),
        // OR when the theme generation moved (the engine installed an override
        // theme, so the tokens baked into the retained tree are stale). The
        // generation gate rebuilds even on a byte-identical descriptor; a settled
        // frame (same descriptor + same generation) reuses the retained tree, so
        // the across-frames cache still holds.
        let needs_build = match &self.gameplay_tree {
            Some(retained) => {
                retained.descriptor != *tree || retained.theme_generation != theme_generation
            }
            None => true,
        };
        if needs_build {
            self.gameplay_tree = Some(RetainedGameplayTree {
                descriptor: tree.clone(),
                theme_generation,
                tree: tree::UiTree::from_descriptor(tree, theme),
            });
        }

        let retained = self
            .gameplay_tree
            .as_mut()
            .expect("retained gameplay tree set above");
        retained.tree.build_draw_data_retained(
            viewport,
            self.text.font_system_mut(),
            image_sizes,
            slot_values,
            time_seconds,
        )
    }

    /// Record the UI batches and shaped-text lines into `view`. Single color
    /// target, no depth, the caller's `load` op controls whether the surface is
    /// cleared first (splash phase clears to black; the gameplay path loads).
    ///
    /// Order matters: quads first, then text. Quad instances upload to the
    /// instance buffer and draw one instanced batch each; then glyphon's
    /// `TextRenderer::render` records its own draw INTO THE SAME render pass,
    /// AFTER the quads, so text composites over the panels/images into the same
    /// surface view. glyphon's atlas upload + CPU layout (`prepare`) runs BEFORE
    /// the pass opens (it needs `device`/`queue`, not the pass). With no quads
    /// and no text the pass still opens so the caller's `load` op lands.
    ///
    /// `texts` is empty on the quad-only / no-text path (no text work runs).
    // The wide signature mirrors a `begin_render_pass` call (target, viewport,
    // load op, draw lists, text) — splitting it into a builder would obscure the
    // single-pass contract the splash + gameplay paths both record through.
    #[allow(clippy::too_many_arguments)]
    pub fn encode(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        view: &wgpu::TextureView,
        viewport: [u32; 2],
        load: wgpu::LoadOp<wgpu::Color>,
        batches: &[UiBatch<'_>],
        texts: &[UiText],
    ) {
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
        let text_buffers = self.text.shape_text(texts, viewport);
        let prepared = self
            .text
            .prepare_text(device, queue, viewport, texts, &text_buffers);

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

/// Upload a CPU RGBA8 `UiTexture` and return the GPU texture. sRGB format so
/// image content decodes on sample (white encodes to white, so the panel texel
/// stays neutral). Mirrors `render::splash::upload_splash_texture` — kept local
/// so the UI pass owns its own upload path. Used here for the 1×1 white texel;
/// the splash logo image goes through `render::splash::upload_splash_texture`.
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
    fn ui_instance_byte_layout_is_64_bytes_no_padding() {
        // The shader's instance vertex layout (four Float32x4 at offsets
        // 0/16/32/48, stride 64) depends on this exact packing.
        assert_eq!(std::mem::size_of::<UiInstance>(), 64);
        assert_eq!(std::mem::align_of::<UiInstance>(), 4);
        // Field offsets the VertexAttribute table hardcodes.
        let probe = UiInstance {
            rect: [1.0, 2.0, 3.0, 4.0],
            uv_rect: [5.0, 6.0, 7.0, 8.0],
            color: [9.0, 10.0, 11.0, 12.0],
            margin: [13.0, 14.0, 15.0, 16.0],
        };
        let bytes: &[u8] = bytemuck::bytes_of(&probe);
        assert_eq!(bytes.len(), 64);
        // First field starts at offset 0, last vec4 at offset 48.
        assert_eq!(&bytes[0..4], &1.0f32.to_le_bytes());
        assert_eq!(&bytes[48..52], &13.0f32.to_le_bytes());
    }

    #[test]
    fn uniform_is_16_bytes() {
        assert_eq!(std::mem::size_of::<UiUniform>(), 16);
    }

    #[test]
    fn zero_margin_corner_rects_collapse() {
        // A plain quad (zero margin) has zero-size corners — the whole rect is
        // the stretched center region.
        let inst = UiInstance::panel([10.0, 20.0, 100.0, 60.0], [1.0; 4], [0.0; 4]);
        for c in inst.corner_rects() {
            assert_eq!(c[2], 0.0, "corner width collapses with zero margin");
            assert_eq!(c[3], 0.0, "corner height collapses with zero margin");
        }
    }

    #[test]
    fn nine_slice_corner_rects_are_fixed_size_and_anchored() {
        // 8px corners on a 100x60 rect at (10,20). Corners keep their 8px size
        // and sit at the four rect corners regardless of center stretch.
        let inst = UiInstance::panel([10.0, 20.0, 100.0, 60.0], [1.0; 4], [8.0, 8.0, 8.0, 8.0]);
        let [tl, tr, bl, br] = inst.corner_rects();
        assert_eq!(tl, [10.0, 20.0, 8.0, 8.0]);
        assert_eq!(tr, [10.0 + 100.0 - 8.0, 20.0, 8.0, 8.0]);
        assert_eq!(bl, [10.0, 20.0 + 60.0 - 8.0, 8.0, 8.0]);
        assert_eq!(br, [10.0 + 100.0 - 8.0, 20.0 + 60.0 - 8.0, 8.0, 8.0]);
    }

    #[test]
    fn corner_rects_clamp_when_margins_exceed_rect() {
        // Margins larger than the rect must not produce overlapping/negative
        // corners — they clamp to the available space (mirrors axis).
        let inst = UiInstance::panel([0.0, 0.0, 10.0, 10.0], [1.0; 4], [8.0, 8.0, 8.0, 8.0]);
        let [tl, tr, _bl, _br] = inst.corner_rects();
        // Left corner gets 8, right corner gets the remaining 2.
        assert_eq!(tl[2], 8.0);
        assert_eq!(tr[2], 2.0);
        assert!(tr[0] >= tl[0] + tl[2] - 1e-6, "corners do not overlap");
    }

    #[test]
    fn draw_list_push_and_clear() {
        let mut list = UiDrawList::new();
        assert!(list.is_empty());
        list.push(UiInstance::image([0.0, 0.0, 5.0, 5.0]));
        assert_eq!(list.len(), 1);
        list.clear();
        assert!(list.is_empty());
    }

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
