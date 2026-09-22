//! Final section-presence policy shared by pack entry wrappers.
//! See: context/lib/build_pipeline.md §PRL section IDs

use super::*;

use postretro_level_format::lightmap::IRRADIANCE_FORMAT_RGBA16F;

/// Borrowed lossless stored-atlas sources retained through final PRL emission.
///
/// The normal id-34/id-35 sections may be BC6H-encoded for their legacy PRL
/// bodies, but a future cluster payload must gather independently encoded cells
/// from the packed RGBA16F stored slots. Keeping this view borrowed makes that
/// source available without cloning either atlas while preserving the existing
/// legacy encoders.
#[derive(Debug, Clone, Copy)]
pub(crate) struct FinalizedShPackSources<'a> {
    pub(crate) octahedral: &'a OctahedralShVolumeSection,
    pub(crate) direct: Option<&'a DirectShVolumeSection>,
}

impl<'a> FinalizedShPackSources<'a> {
    pub(crate) fn new(
        octahedral: &'a OctahedralShVolumeSection,
        direct: Option<&'a DirectShVolumeSection>,
    ) -> anyhow::Result<Self> {
        anyhow::ensure!(
            octahedral.irradiance_format == IRRADIANCE_FORMAT_RGBA16F,
            "final packed OctahedralShVolume source must retain RGBA16F stored slots"
        );
        if let Some(direct) = direct {
            anyhow::ensure!(
                direct.irradiance_format == IRRADIANCE_FORMAT_RGBA16F,
                "final packed DirectShVolume source must retain RGBA16F stored slots"
            );
        }
        Ok(Self { octahedral, direct })
    }
}

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

/// Final SH publication handoff shared by post-bake metadata and pack planning.
///
/// `emission` names exactly the legacy sections selected by existing policy;
/// `sources` retain the corresponding lossless packed atlas inputs for a later
/// cluster-major payload without changing those legacy bodies.
#[derive(Debug, Clone, Copy)]
pub(crate) struct FinalizedShPack<'a> {
    pub(crate) emission: FinalizedShEmissionView<'a>,
    pub(crate) sources: FinalizedShPackSources<'a>,
}

impl<'a> FinalizedShPack<'a> {
    pub(crate) fn new(
        emission: FinalizedShEmissionView<'a>,
        sources: FinalizedShPackSources<'a>,
    ) -> anyhow::Result<Self> {
        anyhow::ensure!(
            sources.octahedral.grid_origin == emission.octahedral.grid_origin
                && sources.octahedral.cell_size == emission.octahedral.cell_size
                && sources.octahedral.grid_dimensions == emission.octahedral.grid_dimensions
                && sources.octahedral.tile_dimension == emission.octahedral.tile_dimension
                && sources.octahedral.tile_border == emission.octahedral.tile_border
                && sources.octahedral.atlas_dimensions == emission.octahedral.atlas_dimensions
                && sources.octahedral.layer_count == emission.octahedral.layer_count
                && sources.octahedral.tiles_per_layer == emission.octahedral.tiles_per_layer
                && sources.octahedral.atlas_tiles_per_row
                    == emission.octahedral.atlas_tiles_per_row,
            "final packed OctahedralShVolume source geometry must match the emitted section"
        );
        match (sources.direct, emission.direct) {
            (None, None) => {}
            (Some(source), Some(emitted)) => anyhow::ensure!(
                source.grid_origin == emitted.grid_origin
                    && source.cell_size == emitted.cell_size
                    && source.grid_dimensions == emitted.grid_dimensions
                    && source.tile_dimension == emitted.tile_dimension
                    && source.tile_border == emitted.tile_border
                    && source.atlas_dimensions == emitted.atlas_dimensions
                    && source.layer_count == emitted.layer_count
                    && source.tiles_per_layer == emitted.tiles_per_layer
                    && source.atlas_tiles_per_row == emitted.atlas_tiles_per_row,
                "final packed DirectShVolume source geometry must match the emitted section"
            ),
            _ => anyhow::bail!(
                "final packed DirectShVolume source presence must match the emitted section"
            ),
        }
        Ok(Self { emission, sources })
    }
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

#[cfg(test)]
mod tests {
    use super::*;
    use postretro_level_format::lightmap::IRRADIANCE_FORMAT_BC6H;

    fn rgba16f_octahedral() -> OctahedralShVolumeSection {
        let mut section = OctahedralShVolumeSection::placeholder();
        section.irradiance_format = IRRADIANCE_FORMAT_RGBA16F;
        section.grid_dimensions = [1, 1, 1];
        section
    }

    fn rgba16f_direct() -> DirectShVolumeSection {
        let mut section = DirectShVolumeSection::placeholder();
        section.irradiance_format = IRRADIANCE_FORMAT_RGBA16F;
        section.grid_dimensions = [1, 1, 1];
        section
    }

    fn emission<'a>(
        octahedral: &'a OctahedralShVolumeSection,
        direct: Option<&'a DirectShVolumeSection>,
    ) -> FinalizedShEmissionView<'a> {
        FinalizedShEmissionView::new(octahedral, direct, None, None, None, None, None, None)
            .expect("test SH sections should have a valid legacy emission view")
    }

    #[test]
    fn finalized_sh_sources_require_lossless_rgba16f_atlases() {
        let compressed_octahedral = OctahedralShVolumeSection::placeholder();
        assert!(
            FinalizedShPackSources::new(&compressed_octahedral, None)
                .expect_err("BC6H octahedral input cannot serve as a stored-slot source")
                .to_string()
                .contains("must retain RGBA16F")
        );

        let octahedral = rgba16f_octahedral();
        let mut compressed_direct = DirectShVolumeSection::placeholder();
        compressed_direct.irradiance_format = IRRADIANCE_FORMAT_BC6H;
        assert!(
            FinalizedShPackSources::new(&octahedral, Some(&compressed_direct))
                .expect_err("BC6H direct input cannot serve as a stored-slot source")
                .to_string()
                .contains("must retain RGBA16F")
        );
    }

    #[test]
    fn finalized_sh_pack_rejects_mismatched_octahedral_source_geometry() {
        let source_octahedral = rgba16f_octahedral();
        let sources = FinalizedShPackSources::new(&source_octahedral, None)
            .expect("RGBA16F source should be accepted");
        let mut emitted_octahedral = source_octahedral.clone();
        emitted_octahedral.tile_dimension += 1;

        assert!(
            FinalizedShPack::new(emission(&emitted_octahedral, None), sources)
                .expect_err("emitted octahedral geometry must match the retained source")
                .to_string()
                .contains("OctahedralShVolume source geometry")
        );
    }

    #[test]
    fn finalized_sh_pack_rejects_mismatched_direct_source_geometry() {
        let source_octahedral = rgba16f_octahedral();
        let source_direct = rgba16f_direct();
        let sources = FinalizedShPackSources::new(&source_octahedral, Some(&source_direct))
            .expect("RGBA16F sources should be accepted");
        let emitted_octahedral = source_octahedral.clone();
        let mut emitted_direct = source_direct.clone();
        emitted_direct.atlas_tiles_per_row += 1;

        assert!(
            FinalizedShPack::new(
                emission(&emitted_octahedral, Some(&emitted_direct)),
                sources
            )
            .expect_err("emitted direct geometry must match the retained source")
            .to_string()
            .contains("DirectShVolume source geometry")
        );
    }

    #[test]
    fn finalized_sh_pack_rejects_direct_source_presence_mismatch() {
        let source_octahedral = rgba16f_octahedral();
        let source_direct = rgba16f_direct();
        let sources = FinalizedShPackSources::new(&source_octahedral, Some(&source_direct))
            .expect("RGBA16F sources should be accepted");
        let emitted_octahedral = source_octahedral.clone();

        assert!(
            FinalizedShPack::new(emission(&emitted_octahedral, None), sources)
                .expect_err("direct source and emitted-section presence must agree")
                .to_string()
                .contains("DirectShVolume source presence")
        );
    }
}
