// PostRetro engine entry point.
// See: context/lib/development_guide.md

use std::sync::Arc;

use anyhow::{Context, Result};
use winit::application::ApplicationHandler;
use winit::event::{KeyEvent, WindowEvent};
use winit::event_loop::{ActiveEventLoop, EventLoop};
use winit::keyboard::{Key, NamedKey};
use winit::window::{Window, WindowAttributes};

fn main() -> Result<()> {
    env_logger::init();
    log::info!("[Engine] PostRetro starting");

    let event_loop = EventLoop::new().context("failed to create event loop")?;

    let mut app = App {
        gpu: None,
        window_state: None,
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
    gpu: Option<GpuState>,
    window_state: Option<WindowState>,
    exit_result: Result<()>,
}

struct GpuState {
    device: wgpu::Device,
    queue: wgpu::Queue,
    surface: wgpu::Surface<'static>,
    surface_config: wgpu::SurfaceConfiguration,
    is_surface_configured: bool,
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

        let gpu = match create_gpu_state(&window) {
            Ok(gpu) => gpu,
            Err(err) => {
                self.exit_result = Err(err);
                event_loop.exit();
                return;
            }
        };

        self.gpu = Some(gpu);
        self.window_state = Some(WindowState { window });

        log::info!("[Engine] Window ready");
    }

    fn suspended(&mut self, _event_loop: &ActiveEventLoop) {
        self.window_state = None;
        self.gpu = None;
        log::info!("[Engine] Suspended");
    }

    fn window_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        _window_id: winit::window::WindowId,
        event: WindowEvent,
    ) {
        match event {
            WindowEvent::Resized(size) if size.width != 0 && size.height != 0 => {
                if let Some(gpu) = self.gpu.as_mut() {
                    gpu.surface_config.width = size.width;
                    gpu.surface_config.height = size.height;
                    gpu.surface.configure(&gpu.device, &gpu.surface_config);
                    gpu.is_surface_configured = true;
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
                if let Some(gpu) = self.gpu.as_ref() {
                    if gpu.is_surface_configured {
                        if let Err(err) = render_frame(gpu) {
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
        self.gpu = None;
        self.window_state = None;
        log::info!("[Engine] Exited");
    }
}

// --- wgpu setup ---

fn create_gpu_state(window: &Arc<Window>) -> Result<GpuState> {
    let size = window.inner_size();

    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
        backends: wgpu::Backends::PRIMARY,
        ..wgpu::InstanceDescriptor::new_without_display_handle()
    });

    let surface = instance
        .create_surface(window.clone())
        .context("failed to create wgpu surface")?;

    let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
        power_preference: wgpu::PowerPreference::default(),
        compatible_surface: Some(&surface),
        force_fallback_adapter: false,
    }))
    .context("no suitable GPU adapter found")?;

    log::info!("[Engine] GPU adapter: {}", adapter.get_info().name);

    let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
        label: Some("PostRetro Device"),
        required_features: wgpu::Features::empty(),
        required_limits: wgpu::Limits::default(),
        ..Default::default()
    }))
    .context("failed to create GPU device")?;

    let surface_caps = surface.get_capabilities(&adapter);
    let surface_format = surface_caps
        .formats
        .iter()
        .copied()
        .find(|f| f.is_srgb())
        .unwrap_or(surface_caps.formats[0]);

    let surface_config = wgpu::SurfaceConfiguration {
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
        format: surface_format,
        width: size.width.max(1),
        height: size.height.max(1),
        present_mode: wgpu::PresentMode::AutoVsync,
        alpha_mode: surface_caps.alpha_modes[0],
        desired_maximum_frame_latency: 2,
        view_formats: vec![],
    };

    surface.configure(&device, &surface_config);

    Ok(GpuState {
        device,
        queue,
        surface,
        surface_config,
        is_surface_configured: true,
    })
}

// --- Rendering ---

fn render_frame(gpu: &GpuState) -> Result<()> {
    let output = match gpu.surface.get_current_texture() {
        wgpu::CurrentSurfaceTexture::Success(tex) => tex,
        wgpu::CurrentSurfaceTexture::Suboptimal(tex) => {
            gpu.surface.configure(&gpu.device, &gpu.surface_config);
            tex
        }
        wgpu::CurrentSurfaceTexture::Timeout | wgpu::CurrentSurfaceTexture::Occluded => {
            return Ok(());
        }
        wgpu::CurrentSurfaceTexture::Outdated => {
            gpu.surface.configure(&gpu.device, &gpu.surface_config);
            return Ok(());
        }
        wgpu::CurrentSurfaceTexture::Lost => {
            anyhow::bail!("surface lost");
        }
        wgpu::CurrentSurfaceTexture::Validation => {
            anyhow::bail!("surface validation error");
        }
    };

    let view = output
        .texture
        .create_view(&wgpu::TextureViewDescriptor::default());

    let mut encoder = gpu
        .device
        .create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("Clear Encoder"),
        });

    // Dark cyberpunk clear color (same as the previous GL clear).
    {
        let _render_pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("Clear Pass"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &view,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color {
                        r: 0.05,
                        g: 0.05,
                        b: 0.08,
                        a: 1.0,
                    }),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            ..Default::default()
        });
    }

    gpu.queue.submit(std::iter::once(encoder.finish()));
    output.present();

    Ok(())
}
