//! Final section-presence policy shared by pack entry wrappers.
//! See: context/lib/build_pipeline.md §PRL section IDs

use super::*;

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
