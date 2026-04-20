// Billboard sprite rendering pass: camera-facing quads for env_smoke_emitter
// entities, expanded in the vertex shader from a storage buffer of per-sprite
// instance data. Lit by the full lighting stack (SH ambient + static
// multi-source specular via the chunk list + dynamic diffuse). Alpha-additive
// blend, depth test enabled, depth write disabled.
//
// See: context/lib/rendering_pipeline.md §7.4

use std::collections::HashMap;

use wgpu::util::DeviceExt;

use crate::fx::smoke::{MAX_SPRITES, SPRITE_INSTANCE_SIZE, SpriteFrame};

/// Byte size of `SpriteDrawParams` (one `vec4<f32>` = 16 B, padded to 16).
pub const SPRITE_DRAW_PARAMS_SIZE: usize = 16;

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

    /// Group 6 bind group: sprite instance storage buffer.
    instance_bind_group: wgpu::BindGroup,
    /// Per-frame upload target for packed sprite instances. Sized at creation
    /// for `MAX_SPRITES * SPRITE_INSTANCE_SIZE` bytes and reused every frame.
    instance_buffer: wgpu::Buffer,

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
        let shader_src = include_str!("../shaders/billboard.wgsl");
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Billboard Shader"),
            source: wgpu::ShaderSource::Wgsl(shader_src.into()),
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

        // Group 6: sprite instance storage buffer.
        let instance_bind_group_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("Sprite Instance BGL"),
                entries: &[wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX | wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: true },
                        has_dynamic_offset: false,
                        min_binding_size: None,
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

        // Instance storage buffer — sized for the upper bound per collection
        // draw. We size it for one emitter's worth of sprites since at
        // retro-scale a single pass batches by collection and we re-upload
        // per-draw; the renderer can allocate more if multiple collections
        // coexist. A future optimization is a single buffer with per-collection
        // offsets, but that's not on the acceptance-gate path.
        let instance_buffer_size = (MAX_SPRITES * SPRITE_INSTANCE_SIZE).max(SPRITE_INSTANCE_SIZE);
        let instance_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Sprite Instance Buffer"),
            size: instance_buffer_size as u64,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let instance_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Sprite Instance Bind Group"),
            layout: &instance_bind_group_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: instance_buffer.as_entire_binding(),
            }],
        });

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
            instance_bind_group,
            instance_buffer,
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

    /// Upload packed sprite instances for one collection and record the draw
    /// on the passed render pass. `packed_bytes` must contain
    /// `live_count * SPRITE_INSTANCE_SIZE` bytes (see
    /// [`crate::fx::smoke::SmokeEmitter::pack_instances`]).
    ///
    /// This is a **per-collection** draw: the caller batches emitters sharing
    /// a collection into a single packed buffer and one call here.
    pub fn record_draw<'a>(
        &'a self,
        queue: &wgpu::Queue,
        pass: &mut wgpu::RenderPass<'a>,
        collection: &str,
        packed_bytes: &[u8],
    ) {
        let Some(sheet) = self.sheets.get(collection) else {
            return;
        };
        let live_count = packed_bytes.len() / SPRITE_INSTANCE_SIZE;
        if live_count == 0 {
            return;
        }
        let capped = live_count.min(MAX_SPRITES);
        let byte_len = capped * SPRITE_INSTANCE_SIZE;
        queue.write_buffer(&self.instance_buffer, 0, &packed_bytes[..byte_len]);

        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(1, &sheet.bind_group, &[]);
        pass.set_bind_group(6, &self.instance_bind_group, &[]);
        // Non-indexed draw of 6 vertices per sprite.
        pass.draw(0..(capped as u32 * 6), 0..1);
    }
}

// --- Tests ---

#[cfg(test)]
mod tests {
    use super::*;

    /// Billboard shader must parse cleanly and declare the expected entry
    /// points. Catches WGSL regressions before they reach pipeline creation.
    #[test]
    fn billboard_wgsl_parses() {
        let src = include_str!("../shaders/billboard.wgsl");
        let module =
            naga::front::wgsl::parse_str(src).expect("billboard.wgsl should parse as WGSL");
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

    /// The `SpriteInstance` WGSL struct must match the CPU-side
    /// `SPRITE_INSTANCE_SIZE` byte layout.
    #[test]
    fn billboard_wgsl_sprite_instance_stride_matches_cpu() {
        let src = include_str!("../shaders/billboard.wgsl");
        let module = naga::front::wgsl::parse_str(src).unwrap();
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
}
