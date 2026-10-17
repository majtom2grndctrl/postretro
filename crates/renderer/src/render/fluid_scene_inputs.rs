// Fluid pass group 5, the scene inputs: scene depth, the eye fog weight and
// the mod's reflection style, built at full init and rebuilt only with the
// scene targets.
// See: context/lib/rendering_pipeline.md §7.3a, §10

use std::num::NonZeroU64;

use bytemuck::{Pod, Zeroable};
use postretro_render_cpu::fluid_surface::reflection_style::FluidReflectionStyle;
use wgpu::util::DeviceExt;

/// Pipeline group of the fluid scene inputs.
pub(crate) const FLUID_SCENE_INPUTS_GROUP: u32 = 5;

/// The fluid scene uniform (binding 1), the shader's `FluidSceneInputs`
/// member for member: the eye fog weight, the reflection style
/// (`FluidReflectionStyle::uniform_value`), and two spares for later
/// scene-wide fluid inputs, such as screen-space reflections'.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Pod, Zeroable)]
pub(crate) struct FluidSceneUniform {
    pub fog_weight: f32,
    pub reflection_style: u32,
    pub spare: [f32; 2],
}
pub(crate) const FLUID_SCENE_UNIFORM_BYTES: u64 = std::mem::size_of::<FluidSceneUniform>() as u64;

impl FluidSceneUniform {
    fn from_inputs(fog_weight: f32, style: FluidReflectionStyle) -> Self {
        Self {
            fog_weight,
            reflection_style: style.uniform_value(),
            spare: [0.0; 2],
        }
    }
}

/// The eye fog weight before any write: a renderer built or recreated
/// mid-level draws fog on its first frame.
const DEFAULT_FLUID_FOG_WEIGHT: f32 = 1.0;

/// Scene depth, loaded under the pass's read-only depth attachment, and the
/// scene uniform. A texture and a uniform, never storage: the shared groups
/// already spend the whole FRAGMENT storage budget.
pub(crate) fn fluid_scene_inputs_bind_group_layout_entries() -> [wgpu::BindGroupLayoutEntry; 2] {
    [
        wgpu::BindGroupLayoutEntry {
            binding: 0,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Depth,
                view_dimension: wgpu::TextureViewDimension::D2,
                multisampled: false,
            },
            count: None,
        },
        wgpu::BindGroupLayoutEntry {
            binding: 1,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Uniform,
                has_dynamic_offset: false,
                min_binding_size: NonZeroU64::new(FLUID_SCENE_UNIFORM_BYTES),
            },
            count: None,
        },
    ]
}

/// Group 5's layout, uniform and bind group. Level install and clear never
/// touch it, so a level with no fluid, or none yet, still has it current.
pub(crate) struct FluidSceneInputs {
    layout: wgpu::BindGroupLayout,
    /// Written only when the fog weight or the reflection style changes.
    uniform_buffer: wgpu::Buffer,
    bind_group: wgpu::BindGroup,
    fog_weight: f32,
    reflection_style: FluidReflectionStyle,
    /// The uniform as last written.
    uploaded: FluidSceneUniform,
    /// The scene depth the bind group was last built over; tests read it.
    #[cfg(test)]
    scene_depth: wgpu::TextureView,
    /// Bind group builds so far, init included; tests read it.
    #[cfg(test)]
    builds: u32,
}

fn create_bind_group(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    scene_depth: &wgpu::TextureView,
    uniform_buffer: &wgpu::Buffer,
) -> wgpu::BindGroup {
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("Fluid Surface Scene Inputs Bind Group (group 5)"),
        layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(scene_depth),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: uniform_buffer.as_entire_binding(),
            },
        ],
    })
}

impl FluidSceneInputs {
    /// Build group 5 over the current scene depth, at full init.
    pub fn new(device: &wgpu::Device, scene_depth: &wgpu::TextureView) -> Self {
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Fluid Surface Scene Inputs BGL (group 5)"),
            entries: &fluid_scene_inputs_bind_group_layout_entries(),
        });
        // Pixel style until the mod's profile says otherwise: frame capture,
        // which loads no manifest, keeps it.
        let initial = FluidSceneUniform::from_inputs(
            DEFAULT_FLUID_FOG_WEIGHT,
            FluidReflectionStyle::default(),
        );
        let uniform_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Fluid Surface Scene Uniform"),
            contents: bytemuck::bytes_of(&initial),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });
        let bind_group = create_bind_group(device, &layout, scene_depth, &uniform_buffer);
        Self {
            layout,
            uniform_buffer,
            bind_group,
            fog_weight: DEFAULT_FLUID_FOG_WEIGHT,
            reflection_style: FluidReflectionStyle::default(),
            uploaded: initial,
            #[cfg(test)]
            scene_depth: scene_depth.clone(),
            #[cfg(test)]
            builds: 1,
        }
    }

    pub fn layout(&self) -> &wgpu::BindGroupLayout {
        &self.layout
    }

    pub fn bind_group(&self) -> &wgpu::BindGroup {
        &self.bind_group
    }

    /// Rebuild over a recreated scene depth. The scene-target rebuild is the
    /// only caller: the frame never builds a bind group.
    pub fn rebuild(&mut self, device: &wgpu::Device, scene_depth: &wgpu::TextureView) {
        self.bind_group =
            create_bind_group(device, &self.layout, scene_depth, &self.uniform_buffer);
        #[cfg(test)]
        {
            self.scene_depth = scene_depth.clone();
            self.builds += 1;
        }
    }

    /// Store the frame's eye fog weight; it uploads before the next fluid pass.
    pub fn set_fog_weight(&mut self, weight: f32) {
        // Clamped so a stray value cannot push murk past the fog colour; a
        // NaN keeps the last weight rather than poisoning the uniform.
        if !weight.is_nan() {
            self.fog_weight = weight.clamp(0.0, 1.0);
        }
    }

    /// Store the mod's reflection style; it uploads before the next fluid
    /// pass. Change-driven: a commit that keeps the style writes nothing.
    pub fn set_reflection_style(&mut self, style: FluidReflectionStyle) {
        self.reflection_style = style;
    }

    /// The uniform as the next upload writes it.
    pub fn pending(&self) -> FluidSceneUniform {
        FluidSceneUniform::from_inputs(self.fog_weight, self.reflection_style)
    }

    /// Write the uniform if the fog weight or the reflection style changed
    /// since it last uploaded.
    pub fn upload(&mut self, queue: &crate::render::uploads::UploadQueue) {
        let uniform = self.pending();
        if uniform == self.uploaded {
            return;
        }
        queue.write_buffer(&self.uniform_buffer, 0, bytemuck::bytes_of(&uniform));
        self.uploaded = uniform;
    }

    #[cfg(test)]
    pub fn fog_weight(&self) -> f32 {
        self.fog_weight
    }

    /// The uniform as last written; tests read it.
    #[cfg(test)]
    pub fn uploaded(&self) -> FluidSceneUniform {
        self.uploaded
    }

    /// The scene depth the bind group was last built over, and the build count.
    #[cfg(test)]
    pub fn scene_depth(&self) -> (&wgpu::TextureView, u32) {
        (&self.scene_depth, self.builds)
    }
}
