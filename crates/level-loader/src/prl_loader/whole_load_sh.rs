// Whole-load SH body gate: a load that reads SH whole refuses a PRL whose SH
// sections omit the bodies only that load reads.
// See: context/lib/build_pipeline.md §PRL section IDs (Whole-load SH bodies)

use postretro_level_format::animated_direct_sh_delta_volumes::ANIMATED_DIRECT_SH_DELTA_VOLUMES_VERSION;
use postretro_level_format::delta_sh_volumes::DELTA_SH_VOLUMES_VERSION;
use postretro_level_format::direct_sh_delta_volumes::DIRECT_SH_DELTA_VOLUMES_VERSION;
use postretro_level_format::direct_sh_volume::DIRECT_SH_VOLUME_VERSION;
use postretro_level_format::sh_body::{
    SH_BODY_SECTIONS, ShBodyPresence, read_sh_body_presence, sh_body_header_len,
};
use postretro_level_format::sh_volume::SH_VOLUME_VERSION;
use postretro_level_format::{ContainerMeta, SectionId};

use crate::PrlLoadError;
use crate::prl_container::PrlContainer;
use crate::prl_file::PrlFile;
use crate::sh_stream::read_section_prefix_positionally;

/// Refuse a whole SH load when any present id 27/34/35/41/45 omits its body.
///
/// The open runs this on the retained file and its table whenever SH will not
/// stream, before the whole image is read or any container is built, so a
/// default bake costs a few header prefixes to refuse. Being before any SH
/// decode, the soft "malformed -> disable" paths of ids 35/41/45 never see an
/// omitted body.
pub(crate) fn require_whole_load_sh_bodies_positionally(
    file: &PrlFile,
    metadata: &ContainerMeta,
) -> Result<(), PrlLoadError> {
    let section_ids = omitted_sections(|section_id, len| {
        read_section_prefix_positionally(file, metadata, section_id, len)
    })?;
    refuse_omitted(section_ids, metadata)
}

/// [`require_whole_load_sh_bodies_positionally`] over an opened container,
/// for loads that build their container without the production open.
#[cfg(test)]
pub(crate) fn require_whole_load_sh_bodies(container: &PrlContainer) -> Result<(), PrlLoadError> {
    refuse_omitted(omitted_sh_body_sections(container)?, container.metadata())
}

/// Ids of the present SH sections whose header reads as body-omitted, in
/// [`SH_BODY_SECTIONS`] order.
pub(super) fn omitted_sh_body_sections(container: &PrlContainer) -> Result<Vec<u32>, PrlLoadError> {
    omitted_sections(|section_id, len| container.section_prefix(section_id, len))
}

fn refuse_omitted(section_ids: Vec<u32>, metadata: &ContainerMeta) -> Result<(), PrlLoadError> {
    if section_ids.is_empty() {
        return Ok(());
    }
    Err(PrlLoadError::ShBodiesOmitted {
        section_ids,
        streamable: metadata
            .find_section(SectionId::ClusterShPayloads as u32)
            .is_some(),
    })
}

/// Reads each present SH section's header prefix through `prefix` and
/// collects the ids whose body is omitted.
///
/// A header the gate cannot judge is left to that section's decoder, which
/// names the fault as it always has: unreadable bounds, a header shorter than
/// the flag's end, or a stale section version. A current-version header whose
/// flag is neither 0 nor 1 fails the load here, as the streaming metadata
/// readers fail it.
fn omitted_sections<P: AsRef<[u8]>>(
    mut prefix: impl FnMut(u32, u64) -> Result<Option<P>, PrlLoadError>,
) -> Result<Vec<u32>, PrlLoadError> {
    let mut omitted = Vec::new();
    for section in SH_BODY_SECTIONS {
        let header_len =
            sh_body_header_len(section).expect("SH_BODY_SECTIONS carry the flag") as u64;
        let Ok(Some(header)) = prefix(section as u32, header_len) else {
            continue;
        };
        let header = header.as_ref();
        if (header.len() as u64) < header_len || !version_is_current(section, header) {
            continue;
        }
        let presence = read_sh_body_presence(section, header).map_err(PrlLoadError::FormatError)?;
        if presence == ShBodyPresence::Omitted {
            omitted.push(section as u32);
        }
    }
    Ok(omitted)
}

/// Whether `header` opens with `section`'s current internal version: a `u32`
/// for ids 34/35, a `u8` for the sparse delta sections.
fn version_is_current(section: SectionId, header: &[u8]) -> bool {
    let u32_version = || {
        header
            .first_chunk::<4>()
            .map(|bytes| u32::from_le_bytes(*bytes))
    };
    match section {
        SectionId::OctahedralShVolume => u32_version() == Some(SH_VOLUME_VERSION),
        SectionId::DirectShVolume => u32_version() == Some(DIRECT_SH_VOLUME_VERSION),
        SectionId::DeltaShVolumes => header.first() == Some(&DELTA_SH_VOLUMES_VERSION),
        SectionId::DirectShDeltaVolumes => header.first() == Some(&DIRECT_SH_DELTA_VOLUMES_VERSION),
        SectionId::AnimatedDirectShDeltaVolumes => {
            header.first() == Some(&ANIMATED_DIRECT_SH_DELTA_VOLUMES_VERSION)
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sh_stream::read_container_positionally;
    use crate::sh_stream_tests::{Id50Body, write_stream_fixture, write_stream_fixture_omitting};

    // The gate reads header prefixes from the retained file before any
    // container exists, or from either container backing.
    #[test]
    fn the_gate_reads_body_presence_from_the_file_or_either_backing() {
        let present = write_stream_fixture(Id50Body::Valid);
        let omitted = write_stream_fixture_omitting(&[
            SectionId::OctahedralShVolume,
            SectionId::DirectShVolume,
        ]);
        let backings = |path: &str| {
            let (file, metadata) = read_container_positionally(path).unwrap();
            [
                PrlContainer::open(path).unwrap(),
                PrlContainer::from_positional(file, metadata, false),
            ]
        };
        let positional = |path: &str| {
            let (file, metadata) = read_container_positionally(path).unwrap();
            require_whole_load_sh_bodies_positionally(&file, &metadata)
        };
        let present_path = present.path.to_str().unwrap();
        positional(present_path).unwrap();
        for container in backings(present_path) {
            assert!(omitted_sh_body_sections(&container).unwrap().is_empty());
            require_whole_load_sh_bodies(&container).unwrap();
        }
        let omitted_path = omitted.path.to_str().unwrap();
        let refused = |result: Result<(), PrlLoadError>| {
            matches!(
                result,
                Err(PrlLoadError::ShBodiesOmitted {
                    section_ids,
                    streamable: true,
                }) if section_ids == [34, 35]
            )
        };
        assert!(refused(positional(omitted_path)));
        for container in backings(omitted_path) {
            assert_eq!(omitted_sh_body_sections(&container).unwrap(), [34, 35]);
            assert!(refused(require_whole_load_sh_bodies(&container)));
        }
    }

    // A header the gate cannot judge is the decoder's to name; only a
    // current-version header with an invalid flag fails at the gate.
    #[test]
    fn the_gate_judges_only_current_version_headers() {
        for section in SH_BODY_SECTIONS {
            let header_len = sh_body_header_len(section).unwrap();
            let width = if matches!(
                section,
                SectionId::OctahedralShVolume | SectionId::DirectShVolume
            ) {
                4
            } else {
                1
            };
            let mut header = vec![0u8; header_len];
            let version = match section {
                SectionId::OctahedralShVolume => SH_VOLUME_VERSION,
                SectionId::DirectShVolume => DIRECT_SH_VOLUME_VERSION,
                SectionId::DeltaShVolumes => u32::from(DELTA_SH_VOLUMES_VERSION),
                SectionId::DirectShDeltaVolumes => u32::from(DIRECT_SH_DELTA_VOLUMES_VERSION),
                _ => u32::from(ANIMATED_DIRECT_SH_DELTA_VOLUMES_VERSION),
            };
            header[..width].copy_from_slice(&version.to_le_bytes()[..width]);
            let judge = |bytes: &[u8]| {
                omitted_sections(|id, _| {
                    Ok(Some(if id == section as u32 {
                        bytes.to_vec()
                    } else {
                        Vec::new()
                    }))
                })
            };

            // Flag 0 at the header's end reads as omitted.
            assert_eq!(judge(&header).unwrap(), [section as u32], "{section:?}");
            // Flag 2: current version, invalid flag, a hard failure.
            header[header_len - width] = 2;
            let error = judge(&header).unwrap_err();
            assert!(
                error.to_string().contains("body-presence flag 2"),
                "{section:?}: {error}"
            );
            // Short or stale: deferred to the decoder.
            assert!(judge(&header[..header_len - 1]).unwrap().is_empty());
            header[0] = header[0].wrapping_sub(1);
            assert!(judge(&header).unwrap().is_empty(), "{section:?}");
        }
    }
}
