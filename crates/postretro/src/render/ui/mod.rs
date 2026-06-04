// UI render pass: hand-rolled instanced quad / 9-slice pipeline for panels and
// images. One instance per panel/image carries (rect, UV rect, color, 9-slice
// margin); the vertex stage expands each instance into 9 regions. All wgpu lives
// here per renderer-owns-GPU. Text is glyphon's own pipeline (Task 3), not this.
// See: context/lib/rendering_pipeline.md · context/plans/in-progress/M13--ui-render-pass-slice

use bytemuck::{Pod, Zeroable};
use wgpu::util::DeviceExt;

use crate::ui_texture::UiTexture;

/// Logical-reference scaling model + device-pixel projection/snap. Pure CPU,
/// no GPU handles — produces a `UiDrawList` the pass uploads at encode time.
pub(crate) mod layout;

const UI_QUAD_WGSL: &str = include_str!("../../shaders/ui_quad.wgsl");

/// 9 regions * 2 triangles * 3 vertices. The vertex shader keys off
/// `vertex_index` to expand one instance into the 9-slice geometry; keep in
/// lockstep with `VERTS_PER_INSTANCE` in `ui_quad.wgsl`.
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
    /// optional 9-slice margin. Color is linear RGBA.
    pub fn panel(rect: [f32; 4], color: [f32; 4], margin: [f32; 4]) -> Self {
        Self {
            rect,
            uv_rect: [0.0, 0.0, 1.0, 1.0],
            color,
            margin,
        }
    }

    /// Textured image: samples the full bound texture, untinted (white).
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
    /// `axis_bounds` margin clamp so layout assertions match what the GPU draws.
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
        // `axis_bounds` in ui_quad.wgsl.
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
/// Built with no wgpu call so layout/scaling logic stays GPU-independent
/// (Task 2 populates it; Task 6 asserts against it). The pass uploads it to the
/// instance buffer at encode time.
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
/// color target with no depth attachment so glyphon (Task 3) can share the pass.
pub(crate) struct UiPass {
    pipeline: wgpu::RenderPipeline,
    bind_group_layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    uniform_buffer: wgpu::Buffer,
    instance_buffer: wgpu::Buffer,
    instance_capacity: usize,
    /// 1×1 white texel bound for solid panels (degenerate UV slice). An
    /// untextured panel and a textured image then share one instanced batch.
    white_view: wgpu::TextureView,
    /// Bind group for the white-texel batch (panels). Rebuilt only if the
    /// uniform buffer changes, which it never does after construction.
    white_bind_group: wgpu::BindGroup,
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
            // Depth disabled: the UI pass attaches no depth target, so glyphon
            // (Task 3) can share this single-color-target configuration.
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

        Self {
            pipeline,
            bind_group_layout,
            sampler,
            uniform_buffer,
            instance_buffer,
            instance_capacity: INITIAL_INSTANCE_CAPACITY,
            white_view,
            white_bind_group,
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

    /// Record the UI batches into `view`. Single color target, no depth, the
    /// caller's `load` op controls whether the surface is cleared first
    /// (splash phase clears to black; the gameplay path loads). Uploads each
    /// batch's draw list to the instance buffer and issues one instanced draw
    /// per batch. Batches with empty draw lists are skipped; with no non-empty
    /// batches the pass still opens (and applies the load op) so the caller's
    /// clear lands even on an empty frame.
    pub fn encode(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        view: &wgpu::TextureView,
        viewport: [u32; 2],
        load: wgpu::LoadOp<wgpu::Color>,
        batches: &[UiBatch<'_>],
    ) {
        queue.write_buffer(
            &self.uniform_buffer,
            0,
            bytemuck::bytes_of(&UiUniform {
                viewport: [viewport[0] as f32, viewport[1] as f32],
                _pad: [0.0, 0.0],
            }),
        );

        // Grow the instance buffer once to the largest batch so each batch's
        // upload fits at offset 0. (Batches are drawn sequentially with the
        // buffer rewritten between them — fine because each draw is recorded
        // before the next overwrite within the same pass.)
        let max_instances = batches.iter().map(|b| b.list.len()).max().unwrap_or(0);
        if max_instances > self.instance_capacity {
            self.grow_instance_buffer(device, max_instances);
        }

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

        pass.set_pipeline(&self.pipeline);
        for batch in batches {
            if batch.list.is_empty() {
                continue;
            }
            let bytes: &[u8] = bytemuck::cast_slice(&batch.list.instances);
            queue.write_buffer(&self.instance_buffer, 0, bytes);
            pass.set_bind_group(0, batch.bind_group, &[]);
            pass.set_vertex_buffer(0, self.instance_buffer.slice(..));
            pass.draw(0..VERTS_PER_INSTANCE, 0..batch.list.len() as u32);
        }
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
/// the logo image reuses `render::splash::upload_splash_texture` in Task 4.
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
        // corners — they clamp to the available space (mirrors axis_bounds).
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
