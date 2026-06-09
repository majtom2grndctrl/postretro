// Billboard sprite rendering pass: camera-facing quads for scripted
// `BillboardEmitterComponent` particles, expanded in the vertex shader from
// a storage buffer of per-sprite instance data. Lit by the full lighting
// stack (SH ambient + static multi-source specular via the chunk list +
// dynamic diffuse). Alpha-additive blend, depth test enabled, depth write
// disabled.
//
// See: context/lib/rendering_pipeline.md §7.4

use std::collections::HashMap;
use std::num::NonZeroU64;

use wgpu::util::DeviceExt;

use crate::fx::smoke::{SPRITE_INSTANCE_SIZE, SpriteFrame};

/// Byte size of `SpriteDrawParams` (one `vec4<f32>` = 16 B, padded to 16).
pub const SPRITE_DRAW_PARAMS_SIZE: usize = 16;

/// Storage-buffer dynamic-offset alignment required by wgpu / WebGPU
/// (`min_storage_buffer_offset_alignment`, 256 on every targeted backend).
/// Each collection's region in the shared instance buffer starts at a multiple
/// of this so it can be addressed by a group-6 dynamic offset. The 32-byte
/// per-instance stride is unchanged *within* a region (256 is a multiple of
/// 32, so the alignment padding is always a whole number of instance slots).
const STORAGE_DYNAMIC_OFFSET_ALIGNMENT: usize = 256;

/// Round `bytes` up to the next multiple of `STORAGE_DYNAMIC_OFFSET_ALIGNMENT`.
fn align_up_to_dynamic_offset(bytes: usize) -> usize {
    bytes.div_ceil(STORAGE_DYNAMIC_OFFSET_ALIGNMENT) * STORAGE_DYNAMIC_OFFSET_ALIGNMENT
}

/// Build the group-6 bind group over a fixed-size *window* of the instance
/// buffer. The binding is declared `has_dynamic_offset: true`, so this single
/// bind group is reused for every collection in a frame —
/// `set_bind_group(6, .., &[offset])` rebases `sprites[0]` in the shader to each
/// collection's 256-byte-aligned region.
///
/// The bound `size` is an explicit window (NOT `as_entire_binding`). wgpu-29
/// derives `maximum_dynamic_offset = buffer.size - window`, and
/// `set_bind_group` errors when any dynamic offset exceeds that maximum. With
/// `as_entire_binding` the window equals the whole buffer, so the maximum is 0
/// and any collection at offset ≥ 256 would be rejected. Binding an explicit
/// window strictly smaller than the buffer leaves headroom
/// (`buffer.size - window`) for the per-collection dynamic offsets. The caller
/// guarantees `window <= buffer.size`, `window` is a multiple of the 256-byte
/// storage alignment, and every collection's offset is `<= buffer.size - window`.
/// Rebuilt when the buffer object changes (growth) or the window changes.
fn build_instance_bind_group(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    buffer: &wgpu::Buffer,
    window: u64,
) -> wgpu::BindGroup {
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("Sprite Instance Bind Group"),
        layout,
        entries: &[wgpu::BindGroupEntry {
            binding: 0,
            resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                buffer,
                offset: 0,
                size: Some(NonZeroU64::new(window).expect("instance window must be non-zero")),
            }),
        }],
    })
}

/// Per-frame layout for the shared instance buffer: each collection's
/// 256-byte-aligned start offset (the dynamic offset passed at draw time) and
/// its live-sprite count (drives the `count * 6` vertex range). Computed in
/// `iter_collections` order; offsets accumulate by each region's *padded* size.
struct CollectionPlacement<'a> {
    collection: &'a str,
    packed_bytes: &'a [u8],
    offset: u32,
    live_count: u32,
}

/// The byte layout of one frame's collections in the shared instance buffer.
struct FrameLayout<'a> {
    /// Per-collection 256-byte-aligned placements, in `iter_collections` order.
    placements: Vec<CollectionPlacement<'a>>,
    /// The largest single collection's *padded* region this frame. The group-6
    /// bind-group window must be at least this so every collection's draw stays
    /// inside the bound storage slice (invariant 1). Always a multiple of the
    /// 256-byte storage alignment (it is a max of `align_up_to_dynamic_offset`
    /// values, which are 256-multiples).
    frame_max_region: usize,
    /// Start offset of the last collection — the largest dynamic offset
    /// `record_draws` will pass to `set_bind_group`. wgpu requires every dynamic
    /// offset `<= maximum_dynamic_offset = capacity - window`, so this is the
    /// binding constraint that sizes the buffer (invariant 2).
    last_offset: usize,
}

/// Plan the frame's buffer layout. Returns the per-collection placements plus
/// the two values `record_draws` needs to size the buffer and the dynamic-offset
/// window: `frame_max_region` (the largest padded region) and `last_offset` (the
/// largest dynamic offset). Collections with zero live sprites are skipped.
/// Returns `None` when nothing is drawable this frame.
///
/// Capacity is no longer folded in here: the buffer is sized in `record_draws`
/// from a *monotonic* window, so the capacity formula lives next to the growth
/// logic that owns the window.
fn plan_frame_layout<'a>(collections: &[(&'a str, &'a [u8])]) -> Option<FrameLayout<'a>> {
    let mut placements = Vec::new();
    let mut cursor = 0usize;
    let mut frame_max_region = 0usize;
    for &(collection, packed_bytes) in collections {
        let live_count = packed_bytes.len() / SPRITE_INSTANCE_SIZE;
        if live_count == 0 {
            continue;
        }
        let region = align_up_to_dynamic_offset(live_count * SPRITE_INSTANCE_SIZE);
        frame_max_region = frame_max_region.max(region);
        placements.push(CollectionPlacement {
            collection,
            packed_bytes,
            offset: cursor as u32,
            live_count: live_count as u32,
        });
        cursor += region;
    }
    if placements.is_empty() {
        return None;
    }
    let last_offset = placements.last().map(|p| p.offset as usize).unwrap_or(0);
    Some(FrameLayout {
        placements,
        frame_max_region,
        last_offset,
    })
}

// `sh_sample.wgsl` reads `sh_total_atlas`, `sh_depth_moments`, and `sh_grid`,
// declared in `billboard.wgsl`; WGSL resolves module-scope names regardless of
// textual order, so appending after is safe. The helper owns the SH
// reconstruction + 8-corner blend symbols (`sh_irradiance`,
// `sample_sh_indirect_corners_depth_aware`, `sample_sh_indirect_corners_without_depth`)
// — billboard must not redeclare them. See rendering_pipeline.md §8.
const BILLBOARD_SHADER_SOURCE: &str = concat!(
    include_str!("../shaders/billboard.wgsl"),
    "\n",
    include_str!("../shaders/sh_sample.wgsl"),
);

/// Stitch a set of animation frames into a single horizontal strip
/// (`N × H` per frame) for GPU upload. All frames must have matching
/// dimensions; frames with mismatched sizes are dropped with a warning.
/// Returns `None` if no frames survive.
fn stitch_frames_to_strip(frames: &[SpriteFrame]) -> Option<(Vec<u8>, u32, u32, u32)> {
    let first = frames.first()?;
    let w = first.width;
    let h = first.height;
    if w == 0 || h == 0 {
        return None;
    }
    let valid: Vec<&SpriteFrame> = frames
        .iter()
        .enumerate()
        .filter_map(|(i, f)| {
            if f.width == w && f.height == h {
                Some(f)
            } else {
                log::warn!(
                    "[Smoke] Frame {i} size {}x{} differs from frame 0 {}x{} — dropping",
                    f.width,
                    f.height,
                    w,
                    h,
                );
                None
            }
        })
        .collect();
    if valid.is_empty() {
        return None;
    }
    let frame_count = valid.len() as u32;
    let strip_w = w * frame_count;
    let mut data = vec![0u8; (strip_w * h * 4) as usize];
    for (fi, frame) in valid.iter().enumerate() {
        let x_offset = fi as u32 * w;
        for y in 0..h {
            let src_row = (y * w * 4) as usize;
            let dst_row = (y * strip_w * 4 + x_offset * 4) as usize;
            let row_bytes = (w * 4) as usize;
            data[dst_row..dst_row + row_bytes]
                .copy_from_slice(&frame.data[src_row..src_row + row_bytes]);
        }
    }
    Some((data, strip_w, h, frame_count))
}

/// One loaded sprite sheet, shared across all emitters whose `collection`
/// matches.
pub struct SpriteSheet {
    /// Sprite sheet texture bind group (group 1 of the billboard pipeline).
    pub bind_group: wgpu::BindGroup,
    /// Number of animation frames. 1 when the collection has a single PNG.
    #[allow(dead_code)]
    pub frame_count: u32,
}

/// Pack `SpriteDrawParams` bytes for a (frame_count, spec_intensity, lifetime) tuple.
fn build_draw_params(
    frame_count: u32,
    spec_intensity: f32,
    lifetime: f32,
) -> [u8; SPRITE_DRAW_PARAMS_SIZE] {
    let mut bytes = [0u8; SPRITE_DRAW_PARAMS_SIZE];
    // params.x = bitcast<f32>(frame_count)
    bytes[0..4].copy_from_slice(&frame_count.to_ne_bytes());
    bytes[4..8].copy_from_slice(&spec_intensity.to_ne_bytes());
    bytes[8..12].copy_from_slice(&lifetime.to_ne_bytes());
    // pad at 12..16 stays zero
    bytes
}

/// GPU resources for the billboard sprite pass.
pub struct SmokePass {
    pipeline: wgpu::RenderPipeline,

    /// Group 1 layout: sprite texture + sampler + draw-params uniform.
    /// Retained so per-collection bind groups can be built post-init as
    /// `register_collection` is called.
    sheet_bind_group_layout: wgpu::BindGroupLayout,

    /// Group 6 layout: the sprite instance storage buffer, declared with
    /// `has_dynamic_offset: true` so each collection draws from its own
    /// 256-byte-aligned region of the single shared buffer. Retained so the
    /// per-frame bind group can be rebuilt when the buffer grows.
    instance_bind_group_layout: wgpu::BindGroupLayout,
    /// Single shared upload target for *all* collections' packed sprite
    /// instances this frame. Grown on demand when a frame's total live-sprite
    /// footprint (padded per collection for dynamic-offset alignment) exceeds
    /// the current capacity. Replaces the old fixed 4096-sprite-per-collection
    /// buffer that silently truncated overflow.
    instance_buffer: wgpu::Buffer,
    /// Current byte capacity of `instance_buffer`.
    instance_buffer_capacity: usize,
    /// Byte size of the window the current group-6 bind group binds (its
    /// explicit `size`). Monotonically non-decreasing — it only ever grows to
    /// the largest single-collection region seen so far, so the bind group is
    /// rebuilt rarely. wgpu derives `maximum_dynamic_offset = capacity - window`
    /// from this, so `capacity` must stay `>= last_offset + window` every frame.
    instance_window: u64,
    /// Group-6 bind group over a `instance_window`-sized window of
    /// `instance_buffer`, bound with a per-collection dynamic offset at draw
    /// time. Rebuilt only when the buffer grows or the window grows (the dynamic
    /// offset, not a new bind group, selects each collection's region within a
    /// frame).
    instance_bind_group: wgpu::BindGroup,

    /// Loaded sprite sheets keyed by collection name. Populated at level load.
    sheets: HashMap<String, SpriteSheet>,

    /// Shared linear sampler for sprite sheets.
    sampler: wgpu::Sampler,
}

impl SmokePass {
    /// Build the billboard pipeline. `bgls` carries the renderer-owned bind
    /// group layouts shared with the forward pass (camera, lighting, SH volume).
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        device: &wgpu::Device,
        surface_format: wgpu::TextureFormat,
        depth_format: wgpu::TextureFormat,
        camera_bgl: &wgpu::BindGroupLayout,
        lighting_bgl: &wgpu::BindGroupLayout,
        sh_bgl: &wgpu::BindGroupLayout,
    ) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Billboard Shader"),
            source: wgpu::ShaderSource::Wgsl(BILLBOARD_SHADER_SOURCE.into()),
        });

        // Group 1: sprite texture (binding 0) + sampler (binding 1)
        // + draw-params uniform (binding 2).
        let sheet_bind_group_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("Sprite Sheet BGL"),
                entries: &[
                    wgpu::BindGroupLayoutEntry {
                        binding: 0,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Texture {
                            sample_type: wgpu::TextureSampleType::Float { filterable: true },
                            view_dimension: wgpu::TextureViewDimension::D2,
                            multisampled: false,
                        },
                        count: None,
                    },
                    wgpu::BindGroupLayoutEntry {
                        binding: 1,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                        count: None,
                    },
                    wgpu::BindGroupLayoutEntry {
                        binding: 2,
                        visibility: wgpu::ShaderStages::VERTEX | wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Buffer {
                            ty: wgpu::BufferBindingType::Uniform,
                            has_dynamic_offset: false,
                            min_binding_size: None,
                        },
                        count: None,
                    },
                ],
            });

        // Group 6: sprite instance storage buffer. `has_dynamic_offset: true`
        // lets each collection draw from its own 256-byte-aligned region of the
        // single shared buffer — the dynamic offset rebases `sprites[0]` in the
        // shader to that collection's first instance.
        //
        // `min_binding_size` is the per-instance stride (shader-side floor): with
        // a dynamic offset and `array<SpriteInstance>` (runtime-sized), it tells
        // wgpu the bound window must cover at least one instance. The bound
        // window we actually pass (`build_instance_bind_group`'s explicit `size`)
        // is `frame_max_region` ≥ 256 B ≥ this 32-byte floor, so it is always
        // satisfied.
        //
        // NOTE: `min_binding_size` does NOT gate the dynamic offset in wgpu-29.
        // The maximum legal dynamic offset is derived solely from the bound
        // window: `maximum_dynamic_offset = buffer.size - bound_size`
        // (`min_binding_size` is validated separately and does not feed it). So
        // the real dynamic-offset gate is the explicit window passed in
        // `build_instance_bind_group`, sized and reserved by `record_draws` — NOT
        // this field. Binding the whole buffer (`as_entire_binding`) would make
        // `bound_size == buffer.size`, forcing `maximum_dynamic_offset == 0` and
        // rejecting every collection past offset 0.
        let instance_bind_group_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("Sprite Instance BGL"),
                entries: &[wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: true },
                        has_dynamic_offset: true,
                        min_binding_size: NonZeroU64::new(SPRITE_INSTANCE_SIZE as u64),
                    },
                    count: None,
                }],
            });

        // Pipeline layout: group 0 (camera), 1 (sheet), 2 (lighting),
        // 3 (SH volume), then groups 4 and 5 are unused by this pipeline
        // (group 6 sits after). wgpu allows a sparse layout — we simply
        // pass placeholder slots as `None` for the unused groups.
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("Billboard Pipeline Layout"),
            bind_group_layouts: &[
                Some(camera_bgl),
                Some(&sheet_bind_group_layout),
                Some(lighting_bgl),
                Some(sh_bgl),
                // Groups 4 and 5 are declared as None so the pipeline layout
                // only references the groups the shader actually binds.
                None,
                None,
                Some(&instance_bind_group_layout),
            ],
            immediate_size: 0,
        });

        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("Billboard Pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                buffers: &[],
                compilation_options: wgpu::PipelineCompilationOptions::default(),
            },
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                front_face: wgpu::FrontFace::Ccw,
                cull_mode: None,
                ..Default::default()
            },
            depth_stencil: Some(wgpu::DepthStencilState {
                format: depth_format,
                // Depth test enabled, write disabled: sprites occlude behind
                // geometry but don't occlude each other or write into the
                // depth buffer (additive blend of translucent smoke).
                depth_write_enabled: Some(false),
                depth_compare: Some(wgpu::CompareFunction::Less),
                stencil: wgpu::StencilState::default(),
                bias: wgpu::DepthBiasState::default(),
            }),
            multisample: wgpu::MultisampleState::default(),
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_main"),
                targets: &[Some(wgpu::ColorTargetState {
                    format: surface_format,
                    // Additive alpha blend: smoke accumulates without
                    // darkening the scene behind it.
                    blend: Some(wgpu::BlendState {
                        color: wgpu::BlendComponent {
                            src_factor: wgpu::BlendFactor::One,
                            dst_factor: wgpu::BlendFactor::One,
                            operation: wgpu::BlendOperation::Add,
                        },
                        alpha: wgpu::BlendComponent {
                            src_factor: wgpu::BlendFactor::One,
                            dst_factor: wgpu::BlendFactor::One,
                            operation: wgpu::BlendOperation::Add,
                        },
                    }),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: wgpu::PipelineCompilationOptions::default(),
            }),
            multiview_mask: None,
            cache: None,
        });

        // Single shared instance storage buffer for all collections. Sized for
        // a modest initial frame footprint and grown on demand by `record_draws`
        // when a frame's padded total exceeds it — no per-collection cap.
        let instance_buffer_capacity = align_up_to_dynamic_offset(1024 * SPRITE_INSTANCE_SIZE);
        let instance_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Sprite Instance Buffer"),
            size: instance_buffer_capacity as u64,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        // Seed the window at one storage-alignment unit (256 B): strictly less
        // than the initial capacity (32768 B), so `maximum_dynamic_offset =
        // capacity - window > 0` even before any growth, and a multiple of the
        // 256-byte storage alignment (invariant 3). `record_draws` grows it
        // monotonically to each frame's `frame_max_region` as needed; the first
        // frame is valid because `record_draws` raises the window to at least
        // that frame's `frame_max_region` and grows capacity to keep
        // `capacity >= last_offset + window` before recording any draw.
        let instance_window = STORAGE_DYNAMIC_OFFSET_ALIGNMENT as u64;
        let instance_bind_group = build_instance_bind_group(
            device,
            &instance_bind_group_layout,
            &instance_buffer,
            instance_window,
        );

        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("Sprite Sampler"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Nearest,
            ..Default::default()
        });

        Self {
            pipeline,
            sheet_bind_group_layout,
            instance_bind_group_layout,
            instance_buffer,
            instance_buffer_capacity,
            instance_window,
            instance_bind_group,
            sheets: HashMap::new(),
            sampler,
        }
    }

    /// Register a sprite sheet collection. Uploads the stitched strip as a
    /// single horizontal-strip RGBA8 texture and creates the per-collection
    /// bind group (group 1). Does nothing if the collection was already
    /// registered, or if the frame list is empty or contains mismatched sizes.
    pub fn register_collection(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        collection: &str,
        frames: &[SpriteFrame],
        spec_intensity: f32,
        lifetime: f32,
    ) {
        if self.sheets.contains_key(collection) {
            return;
        }
        let Some((strip_data, strip_w, strip_h, frame_count)) = stitch_frames_to_strip(frames)
        else {
            log::warn!("[Smoke] Collection '{collection}' had no usable frames");
            return;
        };
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some(&format!("Sprite Sheet: {collection}")),
            size: wgpu::Extent3d {
                width: strip_w,
                height: strip_h,
                depth_or_array_layers: 1,
            },
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
            &strip_data,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(strip_w * 4),
                rows_per_image: Some(strip_h),
            },
            wgpu::Extent3d {
                width: strip_w,
                height: strip_h,
                depth_or_array_layers: 1,
            },
        );
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());

        let params_bytes = build_draw_params(frame_count, spec_intensity, lifetime);
        let params_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some(&format!("Sprite Draw Params: {collection}")),
            contents: &params_bytes,
            usage: wgpu::BufferUsages::UNIFORM,
        });

        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some(&format!("Sprite Sheet Bind Group: {collection}")),
            layout: &self.sheet_bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&self.sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: params_buffer.as_entire_binding(),
                },
            ],
        });

        self.sheets.insert(
            collection.to_string(),
            SpriteSheet {
                bind_group,
                frame_count,
            },
        );
    }

    /// Whether any collection is registered. Used by the renderer to skip the
    /// pass entirely on levels with no emitters.
    pub fn has_any_sheet(&self) -> bool {
        !self.sheets.is_empty()
    }

    /// Upload every collection's packed sprite instances into the single shared
    /// buffer and record one draw call per collection from its own region.
    ///
    /// Each `(collection, packed_bytes)` slice carries
    /// `live_count * SPRITE_INSTANCE_SIZE` bytes (packed by
    /// `scripting::systems::particle_render::pack_particle_instance`). The caller
    /// batches all emitters sharing a collection into one slice, so a collection
    /// still issues exactly one draw — N collections produce N draws.
    ///
    /// **Buffer sizing / growth.** The frame's regions are laid out back-to-back,
    /// each padded up to the 256-byte storage dynamic-offset alignment so its
    /// start offset is a legal dynamic offset (the 32-byte per-instance stride is
    /// unchanged *within* a region). The group-6 bind group binds an explicit
    /// `window` (a monotonic high-water mark of the largest single collection's
    /// padded region), so wgpu's `maximum_dynamic_offset = capacity - window`
    /// stays `>= last_offset`. The buffer is recreated larger when
    /// `last_offset + window` exceeds capacity, and the bind group is rebuilt
    /// when the buffer object or the window changes — there is **no
    /// per-collection cap**, so a single collection may exceed the old fixed
    /// 4096-sprite buffer without silent truncation.
    ///
    /// Each collection is uploaded once at its own offset (no redundant offset-0
    /// re-upload per collection) and drawn via the dynamic-offset bind group,
    /// which rebases `sprites[0]` in the shader to that region's first instance.
    pub fn record_draws<'a>(
        &'a mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        pass: &mut wgpu::RenderPass<'a>,
        collections: &[(&str, &[u8])],
    ) {
        let Some(FrameLayout {
            placements,
            frame_max_region,
            last_offset,
        }) = plan_frame_layout(collections)
        else {
            return;
        };

        // Size the dynamic-offset window and the buffer so wgpu's per-draw
        // `offset <= maximum_dynamic_offset = capacity - window` holds for every
        // collection:
        //   - `new_window = max(current, frame_max_region)` is monotonic and
        //     covers the largest collection's region (invariant 1).
        //   - `required_capacity = last_offset + new_window` makes
        //     `maximum_dynamic_offset = capacity - new_window >= last_offset`,
        //     and `last_offset` is the largest offset (invariant 2).
        // The buffer grows when capacity is short; the bind group is rebuilt
        // when the buffer object changes OR the window changes.
        let new_window = self.instance_window.max(frame_max_region as u64);
        let required_capacity = last_offset + new_window as usize;
        let need_buffer_grow = required_capacity > self.instance_buffer_capacity;
        let need_bg_rebuild = need_buffer_grow || new_window != self.instance_window;

        if need_buffer_grow {
            let new_capacity = align_up_to_dynamic_offset(required_capacity);
            self.instance_buffer = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("Sprite Instance Buffer"),
                size: new_capacity as u64,
                usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            self.instance_buffer_capacity = new_capacity;
        }
        if need_bg_rebuild {
            self.instance_window = new_window;
            self.instance_bind_group = build_instance_bind_group(
                device,
                &self.instance_bind_group_layout,
                &self.instance_buffer,
                self.instance_window,
            );
        }
        // Window never exceeds capacity: window <= last_offset + window =
        // required_capacity <= capacity.
        debug_assert!(self.instance_window <= self.instance_buffer_capacity as u64);

        // Upload each collection at its aligned offset (one write per collection,
        // no full re-upload at offset 0).
        for placement in &placements {
            queue.write_buffer(
                &self.instance_buffer,
                placement.offset as u64,
                placement.packed_bytes,
            );
        }

        pass.set_pipeline(&self.pipeline);
        for placement in &placements {
            let Some(sheet) = self.sheets.get(placement.collection) else {
                continue;
            };
            pass.set_bind_group(1, &sheet.bind_group, &[]);
            pass.set_bind_group(6, &self.instance_bind_group, &[placement.offset]);
            // Non-indexed draw of 6 vertices per sprite, rebased to this
            // collection's region by the group-6 dynamic offset.
            pass.draw(0..(placement.live_count * 6), 0..1);
        }
    }
}

// --- Tests ---

#[cfg(test)]
mod tests {
    use super::*;

    /// Billboard shader must parse cleanly and declare the expected entry
    /// points. Parses the full concatenated source (billboard + the shared
    /// `sh_sample.wgsl` helper) so the helper's compilation in this pipeline is
    /// covered. Catches WGSL regressions before they reach pipeline creation.
    #[test]
    fn billboard_wgsl_parses() {
        let module = naga::front::wgsl::parse_str(BILLBOARD_SHADER_SOURCE)
            .expect("billboard shader should parse as WGSL");
        let has_vs = module
            .entry_points
            .iter()
            .any(|ep| ep.name == "vs_main" && ep.stage == naga::ShaderStage::Vertex);
        let has_fs = module
            .entry_points
            .iter()
            .any(|ep| ep.name == "fs_main" && ep.stage == naga::ShaderStage::Fragment);
        assert!(has_vs, "billboard.wgsl must export @vertex vs_main");
        assert!(has_fs, "billboard.wgsl must export @fragment fs_main");
    }

    /// The full billboard pipeline source (billboard + `sh_sample.wgsl`) must
    /// pass naga's validation, including control-flow uniformity. `parse_str`
    /// alone does not enforce this; a future edit that breaks the shared
    /// helper's compilation in the billboard pipeline is caught here at
    /// `cargo test` time, before GPU pipeline creation.
    #[test]
    fn billboard_wgsl_passes_naga_validation() {
        let module = naga::front::wgsl::parse_str(BILLBOARD_SHADER_SOURCE)
            .expect("billboard shader must parse");
        naga::valid::Validator::new(
            naga::valid::ValidationFlags::all(),
            naga::valid::Capabilities::all(),
        )
        .validate(&module)
        .expect("billboard shader must pass naga validation");
    }

    /// The `SpriteInstance` WGSL struct must match the CPU-side
    /// `SPRITE_INSTANCE_SIZE` byte layout.
    #[test]
    fn billboard_wgsl_sprite_instance_stride_matches_cpu() {
        // Parse the full concatenated source: `billboard.wgsl` references
        // symbols from `sh_sample.wgsl` and cannot parse standalone. The
        // `SpriteInstance` struct span is identical regardless of the appended
        // helper source.
        let module = naga::front::wgsl::parse_str(BILLBOARD_SHADER_SOURCE).unwrap();
        let span = module
            .types
            .iter()
            .find_map(|(_, ty)| match (&ty.name, &ty.inner) {
                (Some(name), naga::TypeInner::Struct { span, .. }) if name == "SpriteInstance" => {
                    Some(*span)
                }
                _ => None,
            })
            .expect("billboard.wgsl should declare struct SpriteInstance");
        assert_eq!(
            span as usize, SPRITE_INSTANCE_SIZE,
            "billboard.wgsl SpriteInstance stride ({span}) must match SPRITE_INSTANCE_SIZE ({SPRITE_INSTANCE_SIZE})",
        );
    }

    #[test]
    fn draw_params_layout() {
        let bytes = build_draw_params(8, 0.3, 3.0);
        assert_eq!(bytes.len(), SPRITE_DRAW_PARAMS_SIZE);
        let count = u32::from_ne_bytes(bytes[0..4].try_into().unwrap());
        assert_eq!(count, 8);
        let spec = f32::from_ne_bytes(bytes[4..8].try_into().unwrap());
        assert!((spec - 0.3).abs() < 1e-6);
        let lifetime = f32::from_ne_bytes(bytes[8..12].try_into().unwrap());
        assert!((lifetime - 3.0).abs() < 1e-6);
    }

    #[test]
    fn stitch_rejects_empty() {
        assert!(stitch_frames_to_strip(&[]).is_none());
    }

    #[test]
    fn stitch_single_frame() {
        let frame = SpriteFrame {
            data: vec![0xFFu8; 4 * 2 * 2], // 2x2 white RGBA
            width: 2,
            height: 2,
        };
        let (data, w, h, count) = stitch_frames_to_strip(&[frame]).unwrap();
        assert_eq!(w, 2);
        assert_eq!(h, 2);
        assert_eq!(count, 1);
        assert_eq!(data.len(), 2 * 2 * 4);
    }

    #[test]
    fn stitch_two_frames() {
        let red = SpriteFrame {
            data: vec![
                255, 0, 0, 255, 255, 0, 0, 255, 255, 0, 0, 255, 255, 0, 0, 255,
            ],
            width: 2,
            height: 2,
        };
        let blue = SpriteFrame {
            data: vec![
                0, 0, 255, 255, 0, 0, 255, 255, 0, 0, 255, 255, 0, 0, 255, 255,
            ],
            width: 2,
            height: 2,
        };
        let (data, w, h, count) = stitch_frames_to_strip(&[red, blue]).unwrap();
        assert_eq!(w, 4);
        assert_eq!(h, 2);
        assert_eq!(count, 2);
        // First column should be red, third column blue.
        assert_eq!(data[0..4], [255, 0, 0, 255]);
        assert_eq!(data[8..12], [0, 0, 255, 255]);
    }

    /// A dummy packed slice of `n` sprite instances (contents irrelevant to the
    /// layout planner — only the byte length matters).
    fn packed(n: usize) -> Vec<u8> {
        vec![0u8; n * SPRITE_INSTANCE_SIZE]
    }

    #[test]
    fn align_up_rounds_to_256_byte_boundary() {
        assert_eq!(align_up_to_dynamic_offset(0), 0);
        assert_eq!(align_up_to_dynamic_offset(1), 256);
        assert_eq!(align_up_to_dynamic_offset(256), 256);
        assert_eq!(align_up_to_dynamic_offset(257), 512);
        // One 32-byte instance still pads up to a full 256-byte region.
        assert_eq!(align_up_to_dynamic_offset(SPRITE_INSTANCE_SIZE), 256);
    }

    #[test]
    fn plan_layout_empty_or_all_zero_returns_none() {
        assert!(plan_frame_layout(&[]).is_none());
        let empty: Vec<u8> = Vec::new();
        assert!(plan_frame_layout(&[("smoke", &empty)]).is_none());
    }

    /// The capacity the buffer must reach for a given frame layout, computed the
    /// same way `record_draws` does from a monotonic window. Mirrors the growth
    /// math so the layout tests can assert the binding contract without a GPU.
    fn required_capacity(layout: &FrameLayout, prior_window: u64) -> (u64, usize) {
        let window = prior_window.max(layout.frame_max_region as u64);
        (window, layout.last_offset + window as usize)
    }

    #[test]
    fn plan_layout_single_collection_starts_at_zero_with_full_count() {
        let bytes = packed(10);
        let layout = plan_frame_layout(&[("smoke", &bytes)]).unwrap();
        assert_eq!(layout.placements.len(), 1);
        assert_eq!(layout.placements[0].offset, 0);
        assert_eq!(layout.placements[0].live_count, 10);
    }

    #[test]
    fn plan_layout_offsets_are_256_aligned_and_non_overlapping() {
        // 10 instances = 320 bytes → padded to 512; next region starts at 512.
        let a = packed(10);
        let b = packed(3);
        let layout = plan_frame_layout(&[("smoke", &a), ("spark", &b)]).unwrap();
        let placements = &layout.placements;
        assert_eq!(placements.len(), 2);
        assert_eq!(placements[0].offset, 0);
        assert_eq!(placements[1].offset, 512);
        for p in placements {
            assert_eq!(
                p.offset as usize % STORAGE_DYNAMIC_OFFSET_ALIGNMENT,
                0,
                "every collection's dynamic offset must be 256-byte aligned",
            );
        }
        // Region 0 spans bytes [0, 320) of live data within its 512-byte padded
        // region, so region 1 at offset 512 cannot overlap it.
        assert!(placements[1].offset as usize >= a.len());
    }

    #[test]
    fn plan_layout_single_collection_exceeds_old_4096_cap_without_truncation() {
        // The old buffer silently truncated a collection to 4096 sprites. The
        // planner preserves the full count and reserves a region large enough to
        // hold all of them.
        let count = 9000;
        let bytes = packed(count);
        let layout = plan_frame_layout(&[("smoke", &bytes)]).unwrap();
        assert_eq!(layout.placements[0].live_count, count as u32);
        // A fresh pass would seed its window at the 256-byte alignment unit;
        // `record_draws` raises it to this frame's `frame_max_region`.
        let (_window, capacity) =
            required_capacity(&layout, STORAGE_DYNAMIC_OFFSET_ALIGNMENT as u64);
        assert!(
            capacity >= count * SPRITE_INSTANCE_SIZE,
            "buffer capacity must hold every live sprite, not just 4096",
        );
    }

    #[test]
    fn plan_layout_capacity_covers_min_binding_size_window_at_every_offset() {
        // The group-6 BGL declares `min_binding_size = SPRITE_INSTANCE_SIZE`
        // (one instance stride). This is the shader-side floor on the bound
        // window — NOT the dynamic-offset gate (see the note in `SmokePass::new`).
        // The capacity must still clear that floor for *every* collection's
        // offset, so each region holds at least one instance. The per-collection
        // padded region (≥ 256 B) the capacity reserves dominates the 32-byte
        // floor, so this holds by construction; the test pins it so a future
        // capacity-formula change can't silently violate the binding contract.
        let a = packed(10);
        let b = packed(3);
        let c = packed(50);
        let layout = plan_frame_layout(&[("smoke", &a), ("spark", &b), ("dust", &c)]).unwrap();
        let (_window, capacity) =
            required_capacity(&layout, STORAGE_DYNAMIC_OFFSET_ALIGNMENT as u64);
        for p in &layout.placements {
            assert!(
                p.offset as usize + SPRITE_INSTANCE_SIZE <= capacity,
                "offset {} + min_binding_size {} must fit in capacity {}",
                p.offset,
                SPRITE_INSTANCE_SIZE,
                capacity,
            );
        }
    }

    #[test]
    fn plan_layout_capacity_covers_the_largest_collection_window() {
        // The dynamic-offset bind-group window is sized to the largest region,
        // so the reported capacity must cover `last_offset + largest_window`
        // even when the largest collection is not last.
        let big = packed(100); // 3200 B → padded 3328 (13 × 256)
        let small = packed(2); // 64 B → padded 256
        let layout = plan_frame_layout(&[("big", &big), ("small", &small)]).unwrap();
        let last = layout.placements.last().unwrap();
        let largest_window = align_up_to_dynamic_offset(100 * SPRITE_INSTANCE_SIZE);
        // The window is the largest region even when it is not the last
        // collection; capacity must cover `last_offset + window`.
        let (window, capacity) =
            required_capacity(&layout, STORAGE_DYNAMIC_OFFSET_ALIGNMENT as u64);
        assert_eq!(window as usize, largest_window);
        assert_eq!(capacity, last.offset as usize + largest_window);
    }

    /// THE regression guard for the wgpu-29 dynamic-offset bug. With
    /// `as_entire_binding()` the bound window equals the whole buffer, so
    /// wgpu-29 derives `maximum_dynamic_offset = buffer.size - window = 0` and
    /// rejects every collection past offset 0. The fix binds an explicit window
    /// (`record_draws`'s monotonic high-water mark) so capacity is sized to
    /// `last_offset + window`. This test pins, for a multi-collection frame,
    /// that the window/capacity math `record_draws` uses keeps every dynamic
    /// offset legal:
    ///   - `last_offset + window <= capacity` (window fits, invariant 2), and
    ///   - `maximum_dynamic_offset = capacity - window >= every placement.offset`
    ///     (every collection's offset is admissible, the exact wgpu-29 gate).
    /// Also checks the window itself is 256-aligned (storage size alignment,
    /// invariant 3) so the bound `size` is a legal storage binding size.
    #[test]
    fn dynamic_offset_never_exceeds_maximum_for_every_collection() {
        // A spread of collection sizes (largest is NOT last) so the window is
        // driven by an interior collection and the gate is non-trivial.
        let a = packed(7); // 224 B → padded 256
        let big = packed(300); // 9600 B → padded 9728 (38 × 256)
        let c = packed(40); // 1280 B → padded 1280 (5 × 256)
        let d = packed(1); // 32 B → padded 256
        let layout = plan_frame_layout(&[("a", &a), ("big", &big), ("c", &c), ("d", &d)]).unwrap();

        // Seed the prior window the way a fresh `SmokePass` does: one alignment
        // unit. `record_draws` raises it to this frame's `frame_max_region`.
        let (window, capacity) =
            required_capacity(&layout, STORAGE_DYNAMIC_OFFSET_ALIGNMENT as u64);

        // Window is a legal storage binding size (multiple of 256) and fits.
        assert_eq!(
            window as usize % STORAGE_DYNAMIC_OFFSET_ALIGNMENT,
            0,
            "bound window must be a multiple of the 256-byte storage alignment",
        );
        assert!(
            layout.last_offset + window as usize <= capacity,
            "last_offset {} + window {} must fit in capacity {} (invariant 2)",
            layout.last_offset,
            window,
            capacity,
        );

        // The wgpu-29 gate: every collection's dynamic offset must be
        // <= maximum_dynamic_offset = capacity - window.
        let maximum_dynamic_offset = capacity - window as usize;
        for p in &layout.placements {
            assert!(
                p.offset as usize <= maximum_dynamic_offset,
                "collection '{}' offset {} exceeds maximum_dynamic_offset {} \
                 (capacity {} - window {})",
                p.collection,
                p.offset,
                maximum_dynamic_offset,
                capacity,
                window,
            );
        }
    }
}
