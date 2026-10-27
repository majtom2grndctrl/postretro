// Planar water reflection pass: the mirror view, its mask and world draws, the resolve raster.
// See: context/lib/rendering_pipeline.md §7.3a

mod api;
#[cfg(test)]
mod frame_content_tests;
#[cfg(test)]
mod frame_light_tests;
#[cfg(test)]
mod frame_port_tests;
#[cfg(test)]
mod frame_test_fixtures;
#[cfg(test)]
mod frame_tests;
#[cfg(test)]
mod frame_tilt_tests;
#[cfg(test)]
mod gate_tests;
#[cfg(feature = "dev-tools")]
mod launch_settings;
#[cfg(test)]
mod lifecycle_tests;
mod mirror_draws;
mod mirror_sprites;
mod pipelines;
mod record;
mod targets;
#[cfg(test)]
mod tests;
#[cfg(any(test, feature = "dev-tools", feature = "capture"))]
mod tuning;

use super::*;
use postretro_render_cpu::fluid_surface::reflection_quality::FluidReflectionQuality;
use postretro_render_cpu::fluid_surface::reflection_style::FluidReflectionStyle;
use postretro_render_cpu::planar_reflection::{MirrorCamera, PlanarPreset, planar_preset};
use targets::PlanarTargets;

/// The planar layer's texel format: premultiplied (c·X, c), filterable, as
/// the screen-space reflection target.
pub(in crate::render) const PLANAR_LAYER_FORMAT: wgpu::TextureFormat =
    wgpu::TextureFormat::Rgba16Float;

/// The mask and resolve uniform, `PlanarInputs` in both planar shaders, as
/// 4-byte words: main view-projection, mirror raster view-projection and its
/// inverse, the main eye with the plane height, the mirror eye, the D and R
/// limits and bands, the mirror and scene extents, then the reflection style
/// (u32) and the displayed offset's multiplier (i32), stored by their bits,
/// and two spare words.
const PLANAR_INPUT_FLOATS: usize = 68;
pub(super) const PLANAR_INPUT_BYTES: u64 = (PLANAR_INPUT_FLOATS * 4) as u64;

pub(super) struct PlanarReflectionPass {
    /// This frame's mirror view; `None` with no selected plane.
    camera: Option<MirrorCamera>,
    /// The mirror's visible cells, copied into reused storage.
    visible: VisibleCells,
    /// Main visible ∪ mirror visible, rebuilt in place each frame the mirror
    /// draws: the cells animated-lightmap compose covers.
    compose_union: VisibleCells,
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
    /// A layer build waits for the next extent commit: a scene extent was
    /// recorded but not committed when the build was asked for. Every commit
    /// path consumes it, so a commit that moves no scene still builds.
    build_deferred: bool,
    draws: mirror_draws::MirrorDraws,
    /// The mirror's billboards and particles, in their own instance store.
    sprites: mirror_sprites::MirrorSprites,
    /// A preset in place of the tier's, held at the tier it was set at: the
    /// diagnostics panel, the launch seed, a capture scene, the D and R
    /// proofs.
    #[cfg(any(test, feature = "dev-tools", feature = "capture"))]
    preset_override: Option<tuning::PresetOverride>,
    /// The panel's planar-off and crop switches and the pending launch seed.
    #[cfg(feature = "dev-tools")]
    dev: tuning::PlanarDevState,
    /// Test-only: the resolve's displayed offset forced to zero, so a proof
    /// can read the same frame untilted. Production never sets it.
    #[cfg(test)]
    zero_offset: bool,
    /// Frames that wrote the planar layer; tests read it.
    #[cfg(test)]
    records: u32,
    /// Mirror target builds; tests read it.
    #[cfg(test)]
    target_builds: u32,
}

impl PlanarReflectionPass {
    /// Build the mirror's group 0 over the shared camera layout, the inputs
    /// uniform, every planar pipeline (the resolve over the shared camera
    /// and world-material layouts) and the mirror's sprite store over the
    /// billboard pipeline's instance layout, at full init and at every tier.
    pub fn new(
        device: &wgpu::Device,
        camera_layout: &wgpu::BindGroupLayout,
        material_layout: &wgpu::BindGroupLayout,
        sprite_instance_layout: &wgpu::BindGroupLayout,
    ) -> Self {
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
            compose_union: VisibleCells::Culled(Vec::new()),
            uniform_buffer,
            uniform_bind_group,
            uploaded_camera: None,
            inputs_buffer,
            uploaded_inputs: None,
            pipelines: pipelines::PlanarPipelines::new(device, camera_layout, material_layout),
            targets: None,
            build_deferred: false,
            draws: mirror_draws::MirrorDraws::default(),
            sprites: mirror_sprites::MirrorSprites::new(device, sprite_instance_layout),
            #[cfg(any(test, feature = "dev-tools", feature = "capture"))]
            preset_override: None,
            #[cfg(feature = "dev-tools")]
            dev: tuning::PlanarDevState::from_environment(),
            #[cfg(test)]
            zero_offset: false,
            #[cfg(test)]
            records: 0,
            #[cfg(test)]
            target_builds: 0,
        }
    }

    /// The preset `quality` runs, with an override set at that tier: `None`
    /// below very high.
    fn preset(&self, quality: FluidReflectionQuality) -> Option<PlanarPreset> {
        let preset = planar_preset(quality)?;
        #[cfg(any(test, feature = "dev-tools", feature = "capture"))]
        if let Some(tuned) = self.preset_override.filter(|tuned| tuned.tier == quality) {
            return Some(tuned.preset);
        }
        Some(preset)
    }

    /// The resolve's displayed-offset multiplier: 1, so a pixel-style cell
    /// tilts as SSR's does; 0 only under the test-only zero-offset seam.
    fn offset_scale(&self) -> i32 {
        #[cfg(test)]
        if self.zero_offset {
            return 0;
        }
        1
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
        let data = mirror_camera_uniform(main, &camera);
        if self.uploaded_camera == Some(data) {
            return;
        }
        queue.write_buffer(&self.uniform_buffer, 0, &data);
        self.uploaded_camera = Some(data);
    }

    /// The cells animated-lightmap compose covers this frame: `main`, plus the
    /// mirror's visible cells on a frame the mirror draws at `quality`, so a
    /// reflected animated face shows its current light. Compose reads no
    /// block, so this adds no residency demand.
    pub fn compose_cells<'a>(
        &'a mut self,
        main: &'a VisibleCells,
        quality: FluidReflectionQuality,
    ) -> &'a VisibleCells {
        if self.camera.is_none() || self.targets.is_none() || self.preset(quality).is_none() {
            return main;
        }
        union_cells(main, &self.visible, &mut self.compose_union);
        &self.compose_union
    }

    /// Drop the level's mirror view, visible cells, targets, world draws and
    /// sprites. Every geometry install, unload included, starts here.
    pub fn clear_level(&mut self) {
        self.camera = None;
        if let VisibleCells::Culled(cells) = &mut self.visible {
            cells.clear();
        } else {
            self.visible = VisibleCells::Culled(Vec::new());
        }
        self.targets = None;
        self.draws.clear();
        self.sprites.clear();
    }
}

/// The mirror's group-0 uniform: the main frame's, with the mirror's raster
/// view-projection and eye, so every world, mover, mesh and billboard draw
/// shades from the mirror camera and every billboard faces it. The world
/// forward pass still binds the main view's group 5, whose SDF shadow factor
/// and scene depth are main-screen aligned, so nothing in the mirror reads
/// them: the SDF flags are cleared (SDF-tagged static lights reflect
/// unshadowed), and the SDF debug views that read those textures or the
/// factor (`SdfShadowMode` 2 to 4) shade the reflection as `On`. The
/// shadowmask views read no screen texture and stay.
fn mirror_camera_uniform(main: &[u8; UNIFORM_SIZE], camera: &MirrorCamera) -> [u8; UNIFORM_SIZE] {
    use postretro_render_cpu::frame_uniforms::SdfShadowMode;
    let mut data = *main;
    for (index, value) in camera.raster_vp.to_cols_array().iter().enumerate() {
        data[index * 4..index * 4 + 4].copy_from_slice(&value.to_ne_bytes());
    }
    for (index, value) in camera.eye.to_array().iter().enumerate() {
        data[64 + index * 4..68 + index * 4].copy_from_slice(&value.to_ne_bytes());
    }
    data[MIRROR_SDF_FLAGS_BYTES].copy_from_slice(&0u32.to_ne_bytes());
    let mode = u32::from_ne_bytes(
        data[MIRROR_SDF_MODE_BYTES]
            .try_into()
            .expect("a four-byte word"),
    );
    if [
        SdfShadowMode::Visualize,
        SdfShadowMode::VisualizeDebugPaths,
        SdfShadowMode::VisualizeNormals,
    ]
    .iter()
    .any(|screen_view| *screen_view as u32 == mode)
    {
        data[MIRROR_SDF_MODE_BYTES].copy_from_slice(&(SdfShadowMode::On as u32).to_ne_bytes());
    }
    data
}

/// `sdf_shadow_flags` and `sdf_shadow_mode` in the group-0 uniform
/// (`pipeline_layout.rs`'s offset table).
const MIRROR_SDF_FLAGS_BYTES: std::ops::Range<usize> = 96..100;
const MIRROR_SDF_MODE_BYTES: std::ops::Range<usize> = 100..104;

/// `main ∪ mirror` into `out`, ascending, allocation-free once grown. Either
/// side drawing all makes the union draw all.
fn union_cells(main: &VisibleCells, mirror: &VisibleCells, out: &mut VisibleCells) {
    let (VisibleCells::Culled(main), VisibleCells::Culled(mirror)) = (main, mirror) else {
        *out = VisibleCells::DrawAll;
        return;
    };
    if !matches!(out, VisibleCells::Culled(_)) {
        *out = VisibleCells::Culled(Vec::new());
    }
    let VisibleCells::Culled(union) = out else {
        return;
    };
    union.clear();
    union.extend_from_slice(main);
    union.extend_from_slice(mirror);
    union.sort_unstable();
    union.dedup();
}
