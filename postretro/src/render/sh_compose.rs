// SH compose compute pass — stub phase.
//
// Once-per-frame compute dispatch that composes the static base SH bands
// plus (eventually) animated per-light deltas into a parallel set of
// "total" SH band 3D textures. All SH consumers (forward, billboard, fog)
// sample the total textures via `ShVolumeResources::bind_group`, so the
// delta data lights them up without any consumer-side branching.
//
// Stub: this pass is a pure base→total copy. No delta data is read or
// applied. Its purpose at this phase is to validate that
//   1. consumers correctly use the total textures, and
//   2. the compose pipeline + bind groups wire up cleanly,
// before Task D's delta payload is landed.
//
// Dispatch order: runs after BVH cull / animated-lightmap compose and
// before the depth pre-pass — see `render/mod.rs`. wgpu infers the
// storage-write → sampled-read barrier from the bind-group usage change
// when the forward pass reaches the SH bind group.
//
// See: context/lib/rendering_pipeline.md §7.1

use super::sh_volume::{SH_BAND_COUNT, ShVolumeResources};

/// GPU-side compose pass. Always allocated alongside `ShVolumeResources` —
/// when the level has no SH section, the base/total textures are 1×1×1
/// dummies and the dispatch is a single tiny workgroup. The cost is
/// negligible and keeping the pipeline live unconditionally keeps the
/// frame-loop control flow simple.
pub struct ShComposeResources {
    pipeline: wgpu::ComputePipeline,
    bind_group: wgpu::BindGroup,
    /// Probe grid dimensions. Drives the dispatch shape — one thread per
    /// probe, rounded up to the (4,4,4) workgroup size.
    grid_dimensions: [u32; 3],
}

impl ShComposeResources {
    /// Build the compose pipeline and bind group from an existing
    /// `ShVolumeResources`. Borrows the base sampled views and total storage
    /// views; the compose-side bind group holds them alive (wgpu refcounts
    /// internally) so they survive even if the consumer-side bind group is
    /// rebuilt later.
    pub fn new(device: &wgpu::Device, sh: &ShVolumeResources) -> Self {
        let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("SH Compose BGL"),
            entries: &compose_bgl_entries(),
        });

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("SH Compose Pipeline Layout"),
            bind_group_layouts: &[Some(&bind_group_layout)],
            immediate_size: 0,
        });

        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("SH Compose Shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("../shaders/sh_compose.wgsl").into()),
        });

        let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("SH Compose Pipeline"),
            layout: Some(&pipeline_layout),
            module: &shader,
            entry_point: Some("compose_main"),
            compilation_options: wgpu::PipelineCompilationOptions::default(),
            cache: None,
        });

        // Grid-dims uniform: vec3<u32> + pad. Static for the level's lifetime.
        let mut grid_bytes = [0u8; 16];
        grid_bytes[0..4].copy_from_slice(&sh.grid_dimensions[0].to_ne_bytes());
        grid_bytes[4..8].copy_from_slice(&sh.grid_dimensions[1].to_ne_bytes());
        grid_bytes[8..12].copy_from_slice(&sh.grid_dimensions[2].to_ne_bytes());
        // bytes[12..16] padding, already zero.
        let grid_buffer = {
            use wgpu::util::DeviceExt;
            device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("SH Compose Grid Dims"),
                contents: &grid_bytes,
                usage: wgpu::BufferUsages::UNIFORM,
            })
        };

        debug_assert_eq!(sh.base_band_views.len(), SH_BAND_COUNT);
        debug_assert_eq!(sh.total_band_storage_views.len(), SH_BAND_COUNT);

        let mut entries: Vec<wgpu::BindGroupEntry> = Vec::with_capacity(SH_BAND_COUNT * 2 + 1);
        for (i, view) in sh.base_band_views.iter().enumerate() {
            entries.push(wgpu::BindGroupEntry {
                binding: i as u32,
                resource: wgpu::BindingResource::TextureView(view),
            });
        }
        for (i, view) in sh.total_band_storage_views.iter().enumerate() {
            entries.push(wgpu::BindGroupEntry {
                binding: (SH_BAND_COUNT + i) as u32,
                resource: wgpu::BindingResource::TextureView(view),
            });
        }
        entries.push(wgpu::BindGroupEntry {
            binding: (SH_BAND_COUNT * 2) as u32,
            resource: grid_buffer.as_entire_binding(),
        });

        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("SH Compose Bind Group"),
            layout: &bind_group_layout,
            entries: &entries,
        });

        Self {
            pipeline,
            bind_group,
            grid_dimensions: sh.grid_dimensions,
        }
    }

    /// Encode the per-frame compose dispatch. Always dispatched — even for
    /// maps without an SH section, the 1×1×1 dummy grid runs in a single
    /// workgroup so the cost is irrelevant.
    pub fn dispatch(&self, encoder: &mut wgpu::CommandEncoder) {
        let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
            label: Some("SH Compose"),
            timestamp_writes: None,
        });
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, &self.bind_group, &[]);
        let wg_x = self.grid_dimensions[0].div_ceil(4).max(1);
        let wg_y = self.grid_dimensions[1].div_ceil(4).max(1);
        let wg_z = self.grid_dimensions[2].div_ceil(4).max(1);
        pass.dispatch_workgroups(wg_x, wg_y, wg_z);
    }
}

fn compose_bgl_entries() -> Vec<wgpu::BindGroupLayoutEntry> {
    let mut entries = Vec::with_capacity(SH_BAND_COUNT * 2 + 1);
    // Bindings 0..9: base SH bands as sampled 3D textures (read via textureLoad).
    for i in 0..SH_BAND_COUNT {
        entries.push(wgpu::BindGroupLayoutEntry {
            binding: i as u32,
            visibility: wgpu::ShaderStages::COMPUTE,
            ty: wgpu::BindingType::Texture {
                // textureLoad with `Float { filterable: false }` is sufficient
                // for the integer-coordinate fetches the compose shader uses.
                sample_type: wgpu::TextureSampleType::Float { filterable: false },
                view_dimension: wgpu::TextureViewDimension::D3,
                multisampled: false,
            },
            count: None,
        });
    }
    // Bindings 9..18: total SH bands as storage-write 3D textures.
    for i in 0..SH_BAND_COUNT {
        entries.push(wgpu::BindGroupLayoutEntry {
            binding: (SH_BAND_COUNT + i) as u32,
            visibility: wgpu::ShaderStages::COMPUTE,
            ty: wgpu::BindingType::StorageTexture {
                access: wgpu::StorageTextureAccess::WriteOnly,
                format: wgpu::TextureFormat::Rgba16Float,
                view_dimension: wgpu::TextureViewDimension::D3,
            },
            count: None,
        });
    }
    // Binding 18: grid-dimensions uniform.
    entries.push(wgpu::BindGroupLayoutEntry {
        binding: (SH_BAND_COUNT * 2) as u32,
        visibility: wgpu::ShaderStages::COMPUTE,
        ty: wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Uniform,
            has_dynamic_offset: false,
            min_binding_size: None,
        },
        count: None,
    });
    entries
}

#[cfg(test)]
mod tests {
    #[test]
    fn sh_compose_shader_parses_and_exports_compose_main() {
        let src = include_str!("../shaders/sh_compose.wgsl");
        let module =
            naga::front::wgsl::parse_str(src).expect("sh_compose.wgsl should parse as WGSL");
        let has_compose = module
            .entry_points
            .iter()
            .any(|ep| ep.name == "compose_main" && ep.stage == naga::ShaderStage::Compute);
        assert!(has_compose, "compose_main entry point missing");
    }
}
