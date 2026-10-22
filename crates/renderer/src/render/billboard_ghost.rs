// Billboard ghost pipeline (always blended) for sprite-bodied projectile ghosts: its
// per-collection instance upload and draw recording over the sprite sheets.
// See: context/lib/rendering_pipeline.md §7.9, §7.4

use postretro_render_cpu::ghost_instance::GHOST_SPRITE_INSTANCE_SIZE;

use super::SCENE_COLOR_FORMAT;
use super::smoke::{
    SmokePass, align_up_to_dynamic_offset, build_instance_bind_group,
    sprite_instance_bind_group_layout_entries,
};

/// Unlit billboard ghost shader with the shared ghost look helper appended.
/// It binds no lights, shadows or SH.
pub(super) const BILLBOARD_GHOST_SHADER_SOURCE: &str = concat!(
    include_str!("../shaders/billboard_ghost.wgsl"),
    "\n",
    include_str!("../shaders/ghost_look.wgsl"),
);

/// The billboard ghost blend: additive, `One + One`, as every billboard
/// blends. Order-independent, so ghost sprites need no sorting against each
/// other or the real billboards; the fragment premultiplies by the sprite's
/// coverage and the presented opacity, so a fading ghost dims toward nothing.
/// Used in both `render.ghosts` modes: a screen-door sprite would shimmer.
pub(super) const BILLBOARD_GHOST_BLEND: wgpu::BlendState = wgpu::BlendState {
    color: wgpu::BlendComponent {
        src_factor: wgpu::BlendFactor::One,
        dst_factor: wgpu::BlendFactor::One,
        operation: wgpu::BlendOperation::Add,
    },
    alpha: wgpu::BlendComponent {
        src_factor: wgpu::BlendFactor::One,
        dst_factor: wgpu::BlendFactor::One,
        operation: wgpu::BlendOperation::Add,
    },
};

/// One collection's ghost sprites this frame: a region of the instance buffer.
#[derive(Debug, Default)]
struct GhostBatch {
    /// Collection id; the string's capacity is reused frame to frame.
    collection: String,
    /// Dynamic offset of the region, a multiple of the storage alignment.
    offset: u32,
    live_count: u32,
}

/// Projectile billboard ghosts, batched per sprite collection into one shared
/// instance buffer drawn by dynamic offset, as the billboard pass draws its
/// own. Bound to the billboard pass's sprite sheets, never to its instance
/// buffer, so the real billboards' layout is untouched.
pub(crate) struct BillboardGhostPass {
    pipeline: wgpu::RenderPipeline,
    instance_bind_group_layout: wgpu::BindGroupLayout,
    instance_buffer: wgpu::Buffer,
    instance_buffer_capacity: usize,
    /// The bound window: the largest padded region seen, monotonic, so the
    /// bind group rebuilds rarely (see `smoke::build_instance_bind_group`).
    instance_window: u64,
    instance_bind_group: wgpu::BindGroup,
    /// This frame's padded regions back to back, retained scratch.
    staging: Vec<u8>,
    batches: Vec<GhostBatch>,
    /// Batches in use this frame; later entries keep their string capacity.
    batch_count: usize,
}

impl BillboardGhostPass {
    /// Layout: group 0 camera, group 1 the billboard pass's sprite-sheet
    /// layout (the same object, so its per-collection bind groups bind here),
    /// group 2 the ghost sprite instances at a dynamic offset.
    pub(crate) fn new(
        device: &wgpu::Device,
        depth_format: wgpu::TextureFormat,
        camera_bgl: &wgpu::BindGroupLayout,
        smoke: &SmokePass,
    ) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Billboard Ghost Shader"),
            source: wgpu::ShaderSource::Wgsl(BILLBOARD_GHOST_SHADER_SOURCE.into()),
        });
        let instance_bind_group_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("Billboard Ghost Instance BGL"),
                entries: &sprite_instance_bind_group_layout_entries(),
            });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("Billboard Ghost Pipeline Layout"),
            bind_group_layouts: &[
                Some(camera_bgl),
                Some(smoke.sheet_bind_group_layout()),
                Some(&instance_bind_group_layout),
            ],
            immediate_size: 0,
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("Billboard Ghost Pipeline"),
            layout: Some(&layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                buffers: &[],
                compilation_options: wgpu::PipelineCompilationOptions::default(),
            },
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                front_face: wgpu::FrontFace::Ccw,
                cull_mode: None,
                ..Default::default()
            },
            depth_stencil: Some(wgpu::DepthStencilState {
                format: depth_format,
                depth_write_enabled: Some(false),
                depth_compare: Some(wgpu::CompareFunction::Less),
                stencil: wgpu::StencilState::default(),
                bias: wgpu::DepthBiasState::default(),
            }),
            multisample: wgpu::MultisampleState::default(),
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_main"),
                targets: &[Some(wgpu::ColorTargetState {
                    format: SCENE_COLOR_FORMAT,
                    blend: Some(BILLBOARD_GHOST_BLEND),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: wgpu::PipelineCompilationOptions::default(),
            }),
            multiview_mask: None,
            cache: None,
        });
        // Ghost sprites are few: one alignment unit of window and four of
        // capacity to start, grown on demand.
        let instance_window = align_up_to_dynamic_offset(1) as u64;
        let instance_buffer_capacity = 4 * instance_window as usize;
        let instance_buffer = create_instance_buffer(device, instance_buffer_capacity);
        let instance_bind_group = build_instance_bind_group(
            device,
            &instance_bind_group_layout,
            &instance_buffer,
            instance_window,
        );
        Self {
            pipeline,
            instance_bind_group_layout,
            instance_buffer,
            instance_buffer_capacity,
            instance_window,
            instance_bind_group,
            staging: Vec::new(),
            batches: Vec::new(),
            batch_count: 0,
        }
    }

    /// Replace this frame's ghost sprites, one `(collection, packed bytes)`
    /// region per collection. Regions start at storage-aligned offsets so each
    /// binds by dynamic offset; the buffer and bound window grow on demand.
    /// No allocation once warm.
    pub(crate) fn upload<'a>(
        &mut self,
        device: &wgpu::Device,
        queue: &crate::render::uploads::UploadQueue,
        collections: impl IntoIterator<Item = (&'a str, &'a [u8])>,
    ) {
        self.staging.clear();
        self.batch_count = 0;
        let mut largest_region = 0usize;
        for (collection, bytes) in collections {
            let live_count = bytes.len() / GHOST_SPRITE_INSTANCE_SIZE;
            if live_count == 0 {
                continue;
            }
            let offset = self.staging.len();
            self.staging.extend_from_slice(bytes);
            let padded = align_up_to_dynamic_offset(self.staging.len());
            self.staging.resize(padded, 0);
            largest_region = largest_region.max(padded - offset);
            if self.batches.len() == self.batch_count {
                self.batches.push(GhostBatch::default());
            }
            let batch = &mut self.batches[self.batch_count];
            batch.collection.clear();
            batch.collection.push_str(collection);
            batch.offset = offset as u32;
            batch.live_count = live_count as u32;
            self.batch_count += 1;
        }
        if self.batch_count == 0 {
            return;
        }
        // `capacity >= last offset + window` keeps every region's dynamic
        // offset within wgpu's `maximum_dynamic_offset = capacity - window`.
        let window = self.instance_window.max(largest_region as u64);
        let last_offset = self.batches[self.batch_count - 1].offset as usize;
        let required = (last_offset + window as usize).max(self.staging.len());
        let grow = required > self.instance_buffer_capacity;
        if grow {
            self.instance_buffer_capacity =
                align_up_to_dynamic_offset(required.next_power_of_two());
            self.instance_buffer = create_instance_buffer(device, self.instance_buffer_capacity);
        }
        if grow || window != self.instance_window {
            self.instance_window = window;
            self.instance_bind_group = build_instance_bind_group(
                device,
                &self.instance_bind_group_layout,
                &self.instance_buffer,
                self.instance_window,
            );
        }
        queue.write_buffer(&self.instance_buffer, 0, &self.staging);
    }

    /// Drop this frame's ghost sprites: the level's collections are gone.
    pub(crate) fn clear(&mut self) {
        self.staging.clear();
        self.batch_count = 0;
    }

    pub(crate) fn has_draws(&self) -> bool {
        self.batch_count > 0
    }

    /// Record one draw per collection with ghost sprites. Group 0 (camera)
    /// must be set by the caller. A collection with no registered sheet this
    /// level draws nothing.
    pub(crate) fn record_draws(&self, pass: &mut wgpu::RenderPass<'_>, smoke: &SmokePass) {
        if self.batch_count == 0 {
            return;
        }
        pass.set_pipeline(&self.pipeline);
        for batch in &self.batches[..self.batch_count] {
            let Some(sheet) = smoke.sheet_bind_group(&batch.collection) else {
                continue;
            };
            pass.set_bind_group(1, sheet, &[]);
            pass.set_bind_group(2, &self.instance_bind_group, &[batch.offset]);
            pass.draw(0..batch.live_count * 6, 0..1);
        }
    }
}

fn create_instance_buffer(device: &wgpu::Device, capacity: usize) -> wgpu::Buffer {
    device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("Billboard Ghost Instance Buffer"),
        size: capacity as u64,
        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    })
}

#[cfg(test)]
#[path = "billboard_ghost_tests.rs"]
mod tests;
