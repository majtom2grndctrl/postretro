// Fluid pass group 5, the scene inputs: scene depth and the eye fog weight,
// built at full init and rebuilt only with the scene targets.
// See: context/lib/rendering_pipeline.md §7.3a, §10

use std::num::NonZeroU64;

use wgpu::util::DeviceExt;

/// Pipeline group of the fluid scene inputs.
pub(crate) const FLUID_SCENE_INPUTS_GROUP: u32 = 5;

/// The fluid scene uniform (binding 1): `[eye fog weight, spare ×3]`. The
/// spares are for later scene-wide fluid inputs, such as reflections'.
type FluidSceneUniform = [f32; 4];
pub(crate) const FLUID_SCENE_UNIFORM_BYTES: u64 = std::mem::size_of::<FluidSceneUniform>() as u64;

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
    /// Written only when the fog weight changes.
    uniform_buffer: wgpu::Buffer,
    bind_group: wgpu::BindGroup,
    fog_weight: f32,
    uploaded_fog_weight: f32,
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
        let initial: FluidSceneUniform = [DEFAULT_FLUID_FOG_WEIGHT, 0.0, 0.0, 0.0];
        let uniform_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Fluid Surface Scene Uniform"),
            contents: bytemuck::cast_slice(&initial),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });
        let bind_group = create_bind_group(device, &layout, scene_depth, &uniform_buffer);
        Self {
            layout,
            uniform_buffer,
            bind_group,
            fog_weight: DEFAULT_FLUID_FOG_WEIGHT,
            uploaded_fog_weight: DEFAULT_FLUID_FOG_WEIGHT,
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

    /// Write the uniform if the fog weight changed since it last uploaded.
    pub fn upload(&mut self, queue: &crate::render::uploads::UploadQueue) {
        if self.fog_weight == self.uploaded_fog_weight {
            return;
        }
        let uniform: FluidSceneUniform = [self.fog_weight, 0.0, 0.0, 0.0];
        queue.write_buffer(&self.uniform_buffer, 0, bytemuck::cast_slice(&uniform));
        self.uploaded_fog_weight = self.fog_weight;
    }

    #[cfg(test)]
    pub fn fog_weight(&self) -> f32 {
        self.fog_weight
    }

    /// The scene depth the bind group was last built over, and the build count.
    #[cfg(test)]
    pub fn scene_depth(&self) -> (&wgpu::TextureView, u32) {
        (&self.scene_depth, self.builds)
    }
}
