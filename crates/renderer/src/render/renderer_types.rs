// Core renderer data types: the Renderer struct, LevelGeometry, GpuTexture,
// ClearColor, and shared rendering constants.
// See: context/lib/rendering_pipeline.md

use super::*;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ClearColor {
    pub r: f64,
    pub g: f64,
    pub b: f64,
    pub a: f64,
}

impl From<ClearColor> for wgpu::Color {
    fn from(color: ClearColor) -> Self {
        Self {
            r: color.r,
            g: color.g,
            b: color.b,
            a: color.a,
        }
    }
}

/// Opaque handle for an acquired swapchain texture ready to present.
pub struct PresentHandle {
    output: wgpu::SurfaceTexture,
}

impl PresentHandle {
    pub(super) fn new(output: wgpu::SurfaceTexture) -> Self {
        Self { output }
    }

    pub(super) fn surface_view(&self) -> wgpu::TextureView {
        self.output
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default())
    }

    pub(super) fn present(self) {
        self.output.present();
    }
}

/// Minimum useful ambient. Default value seeded into the Diagnostics panel slider on first open.
pub const DEFAULT_AMBIENT_FLOOR: f32 = 0.0;

/// Full SH contribution weight — production default. Default value seeded into the Diagnostics panel slider on first open.
pub const DEFAULT_INDIRECT_SCALE: f32 = 0.33;

/// Full dynamic baked-static-direct SH weight — production default. Seeded into
/// the Diagnostics panel slider on first open.
pub const DEFAULT_DYNAMIC_DIRECT_SCALE: f32 = 1.0;

/// Renderer-owned headroom for dynamic lights spawned after level install.
/// All index-parallel direct-light buffers reserve this many records so the
/// game layer can append lights without reallocating or rebinding GPU state.
pub const RUNTIME_DYNAMIC_LIGHT_RESERVE: usize = 256;

pub(crate) struct GpuTexture {
    pub(super) bind_group: wgpu::BindGroup,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[cfg_attr(not(feature = "dev-tools"), allow(dead_code))]
pub enum WorldWireframeMode {
    Off,
    CullStatusTrianglesAlwaysOnTop,
    VisibleTrianglesDepthTested,
}

#[cfg_attr(not(feature = "dev-tools"), allow(dead_code))]
impl WorldWireframeMode {
    pub const ALL_VARIANTS: [Self; 3] = [
        Self::Off,
        Self::CullStatusTrianglesAlwaysOnTop,
        Self::VisibleTrianglesDepthTested,
    ];

    pub const fn label(self) -> &'static str {
        match self {
            Self::Off => "Off",
            Self::CullStatusTrianglesAlwaysOnTop => "Cull-status triangles (all BVH leaves, x-ray)",
            Self::VisibleTrianglesDepthTested => "CPU-visible triangles (depth-tested)",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[cfg_attr(not(feature = "dev-tools"), allow(dead_code))]
pub enum BvhOverlayColorMode {
    CellId,
}

#[cfg_attr(not(feature = "dev-tools"), allow(dead_code))]
impl BvhOverlayColorMode {
    pub const ALL_VARIANTS: [Self; 1] = [Self::CellId];

    pub const fn label(self) -> &'static str {
        match self {
            Self::CellId => "Stable cell ID",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[cfg_attr(not(feature = "dev-tools"), allow(dead_code))]
pub enum BvhOverlayDepthMode {
    DepthTested,
    XRayAlwaysOnTop,
}

#[cfg_attr(not(feature = "dev-tools"), allow(dead_code))]
impl BvhOverlayDepthMode {
    pub const ALL_VARIANTS: [Self; 2] = [Self::DepthTested, Self::XRayAlwaysOnTop];

    pub const fn label(self) -> &'static str {
        match self {
            Self::DepthTested => "Depth-tested",
            Self::XRayAlwaysOnTop => "X-ray / always on top",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[cfg_attr(not(feature = "dev-tools"), allow(dead_code))]
pub struct BvhOverlayBudget {
    pub max_boxes: usize,
    pub stride: usize,
    pub visible_cells_only: bool,
}

impl Default for BvhOverlayBudget {
    fn default() -> Self {
        Self {
            max_boxes: 512,
            stride: 1,
            visible_cells_only: false,
        }
    }
}

#[cfg_attr(not(feature = "dev-tools"), allow(dead_code))]
impl BvhOverlayBudget {
    pub fn sanitized(self) -> Self {
        Self {
            max_boxes: self.max_boxes,
            stride: self.stride.max(1),
            visible_cells_only: self.visible_cells_only,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[cfg_attr(not(feature = "dev-tools"), allow(dead_code))]
pub struct BvhOverlayState {
    pub visible: bool,
    pub color_mode: BvhOverlayColorMode,
    pub depth_mode: BvhOverlayDepthMode,
    pub budget: BvhOverlayBudget,
}

impl Default for BvhOverlayState {
    fn default() -> Self {
        Self {
            visible: false,
            color_mode: BvhOverlayColorMode::CellId,
            depth_mode: BvhOverlayDepthMode::DepthTested,
            budget: BvhOverlayBudget::default(),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[cfg_attr(not(feature = "dev-tools"), allow(dead_code))]
pub struct CellOverlayState {
    pub visible: bool,
    pub depth_mode: BvhOverlayDepthMode,
}

impl Default for CellOverlayState {
    fn default() -> Self {
        Self {
            visible: false,
            depth_mode: BvhOverlayDepthMode::DepthTested,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[cfg_attr(not(feature = "dev-tools"), allow(dead_code))]
pub struct PortalOverlayState {
    pub visible: bool,
    pub depth_mode: BvhOverlayDepthMode,
}

impl Default for PortalOverlayState {
    fn default() -> Self {
        Self {
            visible: false,
            depth_mode: BvhOverlayDepthMode::DepthTested,
        }
    }
}

#[cfg(feature = "dev-tools")]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AgentOverlayState {
    pub enabled: bool,
    pub paths: bool,
    pub velocities: bool,
    pub destinations: bool,
    pub labels: bool,
}

#[cfg(feature = "dev-tools")]
impl Default for AgentOverlayState {
    fn default() -> Self {
        Self {
            enabled: false,
            paths: true,
            velocities: true,
            destinations: true,
            labels: true,
        }
    }
}

/// Which camera-cull path ran for a frame, surfaced to the Spatial diagnostics
/// tab. Diagnostic only — never gates behavior.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CameraCullPath {
    /// Visible-cell candidate cull (valid `CellDrawIndex` + `Culled` + portal
    /// provenance). `candidate_leaves` is the gathered candidate count.
    Candidate { candidate_leaves: u32 },
    /// Whole-BVH tree walk (`DrawAll`, non-portal `Culled` fallback, or an
    /// out-of-range visible cell id).
    TreeWalk,
}

/// CPU-derived camera-cull diagnostics for the Spatial tab. Refreshed after
/// camera visibility is known and before debug UI renders. Exposes
/// candidate-vs-total leaves so a future optional indirect-compaction pass is a
/// measured decision, not a guess. Not a perf gate; reads no GPU buffers.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CameraCullDiagnostics {
    /// Which path the frame used.
    pub path: CameraCullPath,
    /// Total BVH leaves in the level (the tree walk's working set).
    pub total_leaves: u32,
    /// Leaves submitted this frame (passed the frustum predicate and, on the
    /// candidate path, were gathered from visible cells). CPU-derived for both
    /// cull paths so it matches the current Spatial diagnostics frame.
    pub submitted_leaves: u32,
}

impl Default for CameraCullDiagnostics {
    fn default() -> Self {
        Self {
            path: CameraCullPath::TreeWalk,
            total_leaves: 0,
            submitted_leaves: 0,
        }
    }
}

impl CameraCullDiagnostics {
    /// Candidate leaf count for the frame, or `None` on the tree-walk path
    /// (where there is no candidate gather).
    #[cfg(any(feature = "dev-tools", test))]
    pub fn candidate_leaves(&self) -> Option<u32> {
        match self.path {
            CameraCullPath::Candidate { candidate_leaves } => Some(candidate_leaves),
            CameraCullPath::TreeWalk => None,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[cfg_attr(not(feature = "dev-tools"), allow(dead_code))]
pub enum SpatialCellSetDiagnostics {
    DrawAll,
    Cells { count: u32 },
}

impl SpatialCellSetDiagnostics {
    #[cfg(any(feature = "dev-tools", test))]
    pub fn from_visible_cells(cells: &postretro_visibility::VisibleCells) -> Self {
        match cells {
            postretro_visibility::VisibleCells::Culled(cells) => Self::Cells {
                count: cells.len() as u32,
            },
            postretro_visibility::VisibleCells::DrawAll => Self::DrawAll,
        }
    }

    #[cfg(any(feature = "dev-tools", test))]
    pub fn from_cell_slice(cells: &[u32]) -> Self {
        Self::Cells {
            count: cells.len() as u32,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
#[cfg_attr(not(feature = "dev-tools"), allow(dead_code))]
pub enum LocatorDiagnostics {
    NoLevel,
    Trace(postretro_level_loader::CellLocatorTrace),
}

#[derive(Clone, Debug, PartialEq)]
#[cfg_attr(not(feature = "dev-tools"), allow(dead_code))]
pub struct SpatialDiagnostics {
    pub current_cell: Option<u32>,
    pub portal_drawable_cells: SpatialCellSetDiagnostics,
    pub fog_reachable_cells: SpatialCellSetDiagnostics,
    pub locator: LocatorDiagnostics,
}

impl Default for SpatialDiagnostics {
    fn default() -> Self {
        Self {
            current_cell: None,
            portal_drawable_cells: SpatialCellSetDiagnostics::DrawAll,
            fog_reachable_cells: SpatialCellSetDiagnostics::Cells { count: 0 },
            locator: LocatorDiagnostics::NoLevel,
        }
    }
}

/// Hardware anisotropy cap for the Post Retro filtering pool. wgpu 29 requires
/// `anisotropy_clamp >= 1`; 16 is the common ceiling exposed by desktop adapters
/// and the visual point of diminishing returns for grazing-angle sharpness.
pub const POST_RETRO_ANISO_CLAMP: u16 = 16;

pub struct LevelGeometry<'a> {
    pub vertices: &'a [postretro_render_data::geometry::WorldVertex],
    pub indices: &'a [u32],
    pub bvh: &'a BvhTree,
    pub lights: &'a [MapLight],
    pub light_influences: &'a [LightInfluence],
    /// `None` means no `OctahedralShVolumeSection`; renderer binds dummy
    /// 1×1 atlas resources and shader skips octahedral SH sampling.
    pub sh_volume: Option<&'a postretro_level_format::sh_volume::OctahedralShVolumeSection>,
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
    /// Dense baked DIRECT static-light octahedral atlas sampled by the dynamic
    /// pipelines (mesh + billboard). `None` → renderer binds a 4×4 BC6H zero
    /// dummy and the dynamic shaders skip the direct sample (indirect-only).
    pub direct_sh_volume:
        Option<&'a postretro_level_format::direct_sh_volume::DirectShVolumeSection>,
    /// Sparse per-selected-light direct SH deltas used to compose the sampled
    /// direct atlas when static lights promote into shadow pools.
    pub direct_sh_delta_volumes:
        Option<&'a postretro_level_format::direct_sh_delta_volumes::DirectShDeltaVolumesSection>,
    /// Sparse per-animated-baked-light direct SH deltas. Its descriptor map and
    /// CSR light indices use the independent AnimatedBakedLights namespace.
    pub animated_direct_sh_delta_volumes: Option<
        &'a postretro_level_format::animated_direct_sh_delta_volumes::AnimatedDirectShDeltaVolumesSection,
    >,
    /// Normal-free static direct scatter for billboard receivers. The renderer
    /// selects its legacy fallback when this is absent or cannot fit the GPU.
    pub billboard_direct_scatter_volume: Option<
        &'a postretro_level_format::billboard_direct_scatter_volume::BillboardDirectScatterVolumeSection,
    >,
    /// Dense section-48 animated deltas paired by the loader with section 45.
    pub animated_billboard_direct_scatter_delta_volumes: Option<
        &'a postretro_level_format::animated_billboard_direct_scatter_delta_volumes::AnimatedBillboardDirectScatterDeltaVolumesSection,
    >,
    /// Selection-order list of global level-light indices eligible for runtime
    /// entity-shadow promotion.
    pub entity_shadow_lights: &'a [u32],
    /// Optional per-selected-light baked visibility masks for promoted
    /// static-light entity shadows onto world surfaces.
    pub shadowmask_atlas:
        Option<&'a postretro_level_format::shadowmask_atlas::ShadowmaskAtlasSection>,
    /// `None` → no SDF static-occluder atlas; runtime SDF shadow pass disabled.
    /// An empty-geometry section (zero grid dims) is treated the same way.
    pub sdf_atlas: Option<&'a postretro_level_format::sdf_atlas::SdfAtlasSection>,
    /// Whether baked static-direct lightmap samples already include static-light
    /// visibility. `Shadowed` atlases contain the visibility term; `Unshadowed`
    /// atlases leave it for runtime SDF shadowing so the forward pass does not
    /// double-count static-light occlusion. Legacy PRLs default to `Shadowed`.
    pub lightmap_mode: postretro_level_loader::LightmapMode,
    /// Per-cell BVH-leaf draw index (PRL section 37), cross-validated at load.
    /// `None` only for no installed level or an empty-BVH map. Non-empty BVHs
    /// require this index at load; missing or invalid data is a load error.
    /// Whole-BVH tree-walk fallback is a per-frame runtime path for `DrawAll`,
    /// non-portal visibility, out-of-range gathered cell ids, or no candidate
    /// cull pipeline.
    pub cell_draw_index: Option<&'a postretro_level_loader::CellDrawIndex>,
    /// Runtime-loaded local-space kinematic brush mover geometry (PRL section
    /// 43). Uploaded into a renderer-owned dynamic-object pass, never into the
    /// static world BVH/indirect buffers.
    pub kinematic_geometry: Option<&'a postretro_level_loader::KinematicGeometry>,
    pub texture_materials: &'a [postretro_render_data::material::Material],
}

/// First-guess promoted-slot budgets (cache VRAM ≈ 32 MiB spot + 12 MiB cube),
/// tuned on §10 target hardware. See the static-light-entity-shadows plan.
pub(crate) const MAX_PROMOTED_SPOT: usize = 8;
pub(crate) const MAX_PROMOTED_CUBE: usize = 2;

/// Which dynamic shadow pool a promoted static light's world depth is cached
/// into and rendered from.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum PromotedShadowPoolKind {
    Spot,
    Cube,
}

/// One static light promoted into a shadow pool slot this frame. Pinned Task-4
/// contract (see the static-light-entity-shadows plan); consumed by Tasks 5-6.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct PromotedStaticLightRecord {
    /// Index into the level's full light array.
    pub global_light_index: u32,
    /// 0-based position in `EntityShadowLights` order — the space the delta
    /// `affinity_lights` and the per-selected-light weight buffer are indexed by.
    pub selection_index: u32,
    /// Which shadow pool (spot or cube) the light is promoted into.
    pub pool_kind: PromotedShadowPoolKind,
    /// Slot index within that pool.
    pub slot: u32,
    /// Promotion crossfade weight w ∈ [0,1] — 0 is fully baked SH, 1 is fully
    /// the runtime pool term (see rendering_pipeline.md §4 "Promoted static lights").
    pub weight: f32,
}

/// Per-candidate-light promotion tracking across frames: current weight,
/// sticky hold-over time, and which pool slot (if any) the light occupies.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct PromotedStaticLightState {
    pub weight: f32,
    pub sticky_remaining: f32,
    pub pool_kind: Option<PromotedShadowPoolKind>,
    pub slot: u32,
    pub last_score: f32,
}

/// Renderer GPU state. Windowed construction begins with a boot splash before
/// the heavier pipelines/resources are built; offscreen construction builds the
/// full renderer immediately.
///
/// Phase split (see context/lib/boot_sequence.md §1, rendering_pipeline.md §7.8):
/// - **Boot-ready** — windowed `new` creates `device`/`queue`/`surface`/
///   `surface_config`/`boot_splash`; `is_boot_ready()` is true after `new`. The splash path needs only
///   this. Device creation already requests the FULL feature/limit set
///   (`request_renderer_device`) because wgpu features can't be added later.
/// - **Full-ready** — `full` is `Some`; every steady-state path (Frontend,
///   Loading completion, Running, UI pass, scene render) requires it.
pub struct Renderer {
    pub(super) device: wgpu::Device,
    pub(super) queue: wgpu::Queue,
    /// Present surface for a windowed renderer. Offscreen capture deliberately
    /// has no surface and never reaches the present/splash paths.
    pub(super) surface: Option<wgpu::Surface<'static>>,
    pub(super) surface_config: wgpu::SurfaceConfiguration,
    pub(super) is_surface_configured: bool,
    pub(super) surface_reconfigure_pending: bool,

    /// `has_multi_draw_indirect` flag cached for `finish_full_init` and
    /// `install_level_geometry`. Boot-phase: derived from the adapter, needed to
    /// build the full renderer and re-build it across surface recreation.
    pub(super) has_multi_draw_indirect: bool,
    /// Adapter `CUBE_ARRAY_TEXTURES` support, cached at boot so `finish_full_init`
    /// can rebuild the full renderer (cube shadow pool + shared group-5 BGL) from
    /// boot state alone. `Some` cube pool iff this is true (see `FullRenderer`).
    pub(super) cube_array_supported: bool,

    /// Static bloom style cached in boot state so `finish_full_init` rebuilds
    /// the full renderer with the last committed profile after surface recovery.
    pub(super) bloom_render_profile: BloomRenderProfile,

    /// Renderer-owned boot splash pass: clears the swapchain and draws the
    /// decoded logo as a single textured quad. Independent of the UI pass — the
    /// boot path uses it directly so first pixels reach the window before the
    /// full renderer (and UI system) initialize. Holds the uploaded logo between
    /// `install_splash_pixels` and `clear_splash`; its `Option<InstalledLogo>` is
    /// the renderer's "logo-installed" phase bit. See: context/lib/boot_sequence.md §1.
    pub(super) boot_splash: Option<splash_pass::BootSplashPass>,

    /// Full-phase renderer state. `None` until `finish_full_init` builds it;
    /// `is_full_ready()` mirrors `is_some()`. Rebuilt (the old box dropped, GPU
    /// resources released) on surface recreation, so full init is idempotent /
    /// restartable across suspend→resume. Built purely from boot state with no
    /// level loaded (`geometry = None`); level data installs later.
    pub(super) full: Option<Box<FullRenderer>>,
}

/// Full-phase renderer: all the steady-state pipelines, passes, and resources.
/// Built by `finish_full_init` from boot state alone (no level loaded). Every
/// field here was previously inline on `Renderer`; the split lets the boot
/// splash present before any of this is constructed.
pub(super) struct FullRenderer {
    pub(super) pipeline: wgpu::RenderPipeline,
    pub(super) depth_prepass_pipeline: wgpu::RenderPipeline,
    /// `Some` when `POSTRETRO_GPU_TIMING=1` AND the adapter supports both base
    /// and encoder-level timestamp queries; `None` → no timing writes.
    pub(super) frame_timing: Option<FrameTiming>,
    pub(super) vertex_buffer: wgpu::Buffer,
    pub(super) index_buffer: wgpu::Buffer,
    pub(super) index_count: u32,
    pub(super) uniform_buffer: wgpu::Buffer,
    pub(super) uniform_bind_group: wgpu::BindGroup,

    /// Retained so `install_textures` can create material bind groups after init.
    pub(super) texture_bind_group_layout: wgpu::BindGroupLayout,
    /// Retained so `install_level_geometry` can rebuild the lighting bind group.
    pub(super) lighting_bind_group_layout: wgpu::BindGroupLayout,
    /// Post Retro linear+anisotropic samplers, one per distinct uploaded
    /// `mip_count`. Sampler descriptors are identical except for
    /// `lod_max_clamp = (mip_count - 1) as f32`. Keyed by
    /// `LoadedTexture::mip_count`. Engine-lifetime — persists across level
    /// reloads so re-installing the same mip chain reuses the existing sampler.
    /// Placeholders pick up the `1` entry seeded at construction. World and mover
    /// material bind groups bind their matching sampler at group-1 binding 5.
    pub(super) mip_count_aniso_samplers: HashMap<u32, wgpu::Sampler>,
    /// Character-model samplers, one per distinct uploaded `mip_count`. These
    /// use nearest magnification with linear minification and mip filtering,
    /// and disable anisotropy. Like the world sampler pool, descriptors differ
    /// only in `lod_max_clamp` and entries persist across level reloads. Character
    /// material bind groups bind their matching sampler at group-1 binding 5.
    pub(super) mip_count_character_model_samplers: HashMap<u32, wgpu::Sampler>,
    /// Engine-lifetime owners of the loaded textures and views referenced by
    /// material bind groups. Replaced wholesale on every `install_textures`.
    /// Bind groups borrow these handles; dropping the vec invalidates them,
    /// so keep them resident for the level's lifetime.
    #[allow(dead_code)]
    pub(super) loaded_textures: Vec<LoadedTexture>,
    /// Per-texture material properties derived from texture names. Set by
    /// `install_level_geometry`; consumed by `install_textures` to populate
    /// per-material shininess uniforms.
    pub(super) stored_texture_materials: Vec<Material>,
    /// Retained so `install_level_geometry` can pass it to `ShComposeResources`
    /// and `AnimatedLightmapResources` without recreating the layout inline.
    pub(super) uniform_bind_group_layout: wgpu::BindGroupLayout,

    /// GPU half of the debug UI. Lazily constructed by `ensure_debug_ui_gpu`
    /// on first panel open; stays resident for the rest of the session.
    /// `None` until then; never allocated in a no-`dev-tools` build.
    #[cfg(feature = "dev-tools")]
    pub(super) debug_ui_gpu: Option<debug_ui::DebugUiGpu>,

    /// Always bound; maps with zero lights get a 1-element dummy buffer —
    /// wgpu rejects zero-sized storage buffer bindings.
    pub(super) lighting_bind_group: wgpu::BindGroup,
    pub(super) influence_buffer: wgpu::Buffer,
    /// Maximum dynamic-direct prefix accepted from the scripting bridge.
    /// Promoted static records occupy a separate reserved tail.
    pub(super) dynamic_light_capacity: usize,
    pub(super) light_count: u32,
    /// Dynamic-tier records plus promoted static records appended for entity
    /// consumers this frame. Forward world rendering continues to use
    /// `light_count`; mesh/billboard use this total.
    pub(super) total_light_count: u32,
    /// The frame's forward `Uniforms.time` value, cached by
    /// `update_per_frame_uniforms` so the skinned-mesh group-2 params uniform
    /// (`MeshLightParams.time`) is written from the SAME render-clock value the
    /// forward pass uses that frame. The scripted-light animated curves the mesh
    /// dynamic loop evaluates depend on this phase coherence.
    pub(super) mesh_dynamic_time: f32,
    /// Captured alongside the group-0 uniform at the start of the render
    /// frame. Consumers must use this, never the UI-mutated live mask, so a
    /// toggle takes effect atomically on the following frame.
    pub(super) frame_light_term_mask: LightTermMask,
    /// Camera-PVS-visible kinematic mover instances for the beauty pass.
    pub(super) kinematic_mover_draws: Vec<kinematic_brush::KinematicMoverInstance>,
    /// Every present kinematic mover transform, including camera-PVS-culled
    /// shadow casters.
    pub(super) kinematic_mover_shadow_draws: Vec<kinematic_brush::KinematicMoverInstance>,
    /// Conservative world AABBs for every present mover, keyed by authored
    /// mover id. Static-light promotion and rigid shadow occluder recording
    /// read this renderer-owned state.
    pub(super) mover_occluder_aabbs: Vec<rigid_occluder_depth::MoverOccluderAabb>,
    pub(super) ambient_floor: f32,
    pub(super) indirect_scale: f32,
    /// DYNAMIC baked-static-direct SH scale (0..1). Debug instrument for the
    /// entity/billboard direct term, independent of `indirect_scale`. Mirrors
    /// the `indirect_scale` knob — uploaded to the billboard group-0 tail and
    /// the mesh group-4 `DynamicDirectParams` each frame.
    pub(super) dynamic_direct_scale: f32,
    /// Runtime SH probe-occlusion toggle. Default-on; `POSTRETRO_SH_FAST=1`
    /// seeds it off for benchmark/headless runs, and the diagnostics panel can
    /// flip it later. Uploaded through `ShGridInfo`.
    pub(super) probe_occlusion_enabled: bool,

    /// Absent/disabled OctahedralShVolume → dummy 1×1 atlas resources;
    /// `has_sh_volume == 0` skips indirect sampling.
    pub(super) sh_volume_resources: ShVolumeResources,

    /// Static-occluder SDF atlas + bind group. Owned by the renderer; the
    /// bind-group layout is consumed only by the SDF shadow pass — NOT
    /// bound by forward (forward gets only the shadow-factor texture in
    /// group 5). `present` is false when no SDF section is in the PRL;
    /// the shadow pass skips its dispatch in that case.
    pub(super) sdf_atlas_resources: SdfAtlasResources,
    /// Half-resolution per-light SDF shadow pass. Always allocated.
    /// Dispatch is gated on `sdf_atlas_resources.present` and the active
    /// `SdfShadowMode`.
    pub(super) sdf_shadow_pass: SdfShadowPass,
    /// Lightmap bake mode read from the PRL (records whether visibility was
    /// folded into the bake). Under the disjoint-direct design, `sdf` lights
    /// are excluded from `lm_irr` at bake time, so the forward pass never
    /// multiplies SDF visibility into the static-lightmap term; this field
    /// is retained only for legacy-PRL compatibility. Defaults to `Shadowed`
    /// so legacy PRLs decode without error.
    #[allow(dead_code)]
    pub(super) lightmap_mode: postretro_level_loader::LightmapMode,

    /// CPU mirror of animated-light delta volume placements, one entry per
    /// animated light. Empty when the map has no delta SH volumes. Sourced
    /// at level load from the same `DeltaShVolumesSection` `sh_compose` consumes;
    /// surfaced via `Renderer::sh_delta_volumes` for the SH diagnostic overlay.
    #[cfg(feature = "dev-tools")]
    pub(super) sh_delta_volumes_meta: Vec<sh_volume::DeltaVolumeMeta>,

    /// Async readback of the composed SH atlas so irradiance probe markers
    /// reflect live (base + animated-delta) lighting. Rebuilt per level load.
    #[cfg(feature = "dev-tools")]
    pub(super) sh_probe_readback: sh_diagnostics::ShProbeReadback,

    /// Dev-tools toggle: when set, `uniforms.time` is pinned to `frozen_time`,
    /// so all curve-driven animation (SH compose, animated lightmap, scripted
    /// lights) holds still — a debugging aid for isolating time-driven artifacts.
    #[cfg(feature = "dev-tools")]
    pub(super) freeze_time: bool,
    /// Time held while `freeze_time` is set; tracks live time otherwise, so
    /// enabling the freeze holds whatever animation phase is currently showing.
    #[cfg(feature = "dev-tools")]
    pub(super) frozen_time: f32,

    /// Composes base SH bands into the total bands consumers sample. Must run
    /// before the depth pre-pass so the storage→sampled barrier resolves first.
    pub(super) sh_compose: ShComposeResources,

    /// Optional direct-SH compose pass. Disabled entirely when a level has no
    /// selected static direct deltas; in that path the base direct atlas remains
    /// bound and no composed direct texture exists.
    pub(super) direct_sh_compose: DirectShComposeResources,
    /// Optional section-48 compose. Static-only scatter binds its base directly,
    /// while unavailable content keeps the billboard legacy path selected.
    pub(super) billboard_direct_scatter_compose: BillboardDirectScatterComposeResources,

    /// Absent Lightmap section → 1×1 white/neutral placeholder; no shader branch.
    pub(super) lightmap_resources: LightmapResources,

    pub(super) animated_lightmap: animated_lightmap::AnimatedLightmapResources,

    #[allow(dead_code)]
    pub(super) lights_buffer: wgpu::Buffer,
    /// Last bytes uploaded to `lights_buffer`. Reused each frame to skip a
    /// redundant `queue.write_buffer` when the packed bytes are unchanged.
    pub(super) last_lights_upload: Vec<u8>,
    /// Dynamic-prefix mirror of `influence_buffer`, index-parallel to
    /// `last_lights_upload`. Promoted static influences and metadata append
    /// after this prefix during shadow-slot updates.
    pub(super) last_influence_upload: Vec<u8>,
    /// Scratch buffer for the fallback full-repack path. Used only when
    /// `last_lights_upload` is not yet sized to the current light set
    /// (first frame or light-count change). The hot path patches
    /// `last_lights_upload` in place via `patch_shadow_slots` — scratch
    /// is not touched in that branch.
    pub(super) lights_pack_scratch: Vec<u8>,
    /// Scratch buffer for per-frame influence upload. Dynamic influences are
    /// followed by promoted static influences, matching the count-split light
    /// buffer without allocating in the render hot path.
    pub(super) influence_pack_scratch: Vec<u8>,
    #[allow(dead_code)]
    pub(super) level_lights: Vec<MapLight>,
    /// Original full level-light index for each `level_lights` entry.
    pub(super) level_light_source_indices: Vec<usize>,
    pub(super) level_light_influences: Vec<LightInfluence>,
    /// Selected static lights in `EntityShadowLights` order. Source indices are
    /// global level-light indices.
    pub(super) entity_shadow_lights: Vec<MapLight>,
    pub(super) entity_shadow_light_influences: Vec<LightInfluence>,
    pub(super) entity_shadow_light_source_indices: Vec<usize>,
    pub(super) entity_shadow_spec_light_indices: Vec<u32>,
    pub(super) shadowmask_channels: Vec<u8>,
    pub(super) shadowmask_present: bool,
    pub(super) forward_shadowmask_metadata_scratch: Vec<u8>,
    /// Candidate set for the spot/cube shadow pools — sourced from the FULL level
    /// light set filtered by `is_dynamic`. Dynamic-tier lights
    /// (`light_dynamic`/`light_dynamic_spot`) are pool-eligible so dynamic
    /// spotlights shadow static world occluders (pillars). The per-light
    /// `casts_entity_shadows` toggle (FGD `_cast_entity_shadows`) gates only
    /// whether moving-ENTITY occluders draw into the slot, not slot allocation.
    pub(super) shadow_candidate_lights: Vec<MapLight>,
    /// Original full level-light index for each `shadow_candidate_lights` entry.
    pub(super) shadow_candidate_source_indices: Vec<usize>,
    /// Selection index for a shadow candidate. `None` means dynamic-tier.
    pub(super) shadow_candidate_selection_indices: Vec<Option<usize>>,
    /// Candidate-indexed influence volumes paired with `shadow_candidate_lights`.
    /// Missing/short PRL influence data is represented by an uncullable sentinel
    /// so shadow eligibility follows the same degradation contract as forward
    /// direct-light culling.
    pub(super) shadow_candidate_influences: Vec<LightInfluence>,
    /// Lights near zero are excluded from shadow slot ranking. Empty = no suppression.
    pub(super) light_effective_brightness: Vec<f32>,
    /// Cached from `update_per_frame_uniforms` so the shadow pass can re-rank lights.
    pub(super) last_camera_position: Vec3,
    /// Cached camera `view_proj` from `update_per_frame_uniforms`; the shadow
    /// pool derives camera frustum planes from it for cone-frustum culling.
    pub(super) last_view_proj: Mat4,
    pub(super) spot_shadow_pool: SpotShadowPool,
    /// Dynamic point-light cube-array shadow pool. `None` when the adapter lacks
    /// `CUBE_ARRAY_TEXTURES` — point shadows then cleanly off, spot unaffected.
    /// `Some` iff `cube_array_supported`, so its presence mirrors group-5 binding
    /// 5's presence in the shared BGL.
    pub(super) cube_shadow_pool: Option<crate::lighting::cube_shadow::CubeShadowPool>,
    pub(super) kinematic_brush: kinematic_brush::KinematicBrushPass,
    pub(super) rigid_occluder_depth: rigid_occluder_depth::RigidOccluderDepthPass,
    pub(super) promoted_static_states: Vec<PromotedStaticLightState>,
    pub(super) promoted_static_records: Vec<PromotedStaticLightRecord>,
    /// Cache-layer metadata parallel to `promoted_static_records`; packed into
    /// the forward shadowmask metadata tail's `meta1.w` lane.
    pub(super) promoted_static_cache_layers: Vec<i32>,
    pub(super) promoted_static_weights: Vec<f32>,
    pub(super) promoted_static_weight_buffer: wgpu::Buffer,
    pub(super) promoted_static_weight_scratch: Vec<u8>,
    pub(super) promoted_static_last_update_time: Option<f64>,
    /// `None` for maps with an empty/absent `EntityShadowLights` selection —
    /// no light can ever promote, so the ~44 MiB spot/cube depth cache arrays
    /// are never allocated. `Some` only when the selection is non-empty.
    pub(super) promoted_depth_cache: Option<PromotedDepthCache>,
    /// Missing cache-plan entries are defensive degradation, warned once per
    /// installed level rather than once per rendered frame.
    pub(super) promoted_depth_cache_missing_layer_warned: bool,
    pub(super) promoted_depth_cache_frame_plan: PromotedDepthCacheFramePlan,
    pub(super) promoted_depth_cache_promoted_count: u32,
    pub(super) promoted_depth_cache_world_render_skips: u32,
    pub(super) promoted_depth_cache_cull_dispatch_skips: u32,
    pub(super) promoted_depth_cache_timing_open: bool,
    #[cfg(feature = "dev-tools")]
    pub(super) direct_sh_debug_override: DirectShDebugOverride,
    #[cfg(feature = "dev-tools")]
    pub(super) animated_direct_sh_debug_override: AnimatedDirectShDebugOverride,
    /// Per-(cube slot, face) light-space matrix uniforms, dynamic-offset like
    /// `shadow_vs_uniform_buffer`. Slot `slot*6 + face` carries that face's
    /// matrix; the skinned-depth pass selects it by dynamic offset.
    pub(super) cube_shadow_vs_uniform_buffer: wgpu::Buffer,
    pub(super) cube_shadow_vs_bind_group: wgpu::BindGroup,
    /// Dynamic-offset into a single buffer; offset selects the per-slot light-space matrix.
    pub(super) shadow_vs_uniform_buffer: wgpu::Buffer,
    pub(super) shadow_vs_bind_group: wgpu::BindGroup,
    pub(super) shadow_depth_pipeline: wgpu::RenderPipeline,
    /// Rounded up to `min_uniform_buffer_offset_alignment`.
    pub(super) shadow_vs_stride: u32,

    pub(super) depth_view: wgpu::TextureView,

    /// Post-scene compositor seam: owns the `scene_color` offscreen target every
    /// gameplay scene/UI pass renders into, plus the resolve pass that blits it
    /// to the swapchain (the sole gameplay-path swapchain writer). Recreated on
    /// resize alongside `depth_view`. See `render/screen_effects.rs`.
    pub(super) screen_effects: ScreenEffectsPass,

    /// HDR bloom chain. Samples the post-fog `scene_color` target and
    /// composites before capture and presentation resolve.
    pub(super) bloom: BloomPass,

    /// GPU textures indexed by texture index.
    pub(super) gpu_textures: Vec<GpuTexture>,
    pub(super) bvh_leaves: Vec<postretro_render_data::geometry::BvhLeaf>,
    /// Per-cell BVH-leaf draw index (PRL section 37), cloned from the installed
    /// `LevelGeometry`. `None` only when no level is installed, the installed
    /// map has an empty BVH, or resources were released. Non-empty BVHs require
    /// this index at load; missing or invalid data is a load error. Whole-BVH
    /// tree-walk fallback is a per-frame runtime path for `DrawAll`,
    /// non-portal visibility, out-of-range gathered cell ids, or no candidate
    /// cull pipeline.
    pub(super) cell_draw_index: Option<postretro_level_loader::CellDrawIndex>,
    /// `None` for maps with no BVH.
    pub(super) compute_cull: Option<ComputeCullPipeline>,
    /// Candidate-cull GPU path: gathers only visible cells' BVH
    /// leaves (via the baked `cell_draw_index` CSR) and dispatches one
    /// invocation per candidate leaf, writing the SAME global indirect/status
    /// slots as `compute_cull`. Built in lockstep with `compute_cull`; used only
    /// on candidate-eligible frames (valid index + `Culled` + `PrlPortal`),
    /// otherwise the whole-BVH tree walk runs. `None` for maps with no BVH.
    pub(super) candidate_cull: Option<crate::candidate_cull::CandidateCullPipeline>,
    /// Per-slot cone cull for the spot-shadow depth passes. Sibling to
    /// `compute_cull`, sharing its read-only BVH node/leaf buffers. `None` for
    /// maps with no BVH (kept in lockstep with `compute_cull`).
    pub(super) shadow_cull: Option<crate::shadow_cull::ShadowCullPipeline>,
    /// Per-FACE frustum cull for the point cube-shadow depth passes: one
    /// indirect sub-region per `(cube slot, face)` layer
    /// (`CUBE_COUNT × CUBE_FACES` regions), planes from that face's 90°
    /// perspective matrix. Same construction and lockstep-rebuild contract as
    /// `shadow_cull`; additionally `None` when the cube pool itself is off
    /// (adapter lacks `CUBE_ARRAY_TEXTURES`).
    pub(super) cube_shadow_cull: Option<crate::shadow_cull::ShadowCullPipeline>,

    pub(super) wireframe_cull_status_pipeline: wgpu::RenderPipeline,
    pub(super) wireframe_visible_pipeline: wgpu::RenderPipeline,
    pub(super) wireframe_index_buffer: wgpu::Buffer,
    pub(super) wireframe_index_count: u32,
    pub(super) wireframe_cull_status_bgl: wgpu::BindGroupLayout,
    pub(super) world_wireframe_mode: WorldWireframeMode,
    pub(super) wireframe_enabled: bool,

    #[cfg(feature = "dev-tools")]
    pub(super) debug_lines: debug_lines::DebugLineRenderer,
    #[cfg(feature = "dev-tools")]
    pub(super) bvh_overlay: BvhOverlayState,
    #[cfg(feature = "dev-tools")]
    pub(super) cell_overlay: CellOverlayState,
    #[cfg(feature = "dev-tools")]
    pub(super) portal_overlay: PortalOverlayState,
    #[cfg(feature = "dev-tools")]
    pub(super) agent_overlay: AgentOverlayState,
    /// Navmesh overlay toggle, flipped by `Alt+Shift+N`. Read at the emit call
    /// site to decide whether to push region/portal debug lines this frame.
    #[cfg(feature = "dev-tools")]
    pub(super) show_navmesh: bool,

    /// Live dev-tools value. `update_per_frame_uniforms` snapshots it into
    /// `frame_light_term_mask` before scene recording begins.
    pub(super) light_term_mask: LightTermMask,

    /// Debug selector for the SDF static-occluder shadow path. Panel-only
    /// dropdown, surfaces through
    /// `FrameUniforms.sdf_shadow_mode`.
    pub(super) sdf_shadow_mode: SdfShadowMode,

    /// Dev toggle: force per-light SDF visibility to 1.0 in the forward shader.
    /// Panel checkbox; surfaces through `FrameUniforms.sdf_force_visibility_one`.
    /// Drives the no-double-count visual A/B (forced-1.0 must match the
    /// pre-change render). Seeded from the `POSTRETRO_SDF_FORCE_VISIBILITY_ONE`
    /// env flag at construction so a headless/no-UI run can exercise it too.
    pub(super) sdf_force_visibility_one: bool,

    /// Dev toggle: force static-light shadowmask visibility to 1.0 in the
    /// forward world-specular path. Panel checkbox; surfaces through
    /// `FrameUniforms.spec_shadowmask_force_one`. Seeded from
    /// `POSTRETRO_SPEC_SHADOWMASK_FORCE_ONE` for repeatable headless A/B
    /// captures. SDF and dynamic/mover paths remain unaffected.
    pub(super) spec_shadowmask_force_one: bool,

    /// Toggled by Alt+Shift+V; `true` = AutoVsync, `false` = AutoNoVsync.
    pub(super) vsync_enabled: bool,

    pub(super) has_geometry: bool,

    pub(super) debug_frame: u64,
    pub(super) debug_prev_bitmask: (u32, u32),
    pub(super) debug_prev_vp_hash: u32,
    pub(super) debug_prev_visible: (&'static str, usize),
    /// One-shot guard so the candidate-cull out-of-range-cell warning logs once,
    /// not every frame. Reset on each level install so a later level's corrupt
    /// index still warns once.
    pub(super) candidate_cull_oor_logged: bool,
    /// Camera-cull diagnostics for the current Spatial tab frame (candidate vs
    /// tree-walk path, candidate/total/submitted leaves). Refreshed before the
    /// debug UI reads it, then recomputed during pass recording. Diagnostic only
    /// — never gates behavior.
    #[cfg_attr(not(feature = "dev-tools"), allow(dead_code))]
    pub(super) camera_cull_diagnostics: CameraCullDiagnostics,
    /// Last CPU-side visibility/locator snapshot published by the app after
    /// camera visibility runs. Read by the Spatial diagnostics tab.
    #[cfg_attr(not(feature = "dev-tools"), allow(dead_code))]
    pub(super) spatial_diagnostics: SpatialDiagnostics,

    /// Full tree-walk cull-cost estimate, refreshed from the current frame's
    /// visibility independent of which GPU cull strategy ran. This is the
    /// baseline the candidate path beats — it must not be a side effect of the
    /// tree-walk dispatch, or candidate-eligible frames starve it to zero.
    /// `None` when no cull pipeline is loaded (no level / no BVH). Read by the
    /// baseline diagnostics panel (dev-tools).
    #[cfg_attr(not(feature = "dev-tools"), allow(dead_code))]
    pub(super) bvh_cull_diagnostics: Option<crate::compute_cull::BvhCullDiagnostics>,

    /// `POSTRETRO_SHADOW_DEBUG=1`: env-gated shadow-pipeline diagnostics. Cached
    /// at construction so the hot path pays one bool test, not a `getenv`, per
    /// frame. When set, `emit_shadow_debug` logs (via `log::info!`) a compact
    /// per-frame line tracing which shadow decision flips as the camera pitches —
    /// camera pose + per-candidate-light shadow-slot status + the entity
    /// shadow-caster keep/drop tally. Read-only: it never changes culling or
    /// selection behavior. See `context/lib/rendering_pipeline.md` §4, §7.1.
    pub(super) shadow_debug_enabled: bool,
    /// Last `emit_shadow_debug` fingerprint, so the diagnostic logs on CHANGE
    /// (and every ~120 frames as a heartbeat) instead of spamming every frame.
    /// `(slot_occupancy, cube_occupancy, in_pvs, off_pvs)`.
    pub(super) shadow_debug_prev: (u128, u128, u32, u32),

    /// Idle (no draw) on maps with no registered collections. See §7.4.
    pub(super) smoke_pass: SmokePass,

    /// Skinned-mesh forward pass. Idle (no draw) until a model is uploaded via
    /// `load_skinned_model` (driven by the level-load model sweep at level
    /// install, once per distinct `prop_mesh` model).
    pub(super) mesh_pass: mesh_pass::MeshPass,

    /// Per-frame skinned-mesh instance list: surviving (model handle,
    /// interpolated transform, phase seed) tuples. Refilled each frame via
    /// `set_mesh_draws` from the render-frame mesh collector (which classifies
    /// forward visibility via `mesh_pass::mesh_visible` and may retain additional
    /// selected-static-light shadow casters as non-forward instances). Empty when
    /// no mesh entity is visible or shadow-relevant. Planned into per-model draw
    /// groups + palette runs each frame by `mesh_instances::plan_mesh_frame`.
    pub(super) mesh_draws: Vec<mesh_instances::MeshInstanceInput>,

    /// Reusable bone-palette scratch for per-frame per-instance sampling.
    /// `sample_clip` clears then refills it per instance, so steady-state frames
    /// allocate nothing. Lives on the renderer (not in the GPU pass) — it is
    /// CPU-side pose data the pass merely uploads.
    pub(super) bone_palette_scratch: Vec<postretro_model::BonePaletteEntry>,

    /// Wall-clock of the last palette/instance-overflow warning (render clock),
    /// for rate-limiting (mirrors `EmitterBridge`'s `last_warn_time`). Overflow
    /// drops the excess instances; the warning fires at most once per second.
    pub(super) mesh_overflow_last_warn: f32,

    /// CPU-side count of skinned and rigid ENTITY occluders submitted into spot
    /// shadow slots last frame, summed across slots (each counted once per slot
    /// it casts into). Mirrors `shadow-cone-cull`'s submitted-instance counter —
    /// no GPU readback. Verifies the "enemy outside the cone is not drawn"
    /// acceptance criterion: an occluder the per-light cone cull rejects is never
    /// added here. Reset to 0 at the start of the spot-shadow depth loop.
    pub(super) spot_entity_occluders_submitted: u32,

    /// CPU-side count of skinned and rigid ENTITY occluders submitted into CUBE
    /// (point-light) shadow faces last frame, summed across all occupied slots ×
    /// 6 faces (each counted once per face it casts into). Mirrors
    /// `spot_entity_occluders_submitted` — no GPU readback. Verifies that entity
    /// occluders render only for `entity_occluder_eligible` point lights and only
    /// when their bound intersects a face frustum. Reset to 0 at the start of the
    /// cube-shadow depth loop.
    pub(super) cube_entity_occluders_submitted: u32,

    /// CPU-side count of skinned and rigid ENTITY occluder submissions into
    /// promoted static-light shadow slots/faces last frame. This is a subset of
    /// the spot and cube totals above, used to pin that warm promoted slots draw
    /// entities only after the cached world depth copy.
    pub(super) promoted_entity_occluders_submitted: u32,

    /// Instanced UI quad / 9-slice pass for panels and images plus glyphon text.
    /// Built alongside `fog`; records the splash (splash phase) and an empty draw
    /// list on the gameplay path (`render_frame_indirect`). Owns all UI GPU state.
    pub(super) ui: ui::UiPass,

    /// Key→bind-group registry for gameplay/frontend `image` widget assets. The
    /// gameplay UI pass resolves image batches' asset keys through it. The boot
    /// splash does NOT use this — it owns its own logo texture in `boot_splash`.
    pub(super) ui_images: ui::UiImageRegistry,

    /// Once-per-frame published read snapshot: the splash version/tagline line
    /// and the gameplay-path descriptor tree. Set by the App via `set_ui_snapshot`
    /// just before each render call; read when the UI pass records. Stored here so
    /// both render signatures stay stable.
    pub(super) ui_snapshot: ui::UiReadSnapshot,

    /// Frame-local passive presentation instances from the app-side pool. The
    /// renderer lowers them through its FontSystem-owned template path; they are
    /// deliberately separate from the retained UI snapshot and carry no focus,
    /// hit-test, or input state.
    pub(super) presentation_inputs: Vec<PresentationDrawInput>,

    /// Active UI theme: the token table every descriptor tree resolves its
    /// color/spacing/font slots against at build time. Defaults to
    /// `UiTheme::engine_default()` at construction; `set_ui_theme` installs an
    /// override (e.g. from a mod's theme document) and bumps `ui_theme_generation`.
    /// The gameplay render path resolves descriptor-tree tokens against this
    /// instance. The boot splash is renderer-owned (`BootSplashPass`) and does
    /// not use the theme.
    pub(super) ui_theme: ui::theme::UiTheme,
    /// Monotonic UI theme generation, bumped by `set_ui_theme`. The retained
    /// gameplay tree records the generation it was built against; a bump
    /// invalidates the resolved tokens baked into it, so `layout_gameplay_tree`
    /// rebuilds the tree on the next frame even when the descriptor is unchanged.
    pub(super) ui_theme_generation: u64,

    /// Volumetric fog raymarch + composite. Active only when the level has
    /// at least one fog volume uploaded; otherwise the dispatch + composite
    /// are skipped (see `FogPass::active`).
    pub(super) fog: FogPass,

    /// Per-cell bitmask of overlapping fog volumes, loaded from PRL section 31
    /// at level load. When `Some`, the fog pass ORs the masks of reachable
    /// cells each frame to derive the active fog-volume set, culling volumes
    /// not reachable from the camera.
    pub(super) fog_cell_masks: Option<Vec<u32>>,

    /// (min, max) AABBs of fog volumes that are active this frame. Refreshed
    /// each frame via `set_fog_aabbs`; consumed by `collect_fog_spot_lights`
    /// to drop dynamic spots whose influence sphere can't scatter into any
    /// active volume. Empty list short-circuits to "pass everything" —
    /// conservative because the fog pass itself is gated by `FogPass::active`.
    pub(super) active_fog_aabbs: Vec<(Vec3, Vec3)>,
}

impl Renderer {
    /// Borrow the full-phase state. Panics if called before `finish_full_init`
    /// — every caller is on a full-ready-gated path (Frontend/Loading/Running/
    /// UI/scene), so reaching here boot-only is a logic error, not a runtime case.
    #[inline]
    #[track_caller]
    pub(super) fn full(&self) -> &FullRenderer {
        self.full
            .as_ref()
            .expect("renderer full-init must complete before full-ready paths run")
    }

    /// Mutable twin of `full`. Same full-ready contract.
    #[inline]
    #[track_caller]
    pub(super) fn full_mut(&mut self) -> &mut FullRenderer {
        self.full
            .as_mut()
            .expect("renderer full-init must complete before full-ready paths run")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn camera_cull_diagnostics_reports_candidate_leaves_per_path() {
        let candidate = CameraCullDiagnostics {
            path: CameraCullPath::Candidate {
                candidate_leaves: 7,
            },
            total_leaves: 100,
            submitted_leaves: 3,
        };
        assert_eq!(candidate.candidate_leaves(), Some(7));

        let tree_walk = CameraCullDiagnostics {
            path: CameraCullPath::TreeWalk,
            total_leaves: 100,
            submitted_leaves: 42,
        };
        assert_eq!(tree_walk.candidate_leaves(), None);
        // Default is the tree-walk path with zeroed counts.
        assert_eq!(CameraCullDiagnostics::default().candidate_leaves(), None);
    }

    #[test]
    fn spatial_cell_set_diagnostics_counts_culled_cells() {
        let cells = postretro_visibility::VisibleCells::Culled(vec![1, 2, 3]);
        assert_eq!(
            SpatialCellSetDiagnostics::from_visible_cells(&cells),
            SpatialCellSetDiagnostics::Cells { count: 3 }
        );
        assert_eq!(
            SpatialCellSetDiagnostics::from_visible_cells(
                &postretro_visibility::VisibleCells::DrawAll
            ),
            SpatialCellSetDiagnostics::DrawAll
        );
        assert_eq!(
            SpatialCellSetDiagnostics::from_cell_slice(&[4, 5]),
            SpatialCellSetDiagnostics::Cells { count: 2 }
        );
    }

    #[test]
    fn world_wireframe_modes_define_final_spatial_contract() {
        assert_eq!(
            WorldWireframeMode::ALL_VARIANTS,
            [
                WorldWireframeMode::Off,
                WorldWireframeMode::CullStatusTrianglesAlwaysOnTop,
                WorldWireframeMode::VisibleTrianglesDepthTested,
            ],
        );
        assert_eq!(WorldWireframeMode::Off.label(), "Off");
        assert_eq!(
            WorldWireframeMode::CullStatusTrianglesAlwaysOnTop.label(),
            "Cull-status triangles (all BVH leaves, x-ray)",
        );
        assert_eq!(
            WorldWireframeMode::VisibleTrianglesDepthTested.label(),
            "CPU-visible triangles (depth-tested)",
        );
    }

    #[test]
    fn spatial_overlay_defaults_are_off_depth_tested_and_cell_colored() {
        assert_eq!(
            BvhOverlayState::default(),
            BvhOverlayState {
                visible: false,
                color_mode: BvhOverlayColorMode::CellId,
                depth_mode: BvhOverlayDepthMode::DepthTested,
                budget: BvhOverlayBudget {
                    max_boxes: 512,
                    stride: 1,
                    visible_cells_only: false,
                },
            },
        );
        assert_eq!(
            CellOverlayState::default(),
            CellOverlayState {
                visible: false,
                depth_mode: BvhOverlayDepthMode::DepthTested,
            },
        );
        assert_eq!(
            PortalOverlayState::default(),
            PortalOverlayState {
                visible: false,
                depth_mode: BvhOverlayDepthMode::DepthTested,
            },
        );
    }
}
