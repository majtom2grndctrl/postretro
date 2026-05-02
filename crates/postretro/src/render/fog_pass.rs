// Volumetric fog / beam pass GPU resources.
// See: context/lib/rendering_pipeline.md §7.5
//      context/plans/in-progress/fx-volumetric-smoke/index.md Task B
//
// Owns the low-resolution RGBA16F scatter target, the fog-volume AABB
// storage buffer, the fog-params uniform, and the group-6 bind group that
// both the raymarch compute pipeline and the composite blit pipeline read.
//
// The raymarch pipeline reuses group 3 (SH volume) and group 5 (spot shadow
// maps) from the forward pass — the bind-group *objects* are shared, not
// re-uploaded. Group 6 layout is owned here.

use glam::{Mat4, Vec3};
use wgpu::util::DeviceExt;

use crate::fx::fog_volume::{
    self, FOG_PARAMS_SIZE, FOG_POINT_LIGHT_SIZE, FOG_SPOT_LIGHT_SIZE, FOG_VOLUME_SIZE,
    FogPointLight, FogSpotLight, FogVolume, MAX_FOG_POINT_LIGHTS, MAX_FOG_VOLUMES,
    clamp_fog_pixel_scale,
};

/// Format of the low-resolution scatter target.
pub const SCATTER_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;

/// Group 6 binding indices. Mirrored in fog_volume.wgsl and fog_composite.wgsl.
pub const BIND_DEPTH_TEX: u32 = 0;
pub const BIND_VOLUMES: u32 = 1;
pub const BIND_SCATTER_OUT: u32 = 2;
pub const BIND_FOG_PARAMS: u32 = 3;
pub const BIND_FOG_SPOTS: u32 = 4;
pub const BIND_FOG_POINTS: u32 = 5;

pub struct FogPass {
    pub pixel_scale: u32,
    pub step_size: f32,

    // --- Raymarch (compute) pipeline ---
    pub raymarch_pipeline: wgpu::ComputePipeline,
    pub raymarch_bind_group_layout: wgpu::BindGroupLayout,

    // --- Composite (fullscreen blit) pipeline ---
    pub composite_pipeline: wgpu::RenderPipeline,
    pub composite_bind_group: wgpu::BindGroup,
    composite_bgl: wgpu::BindGroupLayout,
    composite_sampler: wgpu::Sampler,

    // --- Buffers ---
    /// Packed fog-volume AABB + params storage buffer. Sized for
    /// `MAX_FOG_VOLUMES` records so the buffer never has to be reallocated.
    pub volumes_buffer: wgpu::Buffer,
    /// Fog-params uniform (inv_view_proj, camera position, step size,
    /// volume count, near/far clip). Rewritten per frame.
    pub params_buffer: wgpu::Buffer,
    /// Per-frame spot-light subset marched by the fog shader.
    pub spots_buffer: wgpu::Buffer,
    /// Per-frame point-light subset marched by the fog shader.
    pub fog_points_buffer: wgpu::Buffer,

    // --- Scatter target ---
    scatter_view: wgpu::TextureView,
    #[allow(dead_code)]
    scatter_texture: wgpu::Texture,
    /// Low-res dimensions currently allocated for the scatter target. Used
    /// to skip reallocation when the surface resizes without changing the
    /// pixel scale.
    scatter_dims: (u32, u32),

    // --- Group 6 bind group, rebuilt whenever the depth view or the
    /// scatter target is recreated (depth on resize, scatter on resize or
    /// pixel-scale change). ---
    pub bind_group: wgpu::BindGroup,

    /// Most recently uploaded fog volume count. Shader loops against this.
    pub volume_count: u32,
    /// Most recent spot light count for dynamic beams.
    #[allow(dead_code)]
    pub spot_count: u32,
}

impl FogPass {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        device: &wgpu::Device,
        surface_width: u32,
        surface_height: u32,
        pixel_scale: u32,
        depth_view: &wgpu::TextureView,
        camera_bgl: &wgpu::BindGroupLayout,
        sh_bgl: &wgpu::BindGroupLayout,
        spot_shadow_bgl: &wgpu::BindGroupLayout,
    ) -> Self {
        let pixel_scale = clamp_fog_pixel_scale(pixel_scale);
        let scatter_dims = scatter_dims_for(surface_width, surface_height, pixel_scale);

        // --- Group 6 layout ---
        let raymarch_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Fog Raymarch BGL (group 6)"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: BIND_DEPTH_TEX,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Depth,
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: BIND_VOLUMES,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: true },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: BIND_SCATTER_OUT,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::StorageTexture {
                        access: wgpu::StorageTextureAccess::WriteOnly,
                        format: SCATTER_FORMAT,
                        view_dimension: wgpu::TextureViewDimension::D2,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: BIND_FOG_PARAMS,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: BIND_FOG_SPOTS,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: true },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: BIND_FOG_POINTS,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: true },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
            ],
        });

        // --- Buffers ---
        //
        // Initial fog-volumes buffer: zero-count dummy record. wgpu rejects
        // zero-sized storage buffers, so we always size for MAX_FOG_VOLUMES
        // and track the real count in `fog.volume_count`.
        let volumes_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Fog Volume AABB Buffer"),
            size: (MAX_FOG_VOLUMES * FOG_VOLUME_SIZE) as u64,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let params_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Fog Params Uniform"),
            size: FOG_PARAMS_SIZE as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        // Upfront allocation for the spot-light storage buffer. Sized for the
        // maximum shadow-map pool (8 slots) so the buffer is never reallocated.
        let spots_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Fog Spot Lights Buffer"),
            contents: &fog_volume::pack_fog_spot_lights(&[]),
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
        });
        // Resize to fit 8 entries so we can rewrite without reallocation.
        let spots_buffer = {
            // Drop the init-only 1-record buffer and re-create at full size.
            drop(spots_buffer);
            device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("Fog Spot Lights Buffer"),
                size: (crate::lighting::spot_shadow::SHADOW_POOL_SIZE * FOG_SPOT_LIGHT_SIZE) as u64,
                usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            })
        };

        // Point-light storage buffer. Sized for MAX_FOG_POINT_LIGHTS so the
        // buffer is never reallocated; per-frame uploads go through
        // `upload_points`.
        let fog_points_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Fog Point Lights Buffer"),
            size: (MAX_FOG_POINT_LIGHTS * FOG_POINT_LIGHT_SIZE) as u64,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        // --- Scatter target ---
        let (scatter_texture, scatter_view) =
            create_scatter_target(device, scatter_dims.0, scatter_dims.1);

        // --- Group 6 bind group ---
        let bind_group = build_group6(
            device,
            &raymarch_bgl,
            depth_view,
            &volumes_buffer,
            &scatter_view,
            &params_buffer,
            &spots_buffer,
            &fog_points_buffer,
        );

        // --- Raymarch compute pipeline ---
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Fog Raymarch Shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("../shaders/fog_volume.wgsl").into()),
        });
        let raymarch_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("Fog Raymarch Pipeline Layout"),
            bind_group_layouts: &[
                Some(camera_bgl),      // group 0
                None,                  // group 1
                None,                  // group 2
                Some(sh_bgl),          // group 3
                None,                  // group 4
                Some(spot_shadow_bgl), // group 5
                Some(&raymarch_bgl),   // group 6
            ],
            immediate_size: 0,
        });
        let raymarch_pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("Fog Raymarch Pipeline"),
            layout: Some(&raymarch_layout),
            module: &shader,
            entry_point: Some("cs_main"),
            compilation_options: wgpu::PipelineCompilationOptions::default(),
            cache: None,
        });

        // --- Composite pipeline (fullscreen blit, additive) ---
        let composite_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("Fog Composite Sampler"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Nearest,
            min_filter: wgpu::FilterMode::Nearest,
            mipmap_filter: wgpu::MipmapFilterMode::Nearest,
            ..Default::default()
        });
        let composite_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Fog Composite BGL"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: false },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::NonFiltering),
                    count: None,
                },
            ],
        });
        let composite_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Fog Composite Bind Group"),
            layout: &composite_bgl,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&scatter_view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&composite_sampler),
                },
            ],
        });
        let composite_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Fog Composite Shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("../shaders/fog_composite.wgsl").into()),
        });
        let composite_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("Fog Composite Pipeline Layout"),
            bind_group_layouts: &[Some(&composite_bgl)],
            immediate_size: 0,
        });
        let composite_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("Fog Composite Pipeline"),
            layout: Some(&composite_layout),
            vertex: wgpu::VertexState {
                module: &composite_shader,
                entry_point: Some("vs_main"),
                buffers: &[],
                compilation_options: wgpu::PipelineCompilationOptions::default(),
            },
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                ..Default::default()
            },
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            fragment: Some(wgpu::FragmentState {
                module: &composite_shader,
                entry_point: Some("fs_main"),
                // Target format is filled in by the caller at pipeline-rebuild
                // time. Keeping it inline here means the surface format is
                // decided at FogPass::new. The renderer passes it in below.
                targets: &[Some(wgpu::ColorTargetState {
                    // Additive: final = scene + fog_scatter. Alpha path is
                    // unused but kept consistent.
                    format: wgpu::TextureFormat::Rgba8UnormSrgb,
                    blend: Some(wgpu::BlendState {
                        color: wgpu::BlendComponent {
                            src_factor: wgpu::BlendFactor::One,
                            dst_factor: wgpu::BlendFactor::One,
                            operation: wgpu::BlendOperation::Add,
                        },
                        alpha: wgpu::BlendComponent::REPLACE,
                    }),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: wgpu::PipelineCompilationOptions::default(),
            }),
            multiview_mask: None,
            cache: None,
        });

        Self {
            pixel_scale,
            step_size: fog_volume::DEFAULT_FOG_STEP_SIZE,
            raymarch_pipeline,
            raymarch_bind_group_layout: raymarch_bgl,
            composite_pipeline,
            composite_bind_group,
            composite_bgl,
            composite_sampler,
            volumes_buffer,
            params_buffer,
            spots_buffer,
            fog_points_buffer,
            scatter_view,
            scatter_texture,
            scatter_dims,
            bind_group,
            volume_count: 0,
            spot_count: 0,
        }
    }

    /// Rebuild the composite pipeline when the surface format changes.
    /// Stored composite bind group stays valid because it only references the
    /// scatter target — unrelated to the output format.
    pub fn rebuild_composite_for_format(
        &mut self,
        device: &wgpu::Device,
        format: wgpu::TextureFormat,
    ) {
        let composite_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Fog Composite Shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("../shaders/fog_composite.wgsl").into()),
        });
        let composite_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("Fog Composite Pipeline Layout"),
            bind_group_layouts: &[Some(&self.composite_bgl)],
            immediate_size: 0,
        });
        self.composite_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("Fog Composite Pipeline"),
            layout: Some(&composite_layout),
            vertex: wgpu::VertexState {
                module: &composite_shader,
                entry_point: Some("vs_main"),
                buffers: &[],
                compilation_options: wgpu::PipelineCompilationOptions::default(),
            },
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                ..Default::default()
            },
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            fragment: Some(wgpu::FragmentState {
                module: &composite_shader,
                entry_point: Some("fs_main"),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: Some(wgpu::BlendState {
                        color: wgpu::BlendComponent {
                            src_factor: wgpu::BlendFactor::One,
                            dst_factor: wgpu::BlendFactor::One,
                            operation: wgpu::BlendOperation::Add,
                        },
                        alpha: wgpu::BlendComponent::REPLACE,
                    }),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: wgpu::PipelineCompilationOptions::default(),
            }),
            multiview_mask: None,
            cache: None,
        });
    }

    /// Resize the scatter target and rebuild the group-6 bind group.
    /// Call on surface resize or `fog_pixel_scale` change.
    pub fn resize(
        &mut self,
        device: &wgpu::Device,
        surface_width: u32,
        surface_height: u32,
        depth_view: &wgpu::TextureView,
    ) {
        let dims = scatter_dims_for(surface_width, surface_height, self.pixel_scale);
        if dims != self.scatter_dims {
            let (tex, view) = create_scatter_target(device, dims.0, dims.1);
            self.scatter_texture = tex;
            self.scatter_view = view;
            self.scatter_dims = dims;
            self.composite_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("Fog Composite Bind Group"),
                layout: &self.composite_bgl,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView(&self.scatter_view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::Sampler(&self.composite_sampler),
                    },
                ],
            });
        }
        // Depth view may have been recreated even if scatter dims are
        // unchanged (e.g., surface resize that happens to match the scale).
        self.bind_group = build_group6(
            device,
            &self.raymarch_bind_group_layout,
            depth_view,
            &self.volumes_buffer,
            &self.scatter_view,
            &self.params_buffer,
            &self.spots_buffer,
            &self.fog_points_buffer,
        );
    }

    /// Set the global `fog_pixel_scale` from worldspawn. Rebuilds the scatter
    /// target if the value changed.
    #[allow(dead_code)]
    pub fn set_pixel_scale(
        &mut self,
        device: &wgpu::Device,
        scale: u32,
        surface_width: u32,
        surface_height: u32,
        depth_view: &wgpu::TextureView,
    ) {
        let clamped = clamp_fog_pixel_scale(scale);
        if clamped == self.pixel_scale {
            return;
        }
        self.pixel_scale = clamped;
        self.resize(device, surface_width, surface_height, depth_view);
    }

    /// Upload the fog volume list. Truncates at `MAX_FOG_VOLUMES` with a
    /// warning. Subsequent frames reuse the buffer until the list changes.
    #[allow(dead_code)]
    pub fn upload_volumes(&mut self, queue: &wgpu::Queue, volumes: &[FogVolume]) {
        let count = volumes.len().min(MAX_FOG_VOLUMES);
        if volumes.len() > MAX_FOG_VOLUMES {
            log::warn!(
                "[FogPass] {} volumes exceeded MAX_FOG_VOLUMES={} — extras dropped",
                volumes.len(),
                MAX_FOG_VOLUMES
            );
        }
        let bytes = fog_volume::pack_fog_volumes(&volumes[..count]);
        if !bytes.is_empty() {
            queue.write_buffer(&self.volumes_buffer, 0, &bytes);
        }
        self.volume_count = count as u32;
    }

    /// Upload the per-frame fog params (inv view-proj, camera pos, step size).
    pub fn upload_params(
        &mut self,
        queue: &wgpu::Queue,
        inv_view_proj: Mat4,
        camera_position: Vec3,
        near_clip: f32,
        far_clip: f32,
    ) {
        let bytes = fog_volume::pack_fog_params(
            inv_view_proj,
            camera_position,
            self.step_size,
            self.volume_count,
            near_clip,
            far_clip,
        );
        queue.write_buffer(&self.params_buffer, 0, &bytes);
    }

    /// Upload the per-frame point-light list for the fog raymarch. Truncates
    /// at `MAX_FOG_POINT_LIGHTS` with a warning.
    #[allow(dead_code)]
    pub fn upload_points(&self, queue: &wgpu::Queue, points: &[FogPointLight]) {
        let count = points.len().min(MAX_FOG_POINT_LIGHTS);
        if points.len() > MAX_FOG_POINT_LIGHTS {
            log::warn!(
                "[FogPass] {} point lights exceeded MAX_FOG_POINT_LIGHTS={} — extras dropped",
                points.len(),
                MAX_FOG_POINT_LIGHTS
            );
        }
        let bytes = fog_volume::pack_fog_point_lights(&points[..count]);
        if !bytes.is_empty() {
            queue.write_buffer(&self.fog_points_buffer, 0, &bytes);
        }
    }

    /// Upload the per-frame spot-light list for the fog raymarch beams.
    #[allow(dead_code)]
    pub fn upload_spots(&mut self, queue: &wgpu::Queue, spots: &[FogSpotLight]) {
        let capped = spots
            .len()
            .min(crate::lighting::spot_shadow::SHADOW_POOL_SIZE);
        let bytes = fog_volume::pack_fog_spot_lights(&spots[..capped]);
        queue.write_buffer(&self.spots_buffer, 0, &bytes);
        self.spot_count = capped as u32;
    }

    /// Current low-res scatter target dimensions.
    pub fn scatter_dims(&self) -> (u32, u32) {
        self.scatter_dims
    }

    /// Whether the pass should execute this frame. Skips the compute
    /// dispatch and composite blit entirely when there are no fog volumes —
    /// the scatter target does not have to be cleared because the composite
    /// isn't issued.
    pub fn active(&self) -> bool {
        self.volume_count > 0
    }
}

// --- Helpers ---

fn scatter_dims_for(width: u32, height: u32, pixel_scale: u32) -> (u32, u32) {
    let scale = pixel_scale.max(1);
    let w = (width / scale).max(1);
    let h = (height / scale).max(1);
    (w, h)
}

fn create_scatter_target(
    device: &wgpu::Device,
    width: u32,
    height: u32,
) -> (wgpu::Texture, wgpu::TextureView) {
    let tex = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("Fog Scatter Target"),
        size: wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: SCATTER_FORMAT,
        usage: wgpu::TextureUsages::STORAGE_BINDING | wgpu::TextureUsages::TEXTURE_BINDING,
        view_formats: &[],
    });
    let view = tex.create_view(&wgpu::TextureViewDescriptor::default());
    (tex, view)
}

#[allow(clippy::too_many_arguments)]
fn build_group6(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    depth_view: &wgpu::TextureView,
    volumes_buffer: &wgpu::Buffer,
    scatter_view: &wgpu::TextureView,
    params_buffer: &wgpu::Buffer,
    spots_buffer: &wgpu::Buffer,
    points_buffer: &wgpu::Buffer,
) -> wgpu::BindGroup {
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("Fog Group 6"),
        layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: BIND_DEPTH_TEX,
                resource: wgpu::BindingResource::TextureView(depth_view),
            },
            wgpu::BindGroupEntry {
                binding: BIND_VOLUMES,
                resource: volumes_buffer.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: BIND_SCATTER_OUT,
                resource: wgpu::BindingResource::TextureView(scatter_view),
            },
            wgpu::BindGroupEntry {
                binding: BIND_FOG_PARAMS,
                resource: params_buffer.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: BIND_FOG_SPOTS,
                resource: spots_buffer.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: BIND_FOG_POINTS,
                resource: points_buffer.as_entire_binding(),
            },
        ],
    })
}

#[cfg(test)]
mod tests {
    /// The fog raymarch shader must parse cleanly and declare the expected
    /// compute entry point. Catches WGSL regressions before pipeline creation.
    #[test]
    fn fog_volume_wgsl_parses() {
        let src = include_str!("../shaders/fog_volume.wgsl");
        let module =
            naga::front::wgsl::parse_str(src).expect("fog_volume.wgsl should parse as WGSL");
        let has_cs = module
            .entry_points
            .iter()
            .any(|ep| ep.name == "cs_main" && ep.stage == naga::ShaderStage::Compute);
        assert!(has_cs, "fog_volume.wgsl must export @compute cs_main");
    }

    /// The fog composite shader must parse and declare fullscreen vertex +
    /// fragment entry points.
    #[test]
    fn fog_composite_wgsl_parses() {
        let src = include_str!("../shaders/fog_composite.wgsl");
        let module =
            naga::front::wgsl::parse_str(src).expect("fog_composite.wgsl should parse as WGSL");
        let has_vs = module
            .entry_points
            .iter()
            .any(|ep| ep.name == "vs_main" && ep.stage == naga::ShaderStage::Vertex);
        let has_fs = module
            .entry_points
            .iter()
            .any(|ep| ep.name == "fs_main" && ep.stage == naga::ShaderStage::Fragment);
        assert!(has_vs, "fog_composite.wgsl must export @vertex vs_main");
        assert!(has_fs, "fog_composite.wgsl must export @fragment fs_main");
    }
}
