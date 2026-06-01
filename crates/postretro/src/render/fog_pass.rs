// Volumetric fog / beam pass GPU resources.
// See: context/lib/rendering_pipeline.md §7.5
//
// Owns the low-resolution RGBA16F scatter target, the fog-volume AABB
// storage buffer, the fog-params uniform, and the group-6 bind group that
// both the raymarch compute pipeline and the composite blit pipeline read.
//
// The raymarch pipeline reuses group 3 (SH volume) and group 5 (spot shadow
// maps) from the forward pass — the bind-group *objects* are shared, not
// re-uploaded. Group 6 layout is owned here.

use glam::{Mat4, Vec3};

use crate::fx::fog_volume::{
    self, FOG_PARAMS_SIZE, FOG_PLANES_BUFFER_CAPACITY, FOG_POINT_LIGHT_SIZE, FOG_SPOT_LIGHT_SIZE,
    FOG_VOLUME_SIZE, FogPointLight, FogSpotLight, FogVolume, MAX_FOG_POINT_LIGHTS, MAX_FOG_VOLUMES,
    clamp_fog_pixel_scale,
};

/// Format of the low-resolution scatter target.
pub const SCATTER_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;

// `sh_sample.wgsl` reads `sh_total_atlas`, `sh_depth_moments`, and `sh_grid`,
// declared in `fog_volume.wgsl`; WGSL resolves module-scope names regardless of
// textual order, so appending after is safe. The helper owns the SH
// reconstruction + 8-corner blend symbols (`sh_irradiance`,
// `sample_sh_indirect_corners_depth_aware`,
// `sample_sh_indirect_corners_without_depth`) — fog must not redeclare them. See
// rendering_pipeline.md §8.
const FOG_SHADER_SOURCE: &str = concat!(
    include_str!("../shaders/fog_volume.wgsl"),
    "\n",
    include_str!("../shaders/sh_sample.wgsl"),
);

/// Group 6 binding indices. Mirrored in fog_volume.wgsl and fog_composite.wgsl.
pub const BIND_DEPTH_TEX: u32 = 0;
pub const BIND_VOLUMES: u32 = 1;
pub const BIND_SCATTER_OUT: u32 = 2;
pub const BIND_FOG_PARAMS: u32 = 3;
pub const BIND_FOG_SPOTS: u32 = 4;
pub const BIND_FOG_POINTS: u32 = 5;
pub const BIND_FOG_PLANES: u32 = 6;

/// Size in bytes of one `vec4<f32>` plane record in the `fog_planes` storage
/// buffer. Each plane is `(nx, ny, nz, d)` packed as four `f32`s.
pub const FOG_PLANE_SIZE: usize = 16;

/// AABB inflation applied per axis on `set_canonical_volumes` upload. 1 mm in
/// world-space meters — large enough to swamp the float rounding that drives
/// boundary-cell flicker as the camera grazes a face, small enough to be a
/// sub-texel visual no-op. Plane-bounded volumes are unaffected: their clip
/// planes live in a separate buffer and bound the volume tightly regardless.
const AABB_EPSILON: f32 = 1.0e-3;

/// Sticky-frame hysteresis window for fog-volume activation. A volume that
/// drops out of the visible cell set stays active for this long (wall-clock
/// seconds) before the repack stops uploading it. Hides single-frame
/// deactivations caused by transient portal narrowing as the camera grazes a
/// portal edge — the slab-clip prologue in the WGSL raymarch early-outs
/// cheaply for volumes the ray doesn't intersect, so a stale-but-sticky
/// activation costs only the repacked-buffer bytes.
const FOG_HYSTERESIS_SECONDS: f64 = 0.3;

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
    /// Fog-params uniform. Rewritten per frame. See `FogParams` for layout.
    pub params_buffer: wgpu::Buffer,
    /// Per-frame spot-light subset marched by the fog shader.
    pub spots_buffer: wgpu::Buffer,
    /// Per-frame point-light subset marched by the fog shader.
    pub fog_points_buffer: wgpu::Buffer,
    /// Per-frame `fog_planes` storage buffer — flat array of `vec4<f32>` planes
    /// indexed by `(plane_offset, plane_count)` on each `FogVolume`. Sized for
    /// the worst case (`MAX_FOG_VOLUMES * 16` planes) so it never reallocates.
    pub fog_planes_buffer: wgpu::Buffer,

    // --- Scatter target ---
    scatter_view: wgpu::TextureView,
    #[allow(dead_code)]
    scatter_texture: wgpu::Texture,
    /// Low-res dimensions currently allocated for the scatter target. Used
    /// to skip reallocation when the surface resizes without changing the
    /// pixel scale.
    scatter_dims: (u32, u32),

    /// Group 6 bind group. Rebuilt on any call to `resize` — the depth
    /// view is always re-bound even when scatter dims are unchanged,
    /// because the surface depth texture is recreated on every resize.
    pub bind_group: wgpu::BindGroup,

    /// Number of dense-packed `FogVolume` records the shader iterates this
    /// frame. Updated per-frame by `repack_active`. Shader loops against this
    /// count; trailing buffer slots are stale-but-safe.
    pub active_count: u32,
    /// Canonical fog-volume list in source order (one entry per
    /// `fog_volume` brush / `fog_lamp` / `fog_tube` entity in the PRL).
    /// Re-packed per-frame by `repack_active`. Empty when no level is loaded
    /// or the level has no fog volumes.
    ///
    /// Each volume's `min`/`max_v` is inflated by `AABB_EPSILON` (1 mm in
    /// world-space meters) per axis on upload in `set_canonical_volumes`. This
    /// hides sub-millimeter ambiguity at AABB faces that otherwise causes a
    /// frame-coherent boundary-cell to flicker in/out of the visible set as
    /// the camera grazes a face. Plane-bounded clip planes live in their own
    /// buffer and clip independently, so the inflation does not bleed fog past
    /// a primitive brush's actual extent.
    canonical_volumes: Vec<FogVolume>,
    /// Bit `i` set ⇒ canonical slot `i` has density > 0 and is eligible for
    /// upload. ANDed with the visible-cell-derived mask to produce the
    /// per-frame `active_mask`.
    live_mask: u32,
    /// Reusable scratch buffer for the per-frame dense repack — capacity
    /// retained between frames to avoid per-frame allocation.
    repack_scratch: Vec<FogVolume>,
    /// Canonical bounding planes parallel to `canonical_volumes` (one entry per
    /// canonical slot; empty inner vec for semantic / zero-plane volumes). The
    /// dense repack patches `plane_offset` per `FogVolume` and accumulates
    /// these into `planes_scratch` for upload.
    canonical_planes: Vec<Vec<[f32; 4]>>,
    /// Reusable scratch buffer for per-frame `fog_planes` upload bytes —
    /// fixed capacity (`FOG_PLANES_BUFFER_CAPACITY` planes × 16 bytes), no
    /// dynamic growth.
    planes_scratch: Vec<[f32; 4]>,
    /// Wall-clock time (seconds) at which each canonical slot was last seen
    /// in `cell_mask & live_mask`. Parallel to `canonical_volumes`; slots
    /// never observed sit at `f64::NEG_INFINITY` so the hysteresis comparison
    /// can never bring a stale volume back. New slots added by
    /// `set_canonical_volumes` initialize to `NEG_INFINITY`; entries are reset
    /// on level load via `clear_for_level_load` so volumes from the previous
    /// level don't leak forward.
    last_active_time: Vec<f64>,
    /// Most recent spot light count for dynamic beams.
    pub spot_count: u32,
    /// Most recent point-light count. Packed into `FogParams.point_count` so
    /// the shader bounds its inner loop against the live count rather than
    /// `arrayLength(&fog_points)` (which would replay stale records when a
    /// frame uploads zero point lights).
    pub point_count: u32,
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
                wgpu::BindGroupLayoutEntry {
                    binding: BIND_FOG_PLANES,
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
        // and track the real count in `fog.active_count`.
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

        // Spots buffer: fixed at SHADOW_POOL_SIZE capacity; never reallocated.
        let spots_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Fog Spot Lights Buffer"),
            size: (crate::lighting::spot_shadow::SHADOW_POOL_SIZE * FOG_SPOT_LIGHT_SIZE) as u64,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        // Point-light storage buffer. Sized for MAX_FOG_POINT_LIGHTS so the
        // buffer is never reallocated; per-frame uploads go through
        // `upload_points`.
        let fog_points_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Fog Point Lights Buffer"),
            size: (MAX_FOG_POINT_LIGHTS * FOG_POINT_LIGHT_SIZE) as u64,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        // Plane payload buffer: every canonical fog volume's bounding planes
        // packed contiguously, indexed via per-volume `plane_offset / plane_count`.
        let fog_planes_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Fog Planes Buffer"),
            size: (FOG_PLANES_BUFFER_CAPACITY * FOG_PLANE_SIZE) as u64,
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
            &fog_planes_buffer,
        );

        // --- Raymarch compute pipeline ---
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Fog Raymarch Shader"),
            source: wgpu::ShaderSource::Wgsl(FOG_SHADER_SOURCE.into()),
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
                // Initial format — renderer calls `rebuild_composite_for_format` immediately
                // after construction to set the real surface format. The pipeline created here
                // is never used before that call.
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
            fog_planes_buffer,
            scatter_view,
            scatter_texture,
            scatter_dims,
            bind_group,
            active_count: 0,
            canonical_volumes: Vec::new(),
            live_mask: 0,
            repack_scratch: Vec::new(),
            canonical_planes: Vec::new(),
            planes_scratch: Vec::with_capacity(FOG_PLANES_BUFFER_CAPACITY),
            last_active_time: Vec::new(),
            spot_count: 0,
            point_count: 0,
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
            &self.fog_planes_buffer,
        );
    }

    /// Set the global `fog_pixel_scale` from worldspawn. Rebuilds the scatter
    /// target if the value changed.
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

    /// Replace the canonical fog-volume list. The list is the per-frame
    /// `FogVolume` records emitted in original PRL record order — `live_mask`
    /// indicates which slots have density > 0 and are eligible for upload.
    /// The buffer is *not* written here; `repack_active` does the dense
    /// repack-and-upload once the per-frame visible-cell mask is known.
    /// Truncates at `MAX_FOG_VOLUMES` with a warning.
    pub fn set_canonical_volumes(
        &mut self,
        volumes: &[FogVolume],
        planes: &[Vec<[f32; 4]>],
        live_mask: u32,
    ) {
        let count = volumes.len().min(MAX_FOG_VOLUMES);
        if volumes.len() > MAX_FOG_VOLUMES {
            log::warn!(
                "[FogPass] {} volumes exceeded MAX_FOG_VOLUMES={} — extras dropped",
                volumes.len(),
                MAX_FOG_VOLUMES
            );
        }
        self.canonical_volumes.clear();
        // Inflate each volume's AABB by AABB_EPSILON per axis on the way in.
        // See the `canonical_volumes` field comment for why. `center`,
        // `half_diag`, and `inv_half_ext` are derived offline by the bridge
        // and intentionally left untouched — the radial / ellipsoid fade math
        // uses the un-inflated extents, while the AABB-membership test the
        // visibility pass keys off of sees the inflated bounds.
        self.canonical_volumes.reserve(count);
        for v in &volumes[..count] {
            let mut inflated = *v;
            inflated.min[0] -= AABB_EPSILON;
            inflated.min[1] -= AABB_EPSILON;
            inflated.min[2] -= AABB_EPSILON;
            inflated.max_v[0] += AABB_EPSILON;
            inflated.max_v[1] += AABB_EPSILON;
            inflated.max_v[2] += AABB_EPSILON;
            self.canonical_volumes.push(inflated);
        }
        self.canonical_planes.clear();
        // The bridge guarantees `planes.len() == volumes.len()`, but defend
        // against truncation by zipping over the kept canonical slots.
        self.canonical_planes
            .extend(planes.iter().take(count).cloned());
        // Pad with empty plane lists if the caller passed fewer planes than
        // volumes — keeps `canonical_planes` and `canonical_volumes` indexed
        // in lockstep so a malformed input degrades to AABB-only volumes.
        // The bridge contracts that `planes.len() == volumes.len()`; if we hit
        // this path, a primitive (plane-bounded) brush volume is silently
        // demoted to AABB-only. Surface that loudly so the upstream desync is
        // diagnosable rather than hidden as visual fog drift.
        if self.canonical_planes.len() < count {
            log::error!(
                "[FogPass] canonical plane list shorter than volume list ({} planes for {} volumes) — primitive volumes will degrade to AABB-only; check FogVolumeBridge",
                self.canonical_planes.len(),
                count,
            );
            while self.canonical_planes.len() < count {
                self.canonical_planes.push(Vec::new());
            }
        }
        // Mask off any bits past `count` so a truncated canonical list cannot
        // leave a dangling live bit. This is belt-and-suspenders against
        // `compute_fog_cell_mask`'s `all_slots_mask`: the two operate on
        // different inputs (this caps `live_mask` at upload time;
        // `all_slots_mask` caps the per-frame visibility-derived `cell_mask`),
        // and we want reserved bits (16..=31) of a forward-compatible PRL
        // stripped regardless of which call site sees them first.
        //
        // `count` is bounded by `MAX_FOG_VOLUMES = 16` via the `.min` above,
        // so the shift is always well-defined; the assert documents and
        // enforces that invariant.
        debug_assert!(
            count <= MAX_FOG_VOLUMES,
            "count {count} exceeds MAX_FOG_VOLUMES ({MAX_FOG_VOLUMES}); .min cap broken"
        );
        let count_mask = (1u32 << count).wrapping_sub(1);
        self.live_mask = live_mask & count_mask;
        // Preserve hysteresis timestamps across frames — `set_canonical_volumes`
        // is called every frame from `upload_fog_volumes`, so wiping the vec
        // here would defeat the sticky window. Grow with NEG_INFINITY for newly
        // added slots; shrink without clearing existing entries. Full reset on
        // level load is the caller's responsibility via `clear_for_level_load`.
        let new_len = self.canonical_volumes.len();
        if self.last_active_time.len() < new_len {
            self.last_active_time.resize(new_len, f64::NEG_INFINITY);
        } else {
            self.last_active_time.truncate(new_len);
        }
    }

    /// Reset hysteresis state for a fresh level. Drops all stored last-active
    /// timestamps to `NEG_INFINITY` so the first frame after load cannot
    /// activate a stale volume via the sticky window.
    pub fn clear_for_level_load(&mut self) {
        self.last_active_time.fill(f64::NEG_INFINITY);
    }

    /// Compute the per-frame `active_mask` as `(cell_mask & live_mask) |
    /// sticky`, where `sticky` is the set of live slots last seen within
    /// `FOG_HYSTERESIS_SECONDS`, dense-pack the surviving canonical slots into
    /// the GPU buffer, and update `active_count`. The hysteresis window hides
    /// single-frame deactivations caused by transient portal narrowing.
    /// The repack scratch buffers retain their capacity across frames, so no
    /// allocation occurs on the steady-state per-frame path.
    pub fn repack_active(&mut self, queue: &wgpu::Queue, cell_mask: u32, now_seconds: f64) {
        let active_mask = compute_active_mask_with_hysteresis(
            &mut self.last_active_time,
            self.live_mask,
            cell_mask,
            now_seconds,
            FOG_HYSTERESIS_SECONDS,
        );
        self.repack_scratch.clear();
        self.planes_scratch.clear();
        // Iterate by bit so we naturally produce dense, source-order output.
        // Each surviving canonical slot's `plane_offset` is patched to point
        // at the current cursor in `planes_scratch`; planes are appended in
        // dense order so the GPU layout matches.
        let mut bits = active_mask;
        while bits != 0 {
            let i = bits.trailing_zeros() as usize;
            bits &= bits - 1;
            if let Some(v) = self.canonical_volumes.get(i) {
                let mut packed = *v;
                let plane_offset = self.planes_scratch.len() as u32;
                packed.plane_offset = plane_offset;
                if let Some(planes) = self.canonical_planes.get(i) {
                    // Trust the bridge's plane_count but clamp against the
                    // canonical plane list to defend against drift; on truncation
                    // we must also correct the GPU-bound count so the shader
                    // doesn't read past the planes we actually copied.
                    let n = (packed.plane_count as usize).min(planes.len());
                    packed.plane_count = n as u32;
                    self.planes_scratch.extend_from_slice(&planes[..n]);
                }
                self.repack_scratch.push(packed);
            }
        }
        self.active_count = self.repack_scratch.len() as u32;
        if self.active_count > 0 {
            // Upload unconditionally when active_count > 0 — the dense layout
            // changes each frame based on the per-frame visible set, so we
            // cannot skip the write even when the count happens to match.
            let bytes = fog_volume::pack_fog_volumes(&self.repack_scratch);
            queue.write_buffer(&self.volumes_buffer, 0, bytes);
            // Conditional upload — asymmetric with the unconditional volumes
            // write above. Semantic-only levels (e.g. fog_lamp / fog_tube
            // with no bounding planes) produce no plane records, leaving
            // `planes_scratch` empty. In that case we skip the write and let
            // the buffer retain stale contents from a previous level; this is
            // safe because the shader guards on `plane_count > 0u` before
            // indexing `fog_planes`, so stale buffer data is never read.
            if !self.planes_scratch.is_empty() {
                let plane_bytes: &[u8] = bytemuck::cast_slice(&self.planes_scratch);
                queue.write_buffer(&self.fog_planes_buffer, 0, plane_bytes);
            }
        }
        // Both the volumes buffer tail past `active_count` and the planes
        // buffer (when no upload happens this frame — e.g. active_count == 0
        // or planes_scratch is empty) may hold stale records from a previous
        // frame. Safe: the shader loops `0..active_count` and gates plane reads
        // on `plane_count > 0u`, so stale slots / stale plane bytes are never
        // observed.
    }

    /// Number of canonical fog-volume slots currently loaded. Used by callers
    /// to derive the `DrawAll` cell-mask (`(1 << canonical_count) - 1`).
    pub fn canonical_volume_count(&self) -> u32 {
        self.canonical_volumes.len() as u32
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
        let params = fog_volume::pack_fog_params(fog_volume::FogParamsInput {
            inv_view_proj,
            camera_position,
            step_size: self.step_size,
            active_count: self.active_count,
            near_clip,
            far_clip,
            point_count: self.point_count,
            spot_count: self.spot_count,
        });
        queue.write_buffer(&self.params_buffer, 0, bytemuck::bytes_of(&params));
    }

    /// Upload the per-frame point-light list for the fog raymarch. Truncates
    /// at `MAX_FOG_POINT_LIGHTS` with a warning. Updates `point_count` so the
    /// next `upload_params` packs the live bound into `FogParams.point_count`.
    pub fn upload_points(&mut self, queue: &wgpu::Queue, points: &[FogPointLight]) {
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
            queue.write_buffer(&self.fog_points_buffer, 0, bytes);
        }
        self.point_count = count as u32;
    }

    /// Upload the per-frame spot-light list for the fog raymarch beams.
    pub fn upload_spots(&mut self, queue: &wgpu::Queue, spots: &[FogSpotLight]) {
        let capped = spots
            .len()
            .min(crate::lighting::spot_shadow::SHADOW_POOL_SIZE);
        let bytes = fog_volume::pack_fog_spot_lights(&spots[..capped]);
        if !bytes.is_empty() {
            queue.write_buffer(&self.spots_buffer, 0, bytes);
        }
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
        self.active_count > 0
    }
}

// --- Helpers ---

/// Pure data-logic helper extracted from `repack_active`: compose the per-frame
/// active mask as `(cell_mask & live_mask) | sticky`, where `sticky` is the set
/// of live slots last seen within `hysteresis_seconds`. Refreshes
/// `last_active_time` entries for slots in the current visible set as a side
/// effect — the sticky window is a wall-clock measurement, so the timestamp
/// update has to happen here. Only iterates set bits of `live_mask` so
/// NEG_INFINITY entries for dead slots stay untouched and reserved bits past
/// the canonical count are never observed.
fn compute_active_mask_with_hysteresis(
    last_active_time: &mut [f64],
    live_mask: u32,
    cell_mask: u32,
    now_seconds: f64,
    hysteresis_seconds: f64,
) -> u32 {
    let in_cell_mask = cell_mask & live_mask;
    let mut refresh = in_cell_mask;
    while refresh != 0 {
        let i = refresh.trailing_zeros() as usize;
        refresh &= refresh - 1;
        if let Some(t) = last_active_time.get_mut(i) {
            *t = now_seconds;
        }
    }
    let mut sticky = 0u32;
    let mut live = live_mask;
    while live != 0 {
        let i = live.trailing_zeros() as usize;
        live &= live - 1;
        let last = last_active_time
            .get(i)
            .copied()
            .unwrap_or(f64::NEG_INFINITY);
        if now_seconds - last < hysteresis_seconds {
            sticky |= 1u32 << i;
        }
    }
    in_cell_mask | sticky
}

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
    planes_buffer: &wgpu::Buffer,
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
            wgpu::BindGroupEntry {
                binding: BIND_FOG_PLANES,
                resource: planes_buffer.as_entire_binding(),
            },
        ],
    })
}

#[cfg(test)]
mod tests {
    use super::{FOG_HYSTERESIS_SECONDS, FOG_SHADER_SOURCE, compute_active_mask_with_hysteresis};
    use glam::Vec3;
    use proptest::prelude::*;

    const TEST_EPSILON: f32 = 1.0e-5;
    const HG_MAX_G: f32 = 0.9;

    fn hg_phase_reference(cos_theta: f32, g: f32) -> f32 {
        let clamped_g = g.clamp(0.0, HG_MAX_G);
        let g2 = clamped_g * clamped_g;
        let denom = 1.0 + g2 - 2.0 * clamped_g * cos_theta.clamp(-1.0, 1.0);
        (1.0 - g2) / (4.0 * std::f32::consts::PI * denom.max(1.0e-4).powf(1.5))
    }

    fn directional_sh_weight_reference(cos_theta: f32, g: f32) -> f32 {
        let clamped_g = g.clamp(0.0, HG_MAX_G);
        if clamped_g <= 0.0 {
            return 0.0;
        }

        let uniform_phase = hg_phase_reference(cos_theta, 0.0);
        let phase = hg_phase_reference(cos_theta, clamped_g);
        let peak = hg_phase_reference(1.0, clamped_g);
        let phase_weight =
            ((phase - uniform_phase) / (peak - uniform_phase).max(1.0e-6)).clamp(0.0, 1.0);
        (clamped_g * phase_weight).clamp(0.0, 1.0)
    }

    fn blend_sh_reference(iso: Vec3, dir: Vec3, g: f32) -> Vec3 {
        // cos_theta is hardcoded to 1.0 here, matching the shader's constant
        // value: `sh_view_direction = -ray.direction`, so
        // `dot(sh_view_direction, -ray.direction) = 1.0` always. Tests pass
        // at this fixed angle; if the shader's angle computation ever changes,
        // this reference function will need to accept a variable cos_theta.
        let weight = directional_sh_weight_reference(1.0, g);
        iso + (dir - iso) * weight
    }

    fn assert_close(actual: f32, expected: f32) {
        assert!(
            (actual - expected).abs() <= TEST_EPSILON,
            "expected {actual} to be within {TEST_EPSILON} of {expected}",
        );
    }

    fn assert_vec3_close(actual: Vec3, expected: Vec3) {
        for (actual, expected) in actual.to_array().into_iter().zip(expected.to_array()) {
            assert_close(actual, expected);
        }
    }

    #[test]
    fn hg_phase_peaks_toward_lobe_direction() {
        let g = 0.65;
        let forward = hg_phase_reference(1.0, g);
        let side = hg_phase_reference(0.0, g);
        let backward = hg_phase_reference(-1.0, g);

        assert!(forward > side, "HG phase should peak along the lobe");
        assert!(
            side > backward,
            "HG phase should fall off away from the lobe"
        );
    }

    #[test]
    fn hg_phase_falloff_is_symmetric_around_lobe_axis() {
        let g = 0.55;
        let cos_theta = 0.35_f32;
        let sin_theta = (1.0 - cos_theta * cos_theta).sqrt();
        let lobe = Vec3::Y;
        let sample_a = Vec3::new(sin_theta, cos_theta, 0.0);
        let sample_b = Vec3::new(0.0, cos_theta, sin_theta);

        assert_close(lobe.dot(sample_a), lobe.dot(sample_b));
        assert_close(
            hg_phase_reference(lobe.dot(sample_a), g),
            hg_phase_reference(lobe.dot(sample_b), g),
        );
    }

    #[test]
    fn hg_phase_g_zero_is_uniform() {
        let expected = 1.0 / (4.0 * std::f32::consts::PI);
        for cos_theta in [-1.0, -0.25, 0.0, 0.5, 1.0] {
            assert_close(hg_phase_reference(cos_theta, 0.0), expected);
        }
    }

    #[test]
    fn hg_phase_is_finite_at_clamp_endpoints() {
        for g in [0.0, HG_MAX_G] {
            for cos_theta in [-1.0, 0.0, 1.0] {
                let phase = hg_phase_reference(cos_theta, g);
                assert!(
                    phase.is_finite(),
                    "phase must be finite for g={g}, cos={cos_theta}"
                );
                assert!(phase >= 0.0, "phase must stay non-negative");
            }
        }
    }

    #[test]
    fn directional_sh_blend_g_zero_returns_isotropic_read() {
        let iso = Vec3::new(1.25, -0.5, 3.0);
        let dir = Vec3::new(-4.0, 2.5, 0.75);
        assert_vec3_close(blend_sh_reference(iso, dir, 0.0), iso);
    }

    #[test]
    fn directional_sh_blend_is_finite_and_bounded_at_endpoints() {
        let iso = Vec3::new(-8.0, 0.25, 10.0);
        let dir = Vec3::new(4.0, -2.0, 3.0);

        for g in [0.0, HG_MAX_G] {
            let blended = blend_sh_reference(iso, dir, g);
            for ((actual, iso), dir) in blended
                .to_array()
                .into_iter()
                .zip(iso.to_array())
                .zip(dir.to_array())
            {
                assert!(actual.is_finite(), "blend component must be finite");
                let lo = iso.min(dir) - TEST_EPSILON;
                let hi = iso.max(dir) + TEST_EPSILON;
                assert!(
                    actual >= lo && actual <= hi,
                    "component {actual} must stay within [{lo}, {hi}] for g={g}",
                );
            }
        }
    }

    proptest! {
        #[test]
        fn directional_sh_blend_is_finite_and_componentwise_bounded_for_arbitrary_coefficients(
            iso_x in -10_000.0f32..10_000.0,
            iso_y in -10_000.0f32..10_000.0,
            iso_z in -10_000.0f32..10_000.0,
            dir_x in -10_000.0f32..10_000.0,
            dir_y in -10_000.0f32..10_000.0,
            dir_z in -10_000.0f32..10_000.0,
            g in 0.0f32..HG_MAX_G,
        ) {
            let iso = Vec3::new(iso_x, iso_y, iso_z);
            let dir = Vec3::new(dir_x, dir_y, dir_z);
            let blended = blend_sh_reference(iso, dir, g);

            for ((actual, iso), dir) in blended
                .to_array()
                .into_iter()
                .zip(iso.to_array())
                .zip(dir.to_array())
            {
                prop_assert!(actual.is_finite(), "blend component must be finite");
                let lo = iso.min(dir) - TEST_EPSILON;
                let hi = iso.max(dir) + TEST_EPSILON;
                prop_assert!(
                    actual >= lo && actual <= hi,
                    "component {actual} must stay within [{lo}, {hi}] for g={g}",
                );
            }
        }
    }

    // Regression: `set_canonical_volumes` runs every frame and previously
    // wiped `last_active_time`, so the 300 ms sticky window never fired and a
    // volume dropped from the visible cell set deactivated immediately.
    #[test]
    fn fog_hysteresis_keeps_slot_active_within_sticky_window_then_drops_after() {
        // Four canonical slots, all live.
        let mut last_active = vec![f64::NEG_INFINITY; 4];
        let live_mask = 0b1111u32;
        let slot_i = 1usize;
        let bit_i = 1u32 << slot_i;

        // Frame 0: slot i is in the visible cell set — must be active.
        let m0 = compute_active_mask_with_hysteresis(
            &mut last_active,
            live_mask,
            bit_i,
            0.0,
            FOG_HYSTERESIS_SECONDS,
        );
        assert!(
            m0 & bit_i != 0,
            "slot must be active on first visible frame"
        );

        // Frame 1: slot i is no longer in the visible cell set, but we're
        // still inside the sticky window — slot must remain active.
        let within_window = FOG_HYSTERESIS_SECONDS * 0.5;
        let m1 = compute_active_mask_with_hysteresis(
            &mut last_active,
            live_mask,
            0,
            within_window,
            FOG_HYSTERESIS_SECONDS,
        );
        assert!(
            m1 & bit_i != 0,
            "slot must remain active within sticky window (mask=0b{:04b})",
            m1
        );

        // Frame 2: well past the sticky window — slot must drop.
        let past_window = FOG_HYSTERESIS_SECONDS * 2.0;
        let m2 = compute_active_mask_with_hysteresis(
            &mut last_active,
            live_mask,
            0,
            past_window,
            FOG_HYSTERESIS_SECONDS,
        );
        assert!(
            m2 & bit_i == 0,
            "slot must drop after sticky window expires (mask=0b{:04b})",
            m2
        );
    }

    /// The fog raymarch shader must parse cleanly and declare the expected
    /// compute entry point. Parses the full concatenated source (fog_volume +
    /// the shared `sh_sample.wgsl` helper) so the helper's compilation in this
    /// pipeline is covered. Catches WGSL regressions before pipeline creation.
    #[test]
    fn fog_volume_wgsl_parses() {
        let module = naga::front::wgsl::parse_str(FOG_SHADER_SOURCE)
            .expect("fog shader should parse as WGSL");
        let has_cs = module
            .entry_points
            .iter()
            .any(|ep| ep.name == "cs_main" && ep.stage == naga::ShaderStage::Compute);
        assert!(has_cs, "fog_volume.wgsl must export @compute cs_main");
    }

    /// The full fog pipeline source (fog_volume + `sh_sample.wgsl`) must pass
    /// naga's validation, including control-flow uniformity. `parse_str` alone
    /// does not enforce this; a future edit that breaks the shared helper's
    /// compilation in the fog pipeline is caught here at `cargo test` time,
    /// before GPU pipeline creation.
    #[test]
    fn fog_volume_wgsl_passes_naga_validation() {
        let module =
            naga::front::wgsl::parse_str(FOG_SHADER_SOURCE).expect("fog shader must parse");
        naga::valid::Validator::new(
            naga::valid::ValidationFlags::all(),
            naga::valid::Capabilities::all(),
        )
        .validate(&module)
        .expect("fog shader must pass naga validation");
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
