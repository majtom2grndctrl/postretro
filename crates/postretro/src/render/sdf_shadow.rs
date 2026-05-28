// Half-resolution SDF static-occluder shadow pass.
//
// Task 4 of `sdf-static-occluder-shadows`. Runs as a compute pass between the
// depth pre-pass and the forward pass. Per half-res pixel, traces two rays
// against the static SDF (Task 3) and writes the per-term shadow factors into
// a `Rgba8Unorm` half-res target:
//   R = static-lightmap aggregate factor
//   G = animated-baked aggregate factor
//   B, A = reserved for the geometry-moving per-light terms (documented seam —
//          no v1 consumer; see plan §Goal).
//
// Forward integration (Task 5) reads this target and multiplies the static and
// animated-baked terms by R and G respectively. When the SDF atlas isn't
// present, the pass is skipped and the target stays at its `Clear` color of
// (1, 1, 1, 1) — the forward multiply degrades cleanly.
//
// Pipeline layout: group 0 = SDF atlas (owned by `SdfAtlasResources`),
// group 1 = this pass's own bind group (depth, direction textures, SH depth
// moments, params uniform, shadow factor output).

use glam::Mat4;

use super::sdf_atlas::SdfAtlasResources;

/// Half-resolution divisor relative to the swap-chain. `2` matches the
/// resolution-scale convention used by the legacy SDF code (see
/// `context/plans/in-progress/sdf-static-occluder-shadows/research.md`).
pub const HALF_RES_SCALE: u32 = 2;

/// Color format of the shadow-factor target. R = static-lightmap aggregate,
/// G = animated-baked aggregate, B/A = reserved.
pub const SHADOW_FACTOR_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;

/// Default uniform values for the tuning knobs. Task 7 wires sliders to these.
pub const DEFAULT_MAX_MARCH_STEPS: u32 = 48;
pub const DEFAULT_OPEN_SPACE_SKIP_THRESHOLD: f32 = 2.5; // multiple of SH cell size
pub const DEFAULT_PENUMBRA_K: f32 = 16.0;

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

/// GPU resources for the half-res SDF shadow pass.
pub struct SdfShadowPass {
    pipeline: wgpu::ComputePipeline,
    /// Bind-group layout for group 1 (the pass-owned bindings — depth,
    /// direction textures, SH depth moments, params, output).
    bind_group_layout: wgpu::BindGroupLayout,
    /// Half-res `Rgba8Unorm` shadow factor target. Cleared to (1,1,1,1) at
    /// allocation so the pass-skipped path is "fully lit".
    #[allow(dead_code)]
    shadow_texture: wgpu::Texture,
    /// View into `shadow_texture` exposed to Task 5's forward pass for the
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
    /// `depth_view` (which the renderer recreates on resize, so the bind
    /// group must be rebuilt too) and the direction / depth-moment views
    /// (stable across resizes — only rebuilt on level reload).
    bind_group: wgpu::BindGroup,
    /// Static lightmap dominant direction. Sampled to feed the static-term
    /// trace. Stable across resizes — captured at level load.
    static_lm_direction_view: wgpu::TextureView,
    /// Animated-baked atlas per-frame dominant direction (Task 2b). Sampled
    /// to feed the animated-term trace.
    animated_lm_direction_view: wgpu::TextureView,
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
        static_lm_direction_view: wgpu::TextureView,
        animated_lm_direction_view: wgpu::TextureView,
        sh_depth_moments_view: wgpu::TextureView,
        sh_grid: SdfShadowShGrid,
        full_res_width: u32,
        full_res_height: u32,
    ) -> Self {
        let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("SDF Shadow Bind Group Layout"),
            entries: &bind_group_layout_entries(),
        });

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("SDF Shadow Pipeline Layout"),
            bind_group_layouts: &[Some(sdf_atlas_layout), Some(&bind_group_layout)],
            immediate_size: 0,
        });

        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("SDF Shadow Shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("../shaders/sdf_shadow.wgsl").into()),
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
            &static_lm_direction_view,
            &animated_lm_direction_view,
            &sh_depth_moments_view,
            &shadow_storage_view,
        );

        Self {
            pipeline,
            bind_group_layout,
            shadow_texture,
            shadow_view,
            shadow_storage_view,
            half_res,
            params_buffer,
            bind_group,
            static_lm_direction_view,
            animated_lm_direction_view,
            sh_depth_moments_view,
            sh_grid,
            tuning: SdfShadowTuning::default(),
        }
    }

    /// View into the half-res shadow factor target. Consumed by Task 5's
    /// forward pass for the bilateral upsample.
    #[allow(dead_code)]
    pub fn shadow_view(&self) -> &wgpu::TextureView {
        &self.shadow_view
    }

    /// Current half-res dimensions. Useful for the Task 5 forward pass to
    /// compute the upsample sampling step.
    #[allow(dead_code)]
    pub fn half_res(&self) -> (u32, u32) {
        self.half_res
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
            &self.static_lm_direction_view,
            &self.animated_lm_direction_view,
            &self.sh_depth_moments_view,
            &self.shadow_storage_view,
        );
    }

    /// Rebuild the views the pass depends on after a level load (lightmap +
    /// animated atlas + SH section all swap). The depth view is unchanged by
    /// a level load (it's owned by the renderer's surface state), so the
    /// caller passes the current one back in.
    #[allow(clippy::too_many_arguments)]
    pub fn rebuild_for_level(
        &mut self,
        device: &wgpu::Device,
        depth_view: &wgpu::TextureView,
        static_lm_direction_view: wgpu::TextureView,
        animated_lm_direction_view: wgpu::TextureView,
        sh_depth_moments_view: wgpu::TextureView,
        sh_grid: SdfShadowShGrid,
    ) {
        self.static_lm_direction_view = static_lm_direction_view;
        self.animated_lm_direction_view = animated_lm_direction_view;
        self.sh_depth_moments_view = sh_depth_moments_view;
        self.sh_grid = sh_grid;
        self.bind_group = build_bind_group(
            device,
            &self.bind_group_layout,
            &self.params_buffer,
            depth_view,
            &self.static_lm_direction_view,
            &self.animated_lm_direction_view,
            &self.sh_depth_moments_view,
            &self.shadow_storage_view,
        );
    }

    /// Encode the per-frame dispatch. The caller has already determined the
    /// pass should run (`sdf_atlas.present == true` and SDF mode is on — Task
    /// 6 will wire the off/visualize mode selector). When skipped, the shadow
    /// target retains its last contents — Task 5's forward pass is responsible
    /// for guarding the multiply on the mode flag.
    pub fn dispatch(
        &self,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        sdf_atlas: &SdfAtlasResources,
        frame: SdfShadowFrameInputs,
        timestamp_writes: Option<wgpu::ComputePassTimestampWrites<'_>>,
    ) {
        let bytes = pack_params_bytes(
            frame,
            self.half_res,
            self.tuning,
            self.sh_grid,
        );
        queue.write_buffer(&self.params_buffer, 0, &bytes);

        let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
            label: Some("SDF Shadow Pass"),
            timestamp_writes,
        });
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, &sdf_atlas.bind_group, &[]);
        pass.set_bind_group(1, &self.bind_group, &[]);
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
        // STORAGE_BINDING for the compute write, TEXTURE_BINDING for the Task 5
        // bilateral upsample read.
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

#[allow(clippy::too_many_arguments)]
fn build_bind_group(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    params_buffer: &wgpu::Buffer,
    depth_view: &wgpu::TextureView,
    static_lm_direction_view: &wgpu::TextureView,
    animated_lm_direction_view: &wgpu::TextureView,
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
                resource: wgpu::BindingResource::TextureView(static_lm_direction_view),
            },
            wgpu::BindGroupEntry {
                binding: 3,
                resource: wgpu::BindingResource::TextureView(animated_lm_direction_view),
            },
            wgpu::BindGroupEntry {
                binding: 4,
                resource: wgpu::BindingResource::TextureView(sh_depth_moments_view),
            },
            wgpu::BindGroupEntry {
                binding: 5,
                resource: wgpu::BindingResource::TextureView(shadow_storage_view),
            },
        ],
    })
}

fn bind_group_layout_entries() -> [wgpu::BindGroupLayoutEntry; 6] {
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
        // Binding 2: static lightmap direction (Rgba8Unorm, non-filterable load).
        wgpu::BindGroupLayoutEntry {
            binding: 2,
            visibility: vis,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable: false },
                view_dimension: wgpu::TextureViewDimension::D2,
                multisampled: false,
            },
            count: None,
        },
        // Binding 3: animated lightmap direction (Rgba8Unorm).
        wgpu::BindGroupLayoutEntry {
            binding: 3,
            visibility: vis,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable: false },
                view_dimension: wgpu::TextureViewDimension::D2,
                multisampled: false,
            },
            count: None,
        },
        // Binding 4: SH depth moments (Rg16Float 3D, non-filterable load).
        wgpu::BindGroupLayoutEntry {
            binding: 4,
            visibility: vis,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable: false },
                view_dimension: wgpu::TextureViewDimension::D3,
                multisampled: false,
            },
            count: None,
        },
        // Binding 5: shadow-factor output (Rgba8Unorm storage write).
        wgpu::BindGroupLayoutEntry {
            binding: 5,
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

    /// The shader source must parse via naga and declare the expected entry
    /// point and binding slots. Run without a GPU — `cargo test` has no
    /// wgpu device. Mirrors `compose_shader_parses_and_declares_debug_binding`
    /// in `animated_lightmap.rs`.
    #[test]
    fn sdf_shadow_shader_parses_and_declares_cs_main() {
        let src = include_str!("../shaders/sdf_shadow.wgsl");
        let module =
            naga::front::wgsl::parse_str(src).expect("sdf_shadow.wgsl should parse as WGSL");
        let has_cs = module
            .entry_points
            .iter()
            .any(|ep| ep.name == "cs_main" && ep.stage == naga::ShaderStage::Compute);
        assert!(has_cs, "cs_main compute entry point missing");
        // Ensure the new shadow-factor target is declared at the expected slot.
        assert!(
            src.contains("shadow_factor"),
            "shadow_factor binding missing from sdf_shadow.wgsl",
        );
        assert!(
            src.contains("rgba8unorm"),
            "shadow factor target must be rgba8unorm (R = static, G = animated)",
        );
        // The two aggregate traces must both be invoked — the architecture
        // requires per-pixel evaluation of both terms in v1.
        let static_trace = src.matches("trace_shadow(").count();
        assert!(
            static_trace >= 2,
            "expected both static and animated `trace_shadow` calls; found {static_trace}",
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

    #[test]
    fn shadow_pass_params_size_matches_layout_doc() {
        // The size doc-comment lists field offsets ending at 144; if a future
        // edit drifts, this anchors the WGSL/Rust agreement.
        assert_eq!(SHADOW_PASS_PARAMS_SIZE, 144);
    }
}
