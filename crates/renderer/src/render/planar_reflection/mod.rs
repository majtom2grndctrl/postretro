// Planar water reflection pass: the mirror view, its mask and world draws, the resolve raster.
// See: context/lib/rendering_pipeline.md §7.3a

mod api;
#[cfg(test)]
mod frame_port_tests;
#[cfg(test)]
mod frame_test_fixtures;
#[cfg(test)]
mod frame_tests;
mod mirror_draws;
mod pipelines;
mod record;
mod targets;
#[cfg(test)]
mod tests;

use super::*;
use postretro_render_cpu::fluid_surface::reflection_quality::FluidReflectionQuality;
use postretro_render_cpu::planar_reflection::{MirrorCamera, PlanarPreset, planar_preset};
use targets::PlanarTargets;

/// The planar layer's texel format: premultiplied (c·X, c), filterable, as
/// the screen-space reflection target.
pub(in crate::render) const PLANAR_LAYER_FORMAT: wgpu::TextureFormat =
    wgpu::TextureFormat::Rgba16Float;

/// The mask and resolve uniform, `PlanarInputs` in both planar shaders, as
/// floats: main view-projection, mirror raster view-projection and its
/// inverse, the main eye with the plane height, the mirror eye, the D and R
/// limits and bands, and the mirror and scene extents.
const PLANAR_INPUT_FLOATS: usize = 64;
pub(super) const PLANAR_INPUT_BYTES: u64 = (PLANAR_INPUT_FLOATS * 4) as u64;

pub(super) struct PlanarReflectionPass {
    /// This frame's mirror view; `None` with no selected plane.
    camera: Option<MirrorCamera>,
    /// The mirror's visible cells, copied into reused storage.
    visible: VisibleCells,
    /// The mirror's group 0: the main camera uniform with the mirror's
    /// raster view-projection and eye.
    uniform_buffer: wgpu::Buffer,
    uniform_bind_group: wgpu::BindGroup,
    uploaded_camera: Option<[u8; UNIFORM_SIZE]>,
    inputs_buffer: wgpu::Buffer,
    uploaded_inputs: Option<[f32; PLANAR_INPUT_FLOATS]>,
    pipelines: pipelines::PlanarPipelines,
    /// Mirror colour and depth at the planar layer's extent; they exist
    /// exactly while the planar layer does.
    targets: Option<PlanarTargets>,
    draws: mirror_draws::MirrorDraws,
    /// A test-only preset in place of the tier's, for the D and R proofs.
    #[cfg(test)]
    preset_override: Option<PlanarPreset>,
    /// Frames that wrote the planar layer; tests read it.
    #[cfg(test)]
    records: u32,
}

impl PlanarReflectionPass {
    /// Build the mirror's group 0 over the shared camera layout, the inputs
    /// uniform, and every planar pipeline, at full init and at every tier.
    pub fn new(device: &wgpu::Device, camera_layout: &wgpu::BindGroupLayout) -> Self {
        let uniform_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Planar Mirror Camera Uniform"),
            size: UNIFORM_SIZE as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let uniform_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Planar Mirror Camera Bind Group"),
            layout: camera_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: uniform_buffer.as_entire_binding(),
            }],
        });
        let inputs_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Planar Mask and Resolve Uniform"),
            size: PLANAR_INPUT_BYTES,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        Self {
            camera: None,
            visible: VisibleCells::Culled(Vec::new()),
            uniform_buffer,
            uniform_bind_group,
            uploaded_camera: None,
            inputs_buffer,
            uploaded_inputs: None,
            pipelines: pipelines::PlanarPipelines::new(device),
            targets: None,
            draws: mirror_draws::MirrorDraws::default(),
            #[cfg(test)]
            preset_override: None,
            #[cfg(test)]
            records: 0,
        }
    }

    /// The preset `quality` runs: `None` below very high.
    fn preset(&self, quality: FluidReflectionQuality) -> Option<PlanarPreset> {
        let preset = planar_preset(quality)?;
        #[cfg(test)]
        if let Some(preset) = self.preset_override {
            return Some(preset.sanitized());
        }
        Some(preset)
    }

    /// Write the mirror's group-0 uniform from this frame's main uniform,
    /// where it changed. Nothing without a mirror view or targets.
    pub fn upload_camera(
        &mut self,
        queue: &crate::render::uploads::UploadQueue,
        main: &[u8; UNIFORM_SIZE],
    ) {
        let Some(camera) = self.camera.filter(|_| self.targets.is_some()) else {
            return;
        };
        let mut data = *main;
        for (index, value) in camera.raster_vp.to_cols_array().iter().enumerate() {
            data[index * 4..index * 4 + 4].copy_from_slice(&value.to_ne_bytes());
        }
        for (index, value) in camera.eye.to_array().iter().enumerate() {
            data[64 + index * 4..68 + index * 4].copy_from_slice(&value.to_ne_bytes());
        }
        // The SDF shadow factor is main-screen aligned: the mirror never reads it.
        data[96..100].copy_from_slice(&0u32.to_ne_bytes());
        if self.uploaded_camera == Some(data) {
            return;
        }
        queue.write_buffer(&self.uniform_buffer, 0, &data);
        self.uploaded_camera = Some(data);
    }

    /// Drop the level's mirror view, visible cells and targets. Every
    /// geometry install, unload included, starts here.
    pub fn clear_level(&mut self) {
        self.camera = None;
        if let VisibleCells::Culled(cells) = &mut self.visible {
            cells.clear();
        } else {
            self.visible = VisibleCells::Culled(Vec::new());
        }
        self.targets = None;
        self.draws.clear();
    }
}
