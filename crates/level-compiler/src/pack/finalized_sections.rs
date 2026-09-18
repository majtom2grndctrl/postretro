//! Final section-presence policy shared by pack entry wrappers.
//! See: context/lib/build_pipeline.md §PRL section IDs

use super::*;

/// Borrowed SH-family view after every existing pack-time presence decision.
/// The same value feeds directory construction and section serialization.
#[derive(Debug, Clone, Copy)]
pub(crate) struct FinalizedShEmissionView<'a> {
    pub(crate) octahedral: &'a OctahedralShVolumeSection,
    pub(crate) direct: Option<&'a DirectShVolumeSection>,
    pub(crate) delta: Option<&'a DeltaShVolumesSection>,
    pub(crate) shadow_selection: Option<&'a EntityShadowLightsSection>,
    pub(crate) direct_delta: Option<&'a DirectShDeltaVolumesSection>,
    pub(crate) animated_direct_delta: Option<&'a AnimatedDirectShDeltaVolumesSection>,
    pub(crate) billboard: Option<&'a BillboardDirectScatterVolumeSection>,
    pub(crate) animated_billboard_delta:
        Option<&'a AnimatedBillboardDirectScatterDeltaVolumesSection>,
}

impl<'a> FinalizedShEmissionView<'a> {
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(
        octahedral: &'a OctahedralShVolumeSection,
        direct: Option<&'a DirectShVolumeSection>,
        delta: Option<&'a DeltaShVolumesSection>,
        shadow_selection: Option<&'a EntityShadowLightsSection>,
        direct_delta: Option<&'a DirectShDeltaVolumesSection>,
        animated_direct_delta: Option<&'a AnimatedDirectShDeltaVolumesSection>,
        billboard: Option<&'a BillboardDirectScatterVolumeSection>,
        animated_billboard_delta: Option<&'a AnimatedBillboardDirectScatterDeltaVolumesSection>,
    ) -> anyhow::Result<Self> {
        let scatter_pair_required = billboard.is_some() && animated_direct_delta.is_some();
        anyhow::ensure!(
            animated_billboard_delta.is_some() == scatter_pair_required,
            "BillboardDirectScatterVolume requires AnimatedBillboardDirectScatterDeltaVolumes exactly when AnimatedDirectShDeltaVolumes is present"
        );
        if let (Some(direct_delta), Some(scatter_delta)) =
            (animated_direct_delta, animated_billboard_delta)
        {
            anyhow::ensure!(
                scatter_delta.animation_descriptor_indices
                    == direct_delta.animation_descriptor_indices
                    && scatter_delta.affinity_factor == direct_delta.affinity_factor
                    && scatter_delta.affinity_dims == direct_delta.affinity_dims
                    && scatter_delta.affinity_offsets == direct_delta.affinity_offsets
                    && scatter_delta.affinity_lights == direct_delta.affinity_lights,
                "AnimatedBillboardDirectScatterDeltaVolumes must duplicate AnimatedDirectShDeltaVolumes descriptor and CSR layout"
            );
        }
        let scatter_pair_fits = animated_billboard_delta.is_none_or(scatter_section_fits_pack_cap);
        if !scatter_pair_fits {
            log::warn!(
                "[Compiler] Billboard direct scatter sections 47/48 withheld during packing: section 48 exceeds the {} byte encoded pack cap",
                MAX_ANIMATED_BILLBOARD_DIRECT_SCATTER_SECTION_BYTES,
            );
        }
        let selected_count = shadow_selection
            .map(|section| section.light_indices.len())
            .unwrap_or(0);
        let usable_direct_delta = direct.zip(direct_delta).is_some_and(|(direct, delta)| {
            direct_sh_delta_is_usable_for_selection(delta, direct, selected_count)
        });
        Ok(Self {
            octahedral,
            direct,
            delta,
            shadow_selection: usable_direct_delta
                .then_some(shadow_selection)
                .flatten()
                .filter(|section| !section.light_indices.is_empty()),
            direct_delta: usable_direct_delta.then_some(direct_delta).flatten(),
            animated_direct_delta,
            billboard: scatter_pair_fits.then_some(billboard).flatten(),
            animated_billboard_delta: scatter_pair_fits
                .then_some(animated_billboard_delta)
                .flatten(),
        })
    }

    pub(crate) fn inventory(
        self,
    ) -> postretro_level_format::cluster_directory::ClusterDirectoryShInventory<'a> {
        postretro_level_format::cluster_directory::ClusterDirectoryShInventory {
            octahedral: Some(self.octahedral),
            direct: self.direct,
            delta: self.delta,
            shadow_selection: self.shadow_selection,
            direct_delta: self.direct_delta,
            animated_direct_delta: self.animated_direct_delta,
            billboard: self.billboard,
            animated_billboard_delta: self.animated_billboard_delta,
        }
    }
}

pub(super) fn scatter_section_fits_pack_cap(
    section: &AnimatedBillboardDirectScatterDeltaVolumesSection,
) -> bool {
    scatter_section_fits_pack_cap_with_limit(
        section,
        MAX_ANIMATED_BILLBOARD_DIRECT_SCATTER_SECTION_BYTES,
    )
}

pub(super) fn scatter_section_fits_pack_cap_with_limit(
    section: &AnimatedBillboardDirectScatterDeltaVolumesSection,
    max_encoded_bytes: u64,
) -> bool {
    section
        .encoded_len()
        .is_some_and(|bytes| bytes <= max_encoded_bytes)
}

pub(crate) fn direct_sh_delta_covers_selection(
    section: &DirectShDeltaVolumesSection,
    selected_light_count: usize,
) -> bool {
    if selected_light_count == 0 {
        return false;
    }

    let mut seen = vec![false; selected_light_count];
    for &selection_index in &section.affinity_lights {
        let Some(slot) = seen.get_mut(selection_index as usize) else {
            return false;
        };
        *slot = true;
    }

    seen.into_iter().all(|has_delta| has_delta)
}

pub(crate) fn direct_sh_delta_has_valid_csr_shape(section: &DirectShDeltaVolumesSection) -> bool {
    let Some(affinity_cell_count) = (section.affinity_dims[0] as usize)
        .checked_mul(section.affinity_dims[1] as usize)
        .and_then(|n| n.checked_mul(section.affinity_dims[2] as usize))
    else {
        return false;
    };
    let Some(expected_offsets_len) = affinity_cell_count.checked_add(1) else {
        return false;
    };
    if section.affinity_offsets.len() != expected_offsets_len {
        return false;
    }
    if section.affinity_offsets.first().copied() != Some(0) {
        return false;
    }
    if !section
        .affinity_offsets
        .windows(2)
        .all(|window| window[0] <= window[1])
    {
        return false;
    }
    if section
        .affinity_offsets
        .last()
        .and_then(|&offset| usize::try_from(offset).ok())
        != Some(section.affinity_lights.len())
    {
        return false;
    }

    section.expected_delta_subblock_f16_count() == Some(section.delta_subblocks.len())
}

pub(crate) fn direct_sh_delta_is_usable_for_selection(
    section: &DirectShDeltaVolumesSection,
    direct: &DirectShVolumeSection,
    selected_light_count: usize,
) -> bool {
    let expected_affinity_dims = [
        direct.grid_dimensions[0].div_ceil(AFFINITY_FACTOR as u32),
        direct.grid_dimensions[1].div_ceil(AFFINITY_FACTOR as u32),
        direct.grid_dimensions[2].div_ceil(AFFINITY_FACTOR as u32),
    ];

    section.affinity_factor == AFFINITY_FACTOR
        && section.affinity_dims == expected_affinity_dims
        && section.tile_dimension == direct.tile_dimension
        && section.tile_border == direct.tile_border
        && direct_sh_delta_has_valid_csr_shape(section)
        && direct_sh_delta_covers_selection(section, selected_light_count)
}
