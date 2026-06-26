// Core renderer data types: the Renderer struct, LevelGeometry, GpuTexture,
// ClearColor, and shared rendering constants.
// See: context/lib/rendering_pipeline.md

use super::*;

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct ClearColor {
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

/// Minimum useful ambient. Default value seeded into the Diagnostics panel slider on first open.
pub const DEFAULT_AMBIENT_FLOOR: f32 = 0.001;

/// Full SH contribution weight — production default. Default value seeded into the Diagnostics panel slider on first open.
pub const DEFAULT_INDIRECT_SCALE: f32 = 1.0;

/// Full dynamic baked-static-direct SH weight — production default. Seeded into
/// the Diagnostics panel slider on first open.
pub const DEFAULT_DYNAMIC_DIRECT_SCALE: f32 = 1.0;

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
    pub fn from_visible_cells(cells: &crate::visibility::VisibleCells) -> Self {
        match cells {
            crate::visibility::VisibleCells::Culled(cells) => Self::Cells {
                count: cells.len() as u32,
            },
            crate::visibility::VisibleCells::DrawAll => Self::DrawAll,
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
    Trace(crate::prl::CellLocatorTrace),
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
        let cells = crate::visibility::VisibleCells::Culled(vec![1, 2, 3]);
        assert_eq!(
            SpatialCellSetDiagnostics::from_visible_cells(&cells),
            SpatialCellSetDiagnostics::Cells { count: 3 }
        );
        assert_eq!(
            SpatialCellSetDiagnostics::from_visible_cells(
                &crate::visibility::VisibleCells::DrawAll
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

/// Hardware anisotropy cap for the Post Retro filtering pool. wgpu 29 requires
/// `anisotropy_clamp >= 1`; 16 is the common ceiling exposed by desktop adapters
/// and the visual point of diminishing returns for grazing-angle sharpness.
pub const POST_RETRO_ANISO_CLAMP: u16 = 16;

pub struct LevelGeometry<'a> {
    pub vertices: &'a [crate::geometry::WorldVertex],
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
    /// `None` → no SDF static-occluder atlas; runtime SDF shadow pass disabled.
    /// An empty-geometry section (zero grid dims) is treated the same way.
    pub sdf_atlas: Option<&'a postretro_level_format::sdf_atlas::SdfAtlasSection>,
    /// Whether baked static-direct lightmap samples already include static-light
    /// visibility. `Shadowed` atlases contain the visibility term; `Unshadowed`
    /// atlases leave it for runtime SDF shadowing so the forward pass does not
    /// double-count static-light occlusion. Legacy PRLs default to `Shadowed`.
    pub lightmap_mode: crate::prl::LightmapMode,
    /// Per-cell BVH-leaf draw index (PRL section 37), cross-validated at load.
    /// `None` only for no installed level or an empty-BVH map. Non-empty BVHs
    /// require this index at load; missing or invalid data is a load error.
    /// Whole-BVH tree-walk fallback is a per-frame runtime path for `DrawAll`,
    /// non-portal visibility, out-of-range gathered cell ids, or no candidate
    /// cull pipeline.
    pub cell_draw_index: Option<&'a crate::prl::CellDrawIndex>,
    pub texture_materials: &'a [crate::material::Material],
}

pub struct Renderer {
    pub(super) device: wgpu::Device,
    pub(super) queue: wgpu::Queue,
    pub(super) surface: wgpu::Surface<'static>,
    pub(super) surface_config: wgpu::SurfaceConfiguration,
    pub(super) is_surface_configured: bool,

    pub(super) pipeline: wgpu::RenderPipeline,
    pub(super) depth_prepass_pipeline: wgpu::RenderPipeline,
    /// `Some` when `POSTRETRO_GPU_TIMING=1` AND adapter supports `TIMESTAMP_QUERY`;
    /// `None` → no `timestamp_writes` attached to any pass.
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
    /// Placeholders pick up the `1` entry seeded at construction. Every material
    /// binds its matching sampler at group-1 binding 5.
    pub(super) mip_count_aniso_samplers: HashMap<u32, wgpu::Sampler>,
    /// Engine-lifetime owners of the loaded textures and views referenced by
    /// material bind groups. Replaced wholesale on every `install_textures`.
    /// Bind groups borrow these handles; dropping the vec invalidates them,
    /// so keep them resident for the level's lifetime.
    #[allow(dead_code)]
    pub(super) loaded_textures: Vec<LoadedTexture>,
    /// `has_multi_draw_indirect` flag cached for `install_level_geometry`.
    pub(super) has_multi_draw_indirect: bool,
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
    pub(super) light_count: u32,
    /// The frame's forward `Uniforms.time` value, cached by
    /// `update_per_frame_uniforms` so the skinned-mesh group-2 params uniform
    /// (`MeshLightParams.time`) is written from the SAME render-clock value the
    /// forward pass uses that frame. The scripted-light animated curves the mesh
    /// dynamic loop evaluates depend on this phase coherence.
    pub(super) mesh_dynamic_time: f32,
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
    pub(super) lightmap_mode: crate::prl::LightmapMode,

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

    /// Absent Lightmap section → 1×1 white/neutral placeholder; no shader branch.
    pub(super) lightmap_resources: LightmapResources,

    pub(super) animated_lightmap: animated_lightmap::AnimatedLightmapResources,

    #[allow(dead_code)]
    pub(super) lights_buffer: wgpu::Buffer,
    /// Last bytes uploaded to `lights_buffer`. Reused each frame to skip a
    /// redundant `queue.write_buffer` when the packed bytes are unchanged.
    pub(super) last_lights_upload: Vec<u8>,
    /// Scratch buffer for the fallback full-repack path. Used only when
    /// `last_lights_upload` is not yet sized to the current light set
    /// (first frame or light-count change). The hot path patches
    /// `last_lights_upload` in place via `patch_shadow_slots` — scratch
    /// is not touched in that branch.
    pub(super) lights_pack_scratch: Vec<u8>,
    #[allow(dead_code)]
    pub(super) level_lights: Vec<MapLight>,
    /// Candidate set for the spot-shadow pool — sourced from the FULL level
    /// light set filtered by `is_dynamic`. Dynamic-tier lights
    /// (`light_dynamic`/`light_dynamic_spot`) are pool-eligible so dynamic
    /// spotlights shadow static world occluders (pillars). The per-light
    /// `casts_entity_shadows` toggle (FGD `_cast_entity_shadows`) gates only
    /// whether moving-ENTITY occluders draw into the slot, not slot allocation.
    pub(super) shadow_candidate_lights: Vec<MapLight>,
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

    /// GPU textures indexed by texture index.
    pub(super) gpu_textures: Vec<GpuTexture>,
    pub(super) bvh_leaves: Vec<crate::geometry::BvhLeaf>,
    /// Per-cell BVH-leaf draw index (PRL section 37), cloned from the installed
    /// `LevelGeometry`. `None` only when no level is installed, the installed
    /// map has an empty BVH, or resources were released. Non-empty BVHs require
    /// this index at load; missing or invalid data is a load error. Whole-BVH
    /// tree-walk fallback is a per-frame runtime path for `DrawAll`,
    /// non-portal visibility, out-of-range gathered cell ids, or no candidate
    /// cull pipeline.
    pub(super) cell_draw_index: Option<crate::prl::CellDrawIndex>,
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
    /// Navmesh overlay toggle, flipped by `Alt+Shift+N`. Read at the emit call
    /// site to decide whether to push region/portal debug lines this frame.
    #[cfg(feature = "dev-tools")]
    pub(super) show_navmesh: bool,

    pub(super) lighting_isolation: LightingIsolation,

    /// DYNAMIC baked-static-direct SH isolation (combined / direct-only /
    /// indirect-only). Separate from `lighting_isolation` (the forward/static
    /// control), so the dynamic-vs-static parity comparison stays valid.
    pub(super) dynamic_direct_isolation: DynamicDirectIsolation,

    /// Debug selector for the SDF static-occluder shadow path. Mirrors
    /// `lighting_isolation` — panel-only dropdown, surfaces through
    /// `FrameUniforms.sdf_shadow_mode`.
    pub(super) sdf_shadow_mode: SdfShadowMode,

    /// Dev toggle: force per-light SDF visibility to 1.0 in the forward shader.
    /// Panel checkbox; surfaces through `FrameUniforms.sdf_force_visibility_one`.
    /// Drives the no-double-count visual A/B (forced-1.0 must match the
    /// pre-change render). Seeded from the `POSTRETRO_SDF_FORCE_VISIBILITY_ONE`
    /// env flag at construction so a headless/no-UI run can exercise it too.
    pub(super) sdf_force_visibility_one: bool,

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
    /// `set_mesh_draws` from the render-frame mesh collector (which culls each
    /// entity via `mesh_pass::mesh_visible` against the frame's `VisibleCells` +
    /// the `LevelWorld`, then emits survivors at their interpolated transform).
    /// Empty when no mesh entity is visible. Planned into per-model draw groups
    /// + palette runs each frame by `mesh_instances::plan_mesh_frame`.
    pub(super) mesh_draws: Vec<mesh_instances::MeshInstanceInput>,

    /// Reusable bone-palette scratch for per-frame per-instance sampling.
    /// `sample_clip` clears then refills it per instance, so steady-state frames
    /// allocate nothing. Lives on the renderer (not in the GPU pass) — it is
    /// CPU-side pose data the pass merely uploads.
    pub(super) bone_palette_scratch: Vec<crate::model::BonePaletteEntry>,

    /// Wall-clock of the last palette/instance-overflow warning (render clock),
    /// for rate-limiting (mirrors `EmitterBridge`'s `last_warn_time`). Overflow
    /// drops the excess instances; the warning fires at most once per second.
    pub(super) mesh_overflow_last_warn: f32,

    /// CPU-side count of skinned ENTITY occluder instances submitted into spot
    /// shadow slots last frame, summed across slots (each instance counted once
    /// per slot it casts into). Mirrors `shadow-cone-cull`'s submitted-instance
    /// counter — no GPU readback. Verifies the "enemy outside the cone is not
    /// drawn" acceptance criterion: an instance the per-light cone cull rejects
    /// is never added here. Reset to 0 at the start of the spot-shadow depth loop.
    pub(super) spot_entity_occluders_submitted: u32,

    /// CPU-side count of skinned ENTITY occluder instances submitted into CUBE
    /// (point-light) shadow faces last frame, summed across all occupied slots ×
    /// 6 faces (each instance counted once per face it casts into). Mirrors
    /// `spot_entity_occluders_submitted` — no GPU readback. Verifies that
    /// entity occluders render only for `entity_occluder_eligible` point lights
    /// and only when their bound intersects a face frustum. Reset to 0 at the
    /// start of the cube-shadow depth loop.
    pub(super) cube_entity_occluders_submitted: u32,

    /// Instanced UI quad / 9-slice pass for panels and images plus glyphon text.
    /// Built alongside `fog`; records the splash (splash phase) and an empty draw
    /// list on the gameplay path (`render_frame_indirect`). Owns all UI GPU state.
    pub(super) ui: ui::UiPass,

    /// The active splash logo's natural reference size (logical-reference px,
    /// `[width, height]`), derived from the uploaded texture's decoded pixel dims.
    /// `Some` between `install_splash_from_loaded` and `clear_splash`; the splash
    /// descriptor tree is rebuilt each frame and this size threads into the
    /// measure seam (keyed by `splash::SPLASH_LOGO_ASSET`) so the logo `image`
    /// node sizes content-driven from the real asset. `None` (frame 0 before
    /// install, and after level handoff) records no splash quads.
    pub(super) splash_logo_size: Option<[f32; 2]>,

    /// Key→bind-group registry for `image` widget assets (only the
    /// pre-registered splash logo key). `install_splash_from_loaded` registers
    /// the uploaded logo PNG under `splash::SPLASH_LOGO_ASSET`; the UI pass
    /// resolves image batches' asset keys through it. Cleared by `clear_splash`.
    pub(super) ui_images: ui::UiImageRegistry,

    /// Once-per-frame published read snapshot: the splash version/tagline line
    /// and the gameplay-path descriptor tree. Set by the App via `set_ui_snapshot`
    /// just before each render call; read when the UI pass records. Stored here so
    /// both render signatures stay stable.
    pub(super) ui_snapshot: ui::UiReadSnapshot,

    /// Active UI theme: the token table every descriptor tree resolves its
    /// color/spacing/font slots against at build time. Defaults to
    /// `UiTheme::engine_default()` at construction; `set_ui_theme` installs an
    /// override (e.g. from a mod's theme document) and bumps `ui_theme_generation`.
    /// Both render paths (`record_splash_ui`, the gameplay block) resolve against
    /// this same instance — the splash's literals resolve to themselves, so the
    /// splash output is unchanged.
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
