// PostRetro engine entry point.
// See: context/development_guide.md

use std::num::NonZeroU32;

use anyhow::{Context, Result};
use glow::HasContext;
use glutin::config::{Config, ConfigTemplateBuilder, GetGlConfig};
use glutin::context::{
    ContextApi, ContextAttributesBuilder, GlProfile, NotCurrentContext, PossiblyCurrentContext,
    Version,
};
use glutin::display::GetGlDisplay;
use glutin::prelude::*;
use glutin::surface::{Surface, SwapInterval, WindowSurface};
use glutin_winit::{DisplayBuilder, GlWindow};
use raw_window_handle::HasWindowHandle;
use winit::application::ApplicationHandler;
use winit::event::{KeyEvent, WindowEvent};
use winit::event_loop::{ActiveEventLoop, EventLoop};
use winit::keyboard::{Key, NamedKey};
use winit::window::{Window, WindowAttributes};

fn main() -> Result<()> {
    env_logger::init();
    log::info!("[Engine] PostRetro starting");

    let event_loop = EventLoop::new().context("failed to create event loop")?;

    let template = ConfigTemplateBuilder::new().with_alpha_size(8);

    let display_builder =
        DisplayBuilder::new().with_window_attributes(Some(window_attributes()));

    let mut app = App {
        template,
        gl_display_state: GlDisplayState::Builder(display_builder),
        gl_context: None,
        gl: None,
        state: None,
        exit_result: Ok(()),
    };

    event_loop
        .run_app(&mut app)
        .context("event loop terminated with error")?;

    app.exit_result
}

fn window_attributes() -> WindowAttributes {
    Window::default_attributes()
        .with_title("PostRetro")
        .with_inner_size(winit::dpi::LogicalSize::new(1280, 720))
}

// --- Application state ---

struct App {
    template: ConfigTemplateBuilder,
    gl_display_state: GlDisplayState,
    gl_context: Option<PossiblyCurrentContext>,
    gl: Option<glow::Context>,
    state: Option<WindowState>,
    exit_result: Result<()>,
}

struct WindowState {
    gl_surface: Surface<WindowSurface>,
    window: Window,
}

enum GlDisplayState {
    Builder(DisplayBuilder),
    Initialized,
}

// --- ApplicationHandler ---

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        let (window, gl_config) = match &self.gl_display_state {
            GlDisplayState::Builder(display_builder) => {
                let (window, gl_config) = match display_builder.clone().build(
                    event_loop,
                    self.template.clone(),
                    pick_gl_config,
                ) {
                    Ok((window, gl_config)) => (window.expect("window not created"), gl_config),
                    Err(err) => {
                        self.exit_result = Err(anyhow::anyhow!("{err}"));
                        event_loop.exit();
                        return;
                    }
                };

                self.gl_display_state = GlDisplayState::Initialized;

                let gl_context = create_gl_context(&window, &gl_config)
                    .expect("failed to create GL 3.3 context");
                self.gl_context = Some(gl_context.treat_as_possibly_current());

                log::info!("[Engine] GL context created");
                (window, gl_config)
            }
            GlDisplayState::Initialized => {
                let gl_config = self.gl_context.as_ref().expect("missing GL context").config();
                let window = glutin_winit::finalize_window(
                    event_loop,
                    window_attributes(),
                    &gl_config,
                )
                .expect("failed to finalize window");
                (window, gl_config)
            }
        };

        let surface_attrs = window
            .build_surface_attributes(Default::default())
            .expect("failed to build surface attributes");

        // SAFETY: surface creation requires a valid native window handle,
        // which winit guarantees while the window is alive.
        let gl_surface = unsafe {
            gl_config
                .display()
                .create_window_surface(&gl_config, &surface_attrs)
                .expect("failed to create GL surface")
        };

        let gl_context = self.gl_context.as_ref().expect("missing GL context");
        gl_context
            .make_current(&gl_surface)
            .expect("failed to make GL context current");

        // SAFETY: loading GL function pointers from a valid, current display.
        self.gl.get_or_insert_with(|| unsafe {
            glow::Context::from_loader_function_cstr(|symbol| {
                gl_config.display().get_proc_address(symbol)
            })
        });

        if let Err(err) = gl_surface.set_swap_interval(
            gl_context,
            SwapInterval::Wait(NonZeroU32::new(1).expect("non-zero")),
        ) {
            log::warn!("[Engine] Failed to set vsync: {err:?}");
        }

        assert!(
            self.state
                .replace(WindowState { gl_surface, window })
                .is_none(),
            "resumed called with existing window state"
        );

        log::info!("[Engine] Window ready");
    }

    fn suspended(&mut self, _event_loop: &ActiveEventLoop) {
        self.state = None;
        if let Some(ctx) = self.gl_context.take() {
            self.gl_context = Some(
                ctx.make_not_current()
                    .expect("failed to uncurrent GL context")
                    .treat_as_possibly_current(),
            );
        }
    }

    fn window_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        _window_id: winit::window::WindowId,
        event: WindowEvent,
    ) {
        match event {
            WindowEvent::Resized(size) if size.width != 0 && size.height != 0 => {
                if let (Some(ws), Some(ctx)) = (self.state.as_ref(), self.gl_context.as_ref()) {
                    ws.gl_surface.resize(
                        ctx,
                        NonZeroU32::new(size.width).expect("non-zero"),
                        NonZeroU32::new(size.height).expect("non-zero"),
                    );
                    if let Some(gl) = self.gl.as_ref() {
                        // SAFETY: viewport is a standard GL call with no preconditions.
                        unsafe {
                            gl.viewport(0, 0, size.width as i32, size.height as i32);
                        }
                    }
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
            _ => {}
        }
    }

    fn about_to_wait(&mut self, _event_loop: &ActiveEventLoop) {
        if let (Some(ws), Some(ctx), Some(gl)) =
            (self.state.as_ref(), self.gl_context.as_ref(), self.gl.as_ref())
        {
            // SAFETY: clear is a standard GL call; context is current.
            unsafe {
                gl.clear_color(0.05, 0.05, 0.08, 1.0);
                gl.clear(glow::COLOR_BUFFER_BIT | glow::DEPTH_BUFFER_BIT);
            }

            ws.gl_surface
                .swap_buffers(ctx)
                .expect("failed to swap buffers");
            ws.window.request_redraw();
        }
    }

    fn exiting(&mut self, _event_loop: &ActiveEventLoop) {
        self.gl = None;
        let _display = self.gl_context.take().map(|ctx| ctx.display());
        self.state = None;
        log::info!("[Engine] Exited");
    }
}

// --- GL setup helpers ---

fn create_gl_context(window: &Window, gl_config: &Config) -> Result<NotCurrentContext> {
    let raw_handle = window.window_handle().ok().map(|wh| wh.as_raw());

    let context_attrs = ContextAttributesBuilder::new()
        .with_profile(GlProfile::Core)
        .with_context_api(ContextApi::OpenGl(Some(Version::new(3, 3))))
        .build(raw_handle);

    let fallback_attrs = ContextAttributesBuilder::new()
        .with_context_api(ContextApi::OpenGl(None))
        .build(raw_handle);

    let display = gl_config.display();

    // SAFETY: context creation requires a valid display and config,
    // both guaranteed by glutin's builder chain.
    let context = unsafe {
        display
            .create_context(gl_config, &context_attrs)
            .or_else(|_| display.create_context(gl_config, &fallback_attrs))
            .context("failed to create OpenGL context")?
    };

    Ok(context)
}

fn pick_gl_config(configs: Box<dyn Iterator<Item = Config> + '_>) -> Config {
    configs
        .reduce(|best, config| {
            if config.num_samples() > best.num_samples() {
                config
            } else {
                best
            }
        })
        .expect("no GL configs available")
}
