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

#[allow(unused_imports)]
pub use postretro_render_cpu::sdf_shadow::{
    DEFAULT_MAX_MARCH_STEPS, DEFAULT_OPEN_SPACE_SKIP_THRESHOLD, DEFAULT_PENUMBRA_K,
    DEFAULT_SURFACE_BIAS_VOXELS, SHADOW_PASS_PARAMS_SIZE, SdfShadowFrameInputs, SdfShadowShGrid,
    SdfShadowTuning, pack_params_bytes,
};

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
    "\n",
    include_str!("../shaders/light_falloff.wgsl"),
);

/// Half-resolution divisor relative to the swap-chain. `2` matches the
/// resolution-scale convention used by the legacy SDF code (see
/// `context/plans/in-progress/sdf-static-occluder-shadows/research.md`).
pub const HALF_RES_SCALE: u32 = 2;

/// Color format of the shadow-factor target. The four channels are the K = 4
/// per-light SDF visibility slices: R = slot 0, G = slot 1, B = slot 2, A = slot 3.
pub const SHADOW_FACTOR_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;

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

    /// Write through to `tuning.surface_bias` (× voxel). Larger = the march
    /// origin is pushed further off the originating surface ALONG ITS NORMAL,
    /// which kills SDF self-shadow blobs on lit faces but, taken too far, begins
    /// to peter-pan / detach a nearby occluder's contact shadow. Clamped
    /// non-negative.
    #[cfg_attr(not(feature = "dev-tools"), allow(dead_code))]
    pub fn set_surface_bias(&mut self, bias: f32) {
        self.tuning.surface_bias = bias.max(0.0);
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
        // TEMP DEBUG: SDF shadow path visualization. Non-zero selects a debug-viz
        // mode (3 = trace-outcome paths, 4 = reconstructed normals); the pass then
        // writes an RGB debug code for slot 0 instead of per-light visibility
        // floats. 0 = production path.
        debug_mode: u32,
    ) {
        let bytes = pack_params_bytes(frame, self.half_res, self.tuning, self.sh_grid, debug_mode);
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
    /// is sampled with hardware trilinear via `textureSampleLevel` + the
    /// filtering sampler (not the old nearest `textureLoad`). It passes even if
    /// the index math is wrong — it only confirms the fine path is wired in and
    /// stays wired; feature correctness is proven by the visual ACs.
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
            src.contains("textureSampleLevel(sdf_atlas, sdf_sampler"),
            "the fine sampler must trilinear-sample sdf_atlas via textureSampleLevel + sdf_sampler",
        );
        assert!(
            !src.contains("textureLoad(sdf_atlas"),
            "the fine atlas must no longer use nearest textureLoad (replaced by trilinear sampling)",
        );
        // The coarse multiply that over-stepped the empty-brick fallback must
        // be gone — sample_coarse_distance returns metric meters directly.
        assert!(
            !src.contains("max(coarse, 0.0) * brick_world_size"),
            "the coarse-unit fix must drop the `* brick_world_size` over-scale",
        );
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

    /// Rust twin of `trace_shadow`'s march loop (`sdf_shadow.wgsl`) against an
    /// analytic field, with the grazing fade at 1 (receiver faces the light)
    /// and no open-space skip. Transcribes the WGSL loop line-for-line so the
    /// penumbra estimator's BEHAVIOR is pinned on CPU; the source-scan assert
    /// below ties it to the shader text. `cone_cap` toggles the voxel cap on
    /// the virtual light size — `false` reproduces the plain receiver-angle
    /// term for the comparison asserts.
    fn trace_twin(
        origin: glam::Vec3,
        dir: glam::Vec3,
        max_dist: f32,
        cone_cap: bool,
        field: impl Fn(glam::Vec3) -> f32,
    ) -> f32 {
        let voxel = 0.5f32; // DEFAULT_VOXEL_SIZE_METERS
        let start_eps = voxel * 0.5;
        let mut t = start_eps;
        let mut factor = 1.0f32;
        let k = 8.0f32; // DEFAULT_PENUMBRA_K
        let cone_scale = if cone_cap { k.max(max_dist / voxel) } else { k };
        let max_t = (64.0f32.min(max_dist - voxel)).max(start_eps);
        let mut ph = 1.0e10f32;
        for _ in 0..64 {
            let p = origin + dir * t;
            let h = field(p);
            if h < voxel * 0.5 {
                return 0.0;
            }
            if t > start_eps && h <= ph {
                let y = h * h / (2.0 * ph.max(voxel * 0.5));
                let estimate = (h * h - y * y).max(0.0).sqrt();
                let soft = cone_scale * estimate / (t - y).max(voxel);
                factor = factor.min(soft);
            }
            ph = h;
            t += h.max(voxel * 0.5);
            if t > max_t {
                break;
            }
        }
        factor.clamp(0.0, 1.0)
    }

    /// The soft term's virtual light size is capped at one SDF voxel
    /// (`cone_scale = max(k, max_dist/voxel)`). Uncapped, the receiver-angle
    /// `k·h/(t−y)` term models a light disk of radius `distance/k` that GROWS
    /// with receiver distance: a ceiling lamp ~0.6 m (≈ one voxel) under the
    /// ceiling sends every shadow ray through the ceiling's near field close
    /// to the light, the oversized disk reads that as occlusion, and every
    /// wall beyond `k·clearance` meters darkens in proportion to its distance
    /// — false shadows across a whole room. The cap keeps such lights fully
    /// lit, leaves short-range (`max_dist ≤ k·voxel`) penumbras exactly as
    /// `k` tunes them, and — being pointwise ≥ the uncapped term — can only
    /// remove darkening, never add it. Hard hits are untouched.
    #[test]
    fn penumbra_cone_caps_virtual_light_size_at_one_voxel() {
        // Ceiling plane at y = 4; lamp 0.6 m below it; receiver on a wall
        // 10 m away. The field models only the ceiling (the mechanism under
        // test); walls/floor don't matter to it.
        let light = glam::Vec3::new(0.0, 3.4, 0.0);
        let origin = glam::Vec3::new(10.0, 2.0, 0.0);
        let to_light = light - origin;
        let max_dist = to_light.length();
        let dir = to_light / max_dist;
        let ceiling = |p: glam::Vec3| 4.0 - p.y;

        // Capped cone: clear line of sight → fully lit.
        let lit = trace_twin(origin, dir, max_dist, true, ceiling);
        assert!(
            lit >= 0.99,
            "clear sight to a ceiling-mounted lamp must be fully lit, got {lit}"
        );

        // The uncapped term on the same march darkens this receiver — the
        // false-shadow regression the cap exists to block.
        let uncapped = trace_twin(origin, dir, max_dist, false, ceiling);
        assert!(
            uncapped < 0.7,
            "sanity: the uncapped receiver-angle term darkens this receiver \
             ({uncapped}) — if it no longer does, this scenario stopped \
             exercising the mechanism"
        );

        // Short range (max_dist ≤ k·voxel): the cap is inactive and the look
        // `k` tunes is unchanged — capped and uncapped agree exactly. The
        // point occluder sits 0.3 m off the ray (below one voxel's soft
        // window near the light, above the 0.25 m hit threshold everywhere)
        // so the runs land in a live penumbra rather than comparing 1.0s.
        let near_light = glam::Vec3::new(0.0, 3.4, 0.0);
        let near_origin = glam::Vec3::new(3.4, 2.4, 0.0);
        let near_to = near_light - near_origin;
        let (near_d, near_dir) = (near_to.length(), near_to.normalize());
        assert!(
            near_d <= 8.0 * 0.5,
            "scenario must sit inside the cap-free range"
        );
        let occluder = near_origin + near_dir * 2.8 + glam::Vec3::new(0.0, 0.0, 0.3);
        let field = |p: glam::Vec3| ceiling(p).min((p - occluder).length());
        let capped_near = trace_twin(near_origin, near_dir, near_d, true, field);
        let uncapped_near = trace_twin(near_origin, near_dir, near_d, false, field);
        assert_eq!(
            capped_near, uncapped_near,
            "within k·voxel of the light the cap must not change the factor"
        );
        assert!(
            (0.01..0.99).contains(&capped_near),
            "the short-range scenario must land in the penumbra (got {capped_near}) \
             so the equality above compares live soft terms, not two 1.0s"
        );

        // A sphere ON the segment still hard-shadows.
        let block_center = origin + dir * (max_dist * 0.5);
        let blocked = trace_twin(origin, dir, max_dist, true, |p| {
            ceiling(p).min((p - block_center).length() - 0.2)
        });
        assert_eq!(blocked, 0.0, "blocking occluder must hard-shadow");

        // Tie the twin to the shader: the capped scale and the soft term
        // appear in BOTH `trace_shadow` and its debug twin.
        let src = include_str!("../shaders/sdf_shadow.wgsl");
        assert_eq!(
            src.matches("let cone_scale = max(k, max_dist / voxel);")
                .count(),
            2,
            "voxel-capped cone scale must appear in trace_shadow and debug_trace_outcome"
        );
        assert_eq!(
            src.matches("let soft = cone_scale * estimate / max(t - y, voxel);")
                .count(),
            2,
            "soft term must use cone_scale in trace_shadow and debug_trace_outcome"
        );
    }
}
