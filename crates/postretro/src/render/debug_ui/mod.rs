// Debug UI overlay: egui context, winit event bridge, diagnostics panel.
// See: context/lib/rendering_pipeline.md §11 · context/lib/input.md §7

use winit::event::WindowEvent;
use winit::window::Window;

use super::LightingIsolation;
use super::Renderer;
use super::SdfShadowMode;
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

/// Diagnostics-panel widget state. The panel binds these to renderer setters
/// each frame; default values mirror the renderer's stock values so the panel
/// reads sensibly before any user interaction has happened.
pub struct DiagnosticsState {
    pub ambient_floor: f32,
    pub indirect_scale: f32,
    // Task 7: SDF / Fog quality sliders. Seeded from the live renderer values
    // on first draw — see the `seeded` flag below.
    pub sdf_max_march_steps: u32,
    pub sdf_open_space_skip_threshold: f32,
    pub sdf_penumbra_k: f32,
    pub fog_step_size: f32,
    pub fog_pixel_scale: u32,
    /// Tracks whether the slider state has been seeded from the live renderer
    /// values. The first time the panel renders, it pulls current values so
    /// the sliders don't snap the world to defaults on first open.
    seeded: bool,
}

impl Default for DiagnosticsState {
    fn default() -> Self {
        Self {
            ambient_floor: super::DEFAULT_AMBIENT_FLOOR,
            indirect_scale: super::DEFAULT_INDIRECT_SCALE,
            // Placeholder values overwritten by the seed-from-renderer pass on
            // first draw (see `draw_diagnostics_panel`). Match the SDF /
            // fog defaults so the struct is still legible in isolation.
            sdf_max_march_steps: super::sdf_shadow::DEFAULT_MAX_MARCH_STEPS,
            sdf_open_space_skip_threshold: super::sdf_shadow::DEFAULT_OPEN_SPACE_SKIP_THRESHOLD,
            sdf_penumbra_k: super::sdf_shadow::DEFAULT_PENUMBRA_K,
            fog_step_size: crate::fx::fog_volume::DEFAULT_FOG_STEP_SIZE,
            fog_pixel_scale: 4,
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
        // Task 7: pull live SDF / Fog tuning so the sliders open at the
        // engine's current values, not the struct-default placeholders.
        state.sdf_max_march_steps = renderer.sdf_max_march_steps();
        state.sdf_open_space_skip_threshold = renderer.sdf_open_space_skip_threshold();
        state.sdf_penumbra_k = renderer.sdf_penumbra_k();
        state.fog_step_size = renderer.fog_step_size();
        state.fog_pixel_scale = renderer.fog_pixel_scale();
        state.seeded = true;
    }

    egui::Window::new("Diagnostics").show(ctx, |ui| {
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

                // Task 6 (sdf-static-occluder-shadows): SdfShadowMode selector.
                // Panel-only, no keyboard chord — mirrors the LightingIsolation
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

                // Pins `uniforms.time` so all curve-driven animation holds still.
                // Diagnostic aid: if a flickering artifact freezes too, it is
                // time/animation-driven; if it keeps moving, it is not.
                let mut frozen = renderer.freeze_time();
                if ui.checkbox(&mut frozen, "Freeze animation time").changed() {
                    renderer.set_freeze_time(frozen);
                }

                // Composes the static base SH only, dropping animated deltas.
                // Diagnostic aid: bisects whether irradiance-marker flicker
                // comes from delta application or base sampling / compose init.
                let mut base_only = renderer.sh_compose_base_only();
                if ui
                    .checkbox(&mut base_only, "SH compose: base only (no animated deltas)")
                    .changed()
                {
                    renderer.set_sh_compose_base_only(base_only);
                }
            });

        egui::CollapsingHeader::new("GPU Timing")
            .default_open(false)
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
            .default_open(false)
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

        // --- Task 7: SDF / Fog quality sliders ---
        //
        // Two feasibility classes per the plan:
        //   * Uniform scalars (SDF max march steps, open-space skip threshold,
        //     penumbra k, fog step_size) — write through to a per-frame uniform
        //     on the next dispatch / upload, no resource rebuild.
        //   * Resolution / allocation (fog_pixel_scale) — drives
        //     `set_fog_pixel_scale`, which rebuilds the scatter target and
        //     bind group. The renderer setter is a no-op when unchanged.
        egui::CollapsingHeader::new("SDF / Fog Quality")
            .default_open(false)
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
    });
}
