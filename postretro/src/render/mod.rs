// Renderer: GPU init, texture upload, depth pre-pass + forward pipelines, and draw.
// See: context/lib/rendering_pipeline.md

pub mod animated_lightmap;
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

use frame_timing::FrameTiming;
use sh_compose::ShComposeResources;
use sh_volume::ShVolumeResources;
use smoke::SmokePass;

use crate::fx::smoke::{SmokeEmitter, SpriteFrame};

// --- WGSL Shaders ---

// Forward shader source is `forward.wgsl` concatenated with the binding-
// agnostic Catmull-Rom helper in `curve_eval.wgsl`. The helper reads
// `anim_samples` by lexical name — `forward.wgsl` declares that storage
// buffer, and WGSL resolves both function and variable references at module scope
// independently of textual declaration order, so appending the helper after `forward.wgsl` is safe.
const SHADER_SOURCE: &str = concat!(
    include_str!("../shaders/forward.wgsl"),
    "\n",
    include_str!("../shaders/curve_eval.wgsl"),
);

// Wireframe overlay: culling-delta debug visualization. See shader header.
const WIREFRAME_SHADER_SOURCE: &str = include_str!("../shaders/wireframe.wgsl");

// Depth pre-pass: vertex-only shader used to populate the shared depth
// buffer before the forward pass, so the forward pass can run with a
// `depth_compare: Equal` test and zero shading overdraw.
const DEPTH_PREPASS_SHADER_SOURCE: &str = include_str!("../shaders/depth_prepass.wgsl");

// Spot light shadow depth pass: vertex-only shader that transforms world
// geometry by a per-slot light-space matrix uniform, writing Depth32Float
// into one layer of the shadow-map array per draw.
const SPOT_SHADOW_SHADER_SOURCE: &str = include_str!("../shaders/spot_shadow.wgsl");

// --- GPU-timing pair indices ---
//
// Pair index `i` maps to query slots `[2i, 2i+1]` in the `FrameTiming`
// query set (see `frame_timing::FrameTiming`). The labels vector passed
// to `FrameTiming::new` is indexed by these constants so the label
// ordering and the callsite indices can't drift: add an entry here,
// bump the label-vec length below, and the per-pass `*_pass_writes(...)`
// callsites read the right label.
const TIMING_PAIR_CULL: usize = 0;
const TIMING_PAIR_ANIMATED_LM_COMPOSE: usize = 1;
const TIMING_PAIR_DEPTH_PREPASS: usize = 2;
const TIMING_PAIR_FORWARD: usize = 3;
const TIMING_PAIR_COUNT: usize = 4;

// --- Uniform buffer layout ---

/// Per-frame uniform data: view-projection, camera world-space position,
/// ambient floor, light count, elapsed time, lighting-isolation mode,
/// and SH indirect scale.
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
///   88..92   lighting_isolation  (u32, cycles 0..=9; chord Alt+Shift+4)
///   92..96   indirect_scale      (f32, per-frame SH indirect multiplier)
const UNIFORM_SIZE: usize = 96;

/// Lighting-term isolation mode for leak/bleed debugging.
///
/// Cycled by the `Alt+Shift+4` diagnostic chord. The fragment shader branches
/// on this value to enable each lighting term independently, so an A/B compare
/// inside a leaky room pins the bug to exactly one lighting path. `Normal` is
/// the default and matches production shading. The ambient floor always
/// contributes so interior geometry is never pitch black.
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
    /// Advance to the next mode in the cycle, wrapping back to `Normal`.
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

    /// Human-readable label for diagnostics logging.
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

/// Default ambient floor applied when the caller doesn't override it.
/// Provisional value from sub-plan 3; tuned via the ambient-floor slider
/// in the settings menu. The right default is the lowest value where a
/// player can still navigate dark areas.
pub const DEFAULT_AMBIENT_FLOOR: f32 = 0.005;

// --- GPU texture ---

/// A GPU-uploaded material with per-texture bind group (group 1): diffuse
/// texture + sampler + specular texture + material uniform.
struct GpuTexture {
    bind_group: wgpu::BindGroup,
}

/// Upload a texture's pixel data to the GPU and return the texture handle.
/// Callers wrap the returned texture in a bind group via
/// `create_material_bind_group`.
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

/// Extract the R channel from RGBA8 pixel data. Used when uploading the
/// grayscale `_s.png` sidecars as single-channel `R8Unorm` — the diffuse
/// loader expands grayscale PNGs to RGBA8, but only the R channel carries
/// specular data, so the G/B/A bytes are dropped to save 4× VRAM.
fn extract_r_channel(rgba: &[u8]) -> Vec<u8> {
    rgba.iter().step_by(4).copied().collect()
}

/// Byte layout of `MaterialUniform`. Layout mirrors the WGSL struct in
/// `forward.wgsl`:
///   0..4    shininess (f32)
///   4..16   pad
///   16..32  pad (vec3<f32> _pad lands here with align-16, rounding the
///           struct size up to 32 in the uniform address space)
const MATERIAL_UNIFORM_SIZE: usize = 32;

fn build_material_uniform(shininess: f32) -> [u8; MATERIAL_UNIFORM_SIZE] {
    let mut bytes = [0u8; MATERIAL_UNIFORM_SIZE];
    bytes[0..4].copy_from_slice(&shininess.to_le_bytes());
    bytes
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
    /// Chunk-light-list section from the `ChunkLightList` PRL section.
    /// `None` when the section is absent — the runtime binds a dummy
    /// payload and the fragment shader's `has_chunk_grid == 0` guard
    /// iterates the full spec buffer. See `lighting::chunk_list`.
    pub chunk_light_list:
        Option<&'a postretro_level_format::chunk_light_list::ChunkLightListSection>,
    /// Per-face animated-light chunk section (ID 24). Consumed by the
    /// animated-lightmap compose pass's cross-section validator. May be
    /// `None` alongside `animated_light_weight_maps` on maps with zero
    /// animated lights.
    pub animated_light_chunks:
        Option<&'a postretro_level_format::animated_light_chunks::AnimatedLightChunksSection>,
    /// Baked per-animated-light weight maps (ID 25). `None` when the map
    /// has no animated lights — the renderer binds a 1×1 zero atlas.
    pub animated_light_weight_maps: Option<
        &'a postretro_level_format::animated_light_weight_maps::AnimatedLightWeightMapsSection,
    >,
    /// Per-animated-light delta SH probe grids (ID 27). Consumed by the SH
    /// compose pass to accumulate per-light animated contributions on top of
    /// the static base SH bands. `None` when the map has no animated lights —
    /// the compose pass falls back to a base→total copy.
    pub delta_sh_volumes:
        Option<&'a postretro_level_format::delta_sh_volumes::DeltaShVolumesSection>,
    /// Per-texture material, indexed by texture (bucket) index. Drives
    /// per-material shininess uploaded to group 1 binding 3.
    pub texture_materials: &'a [crate::material::Material],
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

    /// SH compose compute pass. Runs once per frame before the depth
    /// pre-pass; composes base SH bands into the total SH bands that
    /// `sh_volume_resources.bind_group` exposes to all SH consumers
    /// (forward, billboard, fog). Stub phase: pure base→total copy.
    sh_compose: ShComposeResources,

    /// Group 4 — directional lightmap atlas resources. Always allocated;
    /// when no Lightmap section is present the bind group binds a 1×1
    /// white/neutral placeholder so the shader path never branches.
    /// See `lighting::lightmap`.
    lightmap_resources: LightmapResources,

    /// Animated-lightmap compose pass. Owns the compute pipeline, storage
    /// atlas, per-frame clear + compose dispatch, and (when no weight-map
    /// section is present) a 1×1 zero atlas bound via
    /// `lightmap_resources` for the forward pass. See
    /// `render::animated_lightmap`.
    animated_lightmap: animated_lightmap::AnimatedLightmapResources,

    /// Group 2 — dynamic lights buffer. Holds the packed GpuLight array
    /// (updated per frame for slot assignment).
    #[allow(dead_code)]
    lights_buffer: wgpu::Buffer,
    /// Per-frame light list cached from the level (for slot assignment).
    #[allow(dead_code)]
    level_lights: Vec<MapLight>,
    /// Per-map-light current effective brightness, in `level_lights` order.
    /// Updated each dirty frame by `set_light_effective_brightness` from the
    /// light bridge. Lights whose animation curve evaluates near zero are
    /// excluded from dynamic spot shadow slot ranking. Empty (or shorter than
    /// `level_lights`) means "no suppression"; missing entries are treated
    /// as 1.0.
    light_effective_brightness: Vec<f32>,
    /// Last camera position uploaded via `update_per_frame_uniforms`,
    /// cached so the shadow pass can re-rank lights on its own clock.
    last_camera_position: Vec3,
    /// Spot shadow pool: 8 depth textures with per-frame slot assignment,
    /// plus the sampler, matrix buffer, and bind group consumed by group 5
    /// of the forward shader.
    spot_shadow_pool: SpotShadowPool,
    /// Vertex-stage uniform buffer holding the light-space matrix for the
    /// currently-rendering shadow slot. Rebound per slot via a dynamic
    /// offset into a single buffer sized for `SHADOW_POOL_SIZE` slots.
    shadow_vs_uniform_buffer: wgpu::Buffer,
    /// Bind group for group 0 of the shadow depth pipeline. Uses dynamic
    /// offset so the slot index selects the matrix for each depth pass.
    shadow_vs_bind_group: wgpu::BindGroup,
    /// Depth-only pipeline that renders static geometry into a shadow map
    /// slot using `spot_shadow.wgsl`.
    shadow_depth_pipeline: wgpu::RenderPipeline,
    /// Stride between per-slot entries in `shadow_vs_uniform_buffer`,
    /// rounded up to `min_uniform_buffer_offset_alignment`.
    shadow_vs_stride: u32,

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

    /// Billboard sprite pass. Always allocated; idle (no draw) on maps with
    /// no registered collections. Collections are registered at level load
    /// via `register_smoke_collection`. Draw ordering: after opaque forward
    /// pass, before wireframe overlay. See §7.4.
    smoke_pass: SmokePass,

    /// Scratch buffer reused each frame when packing sprite instance bytes
    /// for the GPU upload. Owns its capacity across frames.
    smoke_pack_scratch: Vec<u8>,
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

        // Raise `max_bind_groups` above the WebGPU downlevel default of 4.
        // The forward pipeline binds groups 0–4 today (camera, material,
        // lights + spec stack, SH volume, lightmap); `lighting-spot-shadows`
        // will add a shadow-pool group. 8 is the WebGPU maximum and is
        // supported on every desktop backend we target.
        // NOTE: the forward fragment shader binds exactly 16 sampled textures
        // (3 material [diffuse, spec, normal — normal at binding 4 was added by
        // the normal-maps plan; previously 15] + 9 SH bands + 3 lightmap +
        // 1 shadow depth), which is the WebGPU spec floor for
        // max_sampled_textures_per_shader_stage. Adding another sampled texture
        // to the forward pass requires bumping this limit (all desktop backends
        // support ≥32) or restructuring the SH bindings (e.g., collapsing the
        // 9 band textures into a texture array).
        let required_limits = wgpu::Limits {
            max_bind_groups: 8,
            // SH compose pass writes 9 storage textures (one per SH band).
            // WebGPU spec floor is 4; all desktop backends (Metal, Vulkan, DX12) support ≥9.
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
            indirect_scale: 1.0,
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

        // Bind group layout for group 1: per-material.
        //   0 = diffuse texture
        //   1 = base sampler (shared across diffuse + specular + normal)
        //   2 = specular texture (R8 in .r channel; 1×1 black fallback)
        //   3 = MaterialUniform (shininess)
        //   4 = normal map texture (Rgba8Unorm tangent-space; 1×1 +Z fallback)
        //       Decode in shader: n = sample.rgb * 2.0 - 1.0.
        //       See context/lib/resource_management.md §4.3.
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

        // Bind group layout for group 2: direct-light + specular/chunk buffers.
        //   0 = dynamic GpuLight array (diffuse direct path)
        //   1 = per-light influence volumes
        //   2 = spec-only static light buffer (lighting-chunk-lists/ Task B)
        //   3 = ChunkGridInfo uniform (has_chunk_grid = 0 → full-buffer fallback)
        //   4 = per-chunk offset table (u32, u32) pairs
        //   5 = flat chunk index list (u32 into spec buffer)
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

        // Cache the level's light list for per-frame slot assignment.
        let level_lights = geometry.map(|g| g.lights.to_vec()).unwrap_or_default();

        // Check for dynamic directional lights and warn (not supported).
        for (idx, light) in level_lights.iter().enumerate() {
            if light.is_dynamic && light.light_type == crate::prl::LightType::Directional {
                log::warn!(
                    "[Renderer] Dynamic directional light (light_sun) at index {} found — not supported. \
                     Will render unshadowed (diffuse + specular only).",
                    idx
                );
            }
        }

        // Pack the map's lights into GPU bytes and create the storage
        // buffer. wgpu rejects a zero-size storage buffer, so we pad to a
        // single dummy record when there are no lights at all; the
        // shader's `light_count` loop bound stays at 0 so the dummy is
        // never read.
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

        // Initialize the spot shadow pool. The group 5 bind group layout
        // is owned here and passed into the pool so the forward pipeline
        // layout and the pool's bind group share a single definition.
        let spot_shadow_bgl = SpotShadowPool::bind_group_layout(&device);
        let spot_shadow_pool = SpotShadowPool::new(&device, &spot_shadow_bgl);
        log::info!(
            "[Renderer] Spot shadow pool initialized (8 × 1024×1024 Depth32Float = 32 MiB VRAM)"
        );

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

        // Spec-only light buffer (group 2 binding 2). Populated from the
        // same static light list; dynamic lights are filtered out by
        // `pack_spec_lights`. Dummy 1-record payload when no static lights
        // remain (empty map, or every light flagged dynamic) so the storage
        // binding is never zero-sized.
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

        // Shared 1×1 black specular fallback. Mirrors the `_n` normal-map
        // convention: absent specular → zeros in the R channel → no
        // highlight without any shader branching. See
        // context/lib/resource_management.md §4.1.
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

        // Shared 1×1 neutral-normal placeholder. Tangent-space +Z encoded in
        // Rgba8Unorm: (0, 0, 1) → (127, 127, 255) in u8. Decode in the shader
        // is `n = sample.rgb * 2.0 - 1.0`, which round-trips to (≈0, ≈0, ≈1).
        // Engine-lifetime: allocated alongside the diffuse-checkerboard and
        // black-spec placeholders; survives level swaps because the renderer
        // owns it directly. See context/lib/resource_management.md §4.3.
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

        // Upload textures to GPU and build per-material bind groups.
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

        // If we have no textures at all, create a single placeholder so we always
        // have something to bind.
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
            level_lights.len(),
        );

        // SH compose pass — populates the total SH bands consumers sample
        // through `sh_volume_resources.bind_group`. Stub: copies base→total
        // each frame. Always allocated; cost is irrelevant for the typical
        // probe-grid shape.
        let sh_compose = ShComposeResources::new(
            &device,
            &sh_volume_resources,
            geometry.and_then(|g| g.sh_volume),
            geometry.and_then(|g| g.delta_sh_volumes),
            &uniform_bind_group_layout,
        );

        // Animated-lightmap compose pass. Owns the compute pipeline, the
        // Rgba16Float storage atlas, and the dispatch-tile buffer. When the
        // PRL has no weight-map section (zero animated lights), this
        // returns a module-owned 1×1 zero texture whose view is still
        // bound on group 4 — the forward shader path never branches.
        // The construction runs the cross-section validator and returns
        // an error for an inconsistent section; we surface it as an
        // initialization panic (map loads are not recoverable here).
        // Env-var-gated debug visualization. Read once at init; no per-frame
        // string reads. See `AnimatedLmDebugConfig::from_env`.
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
        // Always created — placeholder 1×1 white/neutral bindings are used
        // when the level has no Lightmap section. See `lighting::lightmap`.
        // Layout is built up front so the pipeline layout can reference it
        // before the bind group is populated. The animated-contribution
        // atlas is bound on this same group at binding 3; its view comes
        // from `animated_lightmap` (real texture or 1×1 zero dummy).
        let lightmap_bind_group_layout = crate::lighting::lightmap::bind_group_layout(&device);
        let lightmap_resources = LightmapResources::new(
            &device,
            &queue,
            geometry.and_then(|g| g.lightmap),
            &lightmap_bind_group_layout,
            &animated_lightmap.forward_view,
        );

        // Pipeline layout. Group 2 is the direct-lighting storage buffer
        // introduced in sub-plan 3 of the lighting foundation; group 3 is
        // the SH irradiance volume introduced in sub-plan 6; group 4 is the
        // baked directional lightmap atlas (`lighting-lightmaps`); group 5
        // is the dynamic spot-shadow pool (`lighting-spot-shadows`).
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

        // --- Spot shadow depth pipeline ---
        // One depth-only pipeline shared across all 8 slots. The slot's
        // light-space matrix is selected per-draw via a dynamic offset
        // into `shadow_vs_uniform_buffer`. Depth bias matches the plan's
        // tuning (constant=2, slope=1.5) to suppress self-shadow acne
        // without introducing Peter-Panning for the hard-edged look.
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
                    // Match the depth pre-pass: back-face cull on single-sided
                    // brushes, with a conservative depth bias to keep acne
                    // off the lit side. Front-face culling is the typical
                    // "trade acne for Peter Panning" swap but we want the
                    // hard-edged retro look without Peter Panning.
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

        // Per-slot uniform buffer. Each slot gets its own `mat4x4<f32>`
        // aligned to the device's min-uniform-offset so dynamic-offset
        // binds are legal across adapters.
        let min_ubo_align = device.limits().min_uniform_buffer_offset_alignment.max(64);
        let shadow_vs_stride = min_ubo_align; // one 64-byte mat4 rounded up.
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

        // --- GPU frame timing (optional) ---
        // Labels are indexed by the `TIMING_PAIR_*` constants so the
        // query-slot assignment is mechanical — no "keep in sync" comment
        // needed at the `*_pass_writes(...)` callsites.
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

        // --- Billboard sprite pipeline (env_smoke_emitter pass) ---
        // See: context/lib/rendering_pipeline.md §7.4
        let smoke_pass = SmokePass::new(
            &device,
            surface_format,
            DEPTH_FORMAT,
            &uniform_bind_group_layout,
            &lighting_bind_group_layout,
            &sh_volume_resources.bind_group_layout,
        );

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
            smoke_pack_scratch: Vec::new(),
        })
    }

    /// Register a smoke sprite sheet collection. Called once per unique
    /// `env_smoke_emitter.collection` at level load. `frames` is the list of
    /// `smoke_NN.png` animation frames in order. `spec_intensity` and
    /// `lifetime` are the emitter's per-collection lighting and timing
    /// parameters — when multiple emitters share a collection the first
    /// caller's parameters win (the animation frame timing is a
    /// per-collection property, not per-emitter).
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

    /// Toggle the culling-delta wireframe debug overlay on/off.
    pub fn toggle_wireframe(&mut self) -> bool {
        self.wireframe_enabled = !self.wireframe_enabled;
        log::info!(
            "[Renderer] Wireframe overlay: {}",
            if self.wireframe_enabled { "on" } else { "off" },
        );
        self.wireframe_enabled
    }

    /// Advance the lighting-term isolation mode through its nine-step cycle
    /// (Normal → DirectOnly → IndirectOnly → AmbientOnly → LightmapOnly →
    /// StaticSHOnly → AnimatedDeltaOnly → DynamicOnly → SpecularOnly → Normal).
    /// Takes effect on the next `update_per_frame_uniforms` upload.
    ///
    /// Used to A/B compare individual lighting terms when diagnosing leaks:
    /// each "*Only" mode shows the ambient floor plus a single contribution,
    /// while DirectOnly / IndirectOnly group terms by category.
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
    pub fn update_per_frame_uniforms(&mut self, view_proj: Mat4, camera_position: Vec3) {
        let time = self.app_start.elapsed().as_secs_f32();
        let data = build_uniform_data(&FrameUniforms {
            view_proj,
            camera_position,
            ambient_floor: self.ambient_floor,
            light_count: self.light_count,
            time,
            lighting_isolation: self.lighting_isolation,
            indirect_scale: 1.0,
        });
        self.queue.write_buffer(&self.uniform_buffer, 0, &data);
        self.last_camera_position = camera_position;

        // Flush any pending `active` toggles on animated lights. Scripting
        // calls `set_active` during game-logic update (frame order: Input →
        // Game → Audio → Render). The compose pass (Task 5) and this frame's
        // SH-volume fragment pass both read from the descriptor buffer, so
        // the upload must land before either runs.
        self.sh_volume_resources
            .animation
            .upload_descriptors_if_dirty(&self.queue);
    }

    /// Script-facing: toggle an animated light's `active` flag. `slot` is the
    /// animated-light index from the SH section. The change is flushed to the
    /// GPU on the next `update_per_frame_uniforms` call.
    #[allow(dead_code)]
    pub fn set_animated_light_active(&mut self, slot: usize, active: bool) {
        self.sh_volume_resources.animation.set_active(slot, active);
    }

    /// Upload a bridge-produced light byte buffer into the direct-lights
    /// storage buffer. Called by the game layer between Game Logic and Render
    /// once per frame with the light bridge's repacked bytes (see
    /// `crate::scripting::systems::light_bridge`). Bytes must match the
    /// authored-light layout the renderer booted with — same stride, same
    /// light count, same order.
    ///
    /// Frame-ordering constraint: this must run **before**
    /// `update_dynamic_light_slots`, which rewrites the same buffer with
    /// per-frame shadow slot assignments. Slot assignment uses the
    /// bridge-produced colors/intensities as its input, not the reverse.
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

    /// Upload a bridge-produced scripted-light `AnimationDescriptor` byte
    /// buffer into the per-map-light descriptor storage buffer (group 3,
    /// binding `BIND_SCRIPTED_LIGHT_DESCRIPTORS`). One 48-byte record per
    /// map light, in map-light-index order. Lights without an active
    /// animation carry the sentinel descriptor (all zeros); the forward
    /// shader's light loop keys on `is_active` to decide whether to evaluate
    /// a curve or pass through the static `GpuLight` color.
    ///
    /// Bytes must match the pre-allocated buffer size —
    /// `level_lights.len() * ANIMATION_DESCRIPTOR_SIZE`. A mismatched length
    /// logs a warning and returns without uploading (defensive: Plan 2
    /// Sub-plan 4 guarantees the bridge emits exactly that count, but we fail
    /// soft rather than crash the frame if the invariant slips).
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

    /// Upload scripted animation samples into the `anim_samples` GPU buffer,
    /// starting at the scripted-region offset (immediately after FGD samples).
    /// Called once per dirty frame, after `upload_bridge_descriptors`.
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

    /// Byte offset within `anim_samples` where the scripted-animation region
    /// starts. Divide by 4 to get the float index; pass to
    /// `LightBridge::populate_from_level` as `fgd_sample_float_count`.
    pub fn scripted_sample_byte_offset(&self) -> usize {
        self.sh_volume_resources.scripted_sample_byte_offset
    }

    /// Access the cached level-light list. Called at level-load time by the
    /// game layer to seed the light bridge so the bridge can populate the
    /// scripting entity registry.
    pub fn level_lights(&self) -> &[MapLight] {
        &self.level_lights
    }

    /// Cache per-map-light effective brightness from the light bridge for
    /// the next call to `update_dynamic_light_slots`. Called once per dirty
    /// frame from the game layer alongside the other `upload_bridge_*`
    /// methods. An empty slice clears the cache (treated as "no suppression").
    pub fn set_light_effective_brightness(&mut self, effective_brightness: &[f32]) {
        self.light_effective_brightness.clear();
        self.light_effective_brightness
            .extend_from_slice(effective_brightness);
    }

    /// Update the dynamic lights buffer with per-frame shadow slot assignments.
    ///
    /// Ranks visible dynamic spot lights by influence area and assigns slots.
    /// Rewrites the lights buffer with slot indices. Called once per frame
    /// before rendering.
    ///
    /// `effective_brightness` is a per-map-light current brightness in
    /// `level_lights` order, evaluated CPU-side from any active animation
    /// curve (see `light_bridge`). Lights whose effective brightness is below
    /// 0.01 are excluded from the candidate list so an animated-dark light
    /// does not waste one of the 8 shadow slots. An empty (or short) slice
    /// is treated as all-1.0 (no suppression) — the engine's first frame
    /// runs before the bridge produces an update.
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

        // Build the per-light visibility mask. An empty `visible_leaf_mask`
        // is the DrawAll sentinel (empty world, fallback paths) — every
        // light with a real leaf assignment is eligible. Otherwise consult
        // the per-leaf bitmask. Lights with `leaf_index ==
        // ALPHA_LIGHT_LEAF_UNASSIGNED` are always culled (compile-time
        // sentinel for "could not assign to a non-solid leaf"). Lights
        // whose animated brightness is below the suppression threshold
        // are folded in so `rank_lights` drops them.
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

        // Rank lights and get slot assignments.
        let slot_assignment = SpotShadowPool::rank_lights(
            &self.level_lights,
            camera_position,
            camera_near_clip,
            &visible_lights,
            light_influences,
        );

        // Repack lights with slot assignments.
        let lights_data = pack_lights_with_slots(&self.level_lights, &slot_assignment);
        self.queue
            .write_buffer(&self.lights_buffer, 0, &lights_data);

        // Compute the light-space matrix for each assigned slot and upload
        // to both the fragment-side storage buffer (group 5 binding 2) and
        // the vertex-side per-slot uniform buffer (shadow pipeline).
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
    pub fn render_frame_indirect(
        &mut self,
        visible: &VisibleCells,
        visible_leaf_mask: &[bool],
        view_proj: Mat4,
        emitters: &[&SmokeEmitter],
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
                        bm.0,
                        bm.1,
                        self.debug_prev_bitmask.0,
                        self.debug_prev_bitmask.1,
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
                        cur_vis.0,
                        cur_vis.1,
                        self.debug_prev_visible.0,
                        self.debug_prev_visible.1,
                    );
                    self.debug_prev_visible = cur_vis;
                }
            }
        }

        // --- Animated lightmap compose pass ---
        // Compose the per-frame animated-contribution atlas. Runs after
        // BVH cull (independent work, no data dep) and before the depth
        // pre-pass so the compose→sample barrier lands before any forward
        // fragment samples the atlas. wgpu infers the storage→sampled
        // transition from the bind-group usage change. `visible` is
        // forwarded so the compose dispatch filters tiles against the
        // current frame's visible-cell set — see
        // `animated_lightmap::AnimatedLightmapResources::dispatch`.
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

        // --- SH compose pass ---
        // Composes the base SH bands (and, in later phases, animated
        // per-light deltas) into the total SH bands that all SH consumers
        // sample via `sh_volume_resources.bind_group`. Stub phase: pure
        // base→total copy. Encoded before the depth pre-pass so the
        // storage-write → sampled-read barrier resolves before any forward
        // fragment samples SH.
        self.sh_compose
            .dispatch(&mut encoder, &self.uniform_bind_group);

        // --- Dynamic spot shadow slot update + depth pass ---
        // Rank dynamic spot lights, upload slot indices + light-space
        // matrices, then render a depth-only pass per allocated slot.
        // The pass draws all static geometry into the slot's depth layer;
        // per-light BVH culling would be a future optimization.
        // Take the cached bridge-produced brightness out for the call so we
        // don't borrow `self` immutably while also borrowing it mutably; put
        // it back afterwards so the next frame reuses the same allocation.
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
                depth_pass.set_index_buffer(self.index_buffer.slice(..), wgpu::IndexFormat::Uint32);

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
                render_pass.set_bind_group(5, &self.spot_shadow_pool.bind_group, &[]);
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

        // --- Billboard sprite pass (env_smoke_emitter) ---
        // After the opaque forward pass, before the wireframe overlay. Alpha
        // additive; depth test enabled, depth write disabled. Batched by
        // sprite-sheet collection: one draw per collection. See §7.4.
        if self.smoke_pass.has_any_sheet() && !emitters.is_empty() {
            let mut collections: Vec<String> = emitters
                .iter()
                .map(|e| e.collection().to_string())
                .collect();
            collections.sort();
            collections.dedup();

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
            for collection in &collections {
                let scratch = &mut self.smoke_pack_scratch;
                scratch.clear();
                for e in emitters.iter().filter(|e| e.collection() == collection) {
                    e.pack_instances(scratch);
                }
                if scratch.is_empty() {
                    continue;
                }
                self.smoke_pass
                    .record_draw(&self.queue, &mut smoke_pass_enc, collection, scratch);
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
        let data = build_uniform_data(&FrameUniforms {
            view_proj: Mat4::IDENTITY,
            camera_position: camera,
            ambient_floor,
            light_count,
            time: 0.0,
            lighting_isolation: LightingIsolation::Normal,
            indirect_scale: 1.0,
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

        // indirect_scale at bytes 92..96 (passed 1.0).
        let scale = f32::from_ne_bytes(data[92..96].try_into().unwrap());
        assert_eq!(scale, 1.0);
    }
}
