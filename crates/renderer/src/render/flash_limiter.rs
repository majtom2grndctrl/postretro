// Photosensitivity flash limiter: GPU-resident per-cell history and the
// measure/limit compute passes that run ahead of the screen-effects resolve.
// See: context/lib/rendering_pipeline.md §7.8 (Photosensitivity limiter)

use wgpu::util::DeviceExt;

use postretro_render_cpu::flash_limiter::{
    LIMITER_CELL_COUNT, LIMITER_CELLS_X, LIMITER_CELLS_Y, LimiterFrameInput, LimiterFrameUniform,
    pack_limiter_frame,
};

/// Byte sizes of the WGSL storage structs in `flash_limiter.wgsl` /
/// `screen_effects.wgsl`. Buffers are sized from these; the shader declares the
/// fixed-length arrays.
const CELL_MEASURE_BYTES: u64 = 16;
const CELL_STATE_BYTES: u64 = 48;
const CELL_PARAMS_BYTES: u64 = 32;
const GLOBAL_BYTES: u64 = 8 * 4 + 6 * 4;

/// The limiter's GPU state. Measurement samples `scene_color` through the
/// resolve's own composite (group 0 of the screen-effects layout), so there is
/// no second full-resolution target; everything the limiter remembers is a
/// few dozen bytes per cell, kept on the GPU and never read back. It is
/// resolution-independent, so a resize keeps the flash budget and history.
pub(super) struct FlashLimiter {
    frame_buffer: wgpu::Buffer,
    compute_bind_group: wgpu::BindGroup,
    resolve_bind_group_layout: wgpu::BindGroupLayout,
    resolve_bind_group: wgpu::BindGroup,
    measure_pipeline: wgpu::ComputePipeline,
    limit_pipeline: wgpu::ComputePipeline,
    /// False until the first frame is limited; that frame adopts the measured
    /// frame as the last presented one.
    initialized: bool,
    /// Whether the previous frame ran with the limiter on. Enabling starts
    /// history that frame.
    history_live: bool,
}

impl FlashLimiter {
    /// `effects_layout` is the screen-effects group-0 layout (scene color,
    /// sampler, effect uniform); `shader` is the combined screen-effects +
    /// limiter module.
    pub(super) fn new(
        device: &wgpu::Device,
        shader: &wgpu::ShaderModule,
        effects_layout: &wgpu::BindGroupLayout,
    ) -> Self {
        let frame_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Flash Limiter Frame Uniform"),
            size: std::mem::size_of::<LimiterFrameUniform>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let cells = u64::from(LIMITER_CELL_COUNT);
        let storage = |label: &'static str, bytes: u64| {
            device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(label),
                size: bytes,
                usage: wgpu::BufferUsages::STORAGE,
                mapped_at_creation: false,
            })
        };
        let measure_buffer = storage("Flash Limiter Cell Measure", cells * CELL_MEASURE_BYTES);
        let state_buffer = storage("Flash Limiter Cell State", cells * CELL_STATE_BYTES);
        let global_buffer = storage("Flash Limiter Global State", GLOBAL_BYTES);
        // Identity until the first limit pass writes it, so a resolve can never
        // read an uninitialized (all-zero, black) result.
        let mut identity = vec![0.0f32; (cells * CELL_PARAMS_BYTES / 4) as usize];
        for cell in identity.chunks_exact_mut((CELL_PARAMS_BYTES / 4) as usize) {
            cell[0] = 1.0; // gain
        }
        let params_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Flash Limiter Cell Params"),
            contents: bytemuck::cast_slice(&identity),
            usage: wgpu::BufferUsages::STORAGE,
        });

        let storage_entry = |binding: u32, read_only: bool, visibility: wgpu::ShaderStages| {
            wgpu::BindGroupLayoutEntry {
                binding,
                visibility,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Storage { read_only },
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }
        };
        let compute_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Flash Limiter Compute BGL"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                storage_entry(1, false, wgpu::ShaderStages::COMPUTE),
                storage_entry(2, false, wgpu::ShaderStages::COMPUTE),
                storage_entry(3, false, wgpu::ShaderStages::COMPUTE),
                storage_entry(4, false, wgpu::ShaderStages::COMPUTE),
            ],
        });
        let compute_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Flash Limiter Compute Bind Group"),
            layout: &compute_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: frame_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: measure_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: state_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: params_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 4,
                    resource: global_buffer.as_entire_binding(),
                },
            ],
        });

        let resolve_bind_group_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("Flash Limiter Resolve BGL"),
                entries: &[storage_entry(5, true, wgpu::ShaderStages::FRAGMENT)],
            });
        let resolve_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Flash Limiter Resolve Bind Group"),
            layout: &resolve_bind_group_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 5,
                resource: params_buffer.as_entire_binding(),
            }],
        });

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("Flash Limiter Pipeline Layout"),
            bind_group_layouts: &[Some(effects_layout), Some(&compute_layout)],
            immediate_size: 0,
        });
        let compute_pipeline = |entry: &'static str, label: &'static str| {
            device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                label: Some(label),
                layout: Some(&pipeline_layout),
                module: shader,
                entry_point: Some(entry),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                cache: None,
            })
        };
        let measure_pipeline = compute_pipeline("cs_measure_cells", "Flash Limiter Measure");
        let limit_pipeline = compute_pipeline("cs_limit_cells", "Flash Limiter Limit");

        Self {
            frame_buffer,
            compute_bind_group,
            resolve_bind_group_layout,
            resolve_bind_group,
            measure_pipeline,
            limit_pipeline,
            initialized: false,
            history_live: false,
        }
    }

    /// The resolve pipeline's group-1 layout: the per-cell result, read-only.
    pub(super) fn resolve_bind_group_layout(&self) -> &wgpu::BindGroupLayout {
        &self.resolve_bind_group_layout
    }

    pub(super) fn resolve_bind_group(&self) -> &wgpu::BindGroup {
        &self.resolve_bind_group
    }

    /// Decide this frame's limiter uniform: the enable flag read once for both
    /// stages, presented-frame time, and whether history restarts. Enabling
    /// starts history that frame; a fresh limiter adopts its first frame.
    pub(super) fn begin_frame(
        &mut self,
        input: LimiterFrameInput,
        enabled: bool,
    ) -> LimiterFrameUniform {
        let reset = enabled && !self.history_live;
        let frame = pack_limiter_frame(input, enabled, reset, !self.initialized);
        self.initialized = true;
        self.history_live = enabled;
        frame
    }

    /// Record the measure and limit dispatches for one presented frame. Runs
    /// every frame, the limiter on or off: off, it passes content unchanged but
    /// keeps tracking what was presented, so re-enabling compares against the
    /// frame presented just before it.
    pub(super) fn encode(
        &self,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        effects_bind_group: &wgpu::BindGroup,
        frame: &LimiterFrameUniform,
        timestamp_writes: Option<wgpu::ComputePassTimestampWrites<'_>>,
    ) {
        queue.write_buffer(&self.frame_buffer, 0, bytemuck::bytes_of(frame));
        let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
            label: Some("Flash Limiter Pass"),
            timestamp_writes,
        });
        pass.set_bind_group(0, effects_bind_group, &[]);
        pass.set_bind_group(1, &self.compute_bind_group, &[]);
        pass.set_pipeline(&self.measure_pipeline);
        pass.dispatch_workgroups(LIMITER_CELLS_X, LIMITER_CELLS_Y, 1);
        pass.set_pipeline(&self.limit_pipeline);
        pass.dispatch_workgroups(1, 1, 1);
    }
}
