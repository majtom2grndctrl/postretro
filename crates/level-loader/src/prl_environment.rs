// EnvironmentVolumes (id 52) decode: the level default gravity and the static regional volume records.
// See: context/lib/build_pipeline.md §PRL section IDs

use postretro_level_format::SectionId;
use postretro_level_format::environment_volumes::EnvironmentVolumesSection;

use crate::prl::PrlLoadError;
use crate::prl_loader::section_validation_from_error;

/// Decode the always-present environment section. Absence means the PRL was
/// built before level gravity left FogVolumes, whose old layout the current
/// FogVolumes decoder cannot read; the caller checks this first so such a file
/// fails with the recompile message rather than a FogVolumes layout error.
pub(crate) fn decode_environment_volumes(
    data: Option<Vec<u8>>,
) -> Result<EnvironmentVolumesSection, PrlLoadError> {
    let Some(data) = data else {
        return Err(PrlLoadError::StaleFormatMissingSection {
            section: "EnvironmentVolumes",
            id: SectionId::EnvironmentVolumes as u32,
        });
    };
    let section = EnvironmentVolumesSection::from_bytes(&data)
        .map_err(|err| section_validation_from_error("EnvironmentVolumes", err))?;
    log::info!(
        "[PRL] EnvironmentVolumes: {} volume(s), {} brush(es), level_gravity={}",
        section.volumes.len(),
        section
            .volumes
            .iter()
            .map(|volume| volume.brushes.len())
            .sum::<usize>(),
        section.level_gravity,
    );
    Ok(section)
}
