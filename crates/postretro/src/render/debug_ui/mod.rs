// Debug UI: CPU-side egui context + winit event bridge.
// See: context/plans/in-progress/egui-debug-ui-foundation/

use winit::event::WindowEvent;
use winit::window::Window;

/// Placeholder for the GPU-side renderer state populated in Task 5
/// (egui_wgpu::Renderer + screen descriptor). Kept here as a unit struct so
/// `DebugUi` can carry the `Option<DebugUiGpu>` field shape now and avoid
/// churning the type when Task 5 fills it in.
pub struct DebugUiGpu;

/// Diagnostics-panel widget state. The panel binds these to renderer setters
/// each frame; default values mirror the renderer's stock values so the panel
/// reads sensibly before any user interaction has happened.
///
/// Consumed by the panel layout in Task 6; `#[allow(dead_code)]` until then.
#[allow(dead_code)]
pub struct DiagnosticsState {
    pub ambient_floor: f32,
    pub indirect_scale: f32,
}

impl Default for DiagnosticsState {
    fn default() -> Self {
        Self {
            ambient_floor: 0.0,
            indirect_scale: 1.0,
        }
    }
}

/// CPU-side egui state. Lives on `App` as `Option<DebugUi>` so the engine can
/// boot before the renderer is available (the constructor needs the device's
/// `max_texture_dimension_2d` limit). GPU resources live in `gpu` once Task 5
/// installs them.
///
/// Fields/methods are constructed here but not consumed until later tasks in
/// the egui-debug-ui-foundation plan wire them up (Task 5 GPU renderer, Task
/// 6 panel layout, Task 7 input arbitration). `#[allow(dead_code)]` keeps the
/// shape locked in without warnings until then.
#[allow(dead_code)]
pub struct DebugUi {
    ctx: egui::Context,
    winit_state: egui_winit::State,
    pub gpu: Option<DebugUiGpu>,
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
            gpu: None,
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
