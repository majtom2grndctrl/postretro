// Renderer-owned normal-free direct-scatter volume resources for billboards.
// See: context/lib/rendering_pipeline.md §7.4

use postretro_level_format::animated_billboard_direct_scatter_delta_volumes::AnimatedBillboardDirectScatterDeltaVolumesSection;
use postretro_level_format::billboard_direct_scatter_volume::BillboardDirectScatterVolumeSection;
use postretro_render_cpu::frame_uniforms::BillboardScatterMode;
use postretro_render_cpu::sh_compose::u16_slice_to_bytes;
use postretro_render_cpu::sh_volume::BIND_BILLBOARD_DIRECT_SCATTER;
use wgpu::util::DeviceExt;

use super::sh_allocation::{
    billboard_scatter_base_allocation, billboard_scatter_composed_allocation,
    billboard_scatter_dummy_allocation, storage_byte_len, texture_allocation_bytes, volume_3d_fits,
};
use super::sh_residency::{ShAllocationLedger, ShResidencyAllocationState, source_ids};
use super::sh_volume::AnimatedLightBuffers;

/// Renderer-owned textures for the billboard direct-scatter path. The sampled
/// view is selected only during level load: static maps sample the uploaded
/// base, animated maps sample the compose target, and unavailable maps bind a
/// valid 1×1×1 dummy while `has_scatter` remains zero.
pub(super) struct BillboardDirectScatterResources {
    pub(super) has_scatter: BillboardScatterMode,
    pub(super) has_animated_deltas: bool,
    pub(super) base_view: wgpu::TextureView,
    pub(super) sampled_view: wgpu::TextureView,
    pub(super) composed_storage_view: Option<wgpu::TextureView>,
    animated_descriptor_indices: Vec<u32>,
    /// Whole-resident ids 47/48 capacity. Streaming never folds this into
    /// pooled id34/id35 accounting.
    capacity_bytes: u64,
}

/// This decision is intentionally level-fixed. It is made while resources are
/// created, never while a frame is rendered, so group-3's layout and the
/// `has_scatter` uniform cannot drift apart during an animation.
fn scatter_binding_mode(
    base_sh_usable: bool,
    has_base: bool,
    has_animated_companion: bool,
) -> BillboardScatterMode {
    match (base_sh_usable && has_base, has_animated_companion) {
        (false, _) => BillboardScatterMode::Unavailable,
        (true, false) => BillboardScatterMode::StaticBase,
        (true, true) => BillboardScatterMode::ComposedAnimated,
    }
}

impl BillboardDirectScatterResources {
    #[expect(
        clippy::too_many_arguments,
        reason = "GPU resource construction needs the device, queue, section availability, validated section data, and allocation ledger together"
    )]
    pub(super) fn new(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        base_sh_usable: bool,
        base: Option<&BillboardDirectScatterVolumeSection>,
        animated: Option<&AnimatedBillboardDirectScatterDeltaVolumesSection>,
        base_section_present: bool,
        animated_section_present: bool,
        ledger: &mut ShAllocationLedger,
    ) -> Self {
        let animated_fits_device = animated
            .map(|section| scatter_storage_buffers_fit(section, &device.limits()))
            .unwrap_or(true);
        if animated.is_some() && !animated_fits_device {
            log::error!(
                "[Renderer] Billboard direct scatter compose buffers exceed device storage limits (max binding {} B, max buffer {} B); using legacy billboard lighting for this level",
                device.limits().max_storage_buffer_binding_size,
                device.limits().max_buffer_size,
            );
        }
        let animated = animated.filter(|_| animated_fits_device);
        // Section 48 is a required companion whenever the loader exposes an
        // animated scatter pair. If its GPU resources are unavailable, do not
        // silently retain the static base: select whole-scatter legacy mode.
        let scatter_pair_gpu_usable = animated_fits_device;
        let (base_texture, has_scatter) = upload_base_texture(
            device,
            queue,
            base.filter(|_| base_sh_usable && scatter_pair_gpu_usable),
            base_section_present,
            animated_section_present,
            ledger,
        );
        let base_view = base_texture.create_view(&wgpu::TextureViewDescriptor {
            label: Some("Billboard Direct Scatter Base View"),
            dimension: Some(wgpu::TextureViewDimension::D3),
            ..Default::default()
        });

        // A section-48 companion is exposed by the loader only when it is a
        // validated lockstep sibling of section 45. Still require a usable base
        // here: a device-limit fallback must select the legacy billboard path.
        let binding_mode = scatter_binding_mode(
            base_sh_usable && scatter_pair_gpu_usable,
            has_scatter,
            animated.is_some(),
        );
        let has_animated_deltas = binding_mode == BillboardScatterMode::ComposedAnimated;
        let (sampled_view, composed_storage_view) = if has_animated_deltas {
            let dimensions = base
                .expect("a usable animated scatter companion requires its base section")
                .grid_dimensions;
            let allocation = billboard_scatter_composed_allocation(dimensions);
            ledger.record_texture(
                allocation,
                &source_ids([
                    base_section_present.then_some(47),
                    animated_section_present.then_some(48),
                ]),
                false,
                ShResidencyAllocationState::Data,
            );
            let composed = device.create_texture(
                &allocation.descriptor(Some("Billboard Direct Scatter Composed Volume")),
            );
            let sampled = composed.create_view(&wgpu::TextureViewDescriptor {
                label: Some("Billboard Direct Scatter Composed Sampled View"),
                dimension: Some(wgpu::TextureViewDimension::D3),
                ..Default::default()
            });
            let storage = composed.create_view(&wgpu::TextureViewDescriptor {
                label: Some("Billboard Direct Scatter Composed Storage View"),
                dimension: Some(wgpu::TextureViewDimension::D3),
                ..Default::default()
            });
            (sampled, Some(storage))
        } else {
            (base_view.clone(), None)
        };

        Self {
            has_scatter: binding_mode,
            has_animated_deltas,
            base_view,
            sampled_view,
            composed_storage_view,
            animated_descriptor_indices: animated
                .map(|section| section.animation_descriptor_indices.clone())
                .unwrap_or_default(),
            capacity_bytes: scatter_capacity_bytes(base, animated, binding_mode),
        }
    }

    /// Predicate input deliberately reads the descriptor active flags rather
    /// than evaluated curve scale. An active light at a zero curve sample still
    /// needs a dispatch so later curve samples are visible without changing the
    /// level-fixed binding selection.
    pub(super) fn has_active_animated_descriptor(&self, animation: &AnimatedLightBuffers) -> bool {
        self.has_animated_deltas
            && animation.any_active_for_descriptor_indices(&self.animated_descriptor_indices)
    }

    pub(super) fn capacity_bytes(&self) -> u64 {
        self.capacity_bytes
    }
}

fn scatter_capacity_bytes(
    base: Option<&BillboardDirectScatterVolumeSection>,
    animated: Option<&AnimatedBillboardDirectScatterDeltaVolumesSection>,
    binding_mode: BillboardScatterMode,
) -> u64 {
    let dimensions = match (binding_mode, base) {
        (BillboardScatterMode::Unavailable, _) => return 0,
        (_, Some(section)) => section.grid_dimensions,
        _ => return 0,
    };
    let base_bytes = texture_allocation_bytes(billboard_scatter_base_allocation(dimensions));
    if binding_mode != BillboardScatterMode::ComposedAnimated {
        return base_bytes;
    }
    let Some(animated) = animated else {
        debug_assert!(false, "composed scatter mode requires an id-48 companion");
        return base_bytes;
    };
    let Some(storage_bytes) = scatter_storage_buffer_bytes(animated) else {
        debug_assert!(false, "validated id-48 storage sizes must fit u64");
        return base_bytes;
    };
    let composed_bytes =
        texture_allocation_bytes(billboard_scatter_composed_allocation(dimensions));
    let Some(total) = base_bytes
        .checked_add(composed_bytes)
        .and_then(|bytes| bytes.checked_add(32)) // 32-byte `ScatterGrid` uniform.
        .and_then(|bytes| storage_bytes.into_iter().try_fold(bytes, u64::checked_add))
    else {
        // Device-limit validation has already rejected impossible resource
        // sizes. Keep a conservative baseline rather than wrapping a
        // diagnostics-only accounting value if a synthetic caller violates it.
        log::error!("[Renderer] billboard scatter capacity accounting overflowed");
        return base_bytes;
    };
    total
}

/// Append the billboard-only scatter texture to the shared group-3 layout.
/// VERTEX-only is intentional: forward/fog never sample it, so this must not
/// consume the already-full forward fragment sampled-texture budget.
pub(super) fn append_shared_bind_group_layout_entries(
    entries: &mut Vec<wgpu::BindGroupLayoutEntry>,
) {
    entries.push(wgpu::BindGroupLayoutEntry {
        binding: BIND_BILLBOARD_DIRECT_SCATTER,
        visibility: wgpu::ShaderStages::VERTEX,
        ty: wgpu::BindingType::Texture {
            sample_type: wgpu::TextureSampleType::Float { filterable: false },
            view_dimension: wgpu::TextureViewDimension::D3,
            multisampled: false,
        },
        count: None,
    });
}

fn upload_base_texture(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    section: Option<&BillboardDirectScatterVolumeSection>,
    base_section_present: bool,
    animated_section_present: bool,
    ledger: &mut ShAllocationLedger,
) -> (wgpu::Texture, bool) {
    let usable = section.filter(|section| scatter_fits(section.grid_dimensions, &device.limits()));
    if let (Some(section), None) = (section, usable) {
        let dimensions = section.grid_dimensions;
        log::error!(
            "[Renderer] Billboard direct scatter grid {}x{}x{} exceeds device maxTextureDimension3D {}; using legacy billboard lighting for this level",
            dimensions[0],
            dimensions[1],
            dimensions[2],
            device.limits().max_texture_dimension_3d,
        );
    }
    let Some(section) = usable else {
        let allocation = billboard_scatter_dummy_allocation();
        ledger.record_texture(
            allocation,
            &source_ids([
                base_section_present.then_some(47),
                animated_section_present.then_some(48),
            ]),
            false,
            if base_section_present {
                ShResidencyAllocationState::Fallback
            } else {
                ShResidencyAllocationState::Dummy
            },
        );
        return (upload_dummy_texture(device, queue, allocation), false);
    };

    let allocation = billboard_scatter_base_allocation(section.grid_dimensions);
    ledger.record_texture(allocation, &[47], false, ShResidencyAllocationState::Data);
    let texture = device.create_texture_with_data(
        queue,
        &allocation.descriptor(Some("Billboard Direct Scatter Base Volume")),
        wgpu::util::TextureDataOrder::LayerMajor,
        &u16_slice_to_bytes(&section.scatter_rgba),
    );
    (texture, true)
}

fn upload_dummy_texture(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    allocation: super::sh_allocation::TextureAllocation,
) -> wgpu::Texture {
    device.create_texture_with_data(
        queue,
        &allocation.descriptor(Some("Billboard Direct Scatter Dummy Volume")),
        wgpu::util::TextureDataOrder::LayerMajor,
        &[0; 8],
    )
}

fn scatter_fits(dimensions: [u32; 3], limits: &wgpu::Limits) -> bool {
    volume_3d_fits(dimensions, limits)
}

fn scatter_storage_buffers_fit(
    section: &AnimatedBillboardDirectScatterDeltaVolumesSection,
    limits: &wgpu::Limits,
) -> bool {
    let Some(buffer_bytes) = scatter_storage_buffer_bytes(section) else {
        return false;
    };
    buffer_bytes.into_iter().all(|bytes| {
        bytes <= limits.max_storage_buffer_binding_size && bytes <= limits.max_buffer_size
    })
}

/// Byte lengths after the same empty-buffer padding used by the compose
/// uploader: dense deltas, CSR offsets, CSR lights, descriptor indices.
fn scatter_storage_buffer_bytes(
    section: &AnimatedBillboardDirectScatterDeltaVolumesSection,
) -> Option<[u64; 4]> {
    Some([
        padded_slice_bytes(section.delta_rgba.len(), std::mem::size_of::<u16>(), 4)?,
        padded_slice_bytes(
            section.affinity_offsets.len(),
            std::mem::size_of::<u32>(),
            8,
        )?,
        padded_slice_bytes(section.affinity_lights.len(), std::mem::size_of::<u32>(), 4)?,
        padded_slice_bytes(
            section.animation_descriptor_indices.len(),
            std::mem::size_of::<u32>(),
            4,
        )?,
    ])
}

fn padded_slice_bytes(
    element_count: usize,
    element_size: usize,
    empty_minimum: u64,
) -> Option<u64> {
    storage_byte_len(element_count, element_size, empty_minimum)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scatter_base_fixture() -> BillboardDirectScatterVolumeSection {
        BillboardDirectScatterVolumeSection {
            grid_origin: [0.0; 3],
            cell_size: [1.0; 3],
            grid_dimensions: [2, 2, 2],
            scatter_rgba: vec![0; 2 * 2 * 2 * 4],
        }
    }

    fn animated_scatter_fixture() -> AnimatedBillboardDirectScatterDeltaVolumesSection {
        AnimatedBillboardDirectScatterDeltaVolumesSection {
            animation_descriptor_indices: vec![0],
            affinity_factor: 4,
            affinity_dims: [1, 1, 1],
            affinity_offsets: vec![0, 1],
            affinity_lights: vec![0],
            delta_rgba: vec![0; 64 * 4],
        }
    }

    #[test]
    fn scatter_binding_is_vertex_only_3d_texture() {
        let entry = {
            let mut entries = Vec::new();
            append_shared_bind_group_layout_entries(&mut entries);
            entries.pop().expect("scatter entry")
        };
        assert_eq!(entry.binding, BIND_BILLBOARD_DIRECT_SCATTER);
        assert_eq!(entry.visibility, wgpu::ShaderStages::VERTEX);
        assert!(matches!(
            entry.ty,
            wgpu::BindingType::Texture {
                view_dimension: wgpu::TextureViewDimension::D3,
                ..
            }
        ));
    }

    #[test]
    fn scatter_fit_check_requires_nonzero_3d_extent_within_device_limit() {
        let limits = wgpu::Limits {
            max_texture_dimension_3d: 16,
            ..Default::default()
        };
        assert!(scatter_fits([16, 1, 8], &limits));
        assert!(!scatter_fits([0, 1, 1], &limits));
        assert!(!scatter_fits([1, 17, 1], &limits));
    }

    #[test]
    fn animated_scatter_falls_back_when_any_storage_buffer_exceeds_device_limits() {
        let mut section = AnimatedBillboardDirectScatterDeltaVolumesSection {
            animation_descriptor_indices: vec![0],
            affinity_factor: 4,
            affinity_dims: [1, 1, 1],
            affinity_offsets: vec![0, 1],
            affinity_lights: vec![0],
            delta_rgba: vec![0; 64 * 4],
        };
        let exact_bytes = scatter_storage_buffer_bytes(&section).expect("fixture sizes");
        let limits = wgpu::Limits {
            max_storage_buffer_binding_size: exact_bytes[0],
            max_buffer_size: exact_bytes[0],
            ..Default::default()
        };
        assert!(scatter_storage_buffers_fit(&section, &limits));

        let binding_limited = wgpu::Limits {
            max_storage_buffer_binding_size: exact_bytes[0] - 1,
            ..limits.clone()
        };
        assert!(!scatter_storage_buffers_fit(&section, &binding_limited));
        assert_eq!(
            scatter_binding_mode(
                scatter_storage_buffers_fit(&section, &binding_limited),
                true,
                true,
            ),
            BillboardScatterMode::Unavailable,
            "a failed animated-buffer guard must select dummy/legacy mode, not static scatter",
        );

        section.delta_rgba.clear();
        section.affinity_offsets.resize(4, 0);
        let csr_bytes = scatter_storage_buffer_bytes(&section).expect("resized CSR sizes")[1];
        let buffer_limited = wgpu::Limits {
            max_storage_buffer_binding_size: csr_bytes,
            max_buffer_size: csr_bytes - 1,
            ..Default::default()
        };
        assert!(!scatter_storage_buffers_fit(&section, &buffer_limited));
    }

    #[test]
    fn load_fixed_binding_selection_preserves_static_and_invalid_companion_contracts() {
        assert_eq!(
            scatter_binding_mode(true, true, false),
            BillboardScatterMode::StaticBase,
            "a valid section-47 map without section 48 must take scatter"
        );
        assert_eq!(
            scatter_binding_mode(true, true, true),
            BillboardScatterMode::ComposedAnimated,
            "a validated companion must sample the composed map"
        );
        assert_eq!(
            scatter_binding_mode(true, false, true),
            BillboardScatterMode::Unavailable,
            "an unavailable base (including a rejected section 48 pair) must bind the dummy and take legacy lighting"
        );
    }

    #[test]
    fn unusable_base_sh_forces_legacy_scatter_binding_even_when_sections_are_present() {
        assert_eq!(
            scatter_binding_mode(false, true, true),
            BillboardScatterMode::Unavailable,
            "a device-limit SH fallback must bind the dummy scatter texture and clear has_scatter"
        );
    }

    #[test]
    fn load_fixed_scatter_modes_preserve_nonzero_availability_semantics() {
        assert_eq!(BillboardScatterMode::Unavailable as u32, 0);
        assert!(BillboardScatterMode::StaticBase.is_available());
        assert!(BillboardScatterMode::ComposedAnimated.is_available());
        assert_ne!(BillboardScatterMode::StaticBase as u32, 0);
        assert_ne!(BillboardScatterMode::ComposedAnimated as u32, 0);
    }

    #[test]
    fn animated_only_pair_selects_composed_scatter_without_legacy_fallback() {
        // Regression: animated-only maps now have a zero-base id-47 anchor;
        // they must still select the composed resource rather than legacy SH.
        let mode = scatter_binding_mode(true, true, true);
        assert_eq!(mode, BillboardScatterMode::ComposedAnimated);
        assert!(mode.is_available());
    }

    #[test]
    fn whole_resident_scatter_capacity_keeps_id48_buffers_out_of_streamed_metadata() {
        let base = scatter_base_fixture();
        let animated = animated_scatter_fixture();
        let base_bytes =
            texture_allocation_bytes(billboard_scatter_base_allocation(base.grid_dimensions));
        let composed_bytes =
            texture_allocation_bytes(billboard_scatter_composed_allocation(base.grid_dimensions));
        let id48_storage = scatter_storage_buffer_bytes(&animated)
            .expect("small fixture must have representable storage sizes")
            .into_iter()
            .sum::<u64>();
        assert_eq!(
            scatter_capacity_bytes(
                Some(&base),
                Some(&animated),
                BillboardScatterMode::ComposedAnimated,
            ),
            base_bytes + composed_bytes + 32 + id48_storage,
            "whole-resident id47/id48 charge includes both textures, ScatterGrid, and every id48 storage buffer",
        );
        assert_eq!(
            scatter_capacity_bytes(Some(&base), None, BillboardScatterMode::StaticBase),
            base_bytes,
        );
        assert_eq!(
            scatter_capacity_bytes(None, None, BillboardScatterMode::Unavailable),
            0,
        );
    }
}
