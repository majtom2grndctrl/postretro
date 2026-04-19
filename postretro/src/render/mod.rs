// Renderer: GPU init, texture upload, depth pre-pass + forward pipelines, and draw.
// See: context/lib/rendering_pipeline.md

pub mod frame_timing;
pub mod sh_volume;

use std::sync::Arc;
use std::time::Instant;

use anyhow::{Context, Result};
use glam::{Mat4, Vec3};
use wgpu::util::DeviceExt;
use winit::window::Window;

use crate::compute_cull::ComputeCullPipeline;
use crate::geometry::BvhTree;
use crate::lighting::influence::{self, LightInfluence};
use crate::lighting::lightmap::LightmapResources;
use crate::lighting::{GPU_LIGHT_SIZE, pack_lights};
use crate::prl::MapLight;
use crate::texture::{LoadedTexture, TextureSet};
use crate::visibility::VisibleCells;

use frame_timing::FrameTiming;
use sh_volume::ShVolumeResources;

// --- WGSL Shaders ---

const SHADER_SOURCE: &str = include_str!("../shaders/forward.wgsl");

// Wireframe overlay: culling-delta debug visualization. See shader header.
const WIREFRAME_SHADER_SOURCE: &str = include_str!("../shaders/wireframe.wgsl");

// Depth pre-pass: vertex-only shader used to populate the shared depth
// buffer before the forward pass, so the forward pass can run with a
// `depth_compare: Equal` test and zero shading overdraw.
const DEPTH_PREPASS_SHADER_SOURCE: &str = include_str!("../shaders/depth_prepass.wgsl");

// --- GPU-timing pair indices ---
//
// Pair index `i` maps to query slots `[2i, 2i+1]` in the `FrameTiming`
// query set (see `frame_timing::FrameTiming`). The labels vector passed
// to `FrameTiming::new` is indexed by these constants so the label
// ordering and the callsite indices can't drift: add an entry here,
// bump the label-vec length below, and the per-pass `*_pass_writes(...)`
// callsites read the right label.
const TIMING_PAIR_CULL: usize = 0;
const TIMING_PAIR_DEPTH_PREPASS: usize = 1;
const TIMING_PAIR_FORWARD: usize = 2;
const TIMING_PAIR_COUNT: usize = 3;

// --- Uniform buffer layout ---

/// Per-frame uniform data: view-projection, camera world-space position,
/// ambient floor, light count, elapsed time, and lighting-isolation mode.
///
/// Layout must match the WGSL `Uniforms` struct in `forward.wgsl` and
/// `wireframe.wgsl` — both shaders bind the same buffer. std140 rules
/// align `vec3<f32>` to 16 bytes, so `camera_position` (vec3) + trailing
/// `ambient_floor` (f32) share one 16-byte slot. `light_count` (u32)
/// starts a new slot and is padded out to a full vec4 slot for alignment.
///
/// Offsets (bytes):
///   0..64    view_proj           (mat4x4<f32>)
///   64..76   camera_position     (vec3<f32>)
///   76..80   ambient_floor       (f32)
///   80..84   light_count         (u32)
///   84..88   time                (f32, elapsed seconds for SH animation)
///   88..92   lighting_isolation  (u32, cycles 0..4; chord Alt+Shift+4)
///   92..96   _pad                (u32, keeps the struct 16-byte aligned)
const UNIFORM_SIZE: usize = 96;

/// Lighting-term isolation mode for leak/bleed debugging.
///
/// Cycled by the `Alt+Shift+4` diagnostic chord. The fragment shader branches
/// on this value to zero out either the SH indirect term or the direct-light
/// sum, so an A/B compare inside a leaky room pins the bug to exactly one
/// lighting path. `Normal` is the default and matches production shading.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum LightingIsolation {
    Normal = 0,
    DirectOnly = 1,
    IndirectOnly = 2,
    AmbientOnly = 3,
}

impl LightingIsolation {
    /// Advance to the next mode in the cycle, wrapping back to `Normal`.
    pub fn cycle(self) -> Self {
        match self {
            LightingIsolation::Normal => LightingIsolation::DirectOnly,
            LightingIsolation::DirectOnly => LightingIsolation::IndirectOnly,
            LightingIsolation::IndirectOnly => LightingIsolation::AmbientOnly,
            LightingIsolation::AmbientOnly => LightingIsolation::Normal,
        }
    }

    /// Human-readable label for diagnostics logging.
    pub fn label(self) -> &'static str {
        match self {
            LightingIsolation::Normal => "Normal (direct + indirect)",
            LightingIsolation::DirectOnly => "DirectOnly (SH indirect = 0)",
            LightingIsolation::IndirectOnly => "IndirectOnly (direct = 0)",
            LightingIsolation::AmbientOnly => "AmbientOnly (both = 0)",
        }
    }
}

/// CPU-side mirror of the per-frame uniform buffer. Fields map 1:1 to the
/// WGSL uniform layout; `build_uniform_data` is the single source of truth
/// for the byte packing.
struct FrameUniforms {
    view_proj: Mat4,
    camera_position: Vec3,
    ambient_floor: f32,
    light_count: u32,
    time: f32,
    lighting_isolation: LightingIsolation,
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
    // bytes 92..96 stay zero — pad.
    bytes
}

/// Default ambient floor applied when the caller doesn't override it.
/// Provisional value from sub-plan 3; tuned via the ambient-floor slider
/// in the settings menu. The right default is the lowest value where a
/// player can still navigate dark areas.
pub const DEFAULT_AMBIENT_FLOOR: f32 = 0.05;

// --- GPU texture ---

/// A GPU-uploaded texture with its bind group for per-texture binding.
struct GpuTexture {
    bind_group: wgpu::BindGroup,
}

/// Upload a single LoadedTexture to the GPU and create a bind group.
fn upload_texture(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    loaded: &LoadedTexture,
    sampler: &wgpu::Sampler,
    texture_bind_group_layout: &wgpu::BindGroupLayout,
    label: &str,
) -> GpuTexture {
    let size = wgpu::Extent3d {
        width: loaded.width,
        height: loaded.height,
        depth_or_array_layers: 1,
    };

    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some(label),
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
        &loaded.data,
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(4 * loaded.width),
            rows_per_image: Some(loaded.height),
        },
        size,
    );

    let texture_view = texture.create_view(&wgpu::TextureViewDescriptor::default());

    let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some(&format!("{label} Bind Group")),
        layout: texture_bind_group_layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(&texture_view),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::Sampler(sampler),
            },
        ],
    });

    GpuTexture { bind_group }
}

// --- Depth buffer ---

/// Depth format used for the depth buffer.
const DEPTH_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Depth32Float;

/// Create the depth texture and return both the texture and its view
/// (for depth attachment).
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
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
        view_formats: &[],
    });

    let view = depth_texture.create_view(&wgpu::TextureViewDescriptor::default());
    (depth_texture, view)
}

// --- Geometry data ---

/// Geometry data the renderer needs from a level, including the BVH used to
/// build the GPU-driven indirect draw pipeline and the map's light list.
pub struct LevelGeometry<'a> {
    pub vertices: &'a [crate::geometry::WorldVertex],
    pub indices: &'a [u32],
    /// Global BVH loaded from the `Bvh` section. Always present for valid
    /// PRL levels — pre-BVH maps fail earlier in the loader.
    pub bvh: &'a BvhTree,
    /// Direct lights parsed from the AlphaLights PRL section. May be empty
    /// on maps compiled before the Lighting Foundation milestone.
    pub lights: &'a [MapLight],
    /// Per-light influence volumes from the LightInfluence PRL section.
    /// Same length as `lights` when present; empty if absent.
    pub light_influences: &'a [LightInfluence],
    /// Baked SH L2 irradiance volume from the `ShVolume` PRL section. `None`
    /// when the section is absent — the renderer binds dummy 1×1×1 textures
    /// and the shader skips SH sampling.
    pub sh_volume: Option<&'a postretro_level_format::sh_volume::ShVolumeSection>,
    /// Baked directional lightmap atlas from the `Lightmap` PRL section.
    /// `None` when the section is absent — the renderer binds a 1×1 white
    /// placeholder and bumped-Lambert falls back to flat white.
    pub lightmap: Option<&'a postretro_level_format::lightmap::LightmapSection>,
}

// --- Renderer ---

pub struct Renderer {
    device: wgpu::Device,
    queue: wgpu::Queue,
    surface: wgpu::Surface<'static>,
    surface_config: wgpu::SurfaceConfiguration,
    is_surface_configured: bool,

    pipeline: wgpu::RenderPipeline,
    /// Depth-only pipeline used for the pre-pass that populates the
    /// shared depth buffer before the forward pass. The forward pass
    /// then runs with `depth_compare: Equal` and shades each pixel once.
    depth_prepass_pipeline: wgpu::RenderPipeline,
    /// GPU timestamp-query recorder. `Some` when
    /// `POSTRETRO_GPU_TIMING=1` is set AND the adapter advertises the
    /// `TIMESTAMP_QUERY` feature; `None` otherwise so no
    /// `timestamp_writes` fields are attached to any pass.
    frame_timing: Option<FrameTiming>,
    vertex_buffer: wgpu::Buffer,
    index_buffer: wgpu::Buffer,
    index_count: u32,
    uniform_buffer: wgpu::Buffer,
    uniform_bind_group: wgpu::BindGroup,

    /// Group 2 — storage buffer of packed `GpuLight` records, uploaded
    /// once at level load. Always bound; maps with zero lights get a
    /// 1-element dummy buffer so the binding is never empty (wgpu
    /// rejects zero-sized storage buffer bindings).
    lighting_bind_group: wgpu::BindGroup,
    /// Number of real (non-dummy) lights uploaded — this is the loop
    /// bound the fragment shader reads from `uniforms.light_count`.
    light_count: u32,
    /// Ambient floor applied before albedo multiply. Player-facing setting
    /// (0.0–1.0 slider); default is `DEFAULT_AMBIENT_FLOOR`. A scalar
    /// brightness only — no color.
    ambient_floor: f32,

    /// Group 3 — SH irradiance volume resources. Always allocated; when no
    /// SH section is present the bind group binds dummy 1×1×1 textures and
    /// the fragment shader's `has_sh_volume` flag is 0 so SH sampling is
    /// skipped. See `sh_volume` module for layout.
    sh_volume_resources: ShVolumeResources,

    /// Group 4 — directional lightmap atlas resources. Always allocated;
    /// when no Lightmap section is present the bind group binds a 1×1
    /// white/neutral placeholder so the shader path never branches.
    /// See `lighting::lightmap`.
    lightmap_resources: LightmapResources,

    depth_view: wgpu::TextureView,

    /// GPU textures indexed by texture index.
    gpu_textures: Vec<GpuTexture>,
    /// Cached BVH leaves, used by the wireframe overlay to size per-leaf
    /// draw ranges. The renderer no longer consults this for the textured
    /// pass — that flows entirely through the compute shader / indirect
    /// buffer path.
    bvh_leaves: Vec<crate::geometry::BvhLeaf>,
    /// GPU-driven compute culling pipeline. `Some` when the level has a
    /// non-empty BVH; `None` for no-geometry mode.
    compute_cull: Option<ComputeCullPipeline>,

    /// Debug wireframe overlay pipeline (LineList topology, cull-status-driven color).
    wireframe_pipeline: wgpu::RenderPipeline,
    /// Line-list index buffer built from the triangle index buffer at load time.
    /// Layout is 1:1 parallel with the triangle index buffer: each triangle at
    /// triangle-buffer range `[tri_start..tri_end]` (multiple of 3) maps to
    /// line-buffer range `[tri_start*2..tri_end*2]` (6 line indices per 3
    /// triangle indices).
    wireframe_index_buffer: wgpu::Buffer,
    wireframe_index_count: u32,
    /// Bind group layout for the wireframe cull-status storage buffer (group 1).
    wireframe_cull_status_bgl: wgpu::BindGroupLayout,
    /// Whether the culling-delta wireframe overlay is active.
    wireframe_enabled: bool,

    /// Lighting-term isolation mode for leak/bleed debugging. Cycled by the
    /// `Alt+Shift+4` diagnostic chord. When not `Normal`, the fragment
    /// shader zeroes out one or both lighting terms so an A/B compare
    /// isolates which path (direct vs baked SH indirect) is carrying a bad
    /// contribution.
    lighting_isolation: LightingIsolation,

    /// Whether the surface is currently configured with vsync on
    /// (`AutoVsync`) or off (`AutoNoVsync`). Toggled by the
    /// `Alt+Shift+V` diagnostic chord so the frametime meter can be
    /// compared against real CPU cost; initialized to match the
    /// `AutoVsync` default chosen in `Renderer::new`.
    vsync_enabled: bool,

    has_geometry: bool,

    /// Monotonic frame counter for debug logging.
    debug_frame: u64,
    /// Previous frame's visible-cell bitmask fingerprint (popcount, xor_hash).
    debug_prev_bitmask: (u32, u32),
    /// Previous frame's view_proj matrix fingerprint (bitwise xor of all 16 f32 bits).
    debug_prev_vp_hash: u32,
    /// Previous frame's VisibleCells variant label + cell count.
    debug_prev_visible: (&'static str, usize),

    /// Wall-clock timestamp of renderer creation. The per-frame uniform's
    /// `time` field is `app_start.elapsed()`; the fragment shader wraps it
    /// per-light via `fract(time / period + phase)` for SH animation.
    app_start: Instant,
}

impl Renderer {
    /// Create the renderer, taking ownership of all GPU state.
    ///
    /// `geometry` is `None` when no map file was loaded (renders clear color only).
    /// `texture_set` provides CPU-side textures for GPU upload; `None` for no textures.
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

        // Probe for multi_draw_indexed_indirect support via downlevel flags.
        // Available on Vulkan, Metal, DX12; absent on WebGL2 (not a target).
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

        // Probe for TIMESTAMP_QUERY: only enable GPU timing when both the
        // adapter supports it AND the user opted in via env var. On
        // fallback `FrameTiming` is `None` and no timestamp_writes are
        // attached anywhere — zero runtime cost when disabled.
        let adapter_features = adapter.features();
        let gpu_timing_requested = std::env::var("POSTRETRO_GPU_TIMING")
            .ok()
            .as_deref()
            == Some("1");
        let gpu_timing_supported =
            adapter_features.contains(wgpu::Features::TIMESTAMP_QUERY);
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

        // Raise `max_bind_groups` above the WebGPU downlevel default of 4.
        // The forward pipeline binds groups 0–4 today (camera, material,
        // lights, SH volume, lightmap) and the `lighting-spot-shadows` /
        // `lighting-chunk-lists` plans will add more. 8 is the WebGPU
        // maximum and is supported on every desktop backend we target.
        let required_limits = wgpu::Limits {
            max_bind_groups: 8,
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

        // Build vertex and index buffers.
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

        // Uniform buffer (view-projection + camera position + ambient floor
        // + light count). Initial value is the hardcoded default view until
        // `update_per_frame_uniforms` is called from the main loop.
        let view_proj = build_default_view_projection(
            surface_config.width as f32 / surface_config.height as f32,
        );
        let light_count = geometry.map(|g| g.lights.len() as u32).unwrap_or(0);
        let ambient_floor = DEFAULT_AMBIENT_FLOOR;
        let uniform_data = build_uniform_data(&FrameUniforms {
            view_proj,
            camera_position: Vec3::ZERO,
            ambient_floor,
            light_count,
            time: 0.0,
            lighting_isolation: LightingIsolation::Normal,
        });

        let uniform_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Uniform Buffer"),
            contents: &uniform_data,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });

        // Bind group layout for group 0: per-frame uniforms.
        let uniform_bind_group_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("Uniform Bind Group Layout"),
                entries: &[wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX | wgpu::ShaderStages::FRAGMENT,
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

        // Bind group layout for group 1: per-texture.
        let texture_bind_group_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("Texture Bind Group Layout"),
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
                ],
            });

        // Bind group layout for group 2: direct-light storage buffers.
        // 0 = lights, 1 = influence volumes.
        let lighting_bind_group_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("Lighting Bind Group Layout"),
                entries: &[
                    // binding 0: GpuLight array
                    wgpu::BindGroupLayoutEntry {
                        binding: 0,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Buffer {
                            ty: wgpu::BufferBindingType::Storage { read_only: true },
                            has_dynamic_offset: false,
                            min_binding_size: None,
                        },
                        count: None,
                    },
                    // binding 1: influence volumes
                    wgpu::BindGroupLayoutEntry {
                        binding: 1,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Buffer {
                            ty: wgpu::BufferBindingType::Storage { read_only: true },
                            has_dynamic_offset: false,
                            min_binding_size: None,
                        },
                        count: None,
                    },
                ],
            });

        // Pack the map's lights into GPU bytes and create the storage
        // buffer. wgpu rejects a zero-size storage buffer, so we pad to a
        // single dummy record when there are no lights at all; the
        // shader's `light_count` loop bound stays at 0 so the dummy is
        // never read.
        let lights_data = match geometry {
            Some(g) if !g.lights.is_empty() => pack_lights(g.lights),
            _ => vec![0u8; GPU_LIGHT_SIZE],
        };
        let lights_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Direct Lights Storage Buffer"),
            contents: &lights_data,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
        });

        // Influence volume buffer (binding 1). Same dummy strategy as lights.
        let influence_data = match geometry {
            Some(g) if !g.light_influences.is_empty() => {
                influence::pack_influence(g.light_influences)
            }
            _ => vec![0u8; 16], // one dummy vec4<f32>
        };
        let influence_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Light Influence Storage Buffer"),
            contents: &influence_data,
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

        // Upload textures to GPU.
        let gpu_textures = if let Some(tex_set) = texture_set {
            tex_set
                .textures
                .iter()
                .enumerate()
                .map(|(idx, loaded)| {
                    let label = format!("Texture {idx}");
                    upload_texture(
                        &device,
                        &queue,
                        loaded,
                        &base_sampler,
                        &texture_bind_group_layout,
                        &label,
                    )
                })
                .collect()
        } else {
            Vec::new()
        };

        // If we have no textures at all, create a single placeholder so we always
        // have something to bind.
        let gpu_textures = if gpu_textures.is_empty() {
            let placeholder = crate::texture::generate_placeholder();
            vec![upload_texture(
                &device,
                &queue,
                &placeholder,
                &base_sampler,
                &texture_bind_group_layout,
                "Placeholder Texture",
            )]
        } else {
            gpu_textures
        };

        // Store the BVH leaves (for the wireframe overlay) and create the
        // compute cull pipeline off the loaded BVH. Empty-BVH levels skip
        // the pipeline entirely.
        let bvh_leaves: Vec<crate::geometry::BvhLeaf> =
            geometry.map(|g| g.bvh.leaves.clone()).unwrap_or_default();
        let compute_cull = geometry
            .filter(|g| !g.bvh.leaves.is_empty())
            .map(|g| ComputeCullPipeline::new(&device, g.bvh, has_multi_draw_indirect));

        // Depth buffer.
        let (_depth_texture, depth_view) =
            create_depth_texture(&device, surface_config.width, surface_config.height);

        // Group 3: SH irradiance volume (indirect lighting). Always created —
        // when the level has no SH section, dummies are bound and the shader
        // skips SH sampling via the `has_sh_volume` flag in the grid-info
        // uniform. See sub-plan 6.
        let sh_volume_resources = ShVolumeResources::new(
            &device,
            &queue,
            geometry.and_then(|g| g.sh_volume),
        );

        // Group 4: directional lightmap atlas (baked static direct lighting).
        // Always created — placeholder 1×1 white/neutral bindings are used
        // when the level has no Lightmap section. See `lighting::lightmap`.
        // Layout is built up front so the pipeline layout can reference it
        // before the bind group is populated.
        let lightmap_bind_group_layout =
            crate::lighting::lightmap::bind_group_layout(&device);
        let lightmap_resources = LightmapResources::new(
            &device,
            &queue,
            geometry.and_then(|g| g.lightmap),
            &lightmap_bind_group_layout,
        );

        // Pipeline layout. Group 2 is the direct-lighting storage buffer
        // introduced in sub-plan 3 of the lighting foundation; group 3 is
        // the SH irradiance volume introduced in sub-plan 6; group 4 is the
        // baked directional lightmap atlas (`lighting-lightmaps`).
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("Textured Pipeline Layout"),
            bind_group_layouts: &[
                Some(&uniform_bind_group_layout),
                Some(&texture_bind_group_layout),
                Some(&lighting_bind_group_layout),
                Some(&sh_volume_resources.bind_group_layout),
                Some(&lightmap_bind_group_layout),
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
                // Depth pre-pass populates the depth buffer; the forward
                // pass only needs an `Equal` pass-through test so it
                // shades each screen pixel exactly once. Disabling
                // `depth_write_enabled` avoids a redundant rewrite of
                // values the pre-pass already stored.
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

        // --- Wireframe overlay pipeline ---
        // Group 0 = uniforms (view_proj), group 1 = cull_status storage buffer.
        // Draws line lists with depth test disabled so edges render on top.
        // Colors are driven by per-chunk cull status from the compute shader.
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
            // Depth buffer is populated by the depth pre-pass; the forward
            // pass no longer writes depth. `CompareFunction::Always` means
            // wireframe overdraw is intentional — no behavior change needed.
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

        // --- Depth pre-pass pipeline ---
        // Group 0 = uniforms only (view_proj). No fragment stage —
        // wgpu permits `fragment: None` for depth-only pipelines.
        // Depth state writes depth with the standard `Less` test; the
        // forward pipeline then loads this buffer with an `Equal` test.
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

        // --- GPU frame timing (optional) ---
        // Labels are indexed by the `TIMING_PAIR_*` constants so the
        // query-slot assignment is mechanical — no "keep in sync" comment
        // needed at the `*_pass_writes(...)` callsites.
        let frame_timing = if enable_gpu_timing {
            log::info!("[Renderer] GPU timing enabled (POSTRETRO_GPU_TIMING=1)");
            let mut pass_labels = vec![""; TIMING_PAIR_COUNT];
            pass_labels[TIMING_PAIR_CULL] = "cull";
            pass_labels[TIMING_PAIR_DEPTH_PREPASS] = "depth_prepass";
            pass_labels[TIMING_PAIR_FORWARD] = "forward";
            Some(FrameTiming::new(&device, &queue, pass_labels))
        } else {
            None
        };

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
            sh_volume_resources,
            lightmap_resources,
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
        })
    }

    /// Toggle the culling-delta wireframe debug overlay on/off.
    pub fn toggle_wireframe(&mut self) -> bool {
        self.wireframe_enabled = !self.wireframe_enabled;
        log::info!(
            "[Renderer] Wireframe overlay: {}",
            if self.wireframe_enabled { "on" } else { "off" },
        );
        self.wireframe_enabled
    }

    /// Advance the lighting-term isolation mode through its four-step
    /// cycle (Normal → DirectOnly → IndirectOnly → AmbientOnly → Normal).
    /// Takes effect on the next `update_per_frame_uniforms` upload.
    ///
    /// Used to A/B compare direct vs indirect contributions when diagnosing
    /// light leaks: flipping from DirectOnly to IndirectOnly inside a
    /// leaky room reveals whether the bad light is coming from the runtime
    /// direct path or from the baked SH volume.
    pub fn cycle_lighting_isolation(&mut self) -> LightingIsolation {
        self.lighting_isolation = self.lighting_isolation.cycle();
        log::info!(
            "[Renderer] Lighting isolation: {}",
            self.lighting_isolation.label(),
        );
        self.lighting_isolation
    }

    /// Flip between `AutoVsync` and `AutoNoVsync`. Rebuilds the swapchain
    /// via `surface.configure`. Returns the new state (`true` = vsync on).
    ///
    /// Diagnostic-only — triggered by the `Alt+Shift+V` chord so the user
    /// can compare vsync-pinned frametimes against real CPU cost.
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

    /// Whether the surface is currently configured with vsync on.
    /// Read by the title rewrite so the current state is always visible.
    pub fn vsync_enabled(&self) -> bool {
        self.vsync_enabled
    }

    /// Handle window resize. Reconfigures the surface and recreates the depth buffer.
    /// The caller is responsible for updating the view-projection matrix via
    /// `update_per_frame_uniforms` after calling this (the camera owns aspect ratio).
    pub fn resize(&mut self, width: u32, height: u32) {
        if width == 0 || height == 0 {
            return;
        }
        self.surface_config.width = width;
        self.surface_config.height = height;
        self.surface.configure(&self.device, &self.surface_config);
        let (_depth_texture, depth_view) = create_depth_texture(&self.device, width, height);
        self.depth_view = depth_view;
        self.is_surface_configured = true;
    }

    /// Upload the per-frame uniform buffer (view-projection, camera position,
    /// ambient floor, light count, elapsed time, lighting-isolation mode).
    /// The elapsed time is used by the SH animated-light layers to evaluate
    /// curves per frame.
    pub fn update_per_frame_uniforms(&self, view_proj: Mat4, camera_position: Vec3) {
        let time = self.app_start.elapsed().as_secs_f32();
        let data = build_uniform_data(&FrameUniforms {
            view_proj,
            camera_position,
            ambient_floor: self.ambient_floor,
            light_count: self.light_count,
            time,
            lighting_isolation: self.lighting_isolation,
        });
        self.queue.write_buffer(&self.uniform_buffer, 0, &data);
    }

    /// Current ambient floor value (0.0–1.0). Read by the diagnostic
    /// `Alt+Shift+{` / `Alt+Shift+}` chords so each press steps from the
    /// current value rather than a stored target. Will move to the
    /// settings menu when one exists.
    pub fn ambient_floor(&self) -> f32 {
        self.ambient_floor
    }

    /// Update the ambient floor, clamped to [0.0, 1.0]. Takes effect on
    /// the next `update_per_frame_uniforms` upload. Player-facing entry
    /// point is currently the `Alt+Shift+{` / `Alt+Shift+}` diagnostic
    /// chords; will move to the settings menu when one exists.
    pub fn set_ambient_floor(&mut self, value: f32) {
        self.ambient_floor = value.clamp(0.0, 1.0);
    }

    pub fn is_ready(&self) -> bool {
        self.is_surface_configured
    }

    /// Whether the compute cull pipeline is available (level has a non-empty BVH).
    #[allow(dead_code)]
    pub fn has_compute_cull(&self) -> bool {
        self.compute_cull.is_some()
    }

    /// GPU-driven render frame: dispatch the BVH traversal compute shader,
    /// then issue indirect draw calls. This is the only render path.
    ///
    /// `visible` carries the set of potentially-visible cells from the
    /// CPU-side visibility system (portal traversal, PVS, or fallbacks).
    /// The compute shader walks the BVH, frustum-culls each surviving leaf,
    /// checks its cell id against the visible-cell bitmask, and writes one
    /// `DrawIndexedIndirect` per surviving leaf. The render pass consumes
    /// them via `multi_draw_indexed_indirect` (or the singular fallback).
    pub fn render_frame_indirect(&mut self, visible: &VisibleCells, view_proj: Mat4) -> Result<()> {
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

        // Dispatch the BVH traversal compute shader. Portal DFS already
        // produced the visible-cell set on the CPU; the shader writes
        // per-leaf `DrawIndexedIndirect` commands into the indirect buffer
        // in the same command submission — no readback or GPU sync needed.
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

                // Bitmask fingerprint: popcount + xor hash of all words.
                let bm = cull.debug_bitmask_fingerprint();
                if bm != self.debug_prev_bitmask {
                    log::debug!(
                        "[cull f={f}] visible-cell bitmask changed: pop={} hash={:#010x} (was pop={} hash={:#010x})",
                        bm.0, bm.1, self.debug_prev_bitmask.0, self.debug_prev_bitmask.1,
                    );
                    self.debug_prev_bitmask = bm;
                }

                // View-proj fingerprint: XOR of all 16 f32 bit-patterns.
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

                // VisibleCells variant + size.
                let cur_vis = match visible {
                    VisibleCells::Culled(cells) => ("Culled", cells.len()),
                    VisibleCells::DrawAll => ("DrawAll", 0),
                };
                if cur_vis != self.debug_prev_visible {
                    log::debug!(
                        "[cull f={f}] VisibleCells changed: {}(n={}) (was {}(n={}))",
                        cur_vis.0, cur_vis.1, self.debug_prev_visible.0, self.debug_prev_visible.1,
                    );
                    self.debug_prev_visible = cur_vis;
                }
            }
        }

        // --- Depth pre-pass ---
        // Populates the shared depth buffer so the forward pass can run
        // with `depth_compare: Equal` and shade each pixel exactly once
        // (zero shading overdraw). No color attachments, fragment stage
        // is `None`. Uses the same vertex/index buffers + indirect draws
        // as the forward pass but with a layout that only binds group 0.
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
                depth_pass
                    .set_index_buffer(self.index_buffer.slice(..), wgpu::IndexFormat::Uint32);

                if let Some(cull) = &self.compute_cull {
                    // `None` texture callback — the depth pre-pass
                    // pipeline layout binds only group 0, so no
                    // per-bucket texture bind is wanted or legal here.
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
                        // Depth was fully populated by the pre-pass; we
                        // load it unchanged and rely on `depth_compare:
                        // Equal` in the forward pipeline. `StoreOp::Store`
                        // keeps the values around for the wireframe
                        // overlay below.
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
                render_pass.set_vertex_buffer(0, self.vertex_buffer.slice(..));
                render_pass
                    .set_index_buffer(self.index_buffer.slice(..), wgpu::IndexFormat::Uint32);

                if let Some(cull) = &self.compute_cull {
                    // GPU-driven indirect draw path — the only path.
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

        // Culling-delta wireframe overlay: draw ALL BVH leaves color-coded by cull status.
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

// --- Hardcoded view-projection ---

/// Camera at (0, 200, 500) looking at origin.
fn build_default_view_projection(aspect: f32) -> Mat4 {
    let eye = glam::Vec3::new(0.0, 200.0, 500.0);
    let center = glam::Vec3::ZERO;
    let up = glam::Vec3::Y;

    let view = Mat4::look_at_rh(eye, center, up);
    let projection = Mat4::perspective_rh(
        std::f32::consts::FRAC_PI_2, // 90 degree FOV
        aspect,
        0.1,
        4096.0,
    );

    projection * view
}

// --- Byte casting helpers ---

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

/// Build a line-list index buffer from a triangle-list index buffer.
/// Each triangle `[a, b, c]` contributes three line-list edges
/// `[a, b, b, c, c, a]`. Shared edges across triangles are emitted multiple
/// times; this is cheap and fine for a debug overlay. Incomplete trailing
/// indices (not a full triangle) are ignored.
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

// --- Tests ---

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
        // binding 13: array<f32>                 — stride = 4
        let (anim_desc, anim_samples, anim_sh, _count) =
            sh_volume::build_animation_buffers(None);

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
            (
                "anim_sh_data",
                sh_volume::BIND_ANIM_SH_DATA,
                anim_sh.as_slice(),
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
                panic!("forward.wgsl has no buffer at group=3 binding={binding}; \
                        check BIND_* constants match shader @binding decorators");
            }
        }

        // Verify the ShGridInfo uniform payload size.
        let sh_grid_binding = (1 + sh_volume::SH_BAND_COUNT) as u32; // = 10
        let grid_info =
            sh_volume::build_grid_info_bytes([0.0; 3], [1.0; 3], [1, 1, 1], false, 0);
        if let Some(&min) = min_sizes.get(&(3, sh_grid_binding)) {
            assert!(
                grid_info.len() as u64 >= min,
                "sh_grid uniform (group=3, binding={sh_grid_binding}): Rust side \
                 produces {} B but forward.wgsl struct span is {min} B",
                grid_info.len(),
            );
        } else {
            panic!("forward.wgsl has no uniform at group=3 binding={sh_grid_binding}; \
                    check SH_BAND_COUNT matches shader @binding decorators");
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
        assert!(has_vs_main, "depth_prepass.wgsl must export @vertex vs_main");
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
        let data = build_uniform_data(&FrameUniforms {
            view_proj: Mat4::IDENTITY,
            camera_position: camera,
            ambient_floor,
            light_count,
            time: 0.0,
            lighting_isolation: LightingIsolation::Normal,
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

        // Trailing pad (92..96) zero.
        for &b in &data[92..96] {
            assert_eq!(b, 0);
        }
    }
}
