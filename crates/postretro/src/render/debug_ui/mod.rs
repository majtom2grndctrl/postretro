// Debug UI overlay: egui context, winit event bridge, diagnostics panel.
// See: context/lib/rendering_pipeline.md §12 · context/lib/input.md §7

use winit::event::WindowEvent;
use winit::window::Window;

use super::BvhOverlayBudget;
use super::BvhOverlayColorMode;
use super::BvhOverlayDepthMode;
use super::CameraCullPath;
use super::CellOverlayState;
use super::DynamicDirectIsolation;
use super::LightingIsolation;
use super::PortalOverlayState;
use super::Renderer;
use super::SdfShadowMode;
use super::WorldWireframeMode;
use super::frame_timing::FrameTimingSnapshot;
use super::sh_diagnostics::{MarkerMode, ShDiagnosticsState};

/// GPU-side egui state. Lives on `Renderer` (the GPU boundary), constructed
/// lazily on first panel open via `Renderer::ensure_debug_ui_gpu`. The CPU
/// half (`DebugUi`) lives on `App`.
pub struct DebugUiGpu {
    pub renderer: egui_wgpu::Renderer,
}

impl DebugUiGpu {
    /// Constructs `egui_wgpu::Renderer` against the swapchain format.
    /// No depth attachment, no MSAA, no dithering — the engine renders egui
    /// as a 2D overlay after the world draw.
    pub fn new(device: &wgpu::Device, surface_format: wgpu::TextureFormat) -> Self {
        let options = egui_wgpu::RendererOptions {
            msaa_samples: 1,
            depth_stencil_format: None,
            dithering: false,
            predictable_texture_filtering: false,
        };
        Self {
            renderer: egui_wgpu::Renderer::new(device, surface_format, options),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DiagnosticsTab {
    Lighting,
    Volumes,
    Performance,
    Spatial,
}

impl DiagnosticsTab {
    const ALL: [Self; 4] = [
        Self::Lighting,
        Self::Volumes,
        Self::Performance,
        Self::Spatial,
    ];

    const fn label(self) -> &'static str {
        match self {
            Self::Lighting => "Lighting",
            Self::Volumes => "Volumes",
            Self::Performance => "Performance",
            Self::Spatial => "Spatial",
        }
    }
}

/// Diagnostics-panel widget state. The panel binds these to renderer setters
/// each frame; default values mirror the renderer's stock values so the panel
/// reads sensibly before any user interaction has happened.
pub struct DiagnosticsState {
    pub selected_tab: DiagnosticsTab,
    pub ambient_floor: f32,
    pub indirect_scale: f32,
    /// Scale slider for the baked static-direct SH term on entities/billboards.
    /// Independent of `indirect_scale` (which controls the static-surface indirect).
    pub dynamic_direct_scale: f32,
    // SDF and fog controls are seeded from live renderer values on first draw;
    // see the `seeded` flag below.
    pub sdf_max_march_steps: u32,
    pub sdf_open_space_skip_threshold: f32,
    pub sdf_penumbra_k: f32,
    pub sdf_surface_bias: f32,
    pub fog_step_size: f32,
    pub fog_pixel_scale: u32,
    pub spatial_wireframe_mode: WorldWireframeMode,
    pub bvh_overlay_visible: bool,
    pub bvh_color_mode: BvhOverlayColorMode,
    pub bvh_depth_mode: BvhOverlayDepthMode,
    pub bvh_budget: BvhOverlayBudget,
    pub cell_overlay_visible: bool,
    pub cell_depth_mode: BvhOverlayDepthMode,
    pub portal_overlay_visible: bool,
    pub portal_depth_mode: BvhOverlayDepthMode,
    /// Tracks whether the slider state has been seeded from the live renderer
    /// values. The first time the panel renders, it pulls current values so
    /// the sliders don't snap the world to defaults on first open.
    seeded: bool,
}

impl Default for DiagnosticsState {
    fn default() -> Self {
        let cell_overlay = CellOverlayState::default();
        let portal_overlay = PortalOverlayState::default();
        Self {
            selected_tab: DiagnosticsTab::Lighting,
            ambient_floor: super::DEFAULT_AMBIENT_FLOOR,
            indirect_scale: super::DEFAULT_INDIRECT_SCALE,
            dynamic_direct_scale: super::DEFAULT_DYNAMIC_DIRECT_SCALE,
            // Placeholder values overwritten by the seed-from-renderer pass on
            // first draw (see `draw_diagnostics_panel`). Match the SDF /
            // fog defaults so the struct is still legible in isolation.
            sdf_max_march_steps: super::sdf_shadow::DEFAULT_MAX_MARCH_STEPS,
            sdf_open_space_skip_threshold: super::sdf_shadow::DEFAULT_OPEN_SPACE_SKIP_THRESHOLD,
            sdf_penumbra_k: super::sdf_shadow::DEFAULT_PENUMBRA_K,
            sdf_surface_bias: super::sdf_shadow::DEFAULT_SURFACE_BIAS_VOXELS,
            fog_step_size: crate::fx::fog_volume::DEFAULT_FOG_STEP_SIZE,
            fog_pixel_scale: 4,
            spatial_wireframe_mode: WorldWireframeMode::Off,
            bvh_overlay_visible: false,
            bvh_color_mode: BvhOverlayColorMode::CellId,
            bvh_depth_mode: BvhOverlayDepthMode::DepthTested,
            bvh_budget: BvhOverlayBudget::default(),
            cell_overlay_visible: cell_overlay.visible,
            cell_depth_mode: cell_overlay.depth_mode,
            portal_overlay_visible: portal_overlay.visible,
            portal_depth_mode: portal_overlay.depth_mode,
            seeded: false,
        }
    }
}

/// CPU-side egui state. Lives on `App` as `Option<DebugUi>`.
/// Initialized in `resumed()` (needs device `max_texture_dimension_2d`).
/// GPU half (`DebugUiGpu`) lives on `Renderer`; lazy-initialized on first panel open.
pub struct DebugUi {
    pub ctx: egui::Context,
    pub winit_state: egui_winit::State,
    visible: bool,
    pub panel_state: DiagnosticsState,
    pub sh_diagnostics_state: ShDiagnosticsState,
}

impl DebugUi {
    pub fn new(window: &Window, max_texture_side: u32) -> Self {
        let ctx = egui::Context::default();
        let winit_state = egui_winit::State::new(
            ctx.clone(),
            egui::ViewportId::ROOT,
            window,
            Some(window.scale_factor() as f32),
            None,
            Some(max_texture_side as usize),
        );
        Self {
            ctx,
            winit_state,
            visible: false,
            panel_state: DiagnosticsState::default(),
            sh_diagnostics_state: ShDiagnosticsState::default(),
        }
    }

    pub fn on_window_event(
        &mut self,
        window: &Window,
        event: &WindowEvent,
    ) -> egui_winit::EventResponse {
        self.winit_state.on_window_event(window, event)
    }

    pub fn set_visible(&mut self, v: bool) {
        self.visible = v;
    }

    pub fn is_visible(&self) -> bool {
        self.visible
    }
}

/// Renders the Diagnostics panel for one frame. Writes through `renderer`
/// setters when sliders / dropdowns change, so the world picks up the new
/// values on the next `update_per_frame_uniforms` upload.
///
/// `frame_timing` is the most recent averaged GPU-timing window, or `None`
/// when GPU timing is disabled. When present-but-empty (zero passes), the
/// "unavailable" line still renders — defensive against an empty
/// `pass_labels` vec slipping past construction.
pub fn draw_diagnostics_panel(
    ctx: &egui::Context,
    state: &mut DiagnosticsState,
    sh_state: &mut ShDiagnosticsState,
    renderer: &mut Renderer,
    frame_timing: Option<&FrameTimingSnapshot>,
) {
    // Seed slider state from live renderer values on first draw so toggling
    // the panel open does not snap ambient floor / indirect scale to whatever
    // defaults `DiagnosticsState` was constructed with.
    if !state.seeded {
        state.ambient_floor = renderer.ambient_floor();
        state.indirect_scale = renderer.indirect_scale();
        state.dynamic_direct_scale = renderer.dynamic_direct_scale();
        // Pull live SDF and fog tuning so sliders open at the engine's current
        // values, not the struct-default placeholders.
        state.sdf_max_march_steps = renderer.sdf_max_march_steps();
        state.sdf_open_space_skip_threshold = renderer.sdf_open_space_skip_threshold();
        state.sdf_penumbra_k = renderer.sdf_penumbra_k();
        state.sdf_surface_bias = renderer.sdf_surface_bias();
        state.fog_step_size = renderer.fog_step_size();
        state.fog_pixel_scale = renderer.fog_pixel_scale();
        state.spatial_wireframe_mode = renderer.world_wireframe_mode();
        let bvh_overlay = renderer.bvh_overlay_state();
        state.bvh_overlay_visible = bvh_overlay.visible;
        state.bvh_color_mode = bvh_overlay.color_mode;
        state.bvh_depth_mode = bvh_overlay.depth_mode;
        state.bvh_budget = bvh_overlay.budget;
        let cell_overlay = renderer.cell_overlay_state();
        state.cell_overlay_visible = cell_overlay.visible;
        state.cell_depth_mode = cell_overlay.depth_mode;
        let portal_overlay = renderer.portal_overlay_state();
        state.portal_overlay_visible = portal_overlay.visible;
        state.portal_depth_mode = portal_overlay.depth_mode;
        state.seeded = true;
    }
    state.spatial_wireframe_mode = renderer.world_wireframe_mode();

    egui::Window::new("Diagnostics").show(ctx, |ui| {
        draw_tab_selector(ui, state);
        ui.separator();

        match state.selected_tab {
            DiagnosticsTab::Lighting => draw_lighting_tab(ui, state, renderer),
            DiagnosticsTab::Volumes => draw_volumes_tab(ui, state, sh_state, renderer),
            DiagnosticsTab::Performance => draw_performance_tab(ui, frame_timing),
            DiagnosticsTab::Spatial => draw_spatial_tab(ui, state, renderer),
        }
    });
}

fn draw_tab_selector(ui: &mut egui::Ui, state: &mut DiagnosticsState) {
    ui.horizontal(|ui| {
        for tab in DiagnosticsTab::ALL {
            ui.selectable_value(&mut state.selected_tab, tab, tab.label());
        }
    });
}

fn draw_lighting_tab(ui: &mut egui::Ui, state: &mut DiagnosticsState, renderer: &mut Renderer) {
    egui::CollapsingHeader::new("Lighting systems")
        .default_open(true)
        .show(ui, |ui| {
            ui.label("Ambient Floor");
            if ui
                .add(egui::Slider::new(&mut state.ambient_floor, 0.0_f32..=1.0))
                .changed()
            {
                renderer.set_ambient_floor(state.ambient_floor);
            }

            ui.label("Indirect Scale");
            if ui
                .add(egui::Slider::new(&mut state.indirect_scale, 0.0_f32..=1.0))
                .changed()
            {
                renderer.set_indirect_scale(state.indirect_scale);
            }

            // Dynamic baked-static-direct SH controls (entities + billboards).
            // Separate from the forward Indirect Scale / Lighting Isolation
            // above so the dynamic-vs-static parity comparison stays valid.
            ui.label("Dynamic Direct Scale");
            if ui
                .add(egui::Slider::new(
                    &mut state.dynamic_direct_scale,
                    0.0_f32..=1.0,
                ))
                .changed()
            {
                renderer.set_dynamic_direct_scale(state.dynamic_direct_scale);
            }

            let mut dyn_iso = renderer.dynamic_direct_isolation();
            let prev_dyn_iso = dyn_iso;
            egui::ComboBox::from_label("Dynamic Direct Isolation")
                .selected_text(dyn_iso.label())
                .show_ui(ui, |ui| {
                    for variant in DynamicDirectIsolation::ALL_VARIANTS {
                        ui.selectable_value(&mut dyn_iso, variant, variant.label());
                    }
                });
            if dyn_iso != prev_dyn_iso {
                renderer.set_dynamic_direct_isolation(dyn_iso);
            }

            let mut probe_occlusion = renderer.probe_occlusion_enabled();
            if ui
                .checkbox(&mut probe_occlusion, "Probe Occlusion")
                .changed()
            {
                renderer.set_probe_occlusion_enabled(probe_occlusion);
            }

            let mut mode = renderer.lighting_isolation();
            let prev_mode = mode;
            egui::ComboBox::from_label("Lighting Isolation")
                .selected_text(mode.label())
                .show_ui(ui, |ui| {
                    for variant in LightingIsolation::ALL_VARIANTS {
                        ui.selectable_value(&mut mode, variant, variant.label());
                    }
                });
            if mode != prev_mode {
                renderer.set_lighting_isolation(mode);
            }

            // Panel-only, no keyboard chord; mirrors the LightingIsolation
            // dropdown shape directly above. `Off` disables the SDF factor
            // multiply (shadow-map / enemy shadows are unaffected);
            // `Visualize` swaps the shaded color for a grayscale view of
            // the static-aggregate (R) shadow factor.
            let mut sdf_mode = renderer.sdf_shadow_mode();
            let prev_sdf_mode = sdf_mode;
            egui::ComboBox::from_label("SDF Shadow Mode")
                .selected_text(sdf_mode.label())
                .show_ui(ui, |ui| {
                    for variant in SdfShadowMode::ALL_VARIANTS {
                        ui.selectable_value(&mut sdf_mode, variant, variant.label());
                    }
                });
            if sdf_mode != prev_sdf_mode {
                renderer.set_sdf_shadow_mode(sdf_mode);
            }

            // Forces per-light SDF visibility to 1.0 for no-double-count A/B
            // checks. With every SDF light fully lit, additive per-light
            // diffuse should match the unshadowed direct term. Also settable
            // headless via POSTRETRO_SDF_FORCE_VISIBILITY_ONE=1.
            let mut force_vis = renderer.sdf_force_visibility_one();
            if ui
                .checkbox(&mut force_vis, "SDF: force visibility 1.0")
                .changed()
            {
                renderer.set_sdf_force_visibility_one(force_vis);
            }

            // Pins `uniforms.time` so all curve-driven animation holds still.
            // Diagnostic aid: if a flickering artifact freezes too, it is
            // time/animation-driven; if it keeps moving, it is not.
            let mut frozen = renderer.freeze_time();
            if ui.checkbox(&mut frozen, "Freeze animation time").changed() {
                renderer.set_freeze_time(frozen);
            }
        });
}

fn draw_volumes_tab(
    ui: &mut egui::Ui,
    state: &mut DiagnosticsState,
    sh_state: &mut ShDiagnosticsState,
    renderer: &mut Renderer,
) {
    let has_sh = renderer.has_sh_volume();
    let delta_count = renderer.sh_delta_volumes().len();
    if !sh_state.seeded {
        if sh_state.per_light_visible.len() != delta_count {
            sh_state.per_light_visible.clear();
            sh_state.per_light_visible.resize(delta_count, false);
        }
        sh_state.seeded = true;
    }

    egui::CollapsingHeader::new("SH Volumes")
        .default_open(true)
        .show(ui, |ui| {
            if !has_sh {
                ui.label("No SH volume baked");
                return;
            }

            ui.checkbox(&mut sh_state.show_base_aabb, "Show base volume AABB");
            ui.checkbox(&mut sh_state.show_cells, "Show base-grid cells");
            ui.checkbox(&mut sh_state.show_markers, "Show per-probe markers");

            ui.horizontal(|ui| {
                ui.label("Marker mode");
                ui.radio_value(&mut sh_state.marker_mode, MarkerMode::Validity, "Validity");
                ui.radio_value(&mut sh_state.marker_mode, MarkerMode::Uniform, "Uniform");
                ui.radio_value(
                    &mut sh_state.marker_mode,
                    MarkerMode::Irradiance,
                    "Irradiance",
                );
            });

            ui.label("Marker scale");
            ui.add(egui::Slider::new(
                &mut sh_state.marker_scale,
                0.05_f32..=2.0,
            ));

            ui.label("Overlay radius (world units)");
            ui.add(egui::Slider::new(&mut sh_state.cell_radius, 0.0_f32..=64.0));

            ui.separator();
            ui.label("Animated light delta volumes");
            if delta_count == 0 {
                ui.label("(no animated lights)");
            } else {
                if sh_state.per_light_visible.len() != delta_count {
                    sh_state.per_light_visible.resize(delta_count, false);
                }
                ui.horizontal(|ui| {
                    if ui.button("All on").clicked() {
                        sh_state.per_light_visible.fill(true);
                    }
                    if ui.button("All off").clicked() {
                        sh_state.per_light_visible.fill(false);
                    }
                });
                for (i, visible) in sh_state.per_light_visible.iter_mut().enumerate() {
                    ui.checkbox(visible, format!("Delta light #{i}"));
                }
            }
        });

    // Two feasibility classes per the plan:
    //   * Uniform scalars (SDF max march steps, open-space skip threshold,
    //     penumbra k, fog step_size) — write through to a per-frame uniform
    //     on the next dispatch / upload, no resource rebuild.
    //   * Resolution / allocation (fog_pixel_scale) — drives
    //     `set_fog_pixel_scale`, which rebuilds the scatter target and
    //     bind group. The renderer setter is a no-op when unchanged.
    egui::CollapsingHeader::new("SDF / Fog Quality")
        .default_open(true)
        .show(ui, |ui| {
            ui.label("SDF max march steps");
            if ui
                .add(egui::Slider::new(
                    &mut state.sdf_max_march_steps,
                    16_u32..=128,
                ))
                .changed()
            {
                renderer.set_sdf_max_march_steps(state.sdf_max_march_steps);
            }

            ui.label("SDF open-space skip threshold (× SH cell)");
            if ui
                .add(egui::Slider::new(
                    &mut state.sdf_open_space_skip_threshold,
                    0.0_f32..=8.0,
                ))
                .changed()
            {
                renderer.set_sdf_open_space_skip_threshold(state.sdf_open_space_skip_threshold);
            }

            ui.label("SDF penumbra k (larger = harder shadow)");
            if ui
                .add(egui::Slider::new(&mut state.sdf_penumbra_k, 1.0_f32..=64.0))
                .changed()
            {
                renderer.set_sdf_penumbra_k(state.sdf_penumbra_k);
            }

            ui.label("SDF surface bias (× voxel, along normal)");
            if ui
                .add(egui::Slider::new(
                    &mut state.sdf_surface_bias,
                    0.0_f32..=8.0,
                ))
                .changed()
            {
                renderer.set_sdf_surface_bias(state.sdf_surface_bias);
            }

            ui.separator();

            ui.label("Fog step size (world units)");
            if ui
                .add(egui::Slider::new(&mut state.fog_step_size, 0.05_f32..=2.0))
                .changed()
            {
                renderer.set_fog_step_size(state.fog_step_size);
            }

            // fog_pixel_scale is the resource-rebuild knob: integer
            // downscale factor (1 = full res, 8 = max blocky). The
            // renderer setter is a no-op when unchanged, so dragging the
            // slider through unchanged values doesn't thrash the scatter
            // target's bind-group construction.
            ui.label("Fog pixel scale (1 = full res, higher = blockier)");
            if ui
                .add(egui::Slider::new(&mut state.fog_pixel_scale, 1_u32..=8))
                .changed()
            {
                renderer.set_fog_pixel_scale(state.fog_pixel_scale);
            }
        });
}

fn draw_performance_tab(ui: &mut egui::Ui, frame_timing: Option<&FrameTimingSnapshot>) {
    egui::CollapsingHeader::new("GPU Timing")
        .default_open(true)
        .show(ui, |ui| match frame_timing {
            Some(snapshot) if !snapshot.passes.is_empty() => {
                for (label, avg_ms, _skip) in &snapshot.passes {
                    ui.label(format!("{label}: {avg_ms:.2} ms"));
                }
            }
            _ => {
                ui.label("GPU timing unavailable");
            }
        });
}

fn draw_spatial_tab(ui: &mut egui::Ui, state: &mut DiagnosticsState, renderer: &mut Renderer) {
    egui::CollapsingHeader::new("BVH cull baseline (full tree walk)")
        .default_open(true)
        .show(ui, |ui| {
            let Some(diagnostics) = renderer.visibility_diagnostics() else {
                ui.label("No BVH cull data loaded");
                return;
            };

            ui.label("Would-be tree-walk cost, regardless of the active cull path.");

            egui::Grid::new("visibility_diagnostics_grid")
                .num_columns(2)
                .striped(true)
                .show(ui, |ui| {
                    ui.label("BVH node visits");
                    ui.label(diagnostics.estimated_node_visits.to_string());
                    ui.end_row();

                    ui.label("Leaf tests");
                    ui.label(diagnostics.leaf_tests.to_string());
                    ui.end_row();

                    ui.label("Frustum rejects");
                    ui.label(diagnostics.frustum_rejects.to_string());
                    ui.end_row();

                    ui.label("Visible-cell rejects");
                    ui.label(diagnostics.visible_cell_rejects.to_string());
                    ui.end_row();

                    ui.label("Submitted leaves");
                    ui.label(diagnostics.submitted_leaves.to_string());
                    ui.end_row();

                    ui.label("Submitted indices");
                    ui.label(diagnostics.submitted_index_count.to_string());
                    ui.end_row();

                    ui.label("Submitted bucket spans");
                    ui.label(diagnostics.submitted_bucket_spans.to_string());
                    ui.end_row();

                    ui.label("Frontier subtrees");
                    ui.label(diagnostics.frontier.frontier_subtrees.to_string());
                    ui.end_row();

                    ui.label("Frontier work");
                    ui.label(diagnostics.frontier.total_estimated_work.to_string());
                    ui.end_row();

                    ui.label("Max subtree work");
                    ui.label(diagnostics.frontier.max_subtree_work.to_string());
                    ui.end_row();

                    ui.label("Imbalance");
                    ui.label(format!("{:.2}x", diagnostics.frontier.imbalance_ratio));
                    ui.end_row();
                });
        });

    egui::CollapsingHeader::new("World triangle wireframe")
        .default_open(true)
        .show(ui, |ui| {
            let prev_mode = state.spatial_wireframe_mode;
            egui::ComboBox::from_label("Mode")
                .selected_text(state.spatial_wireframe_mode.label())
                .show_ui(ui, |ui| {
                    for mode in WorldWireframeMode::ALL_VARIANTS {
                        ui.selectable_value(&mut state.spatial_wireframe_mode, mode, mode.label());
                    }
                });
            if state.spatial_wireframe_mode != prev_mode {
                renderer.set_world_wireframe_mode(state.spatial_wireframe_mode);
            }
        });

    egui::CollapsingHeader::new("BSP cell bounds")
        .default_open(true)
        .show(ui, |ui| {
            if ui
                .checkbox(
                    &mut state.cell_overlay_visible,
                    "Show drawable BSP cell bounds",
                )
                .changed()
            {
                renderer.set_cell_overlay_visible(state.cell_overlay_visible);
            }

            let prev_depth_mode = state.cell_depth_mode;
            egui::ComboBox::from_label("Cell depth")
                .selected_text(state.cell_depth_mode.label())
                .show_ui(ui, |ui| {
                    for mode in BvhOverlayDepthMode::ALL_VARIANTS {
                        ui.selectable_value(&mut state.cell_depth_mode, mode, mode.label());
                    }
                });
            if state.cell_depth_mode != prev_depth_mode {
                renderer.set_cell_overlay_depth_mode(state.cell_depth_mode);
            }

            ui.label("Visible: green, hidden: blue, DrawAll fallback: amber");
        });

    egui::CollapsingHeader::new("Portals")
        .default_open(true)
        .show(ui, |ui| {
            if ui
                .checkbox(&mut state.portal_overlay_visible, "Show portal edges")
                .changed()
            {
                renderer.set_portal_overlay_visible(state.portal_overlay_visible);
            }

            let prev_depth_mode = state.portal_depth_mode;
            egui::ComboBox::from_label("Portal depth")
                .selected_text(state.portal_depth_mode.label())
                .show_ui(ui, |ui| {
                    for mode in BvhOverlayDepthMode::ALL_VARIANTS {
                        ui.selectable_value(&mut state.portal_depth_mode, mode, mode.label());
                    }
                });
            if state.portal_depth_mode != prev_depth_mode {
                renderer.set_portal_overlay_depth_mode(state.portal_depth_mode);
            }
        });

    egui::CollapsingHeader::new("BVH leaf AABBs")
        .default_open(true)
        .show(ui, |ui| {
            if ui
                .checkbox(
                    &mut state.bvh_overlay_visible,
                    "Show compiled BVH leaf boxes",
                )
                .changed()
            {
                renderer.set_bvh_overlay_visible(state.bvh_overlay_visible);
            }

            let prev_color_mode = state.bvh_color_mode;
            egui::ComboBox::from_label("Color")
                .selected_text(state.bvh_color_mode.label())
                .show_ui(ui, |ui| {
                    for mode in BvhOverlayColorMode::ALL_VARIANTS {
                        ui.selectable_value(&mut state.bvh_color_mode, mode, mode.label());
                    }
                });
            if state.bvh_color_mode != prev_color_mode {
                renderer.set_bvh_overlay_color_mode(state.bvh_color_mode);
            }

            let prev_depth_mode = state.bvh_depth_mode;
            egui::ComboBox::from_label("Depth")
                .selected_text(state.bvh_depth_mode.label())
                .show_ui(ui, |ui| {
                    for mode in BvhOverlayDepthMode::ALL_VARIANTS {
                        ui.selectable_value(&mut state.bvh_depth_mode, mode, mode.label());
                    }
                });
            if state.bvh_depth_mode != prev_depth_mode {
                renderer.set_bvh_overlay_depth_mode(state.bvh_depth_mode);
            }

            ui.separator();

            ui.label("Max boxes");
            let mut max_boxes = state.bvh_budget.max_boxes as u32;
            if ui
                .add(egui::Slider::new(&mut max_boxes, 1_u32..=4096))
                .changed()
            {
                state.bvh_budget.max_boxes = max_boxes as usize;
                renderer.set_bvh_overlay_budget(state.bvh_budget);
            }

            ui.label("Stride sampling");
            let mut stride = state.bvh_budget.stride as u32;
            if ui.add(egui::Slider::new(&mut stride, 1_u32..=64)).changed() {
                state.bvh_budget.stride = stride as usize;
                renderer.set_bvh_overlay_budget(state.bvh_budget);
            }

            if ui
                .checkbox(
                    &mut state.bvh_budget.visible_cells_only,
                    "Visible cells only",
                )
                .changed()
            {
                renderer.set_bvh_overlay_budget(state.bvh_budget);
            }
        });

    egui::CollapsingHeader::new("Camera cull")
        .default_open(true)
        .show(ui, |ui| {
            let diag = renderer.camera_cull_diagnostics();
            let (path_label, candidate_label) = match diag.path {
                CameraCullPath::Candidate { candidate_leaves } => {
                    ("Candidate (visible-cell)", candidate_leaves.to_string())
                }
                CameraCullPath::TreeWalk => ("Tree walk (legacy)", "—".to_string()),
            };
            ui.label(format!("Path: {path_label}"));
            ui.label(format!("Candidate leaves: {candidate_label}"));
            ui.label(format!("Total leaves: {}", diag.total_leaves));
            ui.label(format!("Submitted leaves: {}", diag.submitted_leaves));
            // Candidate vs total exposes future indirect-compaction headroom.
            if let Some(candidates) = diag.candidate_leaves() {
                if diag.total_leaves > 0 {
                    let pct = 100.0 * candidates as f32 / diag.total_leaves as f32;
                    ui.label(format!("Candidate / total: {pct:.1}%"));
                }
            }
        });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn diagnostics_state_defaults_to_lighting_tab() {
        let state = DiagnosticsState::default();

        assert_eq!(state.selected_tab, DiagnosticsTab::Lighting);
        assert_eq!(state.spatial_wireframe_mode, WorldWireframeMode::Off);
        assert!(!state.bvh_overlay_visible);
        assert_eq!(state.bvh_color_mode, BvhOverlayColorMode::CellId);
        assert_eq!(state.bvh_depth_mode, BvhOverlayDepthMode::DepthTested);
        assert_eq!(state.bvh_budget, BvhOverlayBudget::default());
        assert_eq!(
            CellOverlayState {
                visible: state.cell_overlay_visible,
                depth_mode: state.cell_depth_mode,
            },
            CellOverlayState::default(),
        );
        assert_eq!(
            PortalOverlayState {
                visible: state.portal_overlay_visible,
                depth_mode: state.portal_depth_mode,
            },
            PortalOverlayState::default(),
        );
    }

    #[test]
    fn diagnostics_tabs_expose_spatial_without_extra_action() {
        assert_eq!(
            DiagnosticsTab::ALL,
            [
                DiagnosticsTab::Lighting,
                DiagnosticsTab::Volumes,
                DiagnosticsTab::Performance,
                DiagnosticsTab::Spatial,
            ],
        );
        assert_eq!(DiagnosticsTab::Spatial.label(), "Spatial");
    }
}
