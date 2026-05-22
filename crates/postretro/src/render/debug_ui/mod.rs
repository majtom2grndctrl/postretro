// Debug UI overlay: egui context, winit event bridge, diagnostics panel.
// See: context/lib/rendering_pipeline.md §11 · context/lib/input.md §7

use winit::event::WindowEvent;
use winit::window::Window;

use super::GraphicsMode;
use super::LightingIsolation;
use super::Renderer;
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
            });

        // Player-facing aesthetic, kept distinct from the developer lighting
        // diagnostics above. Read the mode fresh each frame so the combo
        // tracks the live renderer even when a manifest default or hot-reload
        // changed it underneath the panel.
        egui::CollapsingHeader::new("Rendering")
            .default_open(true)
            .show(ui, |ui| {
                let mut graphics_mode = renderer.graphics_mode();
                let prev_graphics_mode = graphics_mode;
                egui::ComboBox::from_label("Graphics Mode")
                    .selected_text(graphics_mode.label())
                    .show_ui(ui, |ui| {
                        for variant in GraphicsMode::ALL_VARIANTS {
                            ui.selectable_value(&mut graphics_mode, variant, variant.label());
                        }
                    });
                if graphics_mode != prev_graphics_mode {
                    renderer.set_graphics_mode(graphics_mode);
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
    });
}
