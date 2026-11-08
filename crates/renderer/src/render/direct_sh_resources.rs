// Direct-SH atlas resources shared by the dynamic-receiver compose passes.
// See: context/lib/rendering_pipeline.md §4, §7.1

use crate::render::gpu_create::GpuCreate;
use postretro_level_format::animated_direct_sh_delta_volumes::AnimatedDirectShDeltaVolumesSection;
use postretro_level_format::direct_sh_delta_volumes::DirectShDeltaVolumesSection;
use postretro_level_format::direct_sh_volume::DirectShVolumeSection;
#[cfg(test)]
use postretro_level_format::lightmap::IRRADIANCE_FORMAT_BC6H;
use postretro_render_cpu::sh_volume::{
    BIND_DYNAMIC_DIRECT_PARAMS, BIND_SH_DIRECT_ATLAS, build_dynamic_direct_params_bytes,
};

use super::residency::{ResidencyAllocationState, source_ids};
pub(super) use super::sh_allocation::{DirectAtlasLayout, atlas_fits};
use super::sh_allocation::{
    DirectAtlasUsage, ShAllocationKind, buffer_allocation, direct_atlas_usage,
    direct_base_atlas_allocation, direct_base_atlas_dummy_allocation,
    direct_composed_atlas_allocation, texture_allocation_bytes,
};
use super::sh_residency::ShAllocationLedger;
use super::sh_volume::AnimatedLightBuffers;

/// Renderer-owned direct-SH textures and mesh-only dynamic-direct parameters.
///
/// This owns the direct receiver path independently from indirect SH resources
/// so future billboard-specific direct fields can share its load/compose seam
/// without widening the indirect resource owner.
///
/// Compose writes two atlases. The promotion-free direct atlas (`atlas_view`,
/// shared group-3 binding 15) carries every baked direct light whatever the
/// promotion weights; fluid and legacy billboards sample it. The promotion
/// handoff atlas (`handoff_atlas_view`, mesh-superset binding 15) subtracts
/// promoted lights for movers and skinned meshes, which add them back through
/// their shadowed runtime term. Without a compose both name the base.
pub(super) struct DirectShResources {
    pub(super) has_direct: bool,
    pub(super) has_direct_base: bool,
    pub(super) compose_layout: Option<DirectAtlasLayout>,
    pub(super) atlas_view: wgpu::TextureView,
    pub(super) handoff_atlas_view: wgpu::TextureView,
    pub(super) base_atlas_view: wgpu::TextureView,
    pub(super) atlas_storage_view: Option<wgpu::TextureView>,
    pub(super) handoff_storage_view: Option<wgpu::TextureView>,
    pub(super) intermediate_storage_view: Option<wgpu::TextureView>,
    pub(super) intermediate_sampled_view: Option<wgpu::TextureView>,
    dynamic_direct_params_buffer: wgpu::Buffer,
    animated_descriptor_indices: Vec<u32>,
}

impl DirectShResources {
    #[allow(clippy::too_many_arguments)]
    pub(super) fn new(
        device: &wgpu::Device,
        queue: &dyn crate::render::queue_writes::QueueWrites,
        direct_section: Option<&DirectShVolumeSection>,
        direct_delta_section: Option<&DirectShDeltaVolumesSection>,
        animated_direct_delta_section: Option<&AnimatedDirectShDeltaVolumesSection>,
        fallback_layout: Option<DirectAtlasLayout>,
        direct_section_present: bool,
        ledger: &mut ShAllocationLedger,
    ) -> Self {
        let direct_usage = resolve_direct_atlas_usage(
            direct_section,
            direct_delta_section,
            animated_direct_delta_section,
            fallback_layout,
        );
        let (base_atlas_texture, has_direct_base) = upload_direct_atlas_texture(
            device,
            queue,
            direct_section,
            direct_section_present,
            ledger,
        );
        let has_animated_direct =
            animated_direct_delta_section.is_some() && fallback_layout.is_some();
        let has_direct = direct_atlas_present(has_direct_base, has_animated_direct);
        let direct_sources = source_ids([
            direct_section_present.then_some(35),
            direct_delta_section.map(|_| 41),
            animated_direct_delta_section.map(|_| 45),
        ]);
        let base_atlas_view = base_atlas_texture.create_view(&wgpu::TextureViewDescriptor {
            label: Some("SH Direct Octahedral Base Atlas View"),
            dimension: Some(wgpu::TextureViewDimension::D2Array),
            ..Default::default()
        });
        let (
            atlas_view,
            handoff_atlas_view,
            atlas_storage_view,
            handoff_storage_view,
            intermediate_storage_view,
            intermediate_sampled_view,
        ) = if has_direct && direct_usage.needs_composed_atlas {
            let (atlas_sampled, atlas_storage) = create_direct_composed_atlas_views(
                device,
                ShAllocationKind::DirectComposedAtlas,
                direct_usage,
                "SH Direct Composed Atlas",
                &direct_sources,
                ledger,
            );
            let (handoff_sampled, handoff_storage) = create_direct_composed_atlas_views(
                device,
                ShAllocationKind::DirectHandoffAtlas,
                direct_usage,
                "SH Direct Promotion Handoff Atlas",
                &direct_sources,
                ledger,
            );
            log_direct_atlas_bytes(direct_usage);
            if direct_usage.needs_intermediate_atlas {
                let intermediate = create_direct_composed_atlas_texture(
                    device,
                    ShAllocationKind::DirectIntermediateAtlas,
                    direct_usage.atlas_dimensions,
                    direct_usage.layer_count,
                    "SH Direct Compose Intermediate Atlas",
                    &direct_sources,
                    ledger,
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
                    atlas_sampled,
                    handoff_sampled,
                    Some(atlas_storage),
                    Some(handoff_storage),
                    Some(intermediate_storage),
                    Some(intermediate_sampled),
                )
            } else {
                (
                    atlas_sampled,
                    handoff_sampled,
                    Some(atlas_storage),
                    Some(handoff_storage),
                    None,
                    None,
                )
            }
        } else {
            (
                base_atlas_view.clone(),
                base_atlas_view.clone(),
                None,
                None,
                None,
                None,
            )
        };

        let dynamic_direct_params_bytes = build_dynamic_direct_params_bytes(1.0, has_direct);
        let dynamic_direct_params_allocation = buffer_allocation(
            ShAllocationKind::DirectDynamicParams,
            &dynamic_direct_params_bytes,
            wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        );
        ledger.record_buffer(
            dynamic_direct_params_allocation,
            &direct_sources,
            direct_sources.is_empty(),
            ResidencyAllocationState::Data,
        );
        let dynamic_direct_params_buffer =
            device.new_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("Dynamic Direct Params Uniform"),
                contents: &dynamic_direct_params_bytes,
                usage: dynamic_direct_params_allocation.usage,
            });

        Self {
            has_direct,
            has_direct_base,
            compose_layout: direct_usage.layout(),
            atlas_view,
            handoff_atlas_view,
            base_atlas_view,
            atlas_storage_view,
            handoff_storage_view,
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

    /// The whole-load shared group-3 binding-15 entry: the promotion-free
    /// direct atlas.
    pub(super) fn shared_atlas_entry(&self) -> wgpu::BindGroupEntry<'_> {
        shared_direct_atlas_entry(&self.atlas_view)
    }

    /// The whole-load mesh superset's entries over `shared`: binding 15 names
    /// the promotion handoff atlas, binding 16 the dynamic-direct params.
    pub(super) fn mesh_superset_entries<'a>(
        &'a self,
        shared: &[wgpu::BindGroupEntry<'a>],
    ) -> Vec<wgpu::BindGroupEntry<'a>> {
        mesh_superset_entries(
            shared,
            &self.handoff_atlas_view,
            self.dynamic_direct_params_binding(),
        )
    }

    pub(super) fn write_dynamic_direct_params(
        &self,
        queue: &crate::render::uploads::UploadQueue,
        scale: f32,
    ) {
        let bytes = build_dynamic_direct_params_bytes(scale, self.has_direct);
        queue.write_buffer(&self.dynamic_direct_params_buffer, 0, &bytes);
    }

    /// Streamed id-35 has the same receiver contract as a whole-resident
    /// direct atlas, but its backing texture belongs to the shared renderer
    /// slot pool. Keep this small state transition here so all later dynamic
    /// direct updates continue to write the existing binding-16 uniform.
    pub(super) fn enable_streamed_atlas(
        &mut self,
        queue: &dyn crate::render::queue_writes::QueueWrites,
    ) {
        // Initial setup precedes frame writes. Pool growth revisits this method,
        // but must not directly overwrite pending dynamic-direct params.
        if self.has_direct {
            return;
        }
        self.has_direct = true;
        let bytes = build_dynamic_direct_params_bytes(1.0, true);
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
///
/// VERTEX only here: billboard and fluid read the direct atlas from `vs_main`,
/// and forward and fog never declare it, so no fragment stage sharing this
/// layout samples it. That frees forward's fragment slot for the depth tiles
/// (binding 18). Movers and skinned meshes sample it in `fs_main`, and their
/// mesh superset layout restores FRAGMENT.
pub(super) fn append_shared_bind_group_layout_entries(
    entries: &mut Vec<wgpu::BindGroupLayoutEntry>,
) {
    entries.push(wgpu::BindGroupLayoutEntry {
        binding: BIND_SH_DIRECT_ATLAS,
        visibility: wgpu::ShaderStages::VERTEX,
        ty: wgpu::BindingType::Texture {
            sample_type: wgpu::TextureSampleType::Float { filterable: true },
            view_dimension: wgpu::TextureViewDimension::D2Array,
            multisampled: false,
        },
        count: None,
    });
}

/// The shared group-3 entry for binding 15: the promotion-free direct atlas.
/// Fluid and legacy billboards read it, so promotion never changes them.
pub(super) fn shared_direct_atlas_entry(view: &wgpu::TextureView) -> wgpu::BindGroupEntry<'_> {
    wgpu::BindGroupEntry {
        binding: BIND_SH_DIRECT_ATLAS,
        resource: wgpu::BindingResource::TextureView(view),
    }
}

/// The mesh superset's entries: the shared group-3 entries with binding 15
/// swapped for the promotion handoff atlas, plus the dynamic-direct params.
/// Only movers and skinned meshes bind this group, and only they add promoted
/// lights back through their shadowed runtime term.
pub(super) fn mesh_superset_entries<'a>(
    shared: &[wgpu::BindGroupEntry<'a>],
    handoff_view: &'a wgpu::TextureView,
    dynamic_direct_params: wgpu::BindingResource<'a>,
) -> Vec<wgpu::BindGroupEntry<'a>> {
    let mut entries: Vec<wgpu::BindGroupEntry<'a>> = shared
        .iter()
        .map(|entry| {
            if entry.binding == BIND_SH_DIRECT_ATLAS {
                wgpu::BindGroupEntry {
                    binding: BIND_SH_DIRECT_ATLAS,
                    resource: wgpu::BindingResource::TextureView(handoff_view),
                }
            } else {
                entry.clone()
            }
        })
        .collect();
    debug_assert!(
        entries
            .iter()
            .any(|entry| entry.binding == BIND_SH_DIRECT_ATLAS),
        "the shared group-3 entries must carry binding 15",
    );
    entries.push(wgpu::BindGroupEntry {
        binding: BIND_DYNAMIC_DIRECT_PARAMS,
        resource: dynamic_direct_params,
    });
    entries
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

/// Build the DIRECT static-light atlas texture from the optional section.
/// Absent or unusable data binds a valid 4×4 BC6H dummy and clears `has_direct`.
fn upload_direct_atlas_texture(
    device: &wgpu::Device,
    queue: &dyn crate::render::queue_writes::QueueWrites,
    section: Option<&DirectShVolumeSection>,
    direct_section_present: bool,
    ledger: &mut ShAllocationLedger,
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
        let allocation = direct_base_atlas_dummy_allocation();
        ledger.record_texture(
            allocation,
            &source_ids([direct_section_present.then_some(35)]),
            false,
            if direct_section_present {
                ResidencyAllocationState::Fallback
            } else {
                ResidencyAllocationState::Dummy
            },
        );
        return (upload_direct_atlas_dummy(device, queue, allocation), false);
    };

    let allocation = direct_base_atlas_allocation(section);
    ledger.record_texture(allocation, &[35], false, ResidencyAllocationState::Data);

    let texture = device.new_texture_with_data(
        queue,
        &allocation.descriptor(Some("SH Direct Octahedral Atlas")),
        wgpu::util::TextureDataOrder::LayerMajor,
        &section.atlas,
    );
    (texture, true)
}

fn upload_direct_atlas_dummy(
    device: &wgpu::Device,
    queue: &dyn crate::render::queue_writes::QueueWrites,
    allocation: super::sh_allocation::TextureAllocation,
) -> wgpu::Texture {
    let zero_block = [0u8; 16];
    device.new_texture_with_data(
        queue,
        &allocation.descriptor(Some("SH Direct Octahedral Atlas Dummy")),
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

fn resolve_direct_atlas_usage(
    direct_section: Option<&DirectShVolumeSection>,
    _direct_delta_section: Option<&DirectShDeltaVolumesSection>,
    animated_direct_delta_section: Option<&AnimatedDirectShDeltaVolumesSection>,
    fallback_layout: Option<DirectAtlasLayout>,
) -> DirectAtlasUsage {
    direct_atlas_usage(
        direct_section,
        animated_direct_delta_section.is_some(),
        fallback_layout,
    )
}

fn create_direct_composed_atlas_texture(
    device: &wgpu::Device,
    kind: ShAllocationKind,
    atlas_dimensions: [u32; 2],
    layer_count: u32,
    label: &str,
    sources: &[u16],
    ledger: &mut ShAllocationLedger,
) -> wgpu::Texture {
    let allocation = direct_composed_atlas_allocation(kind, atlas_dimensions, layer_count);
    ledger.record_texture(allocation, sources, false, ResidencyAllocationState::Data);
    device.new_texture(&allocation.descriptor(Some(label)))
}

/// One compose output: its sampled view for consumers and its storage view for
/// the compose passes.
fn create_direct_composed_atlas_views(
    device: &wgpu::Device,
    kind: ShAllocationKind,
    usage: DirectAtlasUsage,
    label: &str,
    sources: &[u16],
    ledger: &mut ShAllocationLedger,
) -> (wgpu::TextureView, wgpu::TextureView) {
    let texture = create_direct_composed_atlas_texture(
        device,
        kind,
        usage.atlas_dimensions,
        usage.layer_count,
        label,
        sources,
        ledger,
    );
    let view = |suffix: &str| {
        texture.create_view(&wgpu::TextureViewDescriptor {
            label: Some(&format!("{label} {suffix} View")),
            dimension: Some(wgpu::TextureViewDimension::D2Array),
            ..Default::default()
        })
    };
    (view("Sampled"), view("Storage"))
}

/// Logs each whole-load direct atlas once per load, so a measurement can read
/// what the promotion-free and handoff pair costs.
fn log_direct_atlas_bytes(usage: DirectAtlasUsage) {
    let atlas_bytes = texture_allocation_bytes(direct_composed_atlas_allocation(
        ShAllocationKind::DirectComposedAtlas,
        usage.atlas_dimensions,
        usage.layer_count,
    ));
    let intermediate_bytes = if usage.needs_intermediate_atlas {
        atlas_bytes
    } else {
        0
    };
    log::info!(
        "[Renderer] Direct SH atlases: direct (promotion-free) {atlas_bytes} B, promotion handoff {atlas_bytes} B, compose intermediate {intermediate_bytes} B ({}x{}x{} Rgba16Float)",
        usage.atlas_dimensions[0].max(1),
        usage.atlas_dimensions[1].max(1),
        usage.layer_count.max(1),
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use postretro_level_format::delta_sh_volumes::{
        AFFINITY_FACTOR, DEFAULT_DELTA_PROBE_F16_STRIDE, PROBES_PER_CELL,
    };
    use postretro_level_format::octahedral::irradiance_atlas_array_layout;

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
    fn animated_direct_only_allocates_from_stored_base_geometry() {
        // A 128-probe dense grid whose stored set contains only eight tiles.
        // The fallback direct atlas must use this stored geometry, never derive
        // a texture extent from the dense grid dimensions.
        let stored = irradiance_atlas_array_layout([8, 1, 1], 6, 8192).unwrap();
        let mut sh = postretro_level_format::sh_volume::OctahedralShVolumeSection::placeholder();
        sh.grid_dimensions = [8, 4, 4];
        sh.tile_dimension = 6;
        sh.tile_border = 1;
        sh.atlas_dimensions = [stored.atlas_width, stored.atlas_height];
        sh.layer_count = stored.layer_count;
        sh.tiles_per_layer = stored.tiles_per_layer;
        sh.atlas_tiles_per_row = stored.atlas_tiles_per_row;
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
        assert_eq!(usage.atlas_dimensions, [18, 18]);
        assert_eq!(usage.layer_count, 1);
    }

    #[test]
    fn direct_section_is_disabled_when_base_sh_is_unusable() {
        let direct = direct_section();

        assert!(direct_section_when_base_present(true, Some(&direct)).is_some());
        assert!(direct_section_when_base_present(false, Some(&direct)).is_none());
        assert!(direct_section_when_base_present(true, None).is_none());
    }

    #[test]
    fn direct_shared_binding_is_vertex_only_for_billboard_and_fluid() {
        let mut entries = Vec::new();
        append_shared_bind_group_layout_entries(&mut entries);
        let entry = entries
            .iter()
            .find(|entry| entry.binding == BIND_SH_DIRECT_ATLAS)
            .expect("direct resources must append the shared direct atlas binding");

        assert_eq!(BIND_SH_DIRECT_ATLAS, 15);
        // The mesh superset restores FRAGMENT for movers and skinned meshes
        // (`mesh_superset_keeps_direct_atlas_fragment_visible_and_shared_drops_it`).
        assert_eq!(entry.visibility, wgpu::ShaderStages::VERTEX);
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
