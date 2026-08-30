// Direct-SH atlas resources shared by the dynamic-receiver compose passes.
// See: context/lib/rendering_pipeline.md §4, §7.1

use postretro_level_format::animated_direct_sh_delta_volumes::AnimatedDirectShDeltaVolumesSection;
use postretro_level_format::direct_sh_delta_volumes::DirectShDeltaVolumesSection;
use postretro_level_format::direct_sh_volume::DirectShVolumeSection;
use postretro_level_format::lightmap::IRRADIANCE_FORMAT_BC6H;
use postretro_render_cpu::sh_volume::{
    BIND_DYNAMIC_DIRECT_PARAMS, BIND_SH_DIRECT_ATLAS, build_dynamic_direct_params_bytes,
};
use wgpu::util::DeviceExt;

use super::sh_volume::AnimatedLightBuffers;

/// Direct-SH atlas geometry shared by the promotion and animated-add compose
/// passes. It is captured at level load so the compose passes do not need to
/// reach back into the indirect SH resource owner.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct DirectAtlasLayout {
    pub(super) grid_dimensions: [u32; 3],
    pub(super) atlas_dimensions: [u32; 2],
    pub(super) tile_dimension: u32,
    pub(super) tile_border: u32,
    pub(super) atlas_tiles_per_row: u32,
    pub(super) tiles_per_layer: u32,
    pub(super) atlas_layer_count: u32,
}

impl DirectAtlasLayout {
    pub(super) fn from_direct_section(section: &DirectShVolumeSection) -> Self {
        Self {
            grid_dimensions: section.grid_dimensions,
            atlas_dimensions: section.atlas_dimensions,
            tile_dimension: section.tile_dimension,
            tile_border: section.tile_border,
            atlas_tiles_per_row: section.atlas_tiles_per_row,
            tiles_per_layer: section.tiles_per_layer,
            atlas_layer_count: section.layer_count,
        }
    }

    pub(super) fn from_sh_section(
        section: &postretro_level_format::sh_volume::OctahedralShVolumeSection,
    ) -> Self {
        Self {
            grid_dimensions: section.grid_dimensions,
            atlas_dimensions: section.atlas_dimensions,
            tile_dimension: section.tile_dimension,
            tile_border: section.tile_border,
            atlas_tiles_per_row: section.atlas_tiles_per_row,
            tiles_per_layer: section.tiles_per_layer,
            atlas_layer_count: section.layer_count,
        }
    }
}

/// Renderer-owned direct-SH textures and mesh-only dynamic-direct parameters.
///
/// This owns the direct receiver path independently from indirect SH resources
/// so future billboard-specific direct fields can share its load/compose seam
/// without widening the indirect resource owner.
pub(super) struct DirectShResources {
    pub(super) has_direct: bool,
    pub(super) has_direct_base: bool,
    pub(super) compose_layout: Option<DirectAtlasLayout>,
    pub(super) atlas_view: wgpu::TextureView,
    pub(super) base_atlas_view: wgpu::TextureView,
    pub(super) composed_storage_view: Option<wgpu::TextureView>,
    pub(super) intermediate_storage_view: Option<wgpu::TextureView>,
    pub(super) intermediate_sampled_view: Option<wgpu::TextureView>,
    dynamic_direct_params_buffer: wgpu::Buffer,
    animated_descriptor_indices: Vec<u32>,
}

impl DirectShResources {
    #[allow(clippy::too_many_arguments)]
    pub(super) fn new(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        direct_section: Option<&DirectShVolumeSection>,
        direct_delta_section: Option<&DirectShDeltaVolumesSection>,
        animated_direct_delta_section: Option<&AnimatedDirectShDeltaVolumesSection>,
        fallback_layout: Option<DirectAtlasLayout>,
    ) -> Self {
        let direct_usage = resolve_direct_atlas_usage(
            direct_section,
            direct_delta_section,
            animated_direct_delta_section,
            fallback_layout,
        );
        let (base_atlas_texture, has_direct_base) =
            upload_direct_atlas_texture(device, queue, direct_section);
        let has_animated_direct =
            animated_direct_delta_section.is_some() && fallback_layout.is_some();
        let has_direct = direct_atlas_present(has_direct_base, has_animated_direct);
        let base_atlas_view = base_atlas_texture.create_view(&wgpu::TextureViewDescriptor {
            label: Some("SH Direct Octahedral Base Atlas View"),
            dimension: Some(wgpu::TextureViewDimension::D2Array),
            ..Default::default()
        });
        let (
            atlas_view,
            composed_storage_view,
            intermediate_storage_view,
            intermediate_sampled_view,
        ) = if has_direct && direct_usage.needs_composed_atlas {
            let texture = create_direct_composed_atlas_texture(
                device,
                direct_usage.atlas_dimensions,
                direct_usage.layer_count,
                "SH Direct Composed Octahedral Atlas",
            );
            let sampled = texture.create_view(&wgpu::TextureViewDescriptor {
                label: Some("SH Direct Composed Octahedral Atlas Sampled View"),
                dimension: Some(wgpu::TextureViewDimension::D2Array),
                ..Default::default()
            });
            let storage = texture.create_view(&wgpu::TextureViewDescriptor {
                label: Some("SH Direct Composed Octahedral Atlas Storage View"),
                dimension: Some(wgpu::TextureViewDimension::D2Array),
                ..Default::default()
            });
            if direct_usage.needs_intermediate_atlas {
                let intermediate = create_direct_composed_atlas_texture(
                    device,
                    direct_usage.atlas_dimensions,
                    direct_usage.layer_count,
                    "SH Direct Compose Intermediate Atlas",
                );
                let intermediate_storage = intermediate.create_view(&wgpu::TextureViewDescriptor {
                    label: Some("SH Direct Compose Intermediate Atlas Storage View"),
                    dimension: Some(wgpu::TextureViewDimension::D2Array),
                    ..Default::default()
                });
                let intermediate_sampled = intermediate.create_view(&wgpu::TextureViewDescriptor {
                    label: Some("SH Direct Compose Intermediate Atlas Sampled View"),
                    dimension: Some(wgpu::TextureViewDimension::D2Array),
                    ..Default::default()
                });
                (
                    sampled,
                    Some(storage),
                    Some(intermediate_storage),
                    Some(intermediate_sampled),
                )
            } else {
                (sampled, Some(storage), None, None)
            }
        } else {
            (base_atlas_view.clone(), None, None, None)
        };

        let dynamic_direct_params_buffer =
            device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("Dynamic Direct Params Uniform"),
                contents: &build_dynamic_direct_params_bytes(1.0, has_direct),
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            });

        Self {
            has_direct,
            has_direct_base,
            compose_layout: direct_usage.layout(),
            atlas_view,
            base_atlas_view,
            composed_storage_view,
            intermediate_storage_view,
            intermediate_sampled_view,
            dynamic_direct_params_buffer,
            animated_descriptor_indices: animated_direct_delta_section
                .map(|section| section.animation_descriptor_indices.clone())
                .unwrap_or_default(),
        }
    }

    pub(super) fn dynamic_direct_params_binding(&self) -> wgpu::BindingResource<'_> {
        self.dynamic_direct_params_buffer.as_entire_binding()
    }

    pub(super) fn write_dynamic_direct_params(&self, queue: &wgpu::Queue, scale: f32) {
        let bytes = build_dynamic_direct_params_bytes(scale, self.has_direct);
        queue.write_buffer(&self.dynamic_direct_params_buffer, 0, &bytes);
    }

    /// The Case-2 dispatch gate reads only the section-45 descriptor index map,
    /// never the indirect section-27 mapping.
    pub(super) fn has_active_animated_descriptor(&self, animation: &AnimatedLightBuffers) -> bool {
        animation.any_active_for_descriptor_indices(&self.animated_descriptor_indices)
    }
}

/// Append direct receiver entries to the shared group-3 layout. Keeping the
/// extension here makes the direct-resource seam the sole owner of future
/// billboard direct textures while preserving the shared builder's callers.
pub(super) fn append_shared_bind_group_layout_entries(
    entries: &mut Vec<wgpu::BindGroupLayoutEntry>,
) {
    entries.push(wgpu::BindGroupLayoutEntry {
        binding: BIND_SH_DIRECT_ATLAS,
        visibility: wgpu::ShaderStages::VERTEX | wgpu::ShaderStages::FRAGMENT,
        ty: wgpu::BindingType::Texture {
            sample_type: wgpu::TextureSampleType::Float { filterable: true },
            view_dimension: wgpu::TextureViewDimension::D2Array,
            multisampled: false,
        },
        count: None,
    });
}

pub(super) fn mesh_dynamic_direct_params_layout_entry() -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding: BIND_DYNAMIC_DIRECT_PARAMS,
        visibility: wgpu::ShaderStages::FRAGMENT,
        ty: wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Uniform,
            has_dynamic_offset: false,
            min_binding_size: None,
        },
        count: None,
    }
}

pub(super) fn atlas_fits(per_layer_dim: [u32; 2], layer_count: u32, limits: &wgpu::Limits) -> bool {
    per_layer_dim[0] > 0
        && per_layer_dim[1] > 0
        && layer_count > 0
        && per_layer_dim[0] <= limits.max_texture_dimension_2d
        && per_layer_dim[1] <= limits.max_texture_dimension_2d
        && layer_count <= limits.max_texture_array_layers
}

/// Build the DIRECT static-light atlas texture from the optional section.
/// Absent or unusable data binds a valid 4×4 BC6H dummy and clears `has_direct`.
fn upload_direct_atlas_texture(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    section: Option<&DirectShVolumeSection>,
) -> (wgpu::Texture, bool) {
    let limits = device.limits();
    let usable = section.filter(|section| {
        let fits = atlas_fits(section.atlas_dimensions, section.layer_count, &limits);
        if !fits {
            log::error!(
                "[Renderer] Direct SH atlas {}x{}x{} exceeds device limits (maxTextureDimension2D {}, maxTextureArrayLayers {}) or is empty; binding the direct dummy for this level",
                section.atlas_dimensions[0],
                section.atlas_dimensions[1],
                section.layer_count,
                limits.max_texture_dimension_2d,
                limits.max_texture_array_layers,
            );
        }
        fits
    });

    let Some(section) = usable else {
        return (upload_direct_atlas_dummy(device, queue), false);
    };

    let (format, width, height) = if section.irradiance_format == IRRADIANCE_FORMAT_BC6H {
        (
            wgpu::TextureFormat::Bc6hRgbUfloat,
            section.atlas_dimensions[0].div_ceil(4) * 4,
            section.atlas_dimensions[1].div_ceil(4) * 4,
        )
    } else {
        (
            wgpu::TextureFormat::Rgba16Float,
            section.atlas_dimensions[0],
            section.atlas_dimensions[1],
        )
    };

    let texture = device.create_texture_with_data(
        queue,
        &wgpu::TextureDescriptor {
            label: Some("SH Direct Octahedral Atlas"),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: section.layer_count,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        },
        wgpu::util::TextureDataOrder::LayerMajor,
        &section.atlas,
    );
    (texture, true)
}

fn upload_direct_atlas_dummy(device: &wgpu::Device, queue: &wgpu::Queue) -> wgpu::Texture {
    let zero_block = [0u8; 16];
    device.create_texture_with_data(
        queue,
        &wgpu::TextureDescriptor {
            label: Some("SH Direct Octahedral Atlas Dummy"),
            size: wgpu::Extent3d {
                width: 4,
                height: 4,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Bc6hRgbUfloat,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        },
        wgpu::util::TextureDataOrder::LayerMajor,
        &zero_block,
    )
}

pub(super) fn direct_section_when_base_present(
    base_present: bool,
    direct_section: Option<&DirectShVolumeSection>,
) -> Option<&DirectShVolumeSection> {
    direct_section.filter(|_| base_present)
}

fn direct_atlas_present(has_direct_base: bool, has_animated_direct: bool) -> bool {
    has_direct_base || has_animated_direct
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct DirectAtlasUsage {
    needs_composed_atlas: bool,
    needs_intermediate_atlas: bool,
    atlas_dimensions: [u32; 2],
    layer_count: u32,
    layout: Option<DirectAtlasLayout>,
}

impl DirectAtlasUsage {
    fn layout(self) -> Option<DirectAtlasLayout> {
        self.layout
    }
}

fn resolve_direct_atlas_usage(
    direct_section: Option<&DirectShVolumeSection>,
    _direct_delta_section: Option<&DirectShDeltaVolumesSection>,
    animated_direct_delta_section: Option<&AnimatedDirectShDeltaVolumesSection>,
    fallback_layout: Option<DirectAtlasLayout>,
) -> DirectAtlasUsage {
    let has_direct_base = direct_section.is_some();
    let has_animated_direct = animated_direct_delta_section.is_some();
    if !has_direct_base && !has_animated_direct {
        return DirectAtlasUsage::default();
    }
    let layout = direct_section
        .map(DirectAtlasLayout::from_direct_section)
        .or(fallback_layout);
    let Some(layout) = layout else {
        return DirectAtlasUsage::default();
    };
    DirectAtlasUsage {
        needs_composed_atlas: true,
        needs_intermediate_atlas: has_animated_direct,
        atlas_dimensions: layout.atlas_dimensions,
        layer_count: layout.atlas_layer_count,
        layout: Some(layout),
    }
}

fn create_direct_composed_atlas_texture(
    device: &wgpu::Device,
    atlas_dimensions: [u32; 2],
    layer_count: u32,
    label: &str,
) -> wgpu::Texture {
    device.create_texture(&wgpu::TextureDescriptor {
        label: Some(label),
        size: wgpu::Extent3d {
            width: atlas_dimensions[0].max(1),
            height: atlas_dimensions[1].max(1),
            depth_or_array_layers: layer_count.max(1),
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba16Float,
        usage: wgpu::TextureUsages::STORAGE_BINDING | wgpu::TextureUsages::TEXTURE_BINDING,
        view_formats: &[],
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use postretro_level_format::delta_sh_volumes::{
        AFFINITY_FACTOR, DEFAULT_DELTA_PROBE_F16_STRIDE, PROBES_PER_CELL,
    };

    fn direct_section() -> DirectShVolumeSection {
        DirectShVolumeSection {
            grid_origin: [0.0; 3],
            cell_size: [1.0; 3],
            grid_dimensions: [1, 1, 1],
            tile_dimension: 6,
            tile_border: 1,
            atlas_dimensions: [6, 6],
            layer_count: 1,
            tiles_per_layer: 1,
            atlas_tiles_per_row: 1,
            irradiance_format: IRRADIANCE_FORMAT_BC6H,
            atlas: vec![0; 64],
        }
    }

    #[test]
    fn atlas_fits_preserves_per_layer_and_array_limit_validation() {
        let limits = wgpu::Limits {
            max_texture_dimension_2d: 64,
            max_texture_array_layers: 4,
            ..Default::default()
        };

        assert!(atlas_fits([64, 32], 4, &limits));
        assert!(atlas_fits([1, 1], 1, &limits));
        assert!(!atlas_fits([0, 32], 1, &limits));
        assert!(!atlas_fits([65, 32], 1, &limits));
        assert!(!atlas_fits([32, 65], 1, &limits));
        assert!(!atlas_fits([32, 32], 0, &limits));
        assert!(!atlas_fits([32, 32], 5, &limits));
    }

    #[test]
    fn direct_atlas_usage_allocates_composed_texture_for_every_direct_base() {
        let direct = direct_section();
        let empty_delta = DirectShDeltaVolumesSection {
            affinity_factor: AFFINITY_FACTOR,
            affinity_dims: [1, 1, 1],
            tile_dimension: 6,
            tile_border: 1,
            valid_probe_masks: vec![u64::MAX],
            cell_levels: vec![0],
            affinity_offsets: vec![0, 0],
            affinity_lights: Vec::new(),
            delta_subblocks: Vec::new(),
        };
        let nonempty_delta = DirectShDeltaVolumesSection {
            affinity_lights: vec![0],
            affinity_offsets: vec![0, 1],
            delta_subblocks: vec![0; PROBES_PER_CELL * DEFAULT_DELTA_PROBE_F16_STRIDE],
            ..empty_delta.clone()
        };

        // Regression: base-only maps must compose so bit 3 can replace the
        // immutable base with zero for mesh, mover, and billboard receivers.
        let base_only = resolve_direct_atlas_usage(Some(&direct), None, None, None);
        assert!(base_only.needs_composed_atlas);
        assert!(!base_only.needs_intermediate_atlas);
        assert_eq!(base_only.atlas_dimensions, [6, 6]);
        assert_eq!(base_only.layer_count, 1);

        let empty_delta_usage =
            resolve_direct_atlas_usage(Some(&direct), Some(&empty_delta), None, None);
        assert!(empty_delta_usage.needs_composed_atlas);
        assert!(!empty_delta_usage.needs_intermediate_atlas);
        let usage = resolve_direct_atlas_usage(Some(&direct), Some(&nonempty_delta), None, None);
        assert!(usage.needs_composed_atlas);
        assert!(!usage.needs_intermediate_atlas);
        assert_eq!(usage.atlas_dimensions, [6, 6]);
        assert_eq!(usage.layer_count, 1);
    }

    #[test]
    fn animated_direct_only_enables_direct_composed_and_intermediate_atlas() {
        let mut sh = postretro_level_format::sh_volume::OctahedralShVolumeSection::placeholder();
        sh.atlas_dimensions = [12, 6];
        sh.layer_count = 2;
        let animated = AnimatedDirectShDeltaVolumesSection {
            affinity_factor: AFFINITY_FACTOR,
            affinity_dims: [1, 1, 1],
            tile_dimension: 6,
            tile_border: 1,
            animation_descriptor_indices: vec![0],
            valid_probe_masks: vec![u64::MAX],
            cell_levels: vec![0],
            affinity_offsets: vec![0, 1],
            affinity_lights: vec![0],
            delta_subblocks: vec![0; PROBES_PER_CELL * DEFAULT_DELTA_PROBE_F16_STRIDE],
        };

        let usage = resolve_direct_atlas_usage(
            None,
            None,
            Some(&animated),
            Some(DirectAtlasLayout::from_sh_section(&sh)),
        );
        assert!(direct_atlas_present(false, true));
        assert!(usage.needs_composed_atlas);
        assert!(usage.needs_intermediate_atlas);
        assert_eq!(usage.atlas_dimensions, sh.atlas_dimensions);
        assert_eq!(usage.layer_count, sh.layer_count);
    }

    #[test]
    fn direct_section_is_disabled_when_base_sh_is_unusable() {
        let direct = direct_section();

        assert!(direct_section_when_base_present(true, Some(&direct)).is_some());
        assert!(direct_section_when_base_present(false, Some(&direct)).is_none());
        assert!(direct_section_when_base_present(true, None).is_none());
    }

    #[test]
    fn direct_shared_binding_preserves_billboard_vertex_and_mesh_fragment_visibility() {
        let mut entries = Vec::new();
        append_shared_bind_group_layout_entries(&mut entries);
        let entry = entries
            .iter()
            .find(|entry| entry.binding == BIND_SH_DIRECT_ATLAS)
            .expect("direct resources must append the shared direct atlas binding");

        assert_eq!(BIND_SH_DIRECT_ATLAS, 15);
        assert_eq!(
            entry.visibility,
            wgpu::ShaderStages::VERTEX | wgpu::ShaderStages::FRAGMENT
        );
        assert!(matches!(
            entry.ty,
            wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable: true },
                view_dimension: wgpu::TextureViewDimension::D2Array,
                multisampled: false,
            }
        ));
    }
}
