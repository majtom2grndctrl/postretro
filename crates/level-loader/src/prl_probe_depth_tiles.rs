// ProbeDepthTiles (id 59) load: decode against id 34's lattice, and the id-34
// moment sign-bit check the tile reference encoding depends on.
// See: context/lib/build_pipeline.md §PRL section IDs · context/lib/rendering_pipeline.md §4

use postretro_level_format::SectionId;
use postretro_level_format::probe_depth_tiles::{
    ProbeDepthTilesSection, moment_sign_bit_set, probe_depth_tile_texture_bytes,
};
use postretro_level_format::sh_volume::OctahedralShProbe;

use crate::PrlLoadError;
use crate::prl_container::PrlContainer;
use crate::prl_loader::{section_validation, section_validation_from_error};

/// Refuse id-34 probe metadata in which a valid probe's averaged moment has
/// its sign bit set: the moment texel's G would read as a tile reference.
/// Invalid probes never reach the texel, so they are not checked.
pub(crate) fn refuse_signed_valid_moments(
    probes: &[OctahedralShProbe],
) -> Result<(), PrlLoadError> {
    match probes.iter().position(|probe| {
        probe.validity != 0 && moment_sign_bit_set(probe.mean_distance, probe.mean_sq_distance)
    }) {
        None => Ok(()),
        Some(index) => {
            let probe = &probes[index];
            Err(section_validation(
                "OctahedralShVolume",
                format!(
                    "valid probe {index} carries a negative depth moment (E[d] bits {:#06x}, \
                     E[d²] bits {:#06x}); re-bake the level",
                    probe.mean_distance, probe.mean_sq_distance
                ),
            ))
        }
    }
}

/// Read optional id 59 and validate it against id 34's grid and validity.
/// Absent is `None`; malformed or inconsistent refuses the level by name.
pub(crate) fn read_probe_depth_tiles(
    container: &PrlContainer,
    grid_dimensions: [u32; 3],
    probes: &[OctahedralShProbe],
) -> Result<Option<ProbeDepthTilesSection>, PrlLoadError> {
    let Some(data) = container.read_section(SectionId::ProbeDepthTiles as u32)? else {
        return Ok(None);
    };
    let section = decode_probe_depth_tiles(&data, grid_dimensions, probes)?;
    log::info!(
        "[PRL] ProbeDepthTiles: {} tile(s), {} B tile texture",
        section.tile_count(),
        probe_depth_tile_texture_bytes(section.tile_count() as u32),
    );
    Ok(Some(section))
}

/// Decode id 59's bytes and check every tile names a valid probe of id 34's
/// lattice. Any failure is a `ProbeDepthTiles` refusal.
pub(crate) fn decode_probe_depth_tiles(
    data: &[u8],
    grid_dimensions: [u32; 3],
    probes: &[OctahedralShProbe],
) -> Result<ProbeDepthTilesSection, PrlLoadError> {
    let section = ProbeDepthTilesSection::from_bytes(data)
        .map_err(|error| section_validation_from_error("ProbeDepthTiles", error))?;
    section
        .validate_against_lattice(grid_dimensions, |index| {
            probes
                .get(index as usize)
                .is_some_and(|probe| probe.validity != 0)
        })
        .map_err(|error| section_validation_from_error("ProbeDepthTiles", error))?;
    Ok(section)
}

#[cfg(test)]
#[path = "prl_probe_depth_tiles_tests.rs"]
mod tests;
