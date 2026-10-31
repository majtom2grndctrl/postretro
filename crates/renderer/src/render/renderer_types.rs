// Core renderer types: the boot-phase Renderer struct, presentation and
// capture plain types, GpuTexture, and shared rendering constants.
// See: context/lib/rendering_pipeline.md

mod diagnostics;
mod full_renderer;
mod level_geometry;
mod promoted_lights;

use super::*;

#[cfg(feature = "dev-tools")]
pub use diagnostics::AgentOverlayState;
pub use diagnostics::{
    BvhOverlayBudget, BvhOverlayColorMode, BvhOverlayDepthMode, BvhOverlayState,
    CameraCullDiagnostics, CameraCullPath, CellOverlayState, LocatorDiagnostics,
    PortalOverlayState, SpatialCellSetDiagnostics, SpatialDiagnostics, WorldWireframeMode,
};
pub(super) use full_renderer::FullRenderer;
pub(super) use full_renderer::sh_sampler_lattice;
pub use level_geometry::{LevelGeometry, LevelGeometryLightmapStreaming, LevelGeometryShStorage};
pub(super) use promoted_lights::animated_baked_promotion_weight;
pub(crate) use promoted_lights::{
    MAX_ANIMATED_BAKED_LIGHTS, MAX_PROMOTED_CUBE, MAX_PROMOTED_SPOT, PromotedBakedLightRecord,
    PromotedBakedLightSource, PromotedBakedLightState, PromotedShadowPoolKind,
};

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ClearColor {
    pub r: f64,
    pub g: f64,
    pub b: f64,
    pub a: f64,
}

/// The boot splash background: the linear form of the splash art's uniform
/// sRGB 8-bit `(28, 33, 39)` background, for an sRGB attachment. The boot
/// splash pass clears to it, and Loading frames clear to it too so the
/// splash→loading-screen handoff has no color step. Derivation:
/// `splash_pass.rs`.
pub const SPLASH_CLEAR_COLOR: ClearColor = ClearColor {
    r: 0.011612,
    g: 0.015209,
    b: 0.020289,
    a: 1.0,
};

/// Adapter identity retained as plain data for capture measurement reports.
///
/// The renderer obtains this while it still owns the `wgpu::Adapter`; callers
/// never receive a GPU handle or descriptor.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CaptureAdapterIdentity {
    pub name: String,
    pub backend: String,
    pub device_type: String,
}

/// Why capture can or cannot collect timestamp-query windows.
///
/// `Active` only means the renderer has a readable timing seam. A report still
/// waits for a full 120-readback window before calling timing `available`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CaptureGpuTimingState {
    NotRequested,
    Unsupported,
    PlainBuildUnavailable,
    Active,
}

/// One renderer-owned pass value from a completed GPU timing window.
#[derive(Debug, Clone, PartialEq)]
pub struct CaptureGpuTimingPass {
    pub label: &'static str,
    /// Mean over `sampled_readbacks` only; `None` when the pass ran in none.
    pub average_ms: Option<f32>,
    /// Readbacks in which the pass ran and decoded a sample.
    pub sampled_readbacks: u32,
    /// Readbacks in which the pass ran but decoded malformed.
    pub malformed_readbacks: u32,
}

/// A completed 120-readback GPU timing window made safe for non-renderer code.
#[derive(Debug, Clone, PartialEq)]
pub struct CaptureGpuTimingWindow {
    /// Readbacks in the window; bounds every pass's `sampled_readbacks`.
    pub readbacks: u32,
    pub passes: Vec<CaptureGpuTimingPass>,
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

    /// wgpu 30 presents through the queue, not the surface texture.
    pub(super) fn present(self, queue: &wgpu::Queue) {
        queue.present(self.output);
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

/// One installed world/mover material: its group-1 bind group plus the uniform
/// buffer behind binding 3 and the GPU-free plan that fills it.
///
/// The buffer handle is retained so the player's Surface Depth switch can be
/// applied live by rewriting buffer contents rather than rebuilding bind
/// groups. Ownership follows the level: this vector is replaced wholesale by
/// `install_textures` and dropped with the level, so there is still no
/// reference counting and nothing to release by hand (`resource_management.md`
/// §8.2).
///
/// Skinned models deliberately do NOT flow through here: they are out of
/// Surface Depth's scope, they bind `Material::Default` against a
/// neutral single-channel specular placeholder, and their bind groups are
/// owned by the mesh pass.
pub(crate) struct GpuTexture {
    pub(super) bind_group: wgpu::BindGroup,
    pub(super) uniform_buffer: wgpu::Buffer,
    pub(super) uniform_plan: postretro_render_cpu::material_plan::MaterialUniformPlan,
}

impl From<super::material_plan::MaterialBinding> for GpuTexture {
    fn from(binding: super::material_plan::MaterialBinding) -> Self {
        Self {
            bind_group: binding.bind_group,
            uniform_buffer: binding.uniform_buffer,
            uniform_plan: binding.uniform_plan,
        }
    }
}

/// Hardware anisotropy cap for the Post Retro filtering pool. wgpu requires
/// `anisotropy_clamp >= 1`; 16 is the common ceiling exposed by desktop adapters
/// and the visual point of diminishing returns for grazing-angle sharpness.
pub const POST_RETRO_ANISO_CLAMP: u16 = 16;

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
    pub(super) queue: UploadQueue,
    #[cfg(test)]
    pub(super) injected_acquire_failure: bool,
    /// Present surface for a windowed renderer. Offscreen capture deliberately
    /// has no surface and never reaches the present/splash paths.
    pub(super) surface: Option<wgpu::Surface<'static>>,
    pub(super) surface_config: wgpu::SurfaceConfiguration,
    /// Recorded window size, scale factor and render resolution, and the
    /// surface and scene extents last committed from them. `surface_config`
    /// mirrors the committed surface; every scene target sizes to the
    /// committed scene extent (`renderer_extent.rs`).
    pub(super) extent_state: postretro_render_cpu::render_extent::ExtentState,
    pub(super) is_surface_configured: bool,
    pub(super) surface_reconfigure_pending: bool,
    /// CPU stage timing gate, handed in by the binary after construction.
    pub(super) cpu_timing: postretro_stage_timing::TimingGate,
    /// Duration of the last surface texture request, when timing is on. Taken
    /// into the frame result so the binary can count the vsync block as wait.
    pub(super) last_acquire_nanos: Option<u64>,
    /// This frame's CPU stage values. Shared so a scope can span `&mut self`
    /// helper calls; cleared at each frame's first recording step.
    pub(super) cpu_frame:
        std::rc::Rc<postretro_stage_timing::StageFrame<super::cpu_stages::RenderStage>>,

    /// `has_multi_draw_indirect` flag cached for `finish_full_init` and
    /// `install_level_geometry`. Boot-phase: derived from the adapter, needed to
    /// build the full renderer and re-build it across surface recreation.
    pub(super) has_multi_draw_indirect: bool,
    /// Adapter `CUBE_ARRAY_TEXTURES` support, cached at boot so `finish_full_init`
    /// can rebuild the full renderer (cube shadow pool + shared group-5 BGL) from
    /// boot state alone. `Some` cube pool iff this is true (see `FullRenderer`).
    pub(super) cube_array_supported: bool,

    /// Plain capture-report identity retained at adapter selection time.
    pub(super) capture_adapter_identity: CaptureAdapterIdentity,
    /// Timestamp-query setup state for a capture report. This distinguishes an
    /// inactive request from an adapter or plain-build limitation before any
    /// window-progress analysis.
    pub(super) capture_gpu_timing_state: CaptureGpuTimingState,

    /// Static bloom style cached in boot state so `finish_full_init` rebuilds
    /// the full renderer with the last committed profile after surface recovery.
    pub(super) bloom_render_profile: BloomRenderProfile,

    /// The mod's fluid reflection style, cached in boot state like the bloom
    /// profile so `finish_full_init` hands a rebuilt fluid pass the last
    /// committed style. Offscreen capture never sets it: pixel, the default.
    pub(super) fluid_reflection_style:
        postretro_render_cpu::fluid_surface::reflection_style::FluidReflectionStyle,

    /// The mod's ghost transparency (`render.ghosts`), boot state like the
    /// reflection style so a full-renderer rebuild keeps it. The ghost passes
    /// read it to pick each batch's pipeline. Offscreen capture never sets it:
    /// blend, the default.
    pub(super) ghost_transparency: postretro_render_cpu::ghost_transparency::GhostTransparency,

    /// The player's fluid reflection tier. Boot state, like Surface Depth, so
    /// a change before full init or with no level holds until the next full
    /// init and fluid install consult it, and a full-renderer rebuild after
    /// surface recovery keeps it. Applies live on a full renderer.
    pub(super) reflection_quality:
        postretro_render_cpu::fluid_surface::reflection_quality::FluidReflectionQuality,

    /// Spot shadow-map resolution cached in boot state. Configuration updates
    /// affect the next full-renderer construction or level install; they never
    /// rebuild live GPU resources from the setter.
    pub(super) spot_shadow_map_resolution: u32,

    /// Player-facing Surface Depth switch. Boot state, like the
    /// bloom profile, so a full-renderer rebuild after surface recovery keeps
    /// the player's choice instead of silently returning to the default.
    ///
    /// Unlike the shadow tier this DOES apply live: the setter rewrites every
    /// installed material's uniform buffer. Holding it here is also what makes
    /// a change with no level loaded correct — `install_textures` reads it when
    /// the next level's materials are built.
    pub(super) surface_depth_quality: postretro_render_cpu::surface_depth::SurfaceDepthQuality,

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

impl Renderer {
    /// The renderer-accounted SH residency report for the installed level.
    ///
    /// Static allocation rows are copied from install time, while a streaming
    /// level overlays its current pool snapshot so capture observes growth and
    /// retirement after warmup rather than stale install-time capacity.
    pub fn sh_residency_report(&self) -> Option<ShResidencyReport> {
        let level = self.full.as_ref()?.level.as_ref()?;
        let report = level.sh_residency_report.clone();
        match level.sh_streaming.as_ref() {
            Some(streaming) => {
                Some(report.with_streaming_summary(streaming.streaming_allocation_summary()))
            }
            None => Some(report),
        }
    }

    /// Resident bytes of every lightmap-family texture currently bound.
    pub fn lightmap_residency_report(&self) -> Option<&LightmapResidencyReport> {
        Some(
            &self
                .full
                .as_ref()?
                .level
                .as_ref()?
                .lightmap_residency_report,
        )
    }

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
