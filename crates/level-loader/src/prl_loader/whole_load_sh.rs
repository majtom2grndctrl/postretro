// Whole-load SH body gate: the legacy path refuses a PRL whose SH sections
// omit the bodies only it reads.
// See: context/lib/rendering_pipeline.md §Cluster SH residency

use postretro_level_format::SectionId;
use postretro_level_format::sh_body::{
    SH_BODY_SECTIONS, ShBodyPresence, read_sh_body_presence, sh_body_header_len,
};

use crate::PrlLoadError;
use crate::prl_container::PrlContainer;

/// Refuse a whole load when any present id 27/34/35/41/45 omits its body.
///
/// Runs before any SH decode, so the soft "malformed -> disable" paths of ids
/// 35/41/45 never see an omitted body. Reads only each section's header
/// prefix. A header it cannot read (bad bounds, stale version, invalid flag)
/// is left to that section's decoder, which names the fault as it always has.
pub(super) fn require_whole_load_sh_bodies(container: &PrlContainer) -> Result<(), PrlLoadError> {
    let section_ids = omitted_sh_body_sections(container);
    if section_ids.is_empty() {
        return Ok(());
    }
    Err(PrlLoadError::ShBodiesOmitted {
        section_ids,
        streamable: container.has_section(SectionId::ClusterShPayloads as u32),
    })
}

/// Ids of the present SH sections whose header reads as body-omitted.
pub(super) fn omitted_sh_body_sections(container: &PrlContainer) -> Vec<u32> {
    SH_BODY_SECTIONS
        .into_iter()
        .filter(|&section| body_omitted(container, section))
        .map(|section| section as u32)
        .collect()
}

fn body_omitted(container: &PrlContainer, section: SectionId) -> bool {
    let header_len = sh_body_header_len(section).expect("SH_BODY_SECTIONS carry the flag") as u64;
    match container.section_prefix(section as u32, header_len) {
        Ok(Some(prefix)) => matches!(
            read_sh_body_presence(section, &prefix),
            Ok(ShBodyPresence::Omitted)
        ),
        Ok(None) | Err(_) => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sh_stream::read_container_positionally;
    use crate::sh_stream_tests::{Id50Body, write_stream_fixture, write_stream_fixture_omitting};

    // The gate reads header prefixes from either backing: the whole image, or
    // the retained file a lightmap-streaming load reads positionally.
    #[test]
    fn the_gate_reads_body_presence_from_either_backing() {
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
        for container in backings(present.path.to_str().unwrap()) {
            assert!(omitted_sh_body_sections(&container).is_empty());
            require_whole_load_sh_bodies(&container).unwrap();
        }
        for container in backings(omitted.path.to_str().unwrap()) {
            assert_eq!(omitted_sh_body_sections(&container), [34, 35]);
            assert!(matches!(
                require_whole_load_sh_bodies(&container),
                Err(PrlLoadError::ShBodiesOmitted {
                    streamable: true,
                    ..
                })
            ));
        }
    }
}
