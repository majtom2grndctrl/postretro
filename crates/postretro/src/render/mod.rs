// Renderer: GPU init, texture upload, depth pre-pass + forward pipelines, and draw.
// See: context/lib/rendering_pipeline.md

pub mod animated_lightmap;
pub mod fog_pass;
pub mod frame_timing;
pub mod sh_compose;
pub mod sh_volume;
pub mod smoke;

#[cfg(test)]
mod curve_eval_test;

use std::sync::Arc;
use std::time::Instant;

use anyhow::{Context, Result};
use glam::{Mat4, Vec3};
use wgpu::util::DeviceExt;
use winit::window::Window;

use crate::compute_cull::ComputeCullPipeline;
use crate::geometry::BvhTree;
use crate::lighting::chunk_list::ChunkGrid;
use crate::lighting::influence::{self, LightInfluence};
use crate::lighting::lightmap::LightmapResources;
use crate::lighting::spec_buffer::{SPEC_LIGHT_SIZE, pack_spec_lights};
use crate::lighting::spot_shadow::SpotShadowPool;
use crate::lighting::{GPU_LIGHT_SIZE, pack_lights, pack_lights_with_slots};
use crate::material::Material;
use crate::prl::MapLight;
use crate::texture::{LoadedTexture, TextureSet};
use crate::visibility::VisibleCells;
use postretro_level_format::alpha_lights::ALPHA_LIGHT_LEAF_UNASSIGNED;

use fog_pass::FogPass;
use frame_timing::FrameTiming;
use sh_compose::ShComposeResources;
use sh_volume::ShVolumeResources;
use smoke::SmokePass;

use crate::fx::smoke::SpriteFrame;

// `curve_eval.wgsl` reads `anim_samples` by lexical name; `forward.wgsl`
// declares that buffer. WGSL resolves references at module scope regardless of
// textual order, so appending the helper after `forward.wgsl` is safe.
const SHADER_SOURCE: &str = concat!(
    include_str!("../shaders/forward.wgsl"),
    "\n",
    include_str!("../shaders/curve_eval.wgsl"),
);

const WIREFRAME_SHADER_SOURCE: &str = include_str!("../shaders/wireframe.wgsl");

// Depth pre-pass: vertex-only; enables `depth_compare: Equal` in the forward
// pass so each pixel is shaded exactly once (zero shading overdraw).
const DEPTH_PREPASS_SHADER_SOURCE: &str = include_str!("../shaders/depth_prepass.wgsl");

// Spot shadow depth pass: vertex-only; writes Depth32Float per slot via a
// dynamic-offset uniform selecting the per-slot light-space matrix.
const SPOT_SHADOW_SHADER_SOURCE: &str = include_str!("../shaders/spot_shadow.wgsl");

// Pair index `i` → query slots `[2i, 2i+1]`. Indexed by `FrameTiming::new`'s
// labels vec so label ordering and callsite indices can't drift independently.
const TIMING_PAIR_CULL: usize = 0;
const TIMING_PAIR_ANIMATED_LM_COMPOSE: usize = 1;
const TIMING_PAIR_DEPTH_PREPASS: usize = 2;
const TIMING_PAIR_FORWARD: usize = 3;
const TIMING_PAIR_COUNT: usize = 4;

// std140 aligns vec3<f32> to 16 bytes, so camera_position (vec3) and
// ambient_floor (f32) share one slot. Must match the WGSL `Uniforms` struct
// in forward.wgsl and wireframe.wgsl — both shaders bind the same buffer.
//   0..64    view_proj
//   64..76   camera_position (vec3<f32>)
//   76..80   ambient_floor (f32)
//   80..84   light_count (u32)
//   84..88   time (elapsed seconds for SH animation curves)
//   88..92   lighting_isolation (u32, 0..=9; Alt+Shift+4)
//   92..96   indirect_scale (f32)
const UNIFORM_SIZE: usize = 96;

/// Lighting-term isolation mode for leak/bleed debugging (cycled by Alt+Shift+4).
/// The ambient floor always contributes so interior geometry is never pitch black.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum LightingIsolation {
    Normal = 0,
    NoLightmap = 1,
    DirectOnly = 2,
    IndirectOnly = 3,
    AmbientOnly = 4,
    LightmapOnly = 5,
    StaticSHOnly = 6,
    AnimatedDeltaOnly = 7,
    DynamicOnly = 8,
    SpecularOnly = 9,
}

impl LightingIsolation {
    pub fn cycle(self) -> Self {
        match self {
            LightingIsolation::Normal => LightingIsolation::NoLightmap,
            LightingIsolation::NoLightmap => LightingIsolation::DirectOnly,
            LightingIsolation::DirectOnly => LightingIsolation::IndirectOnly,
            LightingIsolation::IndirectOnly => LightingIsolation::AmbientOnly,
            LightingIsolation::AmbientOnly => LightingIsolation::LightmapOnly,
            LightingIsolation::LightmapOnly => LightingIsolation::StaticSHOnly,
            LightingIsolation::StaticSHOnly => LightingIsolation::AnimatedDeltaOnly,
            LightingIsolation::AnimatedDeltaOnly => LightingIsolation::DynamicOnly,
            LightingIsolation::DynamicOnly => LightingIsolation::SpecularOnly,
            LightingIsolation::SpecularOnly => LightingIsolation::Normal,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            LightingIsolation::Normal => "Normal (all terms)",
            LightingIsolation::NoLightmap => "NoLightmap (all terms except static lightmap)",
            LightingIsolation::DirectOnly => "DirectOnly (lightmap + dynamic + specular)",
            LightingIsolation::IndirectOnly => "IndirectOnly (SH + specular)",
            LightingIsolation::AmbientOnly => "AmbientOnly (ambient floor only)",
            LightingIsolation::LightmapOnly => "LightmapOnly (static lightmap)",
            LightingIsolation::StaticSHOnly => "StaticSHOnly (static SH indirect)",
            LightingIsolation::AnimatedDeltaOnly => "AnimatedDeltaOnly (animated SH delta)",
            LightingIsolation::DynamicOnly => "DynamicOnly (dynamic direct lights)",
            LightingIsolation::SpecularOnly => "SpecularOnly (specular only)",
        }
    }
}

struct FrameUniforms {
    view_proj: Mat4,
    camera_position: Vec3,
    ambient_floor: f32,
    light_count: u32,
    time: f32,
    lighting_isolation: LightingIsolation,
    indirect_scale: f32,
}

fn build_uniform_data(u: &FrameUniforms) -> [u8; UNIFORM_SIZE] {
    let mut bytes = [0u8; UNIFORM_SIZE];
    let cols = u.view_proj.to_cols_array();
    for (i, val) in cols.iter().enumerate() {
        let off = i * 4;
        bytes[off..off + 4].copy_from_slice(&val.to_ne_bytes());
    }
    bytes[64..68].copy_from_slice(&u.camera_position.x.to_ne_bytes());
    bytes[68..72].copy_from_slice(&u.camera_position.y.to_ne_bytes());
    bytes[72..76].copy_from_slice(&u.camera_position.z.to_ne_bytes());
    bytes[76..80].copy_from_slice(&u.ambient_floor.to_ne_bytes());
    bytes[80..84].copy_from_slice(&u.light_count.to_ne_bytes());
    bytes[84..88].copy_from_slice(&u.time.to_ne_bytes());
    let isolation: u32 = u.lighting_isolation as u32;
    bytes[88..92].copy_from_slice(&isolation.to_ne_bytes());
    bytes[92..96].copy_from_slice(&u.indirect_scale.to_ne_bytes());
    bytes
}

/// Lowest value where a player can still navigate dark areas; tuned via the
/// ambient-floor slider (Alt+Shift+{ / Alt+Shift+}).
pub const DEFAULT_AMBIENT_FLOOR: f32 = 0.001;

pub const DEFAULT_INDIRECT_SCALE: f32 = 0.10;

struct GpuTexture {
    bind_group: wgpu::BindGroup,
}

fn upload_texture_data(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    width: u32,
    height: u32,
    data: &[u8],
    format: wgpu::TextureFormat,
    label: &str,
) -> wgpu::Texture {
    let bytes_per_pixel: u32 = match format {
        wgpu::TextureFormat::Rgba8Unorm | wgpu::TextureFormat::Rgba8UnormSrgb => 4,
        wgpu::TextureFormat::R8Unorm => 1,
        other => panic!("upload_texture_data: unsupported format {other:?}"),
    };
    let size = wgpu::Extent3d {
        width,
        height,
        depth_or_array_layers: 1,
    };

    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some(label),
        size,
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
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
        data,
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(bytes_per_pixel * width),
            rows_per_image: Some(height),
        },
        size,
    );

    texture
}

// The diffuse loader expands grayscale PNGs to RGBA8; only R carries specular
// data so G/B/A are dropped to save 4× VRAM before upload as R8Unorm.
fn extract_r_channel(rgba: &[u8]) -> Vec<u8> {
    rgba.iter().step_by(4).copied().collect()
}

// std140 rounds the struct size to a multiple of 16. The trailing vec3<f32>
// _pad field forces the size to 32 bytes to match the WGSL `MaterialUniform`.
//   0..4   shininess (f32)
//   4..32  pad
const MATERIAL_UNIFORM_SIZE: usize = 32;

fn build_material_uniform(shininess: f32) -> [u8; MATERIAL_UNIFORM_SIZE] {
    let mut bytes = [0u8; MATERIAL_UNIFORM_SIZE];
    bytes[0..4].copy_from_slice(&shininess.to_le_bytes());
    bytes
}

const DEPTH_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Depth32Float;

fn create_depth_texture(
    device: &wgpu::Device,
    width: u32,
    height: u32,
) -> (wgpu::Texture, wgpu::TextureView) {
    let size = wgpu::Extent3d {
        width: width.max(1),
        height: height.max(1),
        depth_or_array_layers: 1,
    };

    let depth_texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("Depth Texture"),
        size,
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: DEPTH_FORMAT,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
        view_formats: &[],
    });

    let view = depth_texture.create_view(&wgpu::TextureViewDescriptor::default());
    (depth_texture, view)
}

pub struct LevelGeometry<'a> {
    pub vertices: &'a [crate::geometry::WorldVertex],
    pub indices: &'a [u32],
    pub bvh: &'a BvhTree,
    pub lights: &'a [MapLight],
    pub light_influences: &'a [LightInfluence],
    /// `None` → renderer binds dummy 1×1×1 textures; shader skips SH sampling.
    pub sh_volume: Option<&'a postretro_level_format::sh_volume::ShVolumeSection>,
    /// `None` → 1×1 white placeholder; bumped-Lambert falls back to flat white.
    pub lightmap: Option<&'a postretro_level_format::lightmap::LightmapSection>,
    /// `None` → `has_chunk_grid == 0`; shader iterates the full spec buffer.
    pub chunk_light_list:
        Option<&'a postretro_level_format::chunk_light_list::ChunkLightListSection>,
    /// `None` when the map has zero animated lights.
    pub animated_light_chunks:
        Option<&'a postretro_level_format::animated_light_chunks::AnimatedLightChunksSection>,
    /// `None` → 1×1 zero atlas bound on group 4.
    pub animated_light_weight_maps: Option<
        &'a postretro_level_format::animated_light_weight_maps::AnimatedLightWeightMapsSection,
    >,
    /// `None` → compose pass falls back to a base→total copy.
    pub delta_sh_volumes:
        Option<&'a postretro_level_format::delta_sh_volumes::DeltaShVolumesSection>,
    pub texture_materials: &'a [crate::material::Material],
}

pub struct Renderer {
    device: wgpu::Device,
    queue: wgpu::Queue,
    surface: wgpu::Surface<'static>,
    surface_config: wgpu::SurfaceConfiguration,
    is_surface_configured: bool,

    pipeline: wgpu::RenderPipeline,
    depth_prepass_pipeline: wgpu::RenderPipeline,
    /// `Some` when `POSTRETRO_GPU_TIMING=1` AND adapter supports `TIMESTAMP_QUERY`;
    /// `None` → no `timestamp_writes` attached to any pass.
    frame_timing: Option<FrameTiming>,
    vertex_buffer: wgpu::Buffer,
    index_buffer: wgpu::Buffer,
    index_count: u32,
    uniform_buffer: wgpu::Buffer,
    uniform_bind_group: wgpu::BindGroup,

    /// Always bound; maps with zero lights get a 1-element dummy buffer —
    /// wgpu rejects zero-sized storage buffer bindings.
    lighting_bind_group: wgpu::BindGroup,
    light_count: u32,
    ambient_floor: f32,
    indirect_scale: f32,

    /// Absent SH section → dummy 1×1×1 textures; `has_sh_volume == 0` skips sampling.
    sh_volume_resources: ShVolumeResources,

    /// Composes base SH bands into the total bands consumers sample. Must run
    /// before the depth pre-pass so the storage→sampled barrier resolves first.
    sh_compose: ShComposeResources,

    /// Absent Lightmap section → 1×1 white/neutral placeholder; no shader branch.
    lightmap_resources: LightmapResources,

    animated_lightmap: animated_lightmap::AnimatedLightmapResources,

    #[allow(dead_code)]
    lights_buffer: wgpu::Buffer,
    #[allow(dead_code)]
    level_lights: Vec<MapLight>,
    /// Lights near zero are excluded from shadow slot ranking. Empty = no suppression.
    light_effective_brightness: Vec<f32>,
    /// Cached from `update_per_frame_uniforms` so the shadow pass can re-rank lights.
    last_camera_position: Vec3,
    spot_shadow_pool: SpotShadowPool,
    /// Dynamic-offset into a single buffer; offset selects the per-slot light-space matrix.
    shadow_vs_uniform_buffer: wgpu::Buffer,
    shadow_vs_bind_group: wgpu::BindGroup,
    shadow_depth_pipeline: wgpu::RenderPipeline,
    /// Rounded up to `min_uniform_buffer_offset_alignment`.
    shadow_vs_stride: u32,

    depth_view: wgpu::TextureView,

    /// GPU textures indexed by texture index.
    gpu_textures: Vec<GpuTexture>,
    bvh_leaves: Vec<crate::geometry::BvhLeaf>,
    /// `None` for maps with no BVH.
    compute_cull: Option<ComputeCullPipeline>,

    wireframe_pipeline: wgpu::RenderPipeline,
    wireframe_index_buffer: wgpu::Buffer,
    wireframe_index_count: u32,
    wireframe_cull_status_bgl: wgpu::BindGroupLayout,
    wireframe_enabled: bool,

    lighting_isolation: LightingIsolation,

    /// Toggled by Alt+Shift+V; `true` = AutoVsync, `false` = AutoNoVsync.
    vsync_enabled: bool,

    has_geometry: bool,

    debug_frame: u64,
    debug_prev_bitmask: (u32, u32),
    debug_prev_vp_hash: u32,
    debug_prev_visible: (&'static str, usize),

    /// `app_start.elapsed()` feeds the `time` uniform; shaders wrap it via
    /// `fract(time / period + phase)` for SH animation curves.
    app_start: Instant,

    /// Idle (no draw) on maps with no registered collections. See §7.4.
    smoke_pass: SmokePass,

    /// Volumetric fog raymarch + composite. Active only when the level has
    /// at least one fog volume uploaded; otherwise the dispatch + composite
    /// are skipped (see `FogPass::active`).
    fog: FogPass,
}

impl Renderer {
    /// `geometry` is `None` when no map is loaded (renders clear color only).
    pub fn new(
        window: &Arc<Window>,
        geometry: Option<&LevelGeometry>,
        texture_set: Option<&TextureSet>,
    ) -> Result<Self> {
        let size = window.inner_size();

        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
            backends: wgpu::Backends::PRIMARY,
            ..wgpu::InstanceDescriptor::new_without_display_handle()
        });

        let surface = instance
            .create_surface(window.clone())
            .context("failed to create wgpu surface")?;

        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::default(),
            compatible_surface: Some(&surface),
            force_fallback_adapter: false,
        }))
        .context("no suitable GPU adapter found")?;

        log::info!("[Renderer] GPU adapter: {}", adapter.get_info().name);

        // Vulkan/Metal/DX12 support multi_draw_indexed_indirect; WebGL2 does not
        // (not a target). Fall back to singular draw_indexed_indirect.
        let downlevel = adapter.get_downlevel_capabilities();
        let has_multi_draw_indirect = downlevel
            .flags
            .contains(wgpu::DownlevelFlags::INDIRECT_EXECUTION);
        if has_multi_draw_indirect {
            log::info!("[Renderer] Indirect execution supported (multi_draw_indexed_indirect)");
        } else {
            log::info!(
                "[Renderer] Indirect execution not supported — using singular draw_indexed_indirect fallback"
            );
        }

        // Only enable GPU timing when POSTRETRO_GPU_TIMING=1 AND the adapter
        // supports TIMESTAMP_QUERY. FrameTiming=None → zero runtime cost.
        let adapter_features = adapter.features();
        let gpu_timing_requested =
            std::env::var("POSTRETRO_GPU_TIMING").ok().as_deref() == Some("1");
        let gpu_timing_supported = adapter_features.contains(wgpu::Features::TIMESTAMP_QUERY);
        let enable_gpu_timing = gpu_timing_requested && gpu_timing_supported;
        let mut required_features = wgpu::Features::empty();
        if enable_gpu_timing {
            required_features |= wgpu::Features::TIMESTAMP_QUERY;
        } else if gpu_timing_requested && !gpu_timing_supported {
            log::warn!(
                "[Renderer] POSTRETRO_GPU_TIMING=1 requested but adapter \
                 lacks TIMESTAMP_QUERY support — running without GPU timing"
            );
        }

        // WebGPU downlevel default is 4 bind groups; the forward pipeline uses
        // groups 0–5 (camera, material, lights, SH, lightmap, shadow pool).
        // 8 is the WebGPU cap and is supported on all desktop backends.
        //
        // The forward fragment shader binds exactly 16 sampled textures
        // (3 material + 9 SH bands + 3 lightmap + 1 shadow depth) — the
        // WebGPU spec floor for max_sampled_textures_per_shader_stage. Adding
        // another sampled texture requires bumping this limit or collapsing the
        // 9 SH band textures into a texture array.
        let required_limits = wgpu::Limits {
            max_bind_groups: 8,
            // SH compose writes 9 storage textures. WebGPU floor is 4;
            // Metal/Vulkan/DX12 support ≥9.
            max_storage_textures_per_shader_stage: 9,
            ..wgpu::Limits::default()
        };

        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            label: Some("Postretro Device"),
            required_features,
            required_limits,
            ..Default::default()
        }))
        .context("failed to create GPU device")?;

        let surface_caps = surface.get_capabilities(&adapter);
        let surface_format = surface_caps
            .formats
            .iter()
            .copied()
            .find(|f| f.is_srgb())
            .unwrap_or(surface_caps.formats[0]);

        let surface_config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format: surface_format,
            width: size.width.max(1),
            height: size.height.max(1),
            present_mode: wgpu::PresentMode::AutoVsync,
            alpha_mode: surface_caps.alpha_modes[0],
            desired_maximum_frame_latency: 2,
            view_formats: vec![],
        };
        surface.configure(&device, &surface_config);
        log::info!("[Renderer] vsync on");

        let has_geometry =
            geometry.is_some_and(|g| !g.vertices.is_empty() && !g.indices.is_empty());

        let (vertex_data, index_data, index_count) = if let Some(geom) =
            geometry.filter(|g| !g.vertices.is_empty() && !g.indices.is_empty())
        {
            let count = geom.indices.len() as u32;
            (
                cast_world_vertices_to_bytes(geom.vertices),
                bytemuck_cast_slice_u32(geom.indices),
                count,
            )
        } else {
            (
                vec![0u8; crate::geometry::WorldVertex::STRIDE], // one dummy vertex
                vec![0u8; 4],                                    // one dummy index
                0u32,
            )
        };

        let vertex_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("World Vertex Buffer"),
            contents: &vertex_data,
            usage: wgpu::BufferUsages::VERTEX,
        });

        let index_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("World Index Buffer"),
            contents: &index_data,
            usage: wgpu::BufferUsages::INDEX,
        });

        // Build a line-list index buffer from the triangle index buffer for the
        // wireframe overlay. Each triangle contributes its three edges as line
        // pairs. Shared edges are duplicated (cheap, and avoids a hash set).
        let (wireframe_index_data, wireframe_index_count) = if let Some(geom) =
            geometry.filter(|g| !g.vertices.is_empty() && !g.indices.is_empty())
        {
            let line_indices = build_line_indices_from_triangles(geom.indices);
            let count = line_indices.len() as u32;
            (bytemuck_cast_slice_u32(&line_indices), count)
        } else {
            (vec![0u8; 4], 0u32)
        };

        let wireframe_index_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Wireframe Line Index Buffer"),
            contents: &wireframe_index_data,
            usage: wgpu::BufferUsages::INDEX,
        });

        let view_proj = build_default_view_projection(
            surface_config.width as f32 / surface_config.height as f32,
        );
        // Only dynamic lights go into the GPU real-time loop. Static lights are
        // baked into the lightmap; putting them in the loop would double-apply
        // their direct contribution on top of the bake.
        let (level_lights, dynamic_influences) = filter_dynamic_lights(
            geometry.map(|g| g.lights).unwrap_or(&[]),
            geometry.map(|g| g.light_influences).unwrap_or(&[]),
        );
        let light_count = level_lights.len() as u32;
        let ambient_floor = DEFAULT_AMBIENT_FLOOR;
        let uniform_data = build_uniform_data(&FrameUniforms {
            view_proj,
            camera_position: Vec3::ZERO,
            ambient_floor,
            light_count,
            time: 0.0,
            lighting_isolation: LightingIsolation::Normal,
            indirect_scale: DEFAULT_INDIRECT_SCALE,
        });

        let uniform_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Uniform Buffer"),
            contents: &uniform_data,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });

        let uniform_bind_group_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("Uniform Bind Group Layout"),
                entries: &[wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    // COMPUTE visibility added so the animated-lightmap
                    // compose pass can reuse this layout (same uniform
                    // buffer, `uniforms.time` drives curve sampling).
                    // CONTRACT: `render::animated_lightmap::AnimatedLightmapResources::new`
                    // relies on COMPUTE being set here — dropping it will
                    // fail wgpu validation at compute pipeline creation.
                    // See the doc-comment on that function for detail.
                    visibility: wgpu::ShaderStages::VERTEX
                        | wgpu::ShaderStages::FRAGMENT
                        | wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                }],
            });

        let uniform_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Uniform Bind Group"),
            layout: &uniform_bind_group_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: uniform_buffer.as_entire_binding(),
            }],
        });

        // Group 1: per-material
        //   0 = diffuse (sRGB), 1 = sampler, 2 = specular (R8), 3 = shininess uniform
        //   4 = normal map (Rgba8Unorm, NOT sRGB; decode: n = sample.rgb * 2.0 - 1.0)
        let texture_bind_group_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("Material Bind Group Layout"),
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
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Texture {
                            sample_type: wgpu::TextureSampleType::Float { filterable: true },
                            view_dimension: wgpu::TextureViewDimension::D2,
                            multisampled: false,
                        },
                        count: None,
                    },
                    wgpu::BindGroupLayoutEntry {
                        binding: 3,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Buffer {
                            ty: wgpu::BufferBindingType::Uniform,
                            has_dynamic_offset: false,
                            min_binding_size: None,
                        },
                        count: None,
                    },
                    wgpu::BindGroupLayoutEntry {
                        binding: 4,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Texture {
                            sample_type: wgpu::TextureSampleType::Float { filterable: true },
                            view_dimension: wgpu::TextureViewDimension::D2,
                            multisampled: false,
                        },
                        count: None,
                    },
                ],
            });

        // Group 2: direct-light + specular/chunk buffers
        //   0 = dynamic GpuLight array, 1 = influence volumes
        //   2 = spec-only static lights, 3 = ChunkGridInfo (has_chunk_grid=0 → full scan)
        //   4 = per-chunk offset table, 5 = flat chunk index list
        let storage_entry = |binding: u32| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Storage { read_only: true },
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            count: None,
        };
        let lighting_bind_group_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("Lighting Bind Group Layout"),
                entries: &[
                    storage_entry(0),
                    storage_entry(1),
                    storage_entry(2),
                    wgpu::BindGroupLayoutEntry {
                        binding: 3,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Buffer {
                            ty: wgpu::BufferBindingType::Uniform,
                            has_dynamic_offset: false,
                            min_binding_size: None,
                        },
                        count: None,
                    },
                    storage_entry(4),
                    storage_entry(5),
                ],
            });

        for (idx, light) in level_lights.iter().enumerate() {
            if light.is_dynamic && light.light_type == crate::prl::LightType::Directional {
                log::warn!(
                    "[Renderer] Dynamic directional light (light_sun) at index {} found — not supported. \
                     Will render unshadowed (diffuse + specular only).",
                    idx
                );
            }
        }

        // wgpu rejects zero-size storage buffers; pad to one dummy record when empty.
        // `light_count` stays at 0 so the dummy is never read by the shader.
        let lights_data = if !level_lights.is_empty() {
            pack_lights(&level_lights)
        } else {
            vec![0u8; GPU_LIGHT_SIZE]
        };
        let lights_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Direct Lights Storage Buffer"),
            contents: &lights_data,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
        });

        // BGL owned here so the forward pipeline layout and pool bind group share a definition.
        let spot_shadow_bgl = SpotShadowPool::bind_group_layout(&device);
        let spot_shadow_pool = SpotShadowPool::new(&device, &spot_shadow_bgl);
        log::info!(
            "[Renderer] Spot shadow pool initialized (8 × 1024×1024 Depth32Float = 32 MiB VRAM)"
        );

        // Influence volume buffer (binding 1). Same dummy strategy as lights.
        let influence_data = if !dynamic_influences.is_empty() {
            influence::pack_influence(&dynamic_influences)
        } else {
            vec![0u8; 16]
        };
        let influence_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Light Influence Storage Buffer"),
            contents: &influence_data,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
        });

        // Static-only light buffer for specular; dynamic lights excluded by pack_spec_lights.
        // 1-record dummy when empty (avoids zero-size storage binding).
        let spec_lights_data = {
            let packed = geometry
                .map(|g| pack_spec_lights(g.lights))
                .unwrap_or_default();
            if packed.is_empty() {
                vec![0u8; SPEC_LIGHT_SIZE]
            } else {
                packed
            }
        };
        let spec_lights_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Spec-Only Lights Storage Buffer"),
            contents: &spec_lights_data,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
        });

        // Chunk grid (group 2 bindings 3, 4, 5). Uses the PRL section when
        // present; otherwise binds a fallback payload with `has_chunk_grid = 0`
        // so the shader iterates the full spec buffer.
        let chunk_grid = match geometry.and_then(|g| g.chunk_light_list) {
            Some(sec) => ChunkGrid::from_section(sec),
            None => ChunkGrid::fallback(),
        };
        if chunk_grid.present {
            log::info!(
                "[Renderer] ChunkLightList active (spec-only path is spatially partitioned)"
            );
        } else {
            log::info!(
                "[Renderer] ChunkLightList absent — specular path iterates the full spec buffer"
            );
        }
        let chunk_grid_info_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Chunk Grid Info Uniform"),
            contents: &chunk_grid.grid_info,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });
        let chunk_grid_offsets_buffer =
            device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("Chunk Grid Offset Table"),
                contents: &chunk_grid.offset_table,
                usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            });
        let chunk_grid_indices_buffer =
            device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("Chunk Grid Index List"),
                contents: &chunk_grid.index_list,
                usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            });

        let lighting_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Lighting Bind Group"),
            layout: &lighting_bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: lights_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: influence_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: spec_lights_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: chunk_grid_info_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 4,
                    resource: chunk_grid_offsets_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 5,
                    resource: chunk_grid_indices_buffer.as_entire_binding(),
                },
            ],
        });

        // Create shared sampler: nearest filtering for retro pixel aesthetic, repeat.
        let base_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("Base Texture Sampler"),
            address_mode_u: wgpu::AddressMode::Repeat,
            address_mode_v: wgpu::AddressMode::Repeat,
            address_mode_w: wgpu::AddressMode::Repeat,
            mag_filter: wgpu::FilterMode::Nearest,
            min_filter: wgpu::FilterMode::Nearest,
            mipmap_filter: wgpu::MipmapFilterMode::Nearest,
            ..Default::default()
        });

        // Absent specular → zeros in R channel → no highlight, no shader branch.
        let black_specular_texture = upload_texture_data(
            &device,
            &queue,
            1,
            1,
            &[0u8],
            wgpu::TextureFormat::R8Unorm,
            "Specular Black 1x1",
        );
        let black_specular_view =
            black_specular_texture.create_view(&wgpu::TextureViewDescriptor::default());

        // Tangent-space +Z: (0,0,1) → (127,127,255) in Rgba8Unorm.
        // Shader decode: n = sample.rgb * 2.0 - 1.0 → (≈0, ≈0, ≈1).
        let neutral_normal_texture = upload_texture_data(
            &device,
            &queue,
            1,
            1,
            &[127u8, 127, 255, 255],
            wgpu::TextureFormat::Rgba8Unorm,
            "Normal Neutral 1x1",
        );
        let neutral_normal_view =
            neutral_normal_texture.create_view(&wgpu::TextureViewDescriptor::default());

        let texture_materials: &[Material] = geometry.map(|g| g.texture_materials).unwrap_or(&[]);
        let specular_set: Option<&[Option<LoadedTexture>]> =
            texture_set.map(|s| s.specular.as_slice());
        let normal_set: Option<&[Option<LoadedTexture>]> = texture_set.map(|s| s.normal.as_slice());

        let mut gpu_textures: Vec<GpuTexture> = Vec::new();
        if let Some(tex_set) = texture_set {
            for (idx, loaded) in tex_set.textures.iter().enumerate() {
                let diffuse_tex = upload_texture_data(
                    &device,
                    &queue,
                    loaded.width,
                    loaded.height,
                    &loaded.data,
                    wgpu::TextureFormat::Rgba8UnormSrgb,
                    &format!("Texture {idx} Diffuse"),
                );
                let diffuse_view = diffuse_tex.create_view(&wgpu::TextureViewDescriptor::default());

                let spec_view = match specular_set
                    .and_then(|s| s.get(idx))
                    .and_then(|o| o.as_ref())
                {
                    Some(spec_loaded) => {
                        let r_only = extract_r_channel(&spec_loaded.data);
                        let tex = upload_texture_data(
                            &device,
                            &queue,
                            spec_loaded.width,
                            spec_loaded.height,
                            &r_only,
                            wgpu::TextureFormat::R8Unorm,
                            &format!("Texture {idx} Specular"),
                        );
                        tex.create_view(&wgpu::TextureViewDescriptor::default())
                    }
                    None => black_specular_view.clone(),
                };

                // Normal-map upload: linear `Rgba8Unorm` (NOT sRGB — tangent
                // vectors must not gamma-correct). Falls back to the shared
                // neutral-normal placeholder when no `_n` sibling was present
                // or it failed validation in the loader.
                let normal_view = match normal_set.and_then(|s| s.get(idx)).and_then(|o| o.as_ref())
                {
                    Some(normal_loaded) => {
                        let tex = upload_texture_data(
                            &device,
                            &queue,
                            normal_loaded.width,
                            normal_loaded.height,
                            &normal_loaded.data,
                            wgpu::TextureFormat::Rgba8Unorm,
                            &format!("Texture {idx} Normal"),
                        );
                        tex.create_view(&wgpu::TextureViewDescriptor::default())
                    }
                    None => neutral_normal_view.clone(),
                };

                let material = texture_materials
                    .get(idx)
                    .copied()
                    .unwrap_or(Material::Default);
                let uniform_bytes = build_material_uniform(material.shininess());
                let uniform_buf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some(&format!("Material Uniform {idx}")),
                    contents: &uniform_bytes,
                    usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                });

                let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some(&format!("Material Bind Group {idx}")),
                    layout: &texture_bind_group_layout,
                    entries: &[
                        wgpu::BindGroupEntry {
                            binding: 0,
                            resource: wgpu::BindingResource::TextureView(&diffuse_view),
                        },
                        wgpu::BindGroupEntry {
                            binding: 1,
                            resource: wgpu::BindingResource::Sampler(&base_sampler),
                        },
                        wgpu::BindGroupEntry {
                            binding: 2,
                            resource: wgpu::BindingResource::TextureView(&spec_view),
                        },
                        wgpu::BindGroupEntry {
                            binding: 3,
                            resource: uniform_buf.as_entire_binding(),
                        },
                        wgpu::BindGroupEntry {
                            binding: 4,
                            resource: wgpu::BindingResource::TextureView(&normal_view),
                        },
                    ],
                });
                gpu_textures.push(GpuTexture { bind_group });
            }
        }

        if gpu_textures.is_empty() {
            let placeholder = crate::texture::generate_placeholder();
            let diffuse_tex = upload_texture_data(
                &device,
                &queue,
                placeholder.width,
                placeholder.height,
                &placeholder.data,
                wgpu::TextureFormat::Rgba8UnormSrgb,
                "Placeholder Texture Diffuse",
            );
            let diffuse_view = diffuse_tex.create_view(&wgpu::TextureViewDescriptor::default());
            let uniform_bytes = build_material_uniform(Material::Default.shininess());
            let uniform_buf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("Material Uniform Placeholder"),
                contents: &uniform_bytes,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            });
            let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("Placeholder Material Bind Group"),
                layout: &texture_bind_group_layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView(&diffuse_view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::Sampler(&base_sampler),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: wgpu::BindingResource::TextureView(&black_specular_view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 3,
                        resource: uniform_buf.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 4,
                        resource: wgpu::BindingResource::TextureView(&neutral_normal_view),
                    },
                ],
            });
            gpu_textures.push(GpuTexture { bind_group });
        }

        let bvh_leaves: Vec<crate::geometry::BvhLeaf> =
            geometry.map(|g| g.bvh.leaves.clone()).unwrap_or_default();
        let compute_cull = geometry
            .filter(|g| !g.bvh.leaves.is_empty())
            .map(|g| ComputeCullPipeline::new(&device, g.bvh, has_multi_draw_indirect));

        let (_depth_texture, depth_view) =
            create_depth_texture(&device, surface_config.width, surface_config.height);

        // Absent SH section → dummy 1×1×1 textures; shader skips via `has_sh_volume == 0`.
        let sh_volume_resources = ShVolumeResources::new(
            &device,
            &queue,
            geometry.and_then(|g| g.sh_volume),
            level_lights.len(),
        );

        let sh_compose = ShComposeResources::new(
            &device,
            &sh_volume_resources,
            geometry.and_then(|g| g.sh_volume),
            geometry.and_then(|g| g.delta_sh_volumes),
            &uniform_bind_group_layout,
        );

        // Absent weight-map section → 1×1 zero atlas; forward shader never branches on it.
        // Cross-section validation errors surface as init failures (map loads are unrecoverable).
        let animated_lm_debug = animated_lightmap::AnimatedLmDebugConfig::from_env();
        let animated_lightmap = animated_lightmap::AnimatedLightmapResources::new(
            &device,
            geometry.and_then(|g| g.animated_light_weight_maps),
            geometry.and_then(|g| g.animated_light_chunks),
            &bvh_leaves,
            &sh_volume_resources.animation,
            &uniform_bind_group_layout,
            animated_lm_debug,
        )
        .map_err(|msg| anyhow::anyhow!("[Renderer] animated lightmap init failed: {msg}"))?;

        // Group 4: directional lightmap atlas (baked static direct lighting).
        // Animated-contribution atlas bound at group 4 binding 3 — from `animated_lightmap`
        // (real texture or 1×1 zero dummy). Layout created before the bind group so the
        // pipeline layout can reference it first.
        let lightmap_bind_group_layout = crate::lighting::lightmap::bind_group_layout(&device);
        let lightmap_resources = LightmapResources::new(
            &device,
            &queue,
            geometry.and_then(|g| g.lightmap),
            &lightmap_bind_group_layout,
            &animated_lightmap.forward_view,
        );

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("Textured Pipeline Layout"),
            bind_group_layouts: &[
                Some(&uniform_bind_group_layout),
                Some(&texture_bind_group_layout),
                Some(&lighting_bind_group_layout),
                Some(&sh_volume_resources.bind_group_layout),
                Some(&lightmap_bind_group_layout),
                Some(&spot_shadow_bgl),
            ],
            immediate_size: 0,
        });

        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Textured Shader"),
            source: wgpu::ShaderSource::Wgsl(SHADER_SOURCE.into()),
        });

        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("Textured Pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                buffers: &[wgpu::VertexBufferLayout {
                    array_stride: crate::geometry::WorldVertex::STRIDE as wgpu::BufferAddress,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &[
                        // position: vec3<f32> at offset 0
                        wgpu::VertexAttribute {
                            offset: 0,
                            shader_location: 0,
                            format: wgpu::VertexFormat::Float32x3,
                        },
                        // base_uv: vec2<f32> at offset 12
                        wgpu::VertexAttribute {
                            offset: 12,
                            shader_location: 1,
                            format: wgpu::VertexFormat::Float32x2,
                        },
                        // normal_oct: u16x2 at offset 20
                        wgpu::VertexAttribute {
                            offset: 20,
                            shader_location: 2,
                            format: wgpu::VertexFormat::Uint16x2,
                        },
                        // tangent_packed: u16x2 at offset 24
                        wgpu::VertexAttribute {
                            offset: 24,
                            shader_location: 3,
                            format: wgpu::VertexFormat::Uint16x2,
                        },
                        // lightmap_uv: u16x2 at offset 28 (quantized 0..1 UV)
                        wgpu::VertexAttribute {
                            offset: 28,
                            shader_location: 4,
                            format: wgpu::VertexFormat::Uint16x2,
                        },
                    ],
                }],
                compilation_options: wgpu::PipelineCompilationOptions::default(),
            },
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                front_face: wgpu::FrontFace::Ccw,
                cull_mode: Some(wgpu::Face::Back),
                ..Default::default()
            },
            depth_stencil: Some(wgpu::DepthStencilState {
                format: DEPTH_FORMAT,
                // Pre-pass filled the buffer; Equal test → one shade per pixel.
                // Write disabled to skip redundant rewrite of pre-pass values.
                depth_write_enabled: Some(false),
                depth_compare: Some(wgpu::CompareFunction::Equal),
                stencil: wgpu::StencilState::default(),
                bias: wgpu::DepthBiasState::default(),
            }),
            multisample: wgpu::MultisampleState::default(),
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_main"),
                targets: &[Some(wgpu::ColorTargetState {
                    format: surface_format,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: wgpu::PipelineCompilationOptions::default(),
            }),
            multiview_mask: None,
            cache: None,
        });

        // Wireframe overlay: group 0 = uniforms, group 1 = cull_status storage buffer.
        // Colors are driven by per-leaf cull status from the compute shader.
        let wireframe_cull_status_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("Wireframe Cull Status BGL"),
                entries: &[wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: true },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                }],
            });
        let wireframe_pipeline_layout =
            device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("Wireframe Pipeline Layout"),
                bind_group_layouts: &[
                    Some(&uniform_bind_group_layout),
                    Some(&wireframe_cull_status_layout),
                ],
                immediate_size: 0,
            });

        let wireframe_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Wireframe Shader"),
            source: wgpu::ShaderSource::Wgsl(WIREFRAME_SHADER_SOURCE.into()),
        });

        let wireframe_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("Wireframe Pipeline"),
            layout: Some(&wireframe_pipeline_layout),
            vertex: wgpu::VertexState {
                module: &wireframe_shader,
                entry_point: Some("vs_main"),
                buffers: &[wgpu::VertexBufferLayout {
                    array_stride: crate::geometry::WorldVertex::STRIDE as wgpu::BufferAddress,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &[
                        wgpu::VertexAttribute {
                            offset: 0,
                            shader_location: 0,
                            format: wgpu::VertexFormat::Float32x3,
                        },
                        wgpu::VertexAttribute {
                            offset: 12,
                            shader_location: 1,
                            format: wgpu::VertexFormat::Float32x2,
                        },
                        wgpu::VertexAttribute {
                            offset: 20,
                            shader_location: 2,
                            format: wgpu::VertexFormat::Uint16x2,
                        },
                        wgpu::VertexAttribute {
                            offset: 24,
                            shader_location: 3,
                            format: wgpu::VertexFormat::Uint16x2,
                        },
                    ],
                }],
                compilation_options: wgpu::PipelineCompilationOptions::default(),
            },
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::LineList,
                front_face: wgpu::FrontFace::Ccw,
                cull_mode: None,
                ..Default::default()
            },
            // Always so wireframe draws on top regardless of depth; write disabled
            // since the forward pass already holds the depth buffer contents.
            depth_stencil: Some(wgpu::DepthStencilState {
                format: DEPTH_FORMAT,
                depth_write_enabled: Some(false),
                depth_compare: Some(wgpu::CompareFunction::Always),
                stencil: wgpu::StencilState::default(),
                bias: wgpu::DepthBiasState::default(),
            }),
            multisample: wgpu::MultisampleState::default(),
            fragment: Some(wgpu::FragmentState {
                module: &wireframe_shader,
                entry_point: Some("fs_main"),
                targets: &[Some(wgpu::ColorTargetState {
                    format: surface_format,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: wgpu::PipelineCompilationOptions::default(),
            }),
            multiview_mask: None,
            cache: None,
        });

        // Depth pre-pass: group 0 only, fragment: None (wgpu allows depth-only pipelines).
        let depth_prepass_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("Depth Pre-Pass Pipeline Layout"),
            bind_group_layouts: &[Some(&uniform_bind_group_layout)],
            immediate_size: 0,
        });

        let depth_prepass_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Depth Pre-Pass Shader"),
            source: wgpu::ShaderSource::Wgsl(DEPTH_PREPASS_SHADER_SOURCE.into()),
        });

        let depth_prepass_pipeline =
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("Depth Pre-Pass Pipeline"),
                layout: Some(&depth_prepass_layout),
                vertex: wgpu::VertexState {
                    module: &depth_prepass_shader,
                    entry_point: Some("vs_main"),
                    buffers: &[wgpu::VertexBufferLayout {
                        array_stride: crate::geometry::WorldVertex::STRIDE as wgpu::BufferAddress,
                        step_mode: wgpu::VertexStepMode::Vertex,
                        attributes: &[
                            // Mirrors the forward pipeline's vertex layout so
                            // we can share the same vertex buffer binding.
                            // Only position is used; the remaining attributes
                            // are declared to match the shared layout.
                            wgpu::VertexAttribute {
                                offset: 0,
                                shader_location: 0,
                                format: wgpu::VertexFormat::Float32x3,
                            },
                            wgpu::VertexAttribute {
                                offset: 12,
                                shader_location: 1,
                                format: wgpu::VertexFormat::Float32x2,
                            },
                            wgpu::VertexAttribute {
                                offset: 20,
                                shader_location: 2,
                                format: wgpu::VertexFormat::Uint16x2,
                            },
                            wgpu::VertexAttribute {
                                offset: 24,
                                shader_location: 3,
                                format: wgpu::VertexFormat::Uint16x2,
                            },
                        ],
                    }],
                    compilation_options: wgpu::PipelineCompilationOptions::default(),
                },
                primitive: wgpu::PrimitiveState {
                    topology: wgpu::PrimitiveTopology::TriangleList,
                    front_face: wgpu::FrontFace::Ccw,
                    cull_mode: Some(wgpu::Face::Back),
                    ..Default::default()
                },
                depth_stencil: Some(wgpu::DepthStencilState {
                    format: DEPTH_FORMAT,
                    depth_write_enabled: Some(true),
                    depth_compare: Some(wgpu::CompareFunction::Less),
                    stencil: wgpu::StencilState::default(),
                    bias: wgpu::DepthBiasState::default(),
                }),
                multisample: wgpu::MultisampleState::default(),
                fragment: None,
                multiview_mask: None,
                cache: None,
            });

        // Spot shadow depth pipeline: shared across all 8 slots; slot selected via
        // dynamic offset into shadow_vs_uniform_buffer. Depth bias (constant=2, slope=1.5)
        // suppresses self-shadow acne without Peter-Panning (back-face cull, not front-face).
        let shadow_vs_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Shadow VS BGL"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: true,
                    min_binding_size: std::num::NonZeroU64::new(64),
                },
                count: None,
            }],
        });
        let shadow_pipeline_layout =
            device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("Spot Shadow Pipeline Layout"),
                bind_group_layouts: &[Some(&shadow_vs_bgl)],
                immediate_size: 0,
            });
        let shadow_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Spot Shadow Shader"),
            source: wgpu::ShaderSource::Wgsl(SPOT_SHADOW_SHADER_SOURCE.into()),
        });
        let shadow_depth_pipeline =
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("Spot Shadow Depth Pipeline"),
                layout: Some(&shadow_pipeline_layout),
                vertex: wgpu::VertexState {
                    module: &shadow_shader,
                    entry_point: Some("vs_main"),
                    buffers: &[wgpu::VertexBufferLayout {
                        array_stride: crate::geometry::WorldVertex::STRIDE as wgpu::BufferAddress,
                        step_mode: wgpu::VertexStepMode::Vertex,
                        attributes: &[wgpu::VertexAttribute {
                            offset: 0,
                            shader_location: 0,
                            format: wgpu::VertexFormat::Float32x3,
                        }],
                    }],
                    compilation_options: wgpu::PipelineCompilationOptions::default(),
                },
                primitive: wgpu::PrimitiveState {
                    topology: wgpu::PrimitiveTopology::TriangleList,
                    front_face: wgpu::FrontFace::Ccw,
                    cull_mode: Some(wgpu::Face::Back),
                    ..Default::default()
                },
                depth_stencil: Some(wgpu::DepthStencilState {
                    format: crate::lighting::spot_shadow::SHADOW_DEPTH_FORMAT,
                    depth_write_enabled: Some(true),
                    depth_compare: Some(wgpu::CompareFunction::Less),
                    stencil: wgpu::StencilState::default(),
                    bias: wgpu::DepthBiasState {
                        constant: 2,
                        slope_scale: 1.5,
                        clamp: 0.0,
                    },
                }),
                multisample: wgpu::MultisampleState::default(),
                fragment: None,
                multiview_mask: None,
                cache: None,
            });

        // Align each mat4 slot to min_uniform_buffer_offset_alignment for legal dynamic offsets.
        let min_ubo_align = device.limits().min_uniform_buffer_offset_alignment.max(64);
        let shadow_vs_stride = min_ubo_align;
        let shadow_vs_uniform_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Spot Shadow VS Uniforms"),
            size: (shadow_vs_stride as u64)
                * (crate::lighting::spot_shadow::SHADOW_POOL_SIZE as u64),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let shadow_vs_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Spot Shadow VS Bind Group"),
            layout: &shadow_vs_bgl,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                    buffer: &shadow_vs_uniform_buffer,
                    offset: 0,
                    size: std::num::NonZeroU64::new(64),
                }),
            }],
        });

        let frame_timing = if enable_gpu_timing {
            log::info!("[Renderer] GPU timing enabled (POSTRETRO_GPU_TIMING=1)");
            let mut pass_labels = vec![""; TIMING_PAIR_COUNT];
            pass_labels[TIMING_PAIR_CULL] = "cull";
            pass_labels[TIMING_PAIR_ANIMATED_LM_COMPOSE] = "animated_lm_compose";
            pass_labels[TIMING_PAIR_DEPTH_PREPASS] = "depth_prepass";
            pass_labels[TIMING_PAIR_FORWARD] = "forward";
            Some(FrameTiming::new(&device, &queue, pass_labels))
        } else {
            None
        };

        // Billboard sprite pipeline. See: context/lib/rendering_pipeline.md §7.4
        let smoke_pass = SmokePass::new(
            &device,
            surface_format,
            DEPTH_FORMAT,
            &uniform_bind_group_layout,
            &lighting_bind_group_layout,
            &sh_volume_resources.bind_group_layout,
        );

        // Volumetric fog pass. Pixel scale is the worldspawn default until the
        // app pushes a per-level value via `set_fog_pixel_scale`.
        let mut fog = FogPass::new(
            &device,
            surface_config.width,
            surface_config.height,
            crate::fx::fog_volume::clamp_fog_pixel_scale(0),
            &depth_view,
            &uniform_bind_group_layout,
            &sh_volume_resources.bind_group_layout,
            &spot_shadow_bgl,
        );
        // Match the actual surface format — `FogPass::new` hardcodes
        // `Rgba8UnormSrgb`, but the swapchain may have picked a different
        // sRGB or non-sRGB variant.
        fog.rebuild_composite_for_format(&device, surface_format);

        if has_geometry {
            log::info!(
                "[Renderer] Textured pipeline ready: {} indices, {} textures, bvh_leaves={}",
                index_count,
                gpu_textures.len(),
                bvh_leaves.len(),
            );
            log::info!(
                "[Renderer] Wireframe overlay pipeline ready: {} line indices",
                wireframe_index_count,
            );
        } else {
            log::info!("[Renderer] Pipeline ready (no geometry loaded)");
        }

        Ok(Self {
            device,
            queue,
            surface,
            surface_config,
            is_surface_configured: true,
            pipeline,
            depth_prepass_pipeline,
            frame_timing,
            vertex_buffer,
            index_buffer,
            index_count,
            uniform_buffer,
            uniform_bind_group,
            lighting_bind_group,
            light_count,
            ambient_floor,
            indirect_scale: DEFAULT_INDIRECT_SCALE,
            sh_volume_resources,
            sh_compose,
            lightmap_resources,
            animated_lightmap,
            lights_buffer,
            level_lights,
            light_effective_brightness: Vec::new(),
            last_camera_position: Vec3::ZERO,
            spot_shadow_pool,
            shadow_vs_uniform_buffer,
            shadow_vs_bind_group,
            shadow_depth_pipeline,
            shadow_vs_stride,
            depth_view,
            gpu_textures,
            bvh_leaves,
            compute_cull,
            wireframe_pipeline,
            wireframe_index_buffer,
            wireframe_index_count,
            wireframe_cull_status_bgl: wireframe_cull_status_layout,
            wireframe_enabled: false,
            lighting_isolation: LightingIsolation::Normal,
            vsync_enabled: true,
            has_geometry,
            debug_frame: 0,
            debug_prev_bitmask: (u32::MAX, u32::MAX),
            debug_prev_vp_hash: u32::MAX,
            debug_prev_visible: ("init", usize::MAX),
            app_start: Instant::now(),
            smoke_pass,
            fog,
        })
    }

    /// When multiple emitters share a collection, the first caller's `spec_intensity`
    /// and `lifetime` win — these are per-collection, not per-emitter.
    pub fn register_smoke_collection(
        &mut self,
        collection: &str,
        frames: &[SpriteFrame],
        spec_intensity: f32,
        lifetime: f32,
    ) {
        self.smoke_pass.register_collection(
            &self.device,
            &self.queue,
            collection,
            frames,
            spec_intensity,
            lifetime,
        );
    }

    pub fn toggle_wireframe(&mut self) -> bool {
        self.wireframe_enabled = !self.wireframe_enabled;
        log::info!(
            "[Renderer] Wireframe overlay: {}",
            if self.wireframe_enabled { "on" } else { "off" },
        );
        self.wireframe_enabled
    }

    /// Takes effect on the next `update_per_frame_uniforms` upload.
    pub fn cycle_lighting_isolation(&mut self) -> LightingIsolation {
        self.lighting_isolation = self.lighting_isolation.cycle();
        log::info!(
            "[Renderer] Lighting isolation: {}",
            self.lighting_isolation.label(),
        );
        self.lighting_isolation
    }

    /// Alt+Shift+V diagnostic chord. Rebuilds the swapchain via surface.configure.
    pub fn toggle_vsync(&mut self) -> bool {
        self.vsync_enabled = !self.vsync_enabled;
        self.surface_config.present_mode = if self.vsync_enabled {
            wgpu::PresentMode::AutoVsync
        } else {
            wgpu::PresentMode::AutoNoVsync
        };
        self.surface.configure(&self.device, &self.surface_config);
        self.vsync_enabled
    }

    pub fn vsync_enabled(&self) -> bool {
        self.vsync_enabled
    }

    /// Caller must update view-projection via `update_per_frame_uniforms` — the camera
    /// owns aspect ratio, not the renderer.
    pub fn resize(&mut self, width: u32, height: u32) {
        if width == 0 || height == 0 {
            return;
        }
        self.surface_config.width = width;
        self.surface_config.height = height;
        self.surface.configure(&self.device, &self.surface_config);
        let (_depth_texture, depth_view) = create_depth_texture(&self.device, width, height);
        self.depth_view = depth_view;
        self.fog
            .resize(&self.device, width, height, &self.depth_view);
        self.is_surface_configured = true;
    }

    pub fn update_per_frame_uniforms(&mut self, view_proj: Mat4, camera_position: Vec3) {
        let time = self.app_start.elapsed().as_secs_f32();
        let data = build_uniform_data(&FrameUniforms {
            view_proj,
            camera_position,
            ambient_floor: self.ambient_floor,
            light_count: self.light_count,
            time,
            lighting_isolation: self.lighting_isolation,
            indirect_scale: self.indirect_scale,
        });
        self.queue.write_buffer(&self.uniform_buffer, 0, &data);
        self.last_camera_position = camera_position;

        // Must upload before the compose pass and SH fragment pass — both read
        // the descriptor buffer. set_active is called during Game logic (before Render).
        self.sh_volume_resources
            .animation
            .upload_descriptors_if_dirty(&self.queue);
    }

    /// Flushed to GPU on the next `update_per_frame_uniforms` call.
    #[allow(dead_code)]
    pub fn set_animated_light_active(&mut self, slot: usize, active: bool) {
        self.sh_volume_resources.animation.set_active(slot, active);
    }

    /// Must run **before** `update_dynamic_light_slots` — slot assignment reads
    /// the bridge-produced colors/intensities and then rewrites the same buffer.
    pub fn upload_bridge_lights(&mut self, lights_bytes: &[u8]) {
        debug_assert_eq!(
            lights_bytes.len(),
            self.level_lights.len() * GPU_LIGHT_SIZE,
            "bridge produced {} bytes; expected {} × {} = {}",
            lights_bytes.len(),
            self.level_lights.len(),
            GPU_LIGHT_SIZE,
            self.level_lights.len() * GPU_LIGHT_SIZE,
        );
        if lights_bytes.is_empty() {
            return;
        }
        self.queue
            .write_buffer(&self.lights_buffer, 0, lights_bytes);
    }

    /// Mismatched length logs a warning and skips the upload (fail soft) rather
    /// than crashing the frame if the bridge invariant ever slips.
    pub fn upload_bridge_descriptors(&mut self, descriptor_bytes: &[u8]) {
        let expected = self.level_lights.len() * sh_volume::ANIMATION_DESCRIPTOR_SIZE;
        if descriptor_bytes.len() != expected {
            log::warn!(
                "[Renderer] upload_bridge_descriptors: bridge produced {} bytes; \
                 expected {} × {} = {}. Skipping upload.",
                descriptor_bytes.len(),
                self.level_lights.len(),
                sh_volume::ANIMATION_DESCRIPTOR_SIZE,
                expected,
            );
            return;
        }
        if descriptor_bytes.is_empty() {
            return;
        }
        self.queue.write_buffer(
            &self.sh_volume_resources.scripted_light_descriptors,
            0,
            descriptor_bytes,
        );
    }

    /// Writes into `anim_samples` at the scripted-region offset (after FGD samples).
    pub fn upload_bridge_samples(&mut self, samples_bytes: &[u8]) {
        if samples_bytes.is_empty() {
            return;
        }
        let offset = self.sh_volume_resources.scripted_sample_byte_offset as u64;
        self.queue.write_buffer(
            &self.sh_volume_resources.animation.anim_samples,
            offset,
            samples_bytes,
        );
    }

    /// Divide by 4 for the float index; pass as `fgd_sample_float_count` to `LightBridge`.
    pub fn scripted_sample_byte_offset(&self) -> usize {
        self.sh_volume_resources.scripted_sample_byte_offset
    }

    pub fn level_lights(&self) -> &[MapLight] {
        &self.level_lights
    }

    /// Build the per-frame `FogSpotLight` list from the dynamic spot lights
    /// that received a shadow slot this frame. The raymarch shader uses
    /// `slot` to index `light_space_matrices.m[slot]` for shadow comparison,
    /// so unslotted spots are excluded — they have no usable light-space
    /// matrix in the shader's view.
    ///
    /// Pre-multiplies `color × intensity × effective_brightness` so the GPU
    /// path is purely additive; mirrors the fog point-light packing in
    /// `FogVolumeBridge::update_points`.
    fn collect_fog_spot_lights(&self) -> Vec<crate::fx::fog_volume::FogSpotLight> {
        const BRIGHTNESS_SUPPRESSION_THRESHOLD: f32 = 0.01;
        let slot_assignment = &self.spot_shadow_pool.slot_assignment;
        if slot_assignment.is_empty() {
            return Vec::new();
        }
        let mut out = Vec::new();
        for (light_idx, &slot) in slot_assignment.iter().enumerate() {
            if slot == crate::lighting::spot_shadow::NO_SHADOW_SLOT {
                continue;
            }
            let Some(light) = self.level_lights.get(light_idx) else {
                continue;
            };
            if !matches!(light.light_type, crate::prl::LightType::Spot) {
                continue;
            }
            let multiplier = self
                .light_effective_brightness
                .get(light_idx)
                .copied()
                .unwrap_or(1.0);
            if multiplier < BRIGHTNESS_SUPPRESSION_THRESHOLD {
                continue;
            }
            let intensity = light.intensity * multiplier;
            out.push(crate::fx::fog_volume::FogSpotLight {
                position: [
                    light.origin[0] as f32,
                    light.origin[1] as f32,
                    light.origin[2] as f32,
                ],
                slot,
                direction: light.cone_direction,
                cos_outer: light.cone_angle_outer.cos(),
                color: [
                    light.color[0] * intensity,
                    light.color[1] * intensity,
                    light.color[2] * intensity,
                ],
                range: light.falloff_range,
            });
        }
        out
    }

    /// Upload the per-frame fog-volume buffer. Bytes must be a tightly packed
    /// `[FogVolume]` array; the renderer never sees the script-side type.
    /// Empty input clears the volume count, which `FogPass::active` reads to
    /// skip the compute + composite passes for the rest of the frame.
    pub fn upload_fog_volumes(&mut self, bytes: &[u8]) {
        let stride = std::mem::size_of::<crate::fx::fog_volume::FogVolume>();
        if bytes.is_empty() {
            self.fog.volume_count = 0;
            return;
        }
        if bytes.len() % stride != 0 {
            log::warn!(
                "[Renderer] upload_fog_volumes: byte length {} is not a multiple of \
                 FogVolume stride {}; skipping.",
                bytes.len(),
                stride,
            );
            return;
        }
        // `FogPass::upload_volumes` takes `&[FogVolume]`; recover the typed
        // slice via `bytemuck::cast_slice` so the GPU upload path reuses the
        // existing capped/labelled writer.
        let volumes: &[crate::fx::fog_volume::FogVolume] = bytemuck::cast_slice(bytes);
        self.fog.upload_volumes(&self.queue, volumes);
    }

    /// Upload the per-frame fog point-light buffer. Bytes must be a tightly
    /// packed `[FogPointLight]` array. Empty input is a no-op (the buffer keeps
    /// its previous contents but `volume_count` gates whether the pass runs).
    pub fn upload_fog_points(&mut self, bytes: &[u8]) {
        let stride = std::mem::size_of::<crate::fx::fog_volume::FogPointLight>();
        if bytes.is_empty() {
            return;
        }
        if bytes.len() % stride != 0 {
            log::warn!(
                "[Renderer] upload_fog_points: byte length {} is not a multiple of \
                 FogPointLight stride {}; skipping.",
                bytes.len(),
                stride,
            );
            return;
        }
        let points: &[crate::fx::fog_volume::FogPointLight] = bytemuck::cast_slice(bytes);
        self.fog.upload_points(&self.queue, points);
    }

    /// Set the global `fog_pixel_scale` from worldspawn. No-op when unchanged.
    pub fn set_fog_pixel_scale(&mut self, scale: u32) {
        self.fog.set_pixel_scale(
            &self.device,
            scale,
            self.surface_config.width,
            self.surface_config.height,
            &self.depth_view,
        );
    }

    /// Empty slice = no suppression (all lights eligible for shadow slots).
    pub fn set_light_effective_brightness(&mut self, effective_brightness: &[f32]) {
        self.light_effective_brightness.clear();
        self.light_effective_brightness
            .extend_from_slice(effective_brightness);
    }

    /// Lights with effective brightness below 0.01 are excluded from slot ranking
    /// so an animated-dark light doesn't waste one of the 8 shadow slots.
    /// Empty/short `effective_brightness` = all-1.0 (first frame runs before bridge).
    pub fn update_dynamic_light_slots(
        &mut self,
        camera_position: Vec3,
        camera_near_clip: f32,
        light_influences: &[LightInfluence],
        effective_brightness: &[f32],
        visible_leaf_mask: &[bool],
    ) {
        if self.level_lights.is_empty() {
            return;
        }

        // Empty visible_leaf_mask = DrawAll sentinel; ALPHA_LIGHT_LEAF_UNASSIGNED =
        // compiler couldn't assign the light to a non-solid leaf → always cull.
        const BRIGHTNESS_SUPPRESSION_THRESHOLD: f32 = 0.01;
        let mut visible_lights = vec![false; self.level_lights.len()];
        for (i, light) in self.level_lights.iter().enumerate() {
            let leaf_visible = if light.leaf_index == ALPHA_LIGHT_LEAF_UNASSIGNED {
                false
            } else if visible_leaf_mask.is_empty() {
                true
            } else {
                let li = light.leaf_index as usize;
                li < visible_leaf_mask.len() && visible_leaf_mask[li]
            };
            if !leaf_visible {
                continue;
            }
            let b = effective_brightness.get(i).copied().unwrap_or(1.0);
            if b < BRIGHTNESS_SUPPRESSION_THRESHOLD {
                continue;
            }
            visible_lights[i] = true;
        }

        let slot_assignment = SpotShadowPool::rank_lights(
            &self.level_lights,
            camera_position,
            camera_near_clip,
            &visible_lights,
            light_influences,
        );

        let lights_data = pack_lights_with_slots(&self.level_lights, &slot_assignment);
        self.queue
            .write_buffer(&self.lights_buffer, 0, &lights_data);

        // Upload each slot's light-space matrix to both the fragment-side storage buffer
        // (group 5 binding 2) and the vertex-side dynamic-offset uniform buffer.
        const MAT_BYTES: usize = 64;
        let stride = self.shadow_vs_stride as usize;
        let mut fragment_matrices =
            vec![0u8; MAT_BYTES * crate::lighting::spot_shadow::SHADOW_POOL_SIZE];
        let mut vertex_uniforms =
            vec![0u8; stride * crate::lighting::spot_shadow::SHADOW_POOL_SIZE];
        for (light_idx, &slot) in slot_assignment.iter().enumerate() {
            if slot == crate::lighting::spot_shadow::NO_SHADOW_SLOT {
                continue;
            }
            let m = crate::lighting::spot_shadow::light_space_matrix(&self.level_lights[light_idx]);
            let cols = m.to_cols_array();
            let mut bytes = [0u8; MAT_BYTES];
            for (i, v) in cols.iter().enumerate() {
                bytes[i * 4..i * 4 + 4].copy_from_slice(&v.to_ne_bytes());
            }
            let slot_usize = slot as usize;
            fragment_matrices[slot_usize * MAT_BYTES..(slot_usize + 1) * MAT_BYTES]
                .copy_from_slice(&bytes);
            vertex_uniforms[slot_usize * stride..slot_usize * stride + MAT_BYTES]
                .copy_from_slice(&bytes);
        }
        self.queue.write_buffer(
            &self.spot_shadow_pool.matrices_buffer,
            0,
            &fragment_matrices,
        );
        self.queue
            .write_buffer(&self.shadow_vs_uniform_buffer, 0, &vertex_uniforms);

        self.spot_shadow_pool.slot_assignment = slot_assignment;
    }

    pub fn ambient_floor(&self) -> f32 {
        self.ambient_floor
    }

    pub fn set_ambient_floor(&mut self, value: f32) {
        self.ambient_floor = value.clamp(0.0, 1.0);
    }

    pub fn indirect_scale(&self) -> f32 {
        self.indirect_scale
    }

    /// Takes effect on the next `update_per_frame_uniforms` upload.
    pub fn set_indirect_scale(&mut self, value: f32) {
        self.indirect_scale = value.max(0.0);
    }

    pub fn is_ready(&self) -> bool {
        self.is_surface_configured
    }

    #[allow(dead_code)]
    pub fn has_compute_cull(&self) -> bool {
        self.compute_cull.is_some()
    }

    /// Compute shader writes one `DrawIndexedIndirect` per surviving BVH leaf;
    /// render pass consumes them via multi_draw_indexed_indirect (or singular fallback).
    pub fn render_frame_indirect(
        &mut self,
        visible: &VisibleCells,
        visible_leaf_mask: &[bool],
        view_proj: Mat4,
        particle_collections: &[(&str, &[u8])],
    ) -> Result<()> {
        self.debug_frame = self.debug_frame.wrapping_add(1);
        let output = match self.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(tex) => tex,
            wgpu::CurrentSurfaceTexture::Suboptimal(tex) => {
                self.surface.configure(&self.device, &self.surface_config);
                tex
            }
            wgpu::CurrentSurfaceTexture::Timeout | wgpu::CurrentSurfaceTexture::Occluded => {
                return Ok(());
            }
            wgpu::CurrentSurfaceTexture::Outdated => {
                self.surface.configure(&self.device, &self.surface_config);
                return Ok(());
            }
            wgpu::CurrentSurfaceTexture::Lost => {
                anyhow::bail!("surface lost");
            }
            wgpu::CurrentSurfaceTexture::Validation => {
                anyhow::bail!("surface validation error");
            }
        };

        let view = output
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());

        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("Frame Encoder"),
            });

        // Writes DrawIndexedIndirect commands in the same submission as the render passes —
        // no readback or GPU sync needed between cull and draw.
        if let Some(cull) = &mut self.compute_cull {
            let cull_ts = self
                .frame_timing
                .as_ref()
                .map(|t| t.compute_pass_writes(TIMING_PAIR_CULL));
            cull.dispatch(
                &self.device,
                &self.queue,
                &mut encoder,
                visible,
                &view_proj,
                cull_ts,
            );

            if log::log_enabled!(log::Level::Debug) {
                let f = self.debug_frame;

                let bm = cull.debug_bitmask_fingerprint();
                if bm != self.debug_prev_bitmask {
                    log::debug!(
                        "[cull f={f}] visible-cell bitmask changed: pop={} hash={:#010x} (was pop={} hash={:#010x})",
                        bm.0,
                        bm.1,
                        self.debug_prev_bitmask.0,
                        self.debug_prev_bitmask.1,
                    );
                    self.debug_prev_bitmask = bm;
                }

                let mut vp_hash = 0u32;
                for i in 0..4 {
                    let col = view_proj.col(i);
                    vp_hash ^= col.x.to_bits();
                    vp_hash ^= col.y.to_bits().rotate_left(7);
                    vp_hash ^= col.z.to_bits().rotate_left(13);
                    vp_hash ^= col.w.to_bits().rotate_left(19);
                }
                if vp_hash != self.debug_prev_vp_hash {
                    log::debug!("[cull f={f}] view_proj changed: hash={:#010x}", vp_hash);
                    self.debug_prev_vp_hash = vp_hash;
                }

                let cur_vis = match visible {
                    VisibleCells::Culled(cells) => ("Culled", cells.len()),
                    VisibleCells::DrawAll => ("DrawAll", 0),
                };
                if cur_vis != self.debug_prev_visible {
                    log::debug!(
                        "[cull f={f}] VisibleCells changed: {}(n={}) (was {}(n={}))",
                        cur_vis.0,
                        cur_vis.1,
                        self.debug_prev_visible.0,
                        self.debug_prev_visible.1,
                    );
                    self.debug_prev_visible = cur_vis;
                }
            }
        }

        // Must run before the depth pre-pass so the storage→sampled barrier resolves
        // before any forward fragment samples the atlas (wgpu infers the transition).
        if self.animated_lightmap.is_active() {
            let animated_ts = self
                .frame_timing
                .as_ref()
                .map(|t| t.compute_pass_writes(TIMING_PAIR_ANIMATED_LM_COMPOSE));
            self.animated_lightmap.dispatch(
                &self.queue,
                &mut encoder,
                &self.uniform_bind_group,
                visible,
                animated_ts,
            );
        }

        // Encoded before depth pre-pass so the storage-write → sampled-read barrier
        // resolves before any forward fragment samples SH.
        self.sh_compose
            .dispatch(&mut encoder, &self.uniform_bind_group);

        // mem::take avoids a simultaneous borrow of self; put it back after the call
        // so the next frame reuses the same allocation.
        let eff_brightness = std::mem::take(&mut self.light_effective_brightness);
        self.update_dynamic_light_slots(
            self.last_camera_position,
            crate::lighting::spot_shadow::SHADOW_NEAR_CLIP,
            &[],
            &eff_brightness,
            visible_leaf_mask,
        );
        self.light_effective_brightness = eff_brightness;
        if self.has_geometry && self.index_count > 0 {
            let stride = self.shadow_vs_stride;
            let slot_assignment = self.spot_shadow_pool.slot_assignment.clone();
            let mut used_slots: Vec<u32> = slot_assignment
                .iter()
                .copied()
                .filter(|&s| s != crate::lighting::spot_shadow::NO_SHADOW_SLOT)
                .collect();
            used_slots.sort_unstable();
            used_slots.dedup();
            for slot in used_slots {
                let view = &self.spot_shadow_pool.views[slot as usize];
                let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("Spot Shadow Depth Pass"),
                    color_attachments: &[],
                    depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                        view,
                        depth_ops: Some(wgpu::Operations {
                            load: wgpu::LoadOp::Clear(1.0),
                            store: wgpu::StoreOp::Store,
                        }),
                        stencil_ops: None,
                    }),
                    timestamp_writes: None,
                    ..Default::default()
                });
                pass.set_pipeline(&self.shadow_depth_pipeline);
                pass.set_bind_group(0, &self.shadow_vs_bind_group, &[slot * stride]);
                pass.set_vertex_buffer(0, self.vertex_buffer.slice(..));
                pass.set_index_buffer(self.index_buffer.slice(..), wgpu::IndexFormat::Uint32);
                pass.draw_indexed(0..self.index_count, 0, 0..1);
            }
        }

        // Depth pre-pass: same vertex/index/indirect as the forward pass; layout binds group 0 only.
        {
            let depth_ts = self
                .frame_timing
                .as_ref()
                .map(|t| t.render_pass_writes(TIMING_PAIR_DEPTH_PREPASS));
            let mut depth_pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("Depth Pre-Pass"),
                color_attachments: &[],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &self.depth_view,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(1.0),
                        store: wgpu::StoreOp::Store,
                    }),
                    stencil_ops: None,
                }),
                timestamp_writes: depth_ts,
                ..Default::default()
            });

            if self.has_geometry && self.index_count > 0 {
                depth_pass.set_pipeline(&self.depth_prepass_pipeline);
                depth_pass.set_bind_group(0, &self.uniform_bind_group, &[]);
                depth_pass.set_vertex_buffer(0, self.vertex_buffer.slice(..));
                depth_pass.set_index_buffer(self.index_buffer.slice(..), wgpu::IndexFormat::Uint32);

                if let Some(cull) = &self.compute_cull {
                    // None = no per-bucket texture bind (depth pre-pass layout is group 0 only).
                    cull.draw_indirect(&mut depth_pass, None);
                }
            }
        }

        {
            let forward_ts = self
                .frame_timing
                .as_ref()
                .map(|t| t.render_pass_writes(TIMING_PAIR_FORWARD));
            let mut render_pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("Textured Pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color {
                            r: 0.05,
                            g: 0.05,
                            b: 0.08,
                            a: 1.0,
                        }),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &self.depth_view,
                    depth_ops: Some(wgpu::Operations {
                        // Load: pre-pass filled it; Store: wireframe overlay reads it below.
                        load: wgpu::LoadOp::Load,
                        store: wgpu::StoreOp::Store,
                    }),
                    stencil_ops: None,
                }),
                timestamp_writes: forward_ts,
                ..Default::default()
            });

            if self.has_geometry && self.index_count > 0 {
                render_pass.set_pipeline(&self.pipeline);
                render_pass.set_bind_group(0, &self.uniform_bind_group, &[]);
                render_pass.set_bind_group(2, &self.lighting_bind_group, &[]);
                render_pass.set_bind_group(3, &self.sh_volume_resources.bind_group, &[]);
                render_pass.set_bind_group(4, &self.lightmap_resources.bind_group, &[]);
                render_pass.set_bind_group(5, &self.spot_shadow_pool.bind_group, &[]);
                render_pass.set_vertex_buffer(0, self.vertex_buffer.slice(..));
                render_pass
                    .set_index_buffer(self.index_buffer.slice(..), wgpu::IndexFormat::Uint32);

                if let Some(cull) = &self.compute_cull {
                    let gpu_textures = &self.gpu_textures;
                    cull.draw_indirect(
                        &mut render_pass,
                        Some(&|pass, bucket| {
                            let bind_group = if (bucket as usize) < gpu_textures.len() {
                                &gpu_textures[bucket as usize].bind_group
                            } else {
                                &gpu_textures[0].bind_group
                            };
                            pass.set_bind_group(1, bind_group, &[]);
                        }),
                    );
                }
            }
        }

        // Billboard sprite pass: after opaque forward, before wireframe overlay.
        // Alpha additive; depth test on, depth write off. One draw per collection.
        if self.smoke_pass.has_any_sheet() && !particle_collections.is_empty() {
            let mut smoke_pass_enc = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("Billboard Sprite Pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Load,
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &self.depth_view,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Load,
                        store: wgpu::StoreOp::Store,
                    }),
                    stencil_ops: None,
                }),
                ..Default::default()
            });
            smoke_pass_enc.set_bind_group(0, &self.uniform_bind_group, &[]);
            smoke_pass_enc.set_bind_group(2, &self.lighting_bind_group, &[]);
            smoke_pass_enc.set_bind_group(3, &self.sh_volume_resources.bind_group, &[]);
            for (collection, bytes) in particle_collections {
                if bytes.is_empty() {
                    continue;
                }
                self.smoke_pass
                    .record_draw(&self.queue, &mut smoke_pass_enc, collection, bytes);
            }
        }

        // Volumetric fog: low-res raymarch (compute) + additive composite blit.
        // Skipped entirely when no fog volumes are active for this frame —
        // the scatter target need not be cleared because the composite is not
        // issued. See: context/lib/rendering_pipeline.md §7.5
        if self.fog.active() {
            let inv_view_proj = view_proj.inverse();
            self.fog.upload_params(
                &self.queue,
                inv_view_proj,
                self.last_camera_position,
                crate::camera::NEAR,
                crate::camera::FAR,
            );

            // Repack the dynamic spot lights that own a shadow slot this
            // frame as `FogSpotLight` records — same source the shadow pass
            // already consumed (`level_lights` × `slot_assignment`). Only
            // shadow-slotted spots contribute to the fog beam pass; the
            // raymarch shader looks up `light_space_matrices.m[slot]` to
            // sample shadow occlusion, so a slotless spot has no usable
            // light-space matrix.
            let fog_spots = self.collect_fog_spot_lights();
            self.fog.upload_spots(&self.queue, &fog_spots);

            let (scatter_w, scatter_h) = self.fog.scatter_dims();
            // 8×8 workgroup matches the WGSL `@workgroup_size(8, 8)` declaration
            // in fog_volume.wgsl. Round up so partial tiles still cover the
            // scatter target's edge pixels.
            let groups_x = scatter_w.div_ceil(8);
            let groups_y = scatter_h.div_ceil(8);
            {
                let mut raymarch = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                    label: Some("Fog Raymarch Pass"),
                    timestamp_writes: None,
                });
                raymarch.set_pipeline(&self.fog.raymarch_pipeline);
                raymarch.set_bind_group(0, &self.uniform_bind_group, &[]);
                raymarch.set_bind_group(3, &self.sh_volume_resources.bind_group, &[]);
                raymarch.set_bind_group(5, &self.spot_shadow_pool.bind_group, &[]);
                raymarch.set_bind_group(6, &self.fog.bind_group, &[]);
                raymarch.dispatch_workgroups(groups_x, groups_y, 1);
            }

            let mut composite = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("Fog Composite Pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Load,
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                ..Default::default()
            });
            composite.set_pipeline(&self.fog.composite_pipeline);
            composite.set_bind_group(0, &self.fog.composite_bind_group, &[]);
            // Fullscreen triangle: 3 vertices, 1 instance. Geometry is generated
            // in the vertex shader from `vertex_index` — no vertex buffer.
            composite.draw(0..3, 0..1);
        }

        if self.wireframe_enabled
            && self.has_geometry
            && self.wireframe_index_count > 0
            && !self.bvh_leaves.is_empty()
        {
            if let Some(cull) = &self.compute_cull {
                let cull_status_bind_group =
                    self.device.create_bind_group(&wgpu::BindGroupDescriptor {
                        label: Some("Wireframe Cull Status BG"),
                        layout: &self.wireframe_cull_status_bgl,
                        entries: &[wgpu::BindGroupEntry {
                            binding: 0,
                            resource: cull.cull_status_buffer().as_entire_binding(),
                        }],
                    });

                let mut overlay_pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("Wireframe Overlay Pass"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: &view,
                        depth_slice: None,
                        resolve_target: None,
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Load,
                            store: wgpu::StoreOp::Store,
                        },
                    })],
                    depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                        view: &self.depth_view,
                        depth_ops: Some(wgpu::Operations {
                            load: wgpu::LoadOp::Load,
                            store: wgpu::StoreOp::Store,
                        }),
                        stencil_ops: None,
                    }),
                    ..Default::default()
                });

                overlay_pass.set_pipeline(&self.wireframe_pipeline);
                overlay_pass.set_bind_group(0, &self.uniform_bind_group, &[]);
                overlay_pass.set_bind_group(1, &cull_status_bind_group, &[]);
                overlay_pass.set_vertex_buffer(0, self.vertex_buffer.slice(..));
                overlay_pass.set_index_buffer(
                    self.wireframe_index_buffer.slice(..),
                    wgpu::IndexFormat::Uint32,
                );

                // Draw every BVH leaf with its leaf index as instance_index
                // so the shader can look up the per-leaf cull status.
                for (leaf_idx, leaf) in self.bvh_leaves.iter().enumerate() {
                    let wire_offset = leaf.index_offset * 2;
                    let wire_count = leaf.index_count * 2;
                    let li = leaf_idx as u32;
                    overlay_pass.draw_indexed(wire_offset..wire_offset + wire_count, 0, li..li + 1);
                }
            }
        }

        if let Some(timing) = &self.frame_timing {
            timing.encode_resolve(&mut encoder);
        }

        self.queue.submit(std::iter::once(encoder.finish()));
        output.present();

        if let Some(timing) = self.frame_timing.as_mut() {
            timing.post_submit(&self.device);
        }

        Ok(())
    }
}

fn build_default_view_projection(aspect: f32) -> Mat4 {
    let eye = glam::Vec3::new(0.0, 200.0, 500.0);
    let center = glam::Vec3::ZERO;
    let up = glam::Vec3::Y;

    let view = Mat4::look_at_rh(eye, center, up);
    let projection = Mat4::perspective_rh(std::f32::consts::FRAC_PI_2, aspect, 0.1, 4096.0);

    projection * view
}

fn cast_world_vertices_to_bytes(data: &[crate::geometry::WorldVertex]) -> Vec<u8> {
    let byte_len = data.len() * crate::geometry::WorldVertex::STRIDE;
    let mut bytes = Vec::with_capacity(byte_len);
    for vertex in data {
        for &c in &vertex.position {
            bytes.extend_from_slice(&c.to_ne_bytes());
        }
        for &c in &vertex.base_uv {
            bytes.extend_from_slice(&c.to_ne_bytes());
        }
        for &c in &vertex.normal_oct {
            bytes.extend_from_slice(&c.to_ne_bytes());
        }
        for &c in &vertex.tangent_packed {
            bytes.extend_from_slice(&c.to_ne_bytes());
        }
        for &c in &vertex.lightmap_uv {
            bytes.extend_from_slice(&c.to_ne_bytes());
        }
    }
    bytes
}

// Each triangle [a, b, c] → three line pairs [a,b, b,c, c,a].
// Shared edges are emitted multiple times; fine for a debug overlay.
fn build_line_indices_from_triangles(tri_indices: &[u32]) -> Vec<u32> {
    let tri_count = tri_indices.len() / 3;
    let mut lines = Vec::with_capacity(tri_count * 6);
    for tri in tri_indices.chunks_exact(3) {
        let (a, b, c) = (tri[0], tri[1], tri[2]);
        lines.push(a);
        lines.push(b);
        lines.push(b);
        lines.push(c);
        lines.push(c);
        lines.push(a);
    }
    lines
}

fn bytemuck_cast_slice_u32(data: &[u32]) -> Vec<u8> {
    let byte_len = std::mem::size_of_val(data);
    let mut bytes = Vec::with_capacity(byte_len);
    for &val in data {
        bytes.extend_from_slice(&val.to_ne_bytes());
    }
    bytes
}

// Static lights are baked into the lightmap; including them in the runtime loop
// would double-apply their contribution. Short influences → zero-radius placeholder.
fn filter_dynamic_lights(
    lights: &[MapLight],
    influences: &[LightInfluence],
) -> (Vec<MapLight>, Vec<LightInfluence>) {
    lights
        .iter()
        // enumerate before filter so i is the original index into influences
        .enumerate()
        .filter(|(_, l)| l.is_dynamic)
        .map(|(i, l)| {
            let inf = influences.get(i).cloned().unwrap_or(LightInfluence {
                center: Vec3::ZERO,
                radius: 0.0,
            });
            (l.clone(), inf)
        })
        .unzip()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_view_projection_is_finite() {
        let vp = build_default_view_projection(16.0 / 9.0);
        let cols = vp.to_cols_array();
        for (i, val) in cols.iter().enumerate() {
            assert!(val.is_finite(), "view_proj[{i}] is not finite: {val}");
        }
    }

    #[test]
    fn cast_world_vertices_roundtrips() {
        let input = vec![
            crate::geometry::WorldVertex {
                position: [1.0, 2.0, 3.0],
                base_uv: [0.5, 0.75],
                normal_oct: [32768, 32768],
                tangent_packed: [65535, 32768],
                lightmap_uv: [100, 200],
            },
            crate::geometry::WorldVertex {
                position: [4.0, 5.0, 6.0],
                base_uv: [0.25, 0.125],
                normal_oct: [0, 32768],
                tangent_packed: [32768, 0],
                lightmap_uv: [0, 0],
            },
        ];
        let bytes = cast_world_vertices_to_bytes(&input);
        // 2 vertices * 32 bytes = 64 bytes
        assert_eq!(bytes.len(), 64);

        let pos_x = f32::from_ne_bytes(bytes[0..4].try_into().unwrap());
        let pos_y = f32::from_ne_bytes(bytes[4..8].try_into().unwrap());
        let pos_z = f32::from_ne_bytes(bytes[8..12].try_into().unwrap());
        let uv_u = f32::from_ne_bytes(bytes[12..16].try_into().unwrap());
        let uv_v = f32::from_ne_bytes(bytes[16..20].try_into().unwrap());
        let n_u = u16::from_ne_bytes(bytes[20..22].try_into().unwrap());
        let n_v = u16::from_ne_bytes(bytes[22..24].try_into().unwrap());
        let t_u = u16::from_ne_bytes(bytes[24..26].try_into().unwrap());
        let t_v = u16::from_ne_bytes(bytes[26..28].try_into().unwrap());
        let lm_u = u16::from_ne_bytes(bytes[28..30].try_into().unwrap());
        let lm_v = u16::from_ne_bytes(bytes[30..32].try_into().unwrap());

        assert_eq!([pos_x, pos_y, pos_z], [1.0, 2.0, 3.0]);
        assert_eq!([uv_u, uv_v], [0.5, 0.75]);
        assert_eq!([n_u, n_v], [32768, 32768]);
        assert_eq!([t_u, t_v], [65535, 32768]);
        assert_eq!([lm_u, lm_v], [100, 200]);
    }

    #[test]
    fn byte_cast_u32_roundtrips() {
        let input = vec![100u32, 200, 300];
        let bytes = bytemuck_cast_slice_u32(&input);
        assert_eq!(bytes.len(), 12);

        let mut output = Vec::new();
        for chunk in bytes.chunks_exact(4) {
            output.push(u32::from_ne_bytes(chunk.try_into().unwrap()));
        }
        assert_eq!(output, vec![100, 200, 300]);
    }

    #[test]
    fn uniform_data_has_correct_size() {
        let data = build_uniform_data(&FrameUniforms {
            view_proj: Mat4::IDENTITY,
            camera_position: Vec3::ZERO,
            ambient_floor: 0.05,
            light_count: 0,
            time: 0.0,
            lighting_isolation: LightingIsolation::Normal,
            indirect_scale: 1.0,
        });
        assert_eq!(data.len(), UNIFORM_SIZE);
    }

    #[test]
    fn line_indices_from_single_triangle_produces_three_edges() {
        let tri = vec![0u32, 1, 2];
        let lines = build_line_indices_from_triangles(&tri);
        assert_eq!(lines, vec![0, 1, 1, 2, 2, 0]);
    }

    #[test]
    fn line_indices_from_two_triangles_produces_twelve_indices() {
        let tris = vec![0u32, 1, 2, 3, 4, 5];
        let lines = build_line_indices_from_triangles(&tris);
        assert_eq!(lines.len(), 12);
        assert_eq!(lines, vec![0, 1, 1, 2, 2, 0, 3, 4, 4, 5, 5, 3]);
    }

    #[test]
    fn line_indices_from_empty_input_is_empty() {
        let lines = build_line_indices_from_triangles(&[]);
        assert!(lines.is_empty());
    }

    #[test]
    fn line_indices_ignores_incomplete_trailing_triangle() {
        // 4 indices = 1 full triangle + 1 dangling index.
        let tris = vec![0u32, 1, 2, 3];
        let lines = build_line_indices_from_triangles(&tris);
        assert_eq!(lines, vec![0, 1, 1, 2, 2, 0]);
    }

    /// Regression: both the CPU-side `build_uniform_data` packer and the
    /// CPU-side `pack_light` packer must match the WGSL struct layouts
    /// that the fragment shader compiles against. Parsing the live
    /// shader source with naga catches drift before it reaches a GPU
    /// round-trip (see the similar test in `compute_cull.rs`).
    #[test]
    fn forward_wgsl_struct_strides_match_cpu_layout() {
        let module = naga::front::wgsl::parse_str(SHADER_SOURCE)
            .expect("forward shader should parse as WGSL");

        let mut seen = std::collections::HashMap::new();
        for (_handle, ty) in module.types.iter() {
            if let naga::TypeInner::Struct { span, .. } = &ty.inner
                && let Some(name) = &ty.name
            {
                seen.insert(name.clone(), *span);
            }
        }

        let uniforms_span = seen
            .get("Uniforms")
            .copied()
            .expect("forward shader should declare struct Uniforms");
        assert_eq!(
            uniforms_span as usize, UNIFORM_SIZE,
            "forward.wgsl Uniforms stride ({uniforms_span}) must match UNIFORM_SIZE ({UNIFORM_SIZE})",
        );

        let light_span = seen
            .get("GpuLight")
            .copied()
            .expect("forward shader should declare struct GpuLight");
        assert_eq!(
            light_span as usize,
            crate::lighting::GPU_LIGHT_SIZE,
            "forward.wgsl GpuLight stride ({light_span}) must match GPU_LIGHT_SIZE ({})",
            crate::lighting::GPU_LIGHT_SIZE,
        );
    }

    /// Regression: the SH volume's `ShGridInfo` uniform struct must have
    /// matching byte stride on both sides of the bind group — CPU packer
    /// (`sh_volume::build_grid_info_bytes`) and the fragment shader's
    /// declaration in `forward.wgsl`.
    #[test]
    fn forward_wgsl_sh_grid_info_matches_cpu_layout() {
        let module = naga::front::wgsl::parse_str(SHADER_SOURCE)
            .expect("forward shader should parse as WGSL");

        let mut seen = std::collections::HashMap::new();
        for (_handle, ty) in module.types.iter() {
            if let naga::TypeInner::Struct { span, .. } = &ty.inner
                && let Some(name) = &ty.name
            {
                seen.insert(name.clone(), *span);
            }
        }

        let span = seen
            .get("ShGridInfo")
            .copied()
            .expect("forward shader should declare struct ShGridInfo");
        assert_eq!(
            span as usize,
            sh_volume::SH_GRID_INFO_SIZE,
            "forward.wgsl ShGridInfo stride ({span}) must match SH_GRID_INFO_SIZE ({})",
            sh_volume::SH_GRID_INFO_SIZE,
        );

        let desc_span = seen
            .get("AnimationDescriptor")
            .copied()
            .expect("forward shader should declare struct AnimationDescriptor");
        assert_eq!(
            desc_span as usize,
            sh_volume::ANIMATION_DESCRIPTOR_SIZE,
            "forward.wgsl AnimationDescriptor stride ({desc_span}) must match ANIMATION_DESCRIPTOR_SIZE ({})",
            sh_volume::ANIMATION_DESCRIPTOR_SIZE,
        );
    }

    /// Regression: every storage/uniform buffer binding in `forward.wgsl` must
    /// receive a payload large enough to satisfy wgpu's minimum-binding-size
    /// validation. The original bug was `anim_descriptors` bound with 16 B while
    /// `array<AnimationDescriptor>` requires ≥ 48 B (one full element stride).
    ///
    /// Strategy: parse the live shader with naga, derive the minimum required
    /// size for every buffer binding from the WGSL type information, then check
    /// that the Rust-side dummy payloads (empty-map / no-SH-section case) are
    /// at least that large. Catches mismatches at `cargo test` time, not at
    /// draw time on real hardware.
    #[test]
    fn forward_wgsl_dummy_buffers_meet_shader_min_binding_size() {
        use std::collections::HashMap;

        let module = naga::front::wgsl::parse_str(SHADER_SOURCE)
            .expect("forward shader should parse as WGSL");

        // Build (group, binding) → minimum byte count required by the shader.
        // Only storage and uniform address spaces produce buffer bindings.
        let mut min_sizes: HashMap<(u32, u32), u64> = HashMap::new();
        for (_handle, var) in module.global_variables.iter() {
            let is_buffer = matches!(
                var.space,
                naga::AddressSpace::Storage { .. } | naga::AddressSpace::Uniform
            );
            if !is_buffer {
                continue;
            }
            let Some(rb) = &var.binding else { continue };
            let ty = &module.types[var.ty];
            let min: u64 = match &ty.inner {
                // Unbounded array<T> — shader needs at least one element.
                naga::TypeInner::Array {
                    stride,
                    size: naga::ArraySize::Dynamic,
                    ..
                } => *stride as u64,
                // Bounded array<T, N> — shader needs all N elements.
                naga::TypeInner::Array {
                    stride,
                    size: naga::ArraySize::Constant(n),
                    ..
                } => n.get() as u64 * *stride as u64,
                // Struct — shader needs the full declared span.
                naga::TypeInner::Struct { span, .. } => *span as u64,
                // Scalars / vectors / matrices: trivially satisfied; skip.
                _ => continue,
            };
            min_sizes.insert((rb.group, rb.binding), min);
        }

        // Verify that the empty-map dummy animation buffers (no SH section)
        // satisfy the shader's per-binding size requirements.
        //
        // binding 11: array<AnimationDescriptor> — stride = ANIMATION_DESCRIPTOR_SIZE
        // binding 12: array<f32>                 — stride = 4
        let (anim_desc, anim_samples, _count) = sh_volume::build_animation_buffers(None);

        for (label, binding, buf) in [
            (
                "anim_descriptors",
                sh_volume::BIND_ANIM_DESCRIPTORS,
                anim_desc.as_slice(),
            ),
            (
                "anim_samples",
                sh_volume::BIND_ANIM_SAMPLES,
                anim_samples.as_slice(),
            ),
        ] {
            if let Some(&min) = min_sizes.get(&(3, binding)) {
                assert!(
                    buf.len() as u64 >= min,
                    "dummy {label} buffer (group=3, binding={binding}): Rust side \
                     produces {} B but forward.wgsl min binding size is {min} B \
                     (array element stride — at least one element required)",
                    buf.len(),
                );
            } else {
                panic!(
                    "forward.wgsl has no buffer at group=3 binding={binding}; \
                        check BIND_* constants match shader @binding decorators"
                );
            }
        }

        // Verify the ShGridInfo uniform payload size.
        let sh_grid_binding = (1 + sh_volume::SH_BAND_COUNT) as u32; // = 10
        let grid_info = sh_volume::build_grid_info_bytes([0.0; 3], [1.0; 3], [1, 1, 1], false);
        if let Some(&min) = min_sizes.get(&(3, sh_grid_binding)) {
            assert!(
                grid_info.len() as u64 >= min,
                "sh_grid uniform (group=3, binding={sh_grid_binding}): Rust side \
                 produces {} B but forward.wgsl struct span is {min} B",
                grid_info.len(),
            );
        } else {
            panic!(
                "forward.wgsl has no uniform at group=3 binding={sh_grid_binding}; \
                    check SH_BAND_COUNT matches shader @binding decorators"
            );
        }
    }

    /// The depth pre-pass shader must parse as valid WGSL and declare
    /// the same `Uniforms` struct binding as `forward.wgsl` (only the
    /// leading `view_proj` field is referenced, but the shader still
    /// needs to compile cleanly).
    #[test]
    fn depth_prepass_wgsl_parses() {
        let module = naga::front::wgsl::parse_str(DEPTH_PREPASS_SHADER_SOURCE)
            .expect("depth_prepass.wgsl should parse as WGSL");
        // Sanity: the vertex entry point must be named `vs_main` so the
        // pipeline's `entry_point: Some("vs_main")` resolves.
        let has_vs_main = module
            .entry_points
            .iter()
            .any(|ep| ep.name == "vs_main" && ep.stage == naga::ShaderStage::Vertex);
        assert!(
            has_vs_main,
            "depth_prepass.wgsl must export @vertex vs_main"
        );
    }

    /// Ensure the wireframe shader's `Uniforms` struct stays in sync with
    /// the forward shader's — they share a single uniform buffer binding.
    #[test]
    fn wireframe_wgsl_uniforms_match_forward_layout() {
        let module = naga::front::wgsl::parse_str(WIREFRAME_SHADER_SOURCE)
            .expect("wireframe shader should parse as WGSL");

        let mut seen = std::collections::HashMap::new();
        for (_handle, ty) in module.types.iter() {
            if let naga::TypeInner::Struct { span, .. } = &ty.inner
                && let Some(name) = &ty.name
            {
                seen.insert(name.clone(), *span);
            }
        }

        let uniforms_span = seen
            .get("Uniforms")
            .copied()
            .expect("wireframe shader should declare struct Uniforms");
        assert_eq!(
            uniforms_span as usize, UNIFORM_SIZE,
            "wireframe.wgsl Uniforms stride ({uniforms_span}) must match UNIFORM_SIZE ({UNIFORM_SIZE})",
        );
    }

    #[test]
    fn uniform_data_encodes_view_proj_camera_and_lighting_fields() {
        let camera = Vec3::new(10.0, 20.0, 30.0);
        let ambient_floor = 0.125_f32;
        let light_count = 7_u32;
        let indirect_scale = 0.5_f32;
        let data = build_uniform_data(&FrameUniforms {
            view_proj: Mat4::IDENTITY,
            camera_position: camera,
            ambient_floor,
            light_count,
            time: 0.0,
            lighting_isolation: LightingIsolation::Normal,
            indirect_scale,
        });

        // view_proj: first 64 bytes = 16 f32 identity columns.
        let mut floats = Vec::new();
        for chunk in data.chunks_exact(4).take(16) {
            floats.push(f32::from_ne_bytes(chunk.try_into().unwrap()));
        }
        let identity = Mat4::IDENTITY.to_cols_array();
        for i in 0..16 {
            let epsilon = 1e-6;
            assert!(
                (floats[i] - identity[i]).abs() < epsilon,
                "view_proj[{i}] mismatch: expected {}, got {}",
                identity[i],
                floats[i],
            );
        }

        // camera_position at bytes 64..76.
        let cx = f32::from_ne_bytes(data[64..68].try_into().unwrap());
        let cy = f32::from_ne_bytes(data[68..72].try_into().unwrap());
        let cz = f32::from_ne_bytes(data[72..76].try_into().unwrap());
        assert_eq!(cx, 10.0);
        assert_eq!(cy, 20.0);
        assert_eq!(cz, 30.0);

        // ambient_floor at bytes 76..80.
        let af = f32::from_ne_bytes(data[76..80].try_into().unwrap());
        assert!((af - ambient_floor).abs() < 1e-6);

        // light_count at bytes 80..84.
        let lc = u32::from_ne_bytes(data[80..84].try_into().unwrap());
        assert_eq!(lc, light_count);

        // time at bytes 84..88 (passed 0.0 in this test).
        let t = f32::from_ne_bytes(data[84..88].try_into().unwrap());
        assert_eq!(t, 0.0);

        // lighting_isolation at bytes 88..92 (passed Normal = 0).
        let iso = u32::from_ne_bytes(data[88..92].try_into().unwrap());
        assert_eq!(iso, 0);

        // indirect_scale at bytes 92..96.
        let scale = f32::from_ne_bytes(data[92..96].try_into().unwrap());
        assert!((scale - indirect_scale).abs() < 1e-6);
    }

    /// Static lights are baked into the lightmap; including them in the
    /// runtime direct-light loop would double-apply their contribution on
    /// top of the bake. The filter at renderer init time must drop them
    /// while keeping influences index-aligned with the surviving lights.
    #[test]
    fn dynamic_light_filter_excludes_static_lights() {
        fn mk_light(intensity: f32, is_dynamic: bool) -> MapLight {
            MapLight {
                origin: [0.0, 0.0, 0.0],
                light_type: crate::prl::LightType::Point,
                // intensity doubles as an identity tag so the test can verify
                // ordering after the filter without inspecting other fields.
                intensity,
                color: [1.0, 1.0, 1.0],
                falloff_model: crate::prl::FalloffModel::InverseSquared,
                falloff_range: 10.0,
                cone_angle_inner: 0.0,
                cone_angle_outer: 0.0,
                cone_direction: [0.0, 0.0, -1.0],
                cast_shadows: false,
                is_dynamic,
                tags: vec![],
                leaf_index: 0,
            }
        }

        // Mixed input: dyn, static, dyn, static, dyn — three should survive.
        let lights = vec![
            mk_light(1.0, true),
            mk_light(2.0, false),
            mk_light(3.0, true),
            mk_light(4.0, false),
            mk_light(5.0, true),
        ];
        // Each influence's `radius` doubles as an identity tag so the test
        // can verify alignment between surviving lights and their influence.
        let influences = vec![
            LightInfluence {
                center: Vec3::new(1.0, 0.0, 0.0),
                radius: 1.0,
            },
            LightInfluence {
                center: Vec3::new(2.0, 0.0, 0.0),
                radius: 2.0,
            },
            LightInfluence {
                center: Vec3::new(3.0, 0.0, 0.0),
                radius: 3.0,
            },
            LightInfluence {
                center: Vec3::new(4.0, 0.0, 0.0),
                radius: 4.0,
            },
            LightInfluence {
                center: Vec3::new(5.0, 0.0, 0.0),
                radius: 5.0,
            },
        ];

        let (out_lights, out_influences) = filter_dynamic_lights(&lights, &influences);

        assert_eq!(out_lights.len(), 3, "expected 3 dynamic lights");
        assert_eq!(out_influences.len(), 3, "influences must match lights len");

        // Surviving lights are the dynamic ones (intensity 1, 3, 5) in order.
        assert_eq!(out_lights[0].intensity, 1.0);
        assert_eq!(out_lights[1].intensity, 3.0);
        assert_eq!(out_lights[2].intensity, 5.0);
        assert!(out_lights.iter().all(|l| l.is_dynamic));

        // Influences are aligned with the original light's index — radius
        // 1.0 stays paired with the light tagged 1.0, not shifted.
        assert_eq!(out_influences[0].radius, 1.0);
        assert_eq!(out_influences[1].radius, 3.0);
        assert_eq!(out_influences[2].radius, 5.0);
        assert_eq!(out_influences[0].center, Vec3::new(1.0, 0.0, 0.0));
        assert_eq!(out_influences[1].center, Vec3::new(3.0, 0.0, 0.0));
        assert_eq!(out_influences[2].center, Vec3::new(5.0, 0.0, 0.0));
    }
}
