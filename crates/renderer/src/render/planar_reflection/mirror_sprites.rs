// Mirror billboards: the planar mirror's own sprite instance store, drawn through the billboard pipeline.
// See: context/lib/rendering_pipeline.md §7.3a, §7.4

use crate::render::smoke::{SmokePass, align_up_to_dynamic_offset, build_instance_bind_group};
use postretro_render_cpu::smoke::SPRITE_INSTANCE_SIZE;

/// One collection's mirror sprites this frame: a region of the store.
#[derive(Debug, Default)]
struct SpriteBatch {
    /// Collection id; the string's capacity is reused frame to frame.
    collection: String,
    /// Dynamic offset of the region, a multiple of the storage alignment.
    offset: u32,
    live_count: u32,
}

/// The mirror's billboards and particles, collected over the mirror's cells
/// and frustum, in a buffer of their own: the billboard pass writes its
/// instance buffer from offset 0 each frame, so the mirror cannot share it.
/// Regions are laid out per collection at storage-aligned offsets and drawn
/// by dynamic offset through the billboard pipeline's group-6 layout; the
/// buffer and the bound window grow on demand, never per frame once warm.
pub(super) struct MirrorSprites {
    instance_buffer: wgpu::Buffer,
    instance_buffer_capacity: usize,
    /// The bound window: the largest padded region seen, monotonic, so the
    /// bind group rebuilds rarely (see `smoke::build_instance_bind_group`).
    instance_window: u64,
    instance_bind_group: wgpu::BindGroup,
    /// This frame's padded regions back to back, retained scratch.
    staging: Vec<u8>,
    batches: Vec<SpriteBatch>,
    /// Batches in use this frame; later entries keep their string capacity.
    batch_count: usize,
}

fn create_instance_buffer(device: &wgpu::Device, capacity: usize) -> wgpu::Buffer {
    device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("Planar Mirror Sprite Instance Buffer"),
        size: capacity as u64,
        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    })
}

impl MirrorSprites {
    /// `layout` is the billboard pipeline's group-6 sprite-instance layout.
    pub fn new(device: &wgpu::Device, layout: &wgpu::BindGroupLayout) -> Self {
        let instance_window = align_up_to_dynamic_offset(1) as u64;
        let instance_buffer_capacity = 4 * instance_window as usize;
        let instance_buffer = create_instance_buffer(device, instance_buffer_capacity);
        let instance_bind_group =
            build_instance_bind_group(device, layout, &instance_buffer, instance_window);
        Self {
            instance_buffer,
            instance_buffer_capacity,
            instance_window,
            instance_bind_group,
            staging: Vec::new(),
            batches: Vec::new(),
            batch_count: 0,
        }
    }

    /// Replace this frame's mirror sprites, one `(collection, packed bytes)`
    /// region per collection, and write them where any are live.
    pub fn upload<'a>(
        &mut self,
        device: &wgpu::Device,
        queue: &crate::render::uploads::UploadQueue,
        layout: &wgpu::BindGroupLayout,
        collections: impl IntoIterator<Item = (&'a str, &'a [u8])>,
    ) {
        self.staging.clear();
        self.batch_count = 0;
        let mut largest_region = 0usize;
        for (collection, bytes) in collections {
            let live_count = bytes.len() / SPRITE_INSTANCE_SIZE;
            if live_count == 0 {
                continue;
            }
            let offset = self.staging.len();
            self.staging
                .extend_from_slice(&bytes[..live_count * SPRITE_INSTANCE_SIZE]);
            let padded = align_up_to_dynamic_offset(self.staging.len());
            self.staging.resize(padded, 0);
            largest_region = largest_region.max(padded - offset);
            if self.batches.len() == self.batch_count {
                self.batches.push(SpriteBatch::default());
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
                layout,
                &self.instance_buffer,
                self.instance_window,
            );
        }
        queue.write_buffer(&self.instance_buffer, 0, &self.staging);
    }

    /// Drop this frame's mirror sprites: the level's collections are gone.
    pub fn clear(&mut self) {
        self.staging.clear();
        self.batch_count = 0;
    }

    pub fn has_draws(&self) -> bool {
        self.batch_count > 0
    }

    /// Record one billboard draw per collection with mirror sprites, through
    /// the billboard pipeline. Groups 0 (the mirror's camera), 2 (lighting)
    /// and 3 (SH) must be set by the caller; group 0 carries the mirror's
    /// raster view-projection and eye, so every quad faces the mirror camera.
    /// A collection with no registered sheet this level draws nothing.
    pub fn record_draws(&self, pass: &mut wgpu::RenderPass<'_>, smoke: &SmokePass) {
        if self.batch_count == 0 {
            return;
        }
        pass.set_pipeline(smoke.pipeline());
        for batch in &self.batches[..self.batch_count] {
            let Some(sheet) = smoke.sheet_bind_group(&batch.collection) else {
                continue;
            };
            pass.set_bind_group(1, sheet, &[]);
            pass.set_bind_group(6, &self.instance_bind_group, &[batch.offset]);
            pass.draw(0..batch.live_count * 6, 0..1);
        }
    }
}
