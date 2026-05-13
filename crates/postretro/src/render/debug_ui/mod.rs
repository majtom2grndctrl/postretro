// Debug UI: CPU-side egui context + winit event bridge.
// See: context/plans/in-progress/egui-debug-ui-foundation/

use winit::event::WindowEvent;
use winit::window::Window;

use super::LightingIsolation;
use super::Renderer;
use super::frame_timing::FrameTimingSnapshot;

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

/// CPU-side egui state. Lives on `App` as `Option<DebugUi>` so the engine can
/// boot before the renderer is available (the constructor needs the device's
/// `max_texture_dimension_2d` limit). The GPU half (`DebugUiGpu`) lives on
/// `Renderer` and is constructed lazily on first panel open.
///
/// Fields/methods that are not yet consumed by later tasks (Task 6 panel
/// layout, Task 7 input arbitration) are kept under `#[allow(dead_code)]` to
/// lock in the shape without compiler warnings.
#[allow(dead_code)]
pub struct DebugUi {
    pub ctx: egui::Context,
    pub winit_state: egui_winit::State,
    visible: bool,
    pub panel_state: DiagnosticsState,
}

#[allow(dead_code)]
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

    /// Pointer input is captured by egui only when the panel is visible.
    /// Without the `visible` gate, an invisible egui context would still claim
    /// hover/clicks against any background widgets it has retained from prior
    /// frames.
    pub fn wants_pointer_input(&self) -> bool {
        self.visible && self.ctx.egui_wants_pointer_input()
    }

    pub fn wants_keyboard_input(&self) -> bool {
        // `egui_wants_keyboard_input` is the 0.34 rename; the older name still
        // resolves but warns. Stick with the current spelling so the build is
        // warning-clean.
        self.visible && self.ctx.egui_wants_keyboard_input()
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

        ui.separator();
        ui.label("GPU Timing");
        match frame_timing {
            Some(snapshot) if !snapshot.passes.is_empty() => {
                for (label, avg_ms, _skip) in &snapshot.passes {
                    ui.label(format!("{label}: {avg_ms:.2} ms"));
                }
            }
            _ => {
                ui.label("GPU timing unavailable");
            }
        }
    });
}
