// Half-resolution SDF shadow pass. Runs as a compute pass between the depth
// pre-pass and the forward pass. Per half-res pixel, traces up to K = 4 per-light
// SDF visibility rays, writing the four factors into a `Rgba8Unorm` half-res
// target, one slice per channel:
//   R = per-light SDF visibility slice 0
//   G = per-light SDF visibility slice 1
//   B = per-light SDF visibility slice 2
//   A = per-light SDF visibility slice 3
//
// Forward integration reads this target: each sdf-tagged light's diffuse and
// specular multiply by their slice (read directly via `slice_for_visibility`;
// gated by light selection, not a flag). When the SDF atlas isn't present the
// pass is skipped and the target stays at its prior contents — forward degrades
// cleanly (it gates the multiply on the atlas-present flag).
//
// Pipeline layout: group 0 = SDF atlas (owned by `SdfAtlasResources`),
// group 1 = this pass's own bind group (params uniform, depth, SH depth moments,
// shadow factor output), group 2 = static-light buffers the shared K-selection
// helper reads.

use glam::Mat4;

use super::sdf_atlas::SdfAtlasResources;

/// Full WGSL source for the SDF shadow compute pass: the pass shader plus the
/// shared K-selection helper, textually concatenated (the shared-WGSL-helper
/// pattern — cf. `curve_eval.wgsl`). The forward shader also appends the same
/// `sdf_light_select.wgsl` string, so both select identical lights in identical
/// order — the load-bearing K-selection parity seam. The pass shader declares
/// the `spec_lights` / `chunk_grid` / `chunk_offsets` / `chunk_indices`
/// bindings (group 2) the helper reads by name.
const SDF_SHADOW_SHADER_SOURCE: &str = concat!(
    include_str!("../shaders/sdf_shadow.wgsl"),
    "\n",
    include_str!("../shaders/sdf_light_select.wgsl"),
);

/// Half-resolution divisor relative to the swap-chain. `2` matches the
/// resolution-scale convention used by the legacy SDF code (see
/// `context/plans/in-progress/sdf-static-occluder-shadows/research.md`).
pub const HALF_RES_SCALE: u32 = 2;

/// Color format of the shadow-factor target. The four channels are the K = 4
/// per-light SDF visibility slices: R = slot 0, G = slot 1, B = slot 2, A = slot 3.
pub const SHADOW_FACTOR_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;

/// Default uniform values for the tuning knobs. Task 7 wires sliders to these.
///
/// Retuned for the fine-atlas trace (`sdf-shadow-fine-atlas-trace`). With true
/// per-voxel (0.5 m) distances instead of the ~4 m coarse-brick lower bound,
/// sphere-trace steps shrink sharply near surfaces, so more steps are needed to
/// keep the same open-space reach to a distant occluder — bumped 48 → 64
/// (well under the 256 hard clamp; the open-space early-out keeps the common
/// case cheap, and the perf AC bounds the worst case). The `k*d/t` penumbra
/// estimate sharpens once `d` is a real metric distance, so `penumbra_k` is
/// softened 16 → 8 to keep penumbra width similar rather than over-hard.
///
/// The open-space skip threshold is seeded **loose** (8.0): the skip returns
/// FULLY_LIT before marching when `E[d] > threshold × SH cell`, so a *small*
/// threshold fires the skip on even a moderate gap and suppresses the per-light
/// trace. Seeded loose so rays actually run on typical geometry; tighten via the
/// Task 6 perf gate if open-space cost dominates (see
/// `context/plans/in-progress/sdf-static-occluder-shadows/research.md` step 2
/// for the threshold-sensitivity analysis).
/// These are SEED values; adjust them live via the "SDF / Fog Quality" panel in
/// the debug overlay (`dev-tools` feature).
pub const DEFAULT_MAX_MARCH_STEPS: u32 = 64;
pub const DEFAULT_OPEN_SPACE_SKIP_THRESHOLD: f32 = 8.0; // multiple of SH cell size (loose seed)
pub const DEFAULT_PENUMBRA_K: f32 = 8.0;

/// Size in bytes of the `ShadowPassParams` uniform. Mirrors the WGSL struct
/// in `shaders/sdf_shadow.wgsl`. std140-aligned: vec3<f32>/u32 pairs share
/// 16-byte slots, mat4x4 takes 64.
///
/// Layout:
///   0..64    inv_view_proj           (mat4x4<f32>)
///   64..76   camera_position         (vec3<f32>)
///   76..80   half_res_size_x         (u32)
///   80..84   half_res_size_y         (u32)
///   84..88   max_march_steps         (u32)
///   88..92   open_space_skip_threshold (f32)
///   92..96   penumbra_k              (f32)
///   96..108  sh_grid_origin          (vec3<f32>)
///   108..112 sh_has_volume           (u32)
///   112..124 sh_cell_size            (vec3<f32>)
///   124..128 _pad0                   (u32)
///   128..140 sh_grid_dimensions      (vec3<u32>)
///   140..144 _pad1                   (u32)
pub const SHADOW_PASS_PARAMS_SIZE: usize = 144;

/// Tuning knobs exposed to the Task 7 quality sliders. Held on the pass and
/// uploaded each frame alongside the per-frame camera matrices.
#[derive(Debug, Clone, Copy)]
pub struct SdfShadowTuning {
    pub max_march_steps: u32,
    pub open_space_skip_threshold: f32,
    pub penumbra_k: f32,
}

impl Default for SdfShadowTuning {
    fn default() -> Self {
        Self {
            max_march_steps: DEFAULT_MAX_MARCH_STEPS,
            open_space_skip_threshold: DEFAULT_OPEN_SPACE_SKIP_THRESHOLD,
            penumbra_k: DEFAULT_PENUMBRA_K,
        }
    }
}

/// Per-frame inputs the renderer threads into the pass when encoding.
#[derive(Debug, Clone, Copy)]
pub struct SdfShadowFrameInputs {
    pub inv_view_proj: Mat4,
    pub camera_position: [f32; 3],
}

/// Static SH grid metadata captured at level load. Mirrors the relevant prefix
/// of `ShGridInfo` so the SDF shadow pass can do the open-space skip without
/// binding the whole group-3 bind group.
#[derive(Debug, Clone, Copy)]
pub struct SdfShadowShGrid {
    pub origin: [f32; 3],
    pub cell_size: [f32; 3],
    pub dimensions: [u32; 3],
    pub has_volume: bool,
}

impl Default for SdfShadowShGrid {
    fn default() -> Self {
        Self {
            origin: [0.0; 3],
            cell_size: [1.0; 3],
            dimensions: [1, 1, 1],
            has_volume: false,
        }
    }
}

/// Borrowed references to the static-light buffers (group 2) the K-selection
/// helper reads. These are the SAME buffers the forward pass's lighting bind
/// group references, owned by the renderer and recreated on level load — the
/// pass binds its own group-2 bind group over them.
#[derive(Clone, Copy)]
pub struct SdfShadowLightBuffers<'a> {
    pub spec_lights: &'a wgpu::Buffer,
    pub chunk_grid_info: &'a wgpu::Buffer,
    pub chunk_offsets: &'a wgpu::Buffer,
    pub chunk_indices: &'a wgpu::Buffer,
}

/// GPU resources for the half-res SDF shadow pass.
pub struct SdfShadowPass {
    pipeline: wgpu::ComputePipeline,
    /// Bind-group layout for group 1 (the pass-owned bindings — depth,
    /// direction texture, SH depth moments, params, output).
    bind_group_layout: wgpu::BindGroupLayout,
    /// Bind-group layout for group 2 (the static-light buffers the shared
    /// K-selection helper reads — spec_lights, chunk grid info/offsets/indices).
    light_bind_group_layout: wgpu::BindGroupLayout,
    /// Group-2 bind group over the renderer's light buffers. Rebuilt on level
    /// load (the light buffers are recreated then).
    light_bind_group: wgpu::BindGroup,
    /// Half-res `Rgba8Unorm` shadow factor target. Cleared to (1,1,1,1) at
    /// allocation so the pass-skipped path is "fully lit".
    #[allow(dead_code)]
    shadow_texture: wgpu::Texture,
    /// View into `shadow_texture` exposed to the forward pass for the
    /// bilateral upsample.
    pub shadow_view: wgpu::TextureView,
    /// Storage-write view bound on the pass's own bind group.
    shadow_storage_view: wgpu::TextureView,
    /// Current (width, height) of the shadow texture. Used to recompute the
    /// dispatch grid and to size the `ShadowPassParams` half-res fields.
    half_res: (u32, u32),
    /// Per-frame `ShadowPassParams` uniform.
    params_buffer: wgpu::Buffer,
    /// Bind group built once at construction / rebuilt on resize. References
    /// `depth_view` (recreated by the renderer on resize, so the bind group must
    /// be rebuilt too) and the depth-moment view (stable across resizes — only
    /// rebuilt on level reload).
    bind_group: wgpu::BindGroup,
    /// SH depth moment texture (`E[d]`, `E[d²]`) — open-space skip lookup.
    sh_depth_moments_view: wgpu::TextureView,
    /// SH grid metadata mirrored into the params uniform.
    sh_grid: SdfShadowShGrid,
    /// Live tuning knobs. Mutated by Task 7's sliders; uploaded each frame.
    pub tuning: SdfShadowTuning,
}

impl SdfShadowPass {
    /// Build the shadow-pass resources.
    ///
    /// `sdf_atlas_layout` is the bind-group layout owned by `SdfAtlasResources`
    /// (group 0 of this pipeline). The pass does not modify or rebuild the
    /// atlas bind group — it just borrows the layout to compose its pipeline
    /// layout.
    ///
    /// The pass owns its target and bind group, so a caller need only hold the
    /// `SdfShadowPass` and call `dispatch` once per frame.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        device: &wgpu::Device,
        sdf_atlas_layout: &wgpu::BindGroupLayout,
        depth_view: &wgpu::TextureView,
        sh_depth_moments_view: wgpu::TextureView,
        lights: SdfShadowLightBuffers,
        sh_grid: SdfShadowShGrid,
        full_res_width: u32,
        full_res_height: u32,
    ) -> Self {
        let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("SDF Shadow Bind Group Layout"),
            entries: &bind_group_layout_entries(),
        });
        let light_bind_group_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("SDF Shadow Light Bind Group Layout"),
                entries: &light_bind_group_layout_entries(),
            });

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("SDF Shadow Pipeline Layout"),
            bind_group_layouts: &[
                Some(sdf_atlas_layout),
                Some(&bind_group_layout),
                Some(&light_bind_group_layout),
            ],
            immediate_size: 0,
        });

        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("SDF Shadow Shader"),
            source: wgpu::ShaderSource::Wgsl(SDF_SHADOW_SHADER_SOURCE.into()),
        });

        let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("SDF Shadow Pipeline"),
            layout: Some(&pipeline_layout),
            module: &shader,
            entry_point: Some("cs_main"),
            compilation_options: wgpu::PipelineCompilationOptions::default(),
            cache: None,
        });

        let half_res = compute_half_res(full_res_width, full_res_height);
        let (shadow_texture, shadow_view, shadow_storage_view) =
            create_shadow_target(device, half_res.0, half_res.1);

        let params_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("SDF Shadow Params"),
            size: SHADOW_PASS_PARAMS_SIZE as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let bind_group = build_bind_group(
            device,
            &bind_group_layout,
            &params_buffer,
            depth_view,
            &sh_depth_moments_view,
            &shadow_storage_view,
        );
        let light_bind_group = build_light_bind_group(device, &light_bind_group_layout, lights);

        Self {
            pipeline,
            bind_group_layout,
            light_bind_group_layout,
            light_bind_group,
            shadow_texture,
            shadow_view,
            shadow_storage_view,
            half_res,
            params_buffer,
            bind_group,
            sh_depth_moments_view,
            sh_grid,
            tuning: SdfShadowTuning::default(),
        }
    }

    /// View into the half-res shadow factor target. Consumed by the forward
    /// pass for the bilateral upsample.
    #[allow(dead_code)]
    pub fn shadow_view(&self) -> &wgpu::TextureView {
        &self.shadow_view
    }

    /// Current half-res dimensions. Useful for the forward pass to compute
    /// the upsample sampling step.
    #[allow(dead_code)]
    pub fn half_res(&self) -> (u32, u32) {
        self.half_res
    }

    /// Snapshot of the current tuning knobs. Read by the Task 7 debug-UI
    /// sliders to seed their state on first draw.
    #[cfg_attr(not(feature = "dev-tools"), allow(dead_code))]
    pub fn tuning(&self) -> SdfShadowTuning {
        self.tuning
    }

    /// Write through to `tuning.max_march_steps`. The new value is packed
    /// into `ShadowPassParams` on the next `dispatch`. Clamped to a sensible
    /// range so a runaway slider can't stall the GPU.
    #[cfg_attr(not(feature = "dev-tools"), allow(dead_code))]
    pub fn set_max_march_steps(&mut self, steps: u32) {
        self.tuning.max_march_steps = steps.clamp(1, 256);
    }

    /// Write through to `tuning.open_space_skip_threshold`. Clamped to
    /// non-negative — a negative threshold disables the skip in the shader.
    #[cfg_attr(not(feature = "dev-tools"), allow(dead_code))]
    pub fn set_open_space_skip_threshold(&mut self, threshold: f32) {
        self.tuning.open_space_skip_threshold = threshold.max(0.0);
    }

    /// Write through to `tuning.penumbra_k`. Larger `k` = harder shadow.
    /// Clamped to a positive minimum so the shader's divide stays finite.
    #[cfg_attr(not(feature = "dev-tools"), allow(dead_code))]
    pub fn set_penumbra_k(&mut self, k: f32) {
        self.tuning.penumbra_k = k.max(0.01);
    }

    /// Resize the half-res target on a surface resize. Rebuilds the bind group
    /// because both the depth view and the shadow target view changed.
    pub fn resize(
        &mut self,
        device: &wgpu::Device,
        depth_view: &wgpu::TextureView,
        full_res_width: u32,
        full_res_height: u32,
    ) {
        self.half_res = compute_half_res(full_res_width, full_res_height);
        let (shadow_texture, shadow_view, shadow_storage_view) =
            create_shadow_target(device, self.half_res.0, self.half_res.1);
        self.shadow_texture = shadow_texture;
        self.shadow_view = shadow_view;
        self.shadow_storage_view = shadow_storage_view;
        self.bind_group = build_bind_group(
            device,
            &self.bind_group_layout,
            &self.params_buffer,
            depth_view,
            &self.sh_depth_moments_view,
            &self.shadow_storage_view,
        );
    }

    /// Rebuild the views and light buffers the pass depends on after a level
    /// load (SH section + the static-light buffers swap). The depth view is
    /// unchanged by a level load (it's owned by the renderer's surface state),
    /// so the caller passes the current one back in.
    pub fn rebuild_for_level(
        &mut self,
        device: &wgpu::Device,
        depth_view: &wgpu::TextureView,
        sh_depth_moments_view: wgpu::TextureView,
        lights: SdfShadowLightBuffers,
        sh_grid: SdfShadowShGrid,
    ) {
        self.sh_depth_moments_view = sh_depth_moments_view;
        self.sh_grid = sh_grid;
        self.bind_group = build_bind_group(
            device,
            &self.bind_group_layout,
            &self.params_buffer,
            depth_view,
            &self.sh_depth_moments_view,
            &self.shadow_storage_view,
        );
        self.light_bind_group =
            build_light_bind_group(device, &self.light_bind_group_layout, lights);
    }

    /// Encode the per-frame dispatch. The caller has already determined the
    /// pass should run (`sdf_atlas.present == true` and SDF mode is on — Task
    /// 6 will wire the off/visualize mode selector). When skipped, the shadow
    /// target retains its last contents — the forward pass is responsible for
    /// guarding the multiply on the mode flag.
    pub fn dispatch(
        &self,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        sdf_atlas: &SdfAtlasResources,
        frame: SdfShadowFrameInputs,
        timestamp_writes: Option<wgpu::ComputePassTimestampWrites<'_>>,
    ) {
        let bytes = pack_params_bytes(frame, self.half_res, self.tuning, self.sh_grid);
        queue.write_buffer(&self.params_buffer, 0, &bytes);

        let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
            label: Some("SDF Shadow Pass"),
            timestamp_writes,
        });
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, &sdf_atlas.bind_group, &[]);
        pass.set_bind_group(1, &self.bind_group, &[]);
        pass.set_bind_group(2, &self.light_bind_group, &[]);
        let groups_x = self.half_res.0.div_ceil(8).max(1);
        let groups_y = self.half_res.1.div_ceil(8).max(1);
        pass.dispatch_workgroups(groups_x, groups_y, 1);
    }
}

fn compute_half_res(full_w: u32, full_h: u32) -> (u32, u32) {
    let w = (full_w / HALF_RES_SCALE).max(1);
    let h = (full_h / HALF_RES_SCALE).max(1);
    (w, h)
}

fn create_shadow_target(
    device: &wgpu::Device,
    width: u32,
    height: u32,
) -> (wgpu::Texture, wgpu::TextureView, wgpu::TextureView) {
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("SDF Shadow Factor Target"),
        size: wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: SHADOW_FACTOR_FORMAT,
        // STORAGE_BINDING for the compute write, TEXTURE_BINDING for the
        // forward-pass bilateral upsample read.
        usage: wgpu::TextureUsages::STORAGE_BINDING | wgpu::TextureUsages::TEXTURE_BINDING,
        view_formats: &[],
    });
    let sampled_view = texture.create_view(&wgpu::TextureViewDescriptor {
        label: Some("SDF Shadow Factor Sampled View"),
        ..Default::default()
    });
    let storage_view = texture.create_view(&wgpu::TextureViewDescriptor {
        label: Some("SDF Shadow Factor Storage View"),
        ..Default::default()
    });
    (texture, sampled_view, storage_view)
}

fn build_bind_group(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    params_buffer: &wgpu::Buffer,
    depth_view: &wgpu::TextureView,
    sh_depth_moments_view: &wgpu::TextureView,
    shadow_storage_view: &wgpu::TextureView,
) -> wgpu::BindGroup {
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("SDF Shadow Bind Group"),
        layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: params_buffer.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::TextureView(depth_view),
            },
            wgpu::BindGroupEntry {
                binding: 2,
                resource: wgpu::BindingResource::TextureView(sh_depth_moments_view),
            },
            wgpu::BindGroupEntry {
                binding: 3,
                resource: wgpu::BindingResource::TextureView(shadow_storage_view),
            },
        ],
    })
}

fn bind_group_layout_entries() -> [wgpu::BindGroupLayoutEntry; 4] {
    let vis = wgpu::ShaderStages::COMPUTE;
    [
        // Binding 0: params uniform.
        wgpu::BindGroupLayoutEntry {
            binding: 0,
            visibility: vis,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Uniform,
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            count: None,
        },
        // Binding 1: depth texture (depth_2d, non-filtering — sampled via textureLoad).
        wgpu::BindGroupLayoutEntry {
            binding: 1,
            visibility: vis,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Depth,
                view_dimension: wgpu::TextureViewDimension::D2,
                multisampled: false,
            },
            count: None,
        },
        // Binding 2: SH depth moments (Rg16Float 3D, non-filterable load).
        wgpu::BindGroupLayoutEntry {
            binding: 2,
            visibility: vis,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable: false },
                view_dimension: wgpu::TextureViewDimension::D3,
                multisampled: false,
            },
            count: None,
        },
        // Binding 3: shadow-factor output (Rgba8Unorm storage write).
        wgpu::BindGroupLayoutEntry {
            binding: 3,
            visibility: vis,
            ty: wgpu::BindingType::StorageTexture {
                access: wgpu::StorageTextureAccess::WriteOnly,
                format: SHADOW_FACTOR_FORMAT,
                view_dimension: wgpu::TextureViewDimension::D2,
            },
            count: None,
        },
    ]
}

/// Build the group-2 bind group over the renderer's static-light buffers. The
/// shared K-selection helper reads these (`spec_lights`, `chunk_grid`,
/// `chunk_offsets`, `chunk_indices`) to pick the same lights the forward shader
/// shades.
fn build_light_bind_group(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    lights: SdfShadowLightBuffers,
) -> wgpu::BindGroup {
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("SDF Shadow Light Bind Group"),
        layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: lights.spec_lights.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: lights.chunk_grid_info.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 2,
                resource: lights.chunk_offsets.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 3,
                resource: lights.chunk_indices.as_entire_binding(),
            },
        ],
    })
}

fn light_bind_group_layout_entries() -> [wgpu::BindGroupLayoutEntry; 4] {
    let vis = wgpu::ShaderStages::COMPUTE;
    let storage_ro = wgpu::BindingType::Buffer {
        ty: wgpu::BufferBindingType::Storage { read_only: true },
        has_dynamic_offset: false,
        min_binding_size: None,
    };
    [
        // Binding 0: spec_lights (storage, read).
        wgpu::BindGroupLayoutEntry {
            binding: 0,
            visibility: vis,
            ty: storage_ro,
            count: None,
        },
        // Binding 1: chunk grid info (uniform).
        wgpu::BindGroupLayoutEntry {
            binding: 1,
            visibility: vis,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Uniform,
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            count: None,
        },
        // Binding 2: chunk offsets (storage, read).
        wgpu::BindGroupLayoutEntry {
            binding: 2,
            visibility: vis,
            ty: storage_ro,
            count: None,
        },
        // Binding 3: chunk indices (storage, read).
        wgpu::BindGroupLayoutEntry {
            binding: 3,
            visibility: vis,
            ty: storage_ro,
            count: None,
        },
    ]
}

/// Pack the `ShadowPassParams` uniform. Mirrors the WGSL struct in
/// `sdf_shadow.wgsl` (see `SHADOW_PASS_PARAMS_SIZE` for the layout table).
/// Kept as a free function so it can be unit-tested without a wgpu device.
pub(crate) fn pack_params_bytes(
    frame: SdfShadowFrameInputs,
    half_res: (u32, u32),
    tuning: SdfShadowTuning,
    sh_grid: SdfShadowShGrid,
) -> [u8; SHADOW_PASS_PARAMS_SIZE] {
    let mut bytes = [0u8; SHADOW_PASS_PARAMS_SIZE];
    // 0..64: inv_view_proj (column-major, same convention as the rest of the
    // renderer's mat4 uploads — see `build_uniform_data` in render/mod.rs).
    let cols = frame.inv_view_proj.to_cols_array();
    for (i, v) in cols.iter().enumerate() {
        let off = i * 4;
        bytes[off..off + 4].copy_from_slice(&v.to_ne_bytes());
    }
    // 64..76: camera_position; 76..80: half_res_size_x.
    bytes[64..68].copy_from_slice(&frame.camera_position[0].to_ne_bytes());
    bytes[68..72].copy_from_slice(&frame.camera_position[1].to_ne_bytes());
    bytes[72..76].copy_from_slice(&frame.camera_position[2].to_ne_bytes());
    bytes[76..80].copy_from_slice(&half_res.0.to_ne_bytes());
    // 80..84: half_res_size_y; 84..88: max_march_steps.
    bytes[80..84].copy_from_slice(&half_res.1.to_ne_bytes());
    bytes[84..88].copy_from_slice(&tuning.max_march_steps.to_ne_bytes());
    // 88..92: open_space_skip_threshold; 92..96: penumbra_k.
    bytes[88..92].copy_from_slice(&tuning.open_space_skip_threshold.to_ne_bytes());
    bytes[92..96].copy_from_slice(&tuning.penumbra_k.to_ne_bytes());
    // 96..108: sh_grid_origin; 108..112: sh_has_volume.
    bytes[96..100].copy_from_slice(&sh_grid.origin[0].to_ne_bytes());
    bytes[100..104].copy_from_slice(&sh_grid.origin[1].to_ne_bytes());
    bytes[104..108].copy_from_slice(&sh_grid.origin[2].to_ne_bytes());
    let has_vol: u32 = if sh_grid.has_volume { 1 } else { 0 };
    bytes[108..112].copy_from_slice(&has_vol.to_ne_bytes());
    // 112..124: sh_cell_size; 124..128: _pad0.
    bytes[112..116].copy_from_slice(&sh_grid.cell_size[0].to_ne_bytes());
    bytes[116..120].copy_from_slice(&sh_grid.cell_size[1].to_ne_bytes());
    bytes[120..124].copy_from_slice(&sh_grid.cell_size[2].to_ne_bytes());
    // 128..140: sh_grid_dimensions; 140..144: _pad1.
    bytes[128..132].copy_from_slice(&sh_grid.dimensions[0].to_ne_bytes());
    bytes[132..136].copy_from_slice(&sh_grid.dimensions[1].to_ne_bytes());
    bytes[136..140].copy_from_slice(&sh_grid.dimensions[2].to_ne_bytes());
    bytes
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The composed shader (pass + shared K-selection helper) must parse and
    /// fully validate via naga, declare the `cs_main` entry point, and write the
    /// K-slice target. Run without a GPU — `cargo test` has no wgpu device.
    /// The composed source is what the pipeline actually compiles; the pass
    /// shader alone references the helper's `select_sdf_lights`, so it must be
    /// validated composed (mirrors `forward.wgsl` + `curve_eval.wgsl` in mod.rs).
    #[test]
    fn sdf_shadow_shader_parses_and_declares_cs_main() {
        let src = SDF_SHADOW_SHADER_SOURCE;
        let module =
            naga::front::wgsl::parse_str(src).expect("composed SDF shadow source should parse");
        // Full validation catches type/binding errors a bare parse misses.
        naga::valid::Validator::new(
            naga::valid::ValidationFlags::all(),
            naga::valid::Capabilities::all(),
        )
        .validate(&module)
        .expect("composed SDF shadow source should validate");

        let has_cs = module
            .entry_points
            .iter()
            .any(|ep| ep.name == "cs_main" && ep.stage == naga::ShaderStage::Compute);
        assert!(has_cs, "cs_main compute entry point missing");

        // The K-slice target: an rgba8unorm storage write the entry point fills.
        assert!(
            src.contains("shadow_factor: texture_storage_2d<rgba8unorm, write>"),
            "K-slice target must be an rgba8unorm storage write",
        );
        assert!(
            src.contains("textureStore(\n        shadow_factor,"),
            "cs_main must write the K-slice target via textureStore",
        );
        // Per-light visibility (R/G/B/A): the selection helper drives the
        // per-light rays — one `trace_shadow` per selected light, no animated
        // dominant-direction trace.
        assert!(
            src.contains("select_sdf_lights(world)"),
            "cs_main must select per-light sdf shadows via the shared helper",
        );
        let trace_calls = src.matches("trace_shadow(").count();
        assert!(
            trace_calls >= 1,
            "expected at least the per-light trace_shadow call; found {trace_calls}",
        );
        // The removed static AND animated dominant-direction bindings must be gone.
        assert!(
            !src.contains("static_lm_direction") && !src.contains("animated_lm_direction"),
            "the dominant-direction bindings (static and animated) must be removed",
        );
        // The lightmap-UV gbuffer existed only for the animated trace — it must
        // be gone now that the per-light trace keys on light position.
        assert!(
            !src.contains("lightmap_uv_tex"),
            "the lightmap-UV gbuffer binding must be removed (per-light trace keys on position)",
        );
    }

    /// After dropping the animated dominant-direction trace, the pass-owned
    /// group-1 BGL is exactly four entries: params, depth, SH depth moments,
    /// and the shadow-factor storage output. No lightmap-UV gbuffer, no
    /// animated-direction texture.
    #[test]
    fn sdf_shadow_bgl_has_no_gbuffer_or_direction_bindings() {
        let entries = bind_group_layout_entries();
        assert_eq!(
            entries.len(),
            4,
            "group 1 must have exactly four bindings after removing the animated trace",
        );
        // Binding 3 is the storage-write output (was 4 before the renumber).
        let out = entries
            .iter()
            .find(|e| e.binding == 3)
            .expect("BGL must declare the shadow-factor output at binding 3");
        assert!(matches!(
            out.ty,
            wgpu::BindingType::StorageTexture {
                access: wgpu::StorageTextureAccess::WriteOnly,
                ..
            }
        ));

        let src = include_str!("../shaders/sdf_shadow.wgsl");
        assert!(
            src.contains("@group(1) @binding(3) var shadow_factor"),
            "shadow_factor must be at @group(1) @binding(3) after the renumber",
        );
    }

    /// The group-2 light buffers the shared K-selection helper reads are
    /// declared in the pass-owned BGL and the shader.
    #[test]
    fn sdf_shadow_binds_static_light_buffers_for_selection() {
        let entries = light_bind_group_layout_entries();
        assert_eq!(entries.len(), 4, "group 2 has four light-buffer bindings");
        // spec_lights is a read-only storage buffer.
        assert!(matches!(
            entries[0].ty,
            wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Storage { read_only: true },
                ..
            }
        ));
        // chunk grid info is a uniform.
        assert!(matches!(
            entries[1].ty,
            wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Uniform,
                ..
            }
        ));

        let src = include_str!("../shaders/sdf_shadow.wgsl");
        assert!(
            src.contains("@group(2) @binding(0) var<storage, read> spec_lights")
                && src.contains("@group(2) @binding(1) var<uniform> chunk_grid")
                && src.contains("@group(2) @binding(2) var<storage, read> chunk_offsets")
                && src.contains("@group(2) @binding(3) var<storage, read> chunk_indices"),
            "group-2 light buffers must be declared for the shared K-selection helper",
        );
    }

    /// Fine-path wiring guard (regression guard, not a correctness proof).
    /// Asserts the fine-atlas sampler exists, that `trace_shadow` steps on it
    /// (not solely on the coarse sampler), and that the fine atlas (`sdf_atlas`)
    /// is actually read. It passes even if the index math is wrong — it only
    /// confirms the fine path is wired in and stays wired; feature correctness
    /// is proven by the visual ACs. No mirrored-arithmetic test is added (it
    /// would re-encode the index math and prove nothing).
    #[test]
    fn sdf_shadow_traces_on_fine_atlas_sampler() {
        let src = include_str!("../shaders/sdf_shadow.wgsl");
        assert!(
            src.contains("fn sample_fine_distance("),
            "the fine-atlas distance sampler must be present",
        );
        assert!(
            src.contains("sample_fine_distance(p)"),
            "trace_shadow must step on sample_fine_distance, not the coarse-only field",
        );
        assert!(
            src.contains("textureLoad(sdf_atlas"),
            "the fine sampler must read the fine atlas (sdf_atlas) via textureLoad",
        );
        // The coarse multiply that over-stepped the empty-brick fallback must
        // be gone — sample_coarse_distance returns metric meters directly.
        assert!(
            !src.contains("max(coarse, 0.0) * brick_world_size"),
            "the coarse-unit fix must drop the `* brick_world_size` over-scale",
        );
    }

    /// Validates the uniform packing matches the documented byte layout.
    #[test]
    fn pack_params_bytes_encodes_camera_half_res_and_tuning() {
        let frame = SdfShadowFrameInputs {
            inv_view_proj: Mat4::IDENTITY,
            camera_position: [1.0, 2.0, 3.0],
        };
        let bytes = pack_params_bytes(
            frame,
            (320, 200),
            SdfShadowTuning {
                max_march_steps: 64,
                open_space_skip_threshold: 1.5,
                penumbra_k: 8.0,
            },
            SdfShadowShGrid {
                origin: [-4.0, 0.0, -4.0],
                cell_size: [1.0, 1.0, 1.0],
                dimensions: [8, 4, 8],
                has_volume: true,
            },
        );
        assert_eq!(bytes.len(), SHADOW_PASS_PARAMS_SIZE);

        // Identity matrix: diagonal is 1.0 at columns (0,0), (1,1), (2,2), (3,3).
        let diag0 = f32::from_ne_bytes(bytes[0..4].try_into().unwrap());
        assert_eq!(diag0, 1.0);
        let diag3 = f32::from_ne_bytes(bytes[60..64].try_into().unwrap());
        assert_eq!(diag3, 1.0);

        let cx = f32::from_ne_bytes(bytes[64..68].try_into().unwrap());
        let cz = f32::from_ne_bytes(bytes[72..76].try_into().unwrap());
        assert_eq!(cx, 1.0);
        assert_eq!(cz, 3.0);

        let half_x = u32::from_ne_bytes(bytes[76..80].try_into().unwrap());
        let half_y = u32::from_ne_bytes(bytes[80..84].try_into().unwrap());
        assert_eq!(half_x, 320);
        assert_eq!(half_y, 200);

        let max_steps = u32::from_ne_bytes(bytes[84..88].try_into().unwrap());
        let skip_thresh = f32::from_ne_bytes(bytes[88..92].try_into().unwrap());
        let k = f32::from_ne_bytes(bytes[92..96].try_into().unwrap());
        assert_eq!(max_steps, 64);
        assert_eq!(skip_thresh, 1.5);
        assert_eq!(k, 8.0);

        let sh_origin_x = f32::from_ne_bytes(bytes[96..100].try_into().unwrap());
        let has_vol = u32::from_ne_bytes(bytes[108..112].try_into().unwrap());
        let sh_dim_x = u32::from_ne_bytes(bytes[128..132].try_into().unwrap());
        assert_eq!(sh_origin_x, -4.0);
        assert_eq!(has_vol, 1);
        assert_eq!(sh_dim_x, 8);
    }

    /// Sanity-check the half-res scaling — odd full-res dimensions still
    /// yield a non-zero half-res target.
    #[test]
    fn half_res_clamps_to_one_for_tiny_surfaces() {
        assert_eq!(compute_half_res(1, 1), (1, 1));
        assert_eq!(compute_half_res(0, 0), (1, 1));
        assert_eq!(compute_half_res(320, 200), (160, 100));
        assert_eq!(compute_half_res(3, 5), (1, 2));
    }

    /// Task 7 setters clamp into the documented range and mutate the
    /// in-memory tuning struct. Exercises the seam the debug-UI sliders write
    /// through without needing a wgpu device.
    #[test]
    fn tuning_setters_clamp_and_mutate() {
        let mut tuning = SdfShadowTuning::default();

        // Replicate the clamping the setters apply — keeps the test honest if
        // the bounds change.
        let apply_max_march = |t: &mut SdfShadowTuning, v: u32| {
            t.max_march_steps = v.clamp(1, 256);
        };
        let apply_skip = |t: &mut SdfShadowTuning, v: f32| {
            t.open_space_skip_threshold = v.max(0.0);
        };
        let apply_k = |t: &mut SdfShadowTuning, v: f32| {
            t.penumbra_k = v.max(0.01);
        };

        apply_max_march(&mut tuning, 0);
        assert_eq!(tuning.max_march_steps, 1, "zero clamps up to 1");
        apply_max_march(&mut tuning, 1024);
        assert_eq!(tuning.max_march_steps, 256, "huge value clamps to 256");
        apply_max_march(&mut tuning, 96);
        assert_eq!(tuning.max_march_steps, 96);

        apply_skip(&mut tuning, -1.0);
        assert_eq!(tuning.open_space_skip_threshold, 0.0);
        apply_skip(&mut tuning, 4.0);
        assert_eq!(tuning.open_space_skip_threshold, 4.0);

        apply_k(&mut tuning, 0.0);
        assert!(tuning.penumbra_k > 0.0, "k must stay positive");
        apply_k(&mut tuning, 32.0);
        assert_eq!(tuning.penumbra_k, 32.0);

        // The packing function picks up the mutated tuning verbatim.
        let bytes = pack_params_bytes(
            SdfShadowFrameInputs {
                inv_view_proj: Mat4::IDENTITY,
                camera_position: [0.0; 3],
            },
            (16, 16),
            tuning,
            SdfShadowShGrid::default(),
        );
        let packed_max = u32::from_ne_bytes(bytes[84..88].try_into().unwrap());
        let packed_k = f32::from_ne_bytes(bytes[92..96].try_into().unwrap());
        assert_eq!(packed_max, 96);
        assert_eq!(packed_k, 32.0);
    }

    #[test]
    fn shadow_pass_params_size_matches_layout_doc() {
        // The size doc-comment lists field offsets ending at 144; if a future
        // edit drifts, this anchors the WGSL/Rust agreement.
        assert_eq!(SHADOW_PASS_PARAMS_SIZE, 144);
    }
}
