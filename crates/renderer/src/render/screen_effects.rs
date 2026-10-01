// Screen-space resolve pass, the renderer-owned `scene_color` target every
// gameplay scene pass renders into, and the native-res UI layer it composites.
// See: context/lib/rendering_pipeline.md §7.8

use std::collections::HashMap;

use super::{SCENE_COLOR_FORMAT, UI_LAYER_FORMAT};
use postretro_entities::SlotValue;
use postretro_render_cpu::flash_clamp::ChannelClamp;
use postretro_render_cpu::flash_limiter::{LimiterFrameInput, flash_limiter_enabled};
use postretro_render_cpu::render_extent::Extent;
use postretro_render_cpu::screen_effects::{CoversHud, EffectUniform, pack_effect_uniform};

/// Which screen effects also reach the UI layer. All off: shake, flash and
/// vignette act on the scene and leave the HUD untouched. A HUD-covering effect
/// flips its switch here rather than adding a second source of effect state.
const COVERS_HUD: CoversHud = CoversHud {
    flash: false,
    vignette: false,
    shake: false,
};

/// The offscreen scene target, the native-res UI layer, and the
/// fullscreen-triangle resolve that composites both into the swapchain.
///
/// The resolve runs EVERY frame as the sole swapchain writer for the gameplay
/// path — never skipped at rest. It upscales `scene_color` (scene extent) by
/// nearest integer replication, tonemaps it, applies the screen effects, then
/// composites the UI layer (surface extent) over it without tonemapping.
///
/// **Effect seam.** [`pack_effect_uniform`] packs the frame's `screen.*` slot
/// values into [`EffectUniform`]; the shader applies the math in
/// `screen_effects.wgsl`. At-rest slot values pack to the identity uniform and
/// every effect term ALU-collapses to a no-op.
///
/// **Photosensitivity limiter.** The channel clamp limits the packed flash and
/// vignette before the uniform is written. It is CPU-only; the GPU sees an
/// ordinary effect uniform.
pub struct ScreenEffectsPass {
    /// Linear HDR scene target at the scene extent. Scene passes render here;
    /// the resolve loads it.
    color_texture: wgpu::Texture,
    color_view: wgpu::TextureView,
    /// Premultiplied sRGB UI layer at the surface extent. Game UI renders here,
    /// cleared transparent every frame; the resolve composites it 1:1.
    ui_layer_texture: wgpu::Texture,
    ui_layer_view: wgpu::TextureView,
    /// A zero-initialized 1×1 layer bound by capture, which has no UI.
    empty_ui_layer_view: wgpu::TextureView,
    bind_group_layout: wgpu::BindGroupLayout,
    /// References `color_view` and `ui_layer_view`; rebuilt when either is.
    bind_group: wgpu::BindGroup,
    resolve_pipeline: wgpu::RenderPipeline,
    /// Same shader/operator as the windowed resolve, but targeting deterministic
    /// RGBA8 sRGB capture bytes with transient effects held at rest.
    capture_pipeline: wgpu::RenderPipeline,
    /// Per-frame effect uniform. Written every frame; persists across resize.
    effect_buffer: wgpu::Buffer,
    /// The photosensitivity limiter: `screen.flash` / `screen.vignette` limited
    /// as they pack. CPU-side history, advanced once per resolve frame.
    channel_clamp: ChannelClamp,
    /// [`COVERS_HUD`]; a field so a test can turn one switch on.
    covers_hud: CoversHud,
}

impl ScreenEffectsPass {
    pub fn new(
        device: &wgpu::Device,
        scene: Extent,
        surface: Extent,
        surface_format: wgpu::TextureFormat,
    ) -> Self {
        let (color_texture, color_view) = create_scene_color(device, scene);
        let (ui_layer_texture, ui_layer_view) = create_ui_layer(device, surface);
        let (_empty_ui_layer, empty_ui_layer_view) = create_ui_layer(device, Extent::new(1, 1));

        // Both inputs are read with `textureLoad` at integer texel coordinates,
        // so the resolve needs no sampler.
        let layer_entry = |binding| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable: false },
                view_dimension: wgpu::TextureViewDimension::D2,
                multisampled: false,
            },
            count: None,
        };
        let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Screen Effects BGL"),
            entries: &[
                layer_entry(0),
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                layer_entry(2),
            ],
        });

        // Per-frame effect uniform, initialized at rest for a neutral resolve.
        let effect_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Screen Effects Uniform Buffer"),
            size: std::mem::size_of::<EffectUniform>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let bind_group = create_bind_group(
            device,
            &bind_group_layout,
            &color_view,
            &effect_buffer,
            &ui_layer_view,
        );

        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Screen Effects Shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("../shaders/screen_effects.wgsl").into()),
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("Screen Effects Pipeline Layout"),
            bind_group_layouts: &[Some(&bind_group_layout)],
            immediate_size: 0,
        });
        let resolve_pipeline = create_resolve_pipeline(
            device,
            &layout,
            &shader,
            surface_format,
            "Screen Effects Resolve Pipeline",
        );
        let capture_pipeline = create_resolve_pipeline(
            device,
            &layout,
            &shader,
            wgpu::TextureFormat::Rgba8UnormSrgb,
            "Screen Effects Capture Tonemap Pipeline",
        );

        Self {
            color_texture,
            color_view,
            ui_layer_texture,
            ui_layer_view,
            empty_ui_layer_view,
            bind_group_layout,
            bind_group,
            resolve_pipeline,
            capture_pipeline,
            effect_buffer,
            channel_clamp: ChannelClamp::default(),
            covers_hud: COVERS_HUD,
        }
    }

    /// The scene target view every gameplay scene pass renders into.
    pub fn scene_color_view(&self) -> &wgpu::TextureView {
        &self.color_view
    }

    /// The renderer-owned raw HDR scene target. Post-scene passes sample this
    /// before the display/capture resolves.
    pub(super) fn scene_color_texture(&self) -> &wgpu::Texture {
        &self.color_texture
    }

    /// The native-res UI layer game UI renders into.
    pub(super) fn ui_layer_view(&self) -> &wgpu::TextureView {
        &self.ui_layer_view
    }

    #[cfg(test)]
    pub(super) fn ui_layer_texture(&self) -> &wgpu::Texture {
        &self.ui_layer_texture
    }

    /// Recreate `scene_color` at the scene extent and rebind the resolve.
    pub fn resize(&mut self, device: &wgpu::Device, scene: Extent) {
        let (color_texture, color_view) = create_scene_color(device, scene);
        self.color_texture = color_texture;
        self.color_view = color_view;
        self.rebuild_bind_group(device);
    }

    /// Recreate the UI layer at the surface extent and rebind the resolve, in
    /// the same commit that reconfigures the swapchain, so the composited layer
    /// always matches the swapchain size.
    pub fn resize_ui_layer(&mut self, device: &wgpu::Device, surface: Extent) {
        let (ui_layer_texture, ui_layer_view) = create_ui_layer(device, surface);
        self.ui_layer_texture = ui_layer_texture;
        self.ui_layer_view = ui_layer_view;
        self.rebuild_bind_group(device);
    }

    fn rebuild_bind_group(&mut self, device: &wgpu::Device) {
        self.bind_group = create_bind_group(
            device,
            &self.bind_group_layout,
            &self.color_view,
            &self.effect_buffer,
            &self.ui_layer_view,
        );
    }

    #[cfg(test)]
    pub(super) fn set_covers_hud(&mut self, covers_hud: CoversHud) {
        self.covers_hud = covers_hud;
    }

    /// Record the resolve pass: a fullscreen-triangle blit from `scene_color`
    /// into the swapchain `view`, upscaling by `scene_divisor` (nearest integer
    /// replication), tonemapping with a soft knee, and composing the frame's
    /// scene screen effects (flash/vignette/shake). The UI layer is composited
    /// last, untonemapped; effects reach the UI only through their covers-HUD
    /// switches. The sole swapchain writer for the gameplay path — encoded every
    /// frame, never gated.
    ///
    /// Writes the per-frame effect uniform from the packed `slot_values` first.
    /// At rest all three effect slots collapse to no-ops (see [`pack_effect_uniform`]
    /// and the WGSL), so only the tonemap changes an at-rest scene.
    ///
    /// The channel clamp limits the packed flash and vignette against
    /// `limiter_frame`'s presented-frame time before the uniform is written.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn encode_resolve(
        &mut self,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        swapchain_view: &wgpu::TextureView,
        slot_values: &HashMap<String, SlotValue>,
        limiter_frame: LimiterFrameInput,
        scene_divisor: u32,
        timestamp_writes: Option<wgpu::RenderPassTimestampWrites<'_>>,
    ) {
        let frame = self
            .channel_clamp
            .begin_frame(limiter_frame, flash_limiter_enabled(slot_values));
        let mut uniform = pack_effect_uniform(slot_values);
        self.channel_clamp.apply(&mut uniform, &frame);
        uniform.covers_hud = self.covers_hud.packed();
        uniform.scene_divisor = scene_divisor;
        queue.write_buffer(&self.effect_buffer, 0, bytemuck::bytes_of(&uniform));

        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("Screen Effects Resolve Pass"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: swapchain_view,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    // Resolve covers the full swapchain (fullscreen triangle), so
                    // the prior swapchain contents are fully overwritten. Clear
                    // keeps the load deterministic without an extra dependency.
                    load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes,
            ..Default::default()
        });
        pass.set_pipeline(&self.resolve_pipeline);
        pass.set_bind_group(0, &self.bind_group, &[]);
        pass.draw(0..3, 0..1); // fullscreen triangle from vertex_index — no vertex buffer
    }

    /// Tonemap the raw HDR scene into the fixed RGBA8 sRGB capture format.
    /// Capture intentionally omits transient screen effects, so it writes an
    /// at-rest effect uniform while reusing the resolve shader and its tonemap.
    pub(super) fn encode_capture_tonemap(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        width: u32,
        height: u32,
    ) -> wgpu::Texture {
        // Capture renders at divisor 1 with no UI: an at-rest uniform and the
        // empty layer.
        let at_rest = EffectUniform {
            scene_divisor: 1,
            ..EffectUniform::default()
        };
        queue.write_buffer(&self.effect_buffer, 0, bytemuck::bytes_of(&at_rest));
        let capture_bind_group = create_bind_group(
            device,
            &self.bind_group_layout,
            &self.color_view,
            &self.effect_buffer,
            &self.empty_ui_layer_view,
        );
        let capture_texture = create_capture_color(device, width, height);
        let capture_view = capture_texture.create_view(&wgpu::TextureViewDescriptor::default());
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("Screen Effects Capture Tonemap Pass"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &capture_view,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            ..Default::default()
        });
        pass.set_pipeline(&self.capture_pipeline);
        pass.set_bind_group(0, &capture_bind_group, &[]);
        pass.draw(0..3, 0..1);
        capture_texture
    }
}

/// Allocate the linear HDR `scene_color` target at the scene extent,
/// single-sample. `RENDER_ATTACHMENT` (scene passes draw into it) +
/// `TEXTURE_BINDING` (display and capture resolves load it).
fn create_scene_color(device: &wgpu::Device, scene: Extent) -> (wgpu::Texture, wgpu::TextureView) {
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("Scene Color Texture"),
        size: wgpu::Extent3d {
            width: scene.width,
            height: scene.height,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: SCENE_COLOR_FORMAT,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT
            | wgpu::TextureUsages::TEXTURE_BINDING
            | TEST_COPY_USAGE,
        view_formats: &[],
    });
    let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
    (texture, view)
}

/// Tests seed and read back the resolve's inputs; production targets carry no
/// copy usage.
const TEST_COPY_USAGE: wgpu::TextureUsages = if cfg!(test) {
    wgpu::TextureUsages::COPY_SRC.union(wgpu::TextureUsages::COPY_DST)
} else {
    wgpu::TextureUsages::empty()
};

/// Allocate the UI layer at the surface extent.
fn create_ui_layer(device: &wgpu::Device, surface: Extent) -> (wgpu::Texture, wgpu::TextureView) {
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("UI Layer Texture"),
        size: wgpu::Extent3d {
            width: surface.width,
            height: surface.height,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: UI_LAYER_FORMAT,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT
            | wgpu::TextureUsages::TEXTURE_BINDING
            | TEST_COPY_USAGE,
        view_formats: &[],
    });
    let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
    (texture, view)
}

fn create_capture_color(device: &wgpu::Device, width: u32, height: u32) -> wgpu::Texture {
    device.create_texture(&wgpu::TextureDescriptor {
        label: Some("Scene Capture Tonemap Texture"),
        size: wgpu::Extent3d {
            width: width.max(1),
            height: height.max(1),
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8UnormSrgb,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    })
}

fn create_resolve_pipeline(
    device: &wgpu::Device,
    layout: &wgpu::PipelineLayout,
    shader: &wgpu::ShaderModule,
    target_format: wgpu::TextureFormat,
    label: &'static str,
) -> wgpu::RenderPipeline {
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some(label),
        layout: Some(layout),
        vertex: wgpu::VertexState {
            module: shader,
            entry_point: Some("vs_main"),
            buffers: &[],
            compilation_options: wgpu::PipelineCompilationOptions::default(),
        },
        primitive: wgpu::PrimitiveState {
            topology: wgpu::PrimitiveTopology::TriangleList,
            ..Default::default()
        },
        depth_stencil: None,
        multisample: wgpu::MultisampleState::default(),
        fragment: Some(wgpu::FragmentState {
            module: shader,
            entry_point: Some("fs_main"),
            targets: &[Some(wgpu::ColorTargetState {
                format: target_format,
                blend: None,
                write_mask: wgpu::ColorWrites::ALL,
            })],
            compilation_options: wgpu::PipelineCompilationOptions::default(),
        }),
        multiview_mask: None,
        cache: None,
    })
}

fn create_bind_group(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    color_view: &wgpu::TextureView,
    effect_buffer: &wgpu::Buffer,
    ui_layer_view: &wgpu::TextureView,
) -> wgpu::BindGroup {
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("Screen Effects Bind Group"),
        layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(color_view),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: effect_buffer.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 2,
                resource: wgpu::BindingResource::TextureView(ui_layer_view),
            },
        ],
    })
}

#[cfg(test)]
mod tests {
    /// The resolve shader must parse and declare the fullscreen vertex +
    /// fragment entry points (mirrors `fog_composite_wgsl_parses`).
    #[test]
    fn screen_effects_wgsl_parses() {
        let src = include_str!("../shaders/screen_effects.wgsl");
        let module =
            naga::front::wgsl::parse_str(src).expect("screen_effects.wgsl should parse as WGSL");
        let has_vs = module
            .entry_points
            .iter()
            .any(|ep| ep.name == "vs_main" && ep.stage == naga::ShaderStage::Vertex);
        let has_fs = module
            .entry_points
            .iter()
            .any(|ep| ep.name == "fs_main" && ep.stage == naga::ShaderStage::Fragment);
        assert!(has_vs, "screen_effects.wgsl must export @vertex vs_main");
        assert!(has_fs, "screen_effects.wgsl must export @fragment fs_main");
    }
}
