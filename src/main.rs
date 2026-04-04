// Postretro engine entry point.
// See: context/lib/rendering_pipeline.md

mod bsp;
mod render;

use std::sync::Arc;

use anyhow::{Context, Result};
use winit::application::ApplicationHandler;
use winit::event::{KeyEvent, WindowEvent};
use winit::event_loop::{ActiveEventLoop, EventLoop};
use winit::keyboard::{Key, NamedKey};
use winit::window::{Window, WindowAttributes};

use crate::render::Renderer;

/// CLI flag to force the line-list wireframe fallback path.
const FORCE_LINE_LIST_FLAG: &str = "--force-line-list";

fn main() -> Result<()> {
    env_logger::init();
    log::info!("[Engine] Postretro starting");

    let args: Vec<String> = std::env::args().collect();
    let force_line_list = args.iter().any(|a| a == FORCE_LINE_LIST_FLAG);

    // Load BSP before entering the event loop (heavy I/O must not block the loop).
    let bsp_path = bsp::resolve_bsp_path(&args);
    let bsp_world = match bsp::load_bsp(&bsp_path) {
        Ok(world) => {
            log::info!("[Engine] BSP loaded successfully from {bsp_path}");
            Some(world)
        }
        Err(bsp::BspLoadError::FileNotFound(path)) => {
            log::warn!("[Engine] BSP file not found: {path} — starting without map");
            None
        }
        Err(err) => {
            anyhow::bail!("failed to load BSP: {err}");
        }
    };

    let event_loop = EventLoop::new().context("failed to create event loop")?;

    let mut app = App {
        renderer: None,
        window_state: None,
        bsp_world,
        force_line_list,
        exit_result: Ok(()),
    };

    event_loop
        .run_app(&mut app)
        .context("event loop terminated with error")?;

    app.exit_result
}

fn window_attributes() -> WindowAttributes {
    Window::default_attributes()
        .with_title("Postretro")
        .with_inner_size(winit::dpi::LogicalSize::new(1280, 720))
}

// --- Application state ---

struct App {
    renderer: Option<Renderer>,
    window_state: Option<WindowState>,
    /// BSP world data, held until the renderer is created (ownership transfers on resume).
    bsp_world: Option<bsp::BspWorld>,
    force_line_list: bool,
    exit_result: Result<()>,
}

struct WindowState {
    window: Arc<Window>,
}

// --- ApplicationHandler ---

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        let window = match event_loop.create_window(window_attributes()) {
            Ok(w) => Arc::new(w),
            Err(err) => {
                self.exit_result = Err(anyhow::anyhow!("failed to create window: {err}"));
                event_loop.exit();
                return;
            }
        };

        let renderer = match Renderer::new(
            &window,
            self.bsp_world.as_ref(),
            self.force_line_list,
        ) {
            Ok(r) => r,
            Err(err) => {
                self.exit_result = Err(err);
                event_loop.exit();
                return;
            }
        };

        self.renderer = Some(renderer);
        self.window_state = Some(WindowState { window });

        log::info!("[Engine] Window ready");
    }

    fn suspended(&mut self, _event_loop: &ActiveEventLoop) {
        self.window_state = None;
        self.renderer = None;
        log::info!("[Engine] Suspended");
    }

    fn window_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        _window_id: winit::window::WindowId,
        event: WindowEvent,
    ) {
        match event {
            WindowEvent::Resized(size) => {
                if let Some(renderer) = self.renderer.as_mut() {
                    renderer.resize(size.width, size.height);
                }
            }
            WindowEvent::CloseRequested
            | WindowEvent::KeyboardInput {
                event:
                    KeyEvent {
                        logical_key: Key::Named(NamedKey::Escape),
                        ..
                    },
                ..
            } => {
                log::info!("[Engine] Shutting down");
                event_loop.exit();
            }
            WindowEvent::RedrawRequested => {
                if let Some(renderer) = self.renderer.as_ref() {
                    if renderer.is_ready() {
                        if let Err(err) = renderer.render_frame() {
                            self.exit_result = Err(err);
                            event_loop.exit();
                        }
                    }
                }
            }
            _ => {}
        }
    }

    fn about_to_wait(&mut self, _event_loop: &ActiveEventLoop) {
        if let Some(ws) = self.window_state.as_ref() {
            ws.window.request_redraw();
        }
    }

    fn exiting(&mut self, _event_loop: &ActiveEventLoop) {
        self.renderer = None;
        self.window_state = None;
        log::info!("[Engine] Exited");
    }
}
