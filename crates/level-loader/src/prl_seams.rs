//! Optional level-seam section: absent means no seams, malformed bytes fail the load.
//! See: context/lib/build_pipeline.md §PRL section IDs (Seams, id 61)
use crate::prl::PrlLoadError;
use crate::prl_loader::section_validation_from_error;
use postretro_level_format::seams::SeamsSection;

pub(crate) fn decode_seams(data: Option<Vec<u8>>) -> Result<SeamsSection, PrlLoadError> {
    match data {
        None => Ok(SeamsSection::default()),
        Some(data) => SeamsSection::from_bytes(&data)
            .map_err(|error| section_validation_from_error("Seams", error)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{load_prl, prl_load_test_fixtures::write_prl_load_fixture};
    use postretro_level_format::environment_volumes::{EnvironmentBrush, EnvironmentPlane};
    use postretro_level_format::seams::{SeamActivation, SeamMode, SeamRecord};
    use postretro_level_format::{SectionBlob, SectionId};

    fn load(
        blobs: impl IntoIterator<Item = SectionBlob>,
        name: &str,
    ) -> Result<SeamsSection, String> {
        let path = write_prl_load_fixture(blobs, name);
        let result = load_prl(path.to_str().unwrap());
        std::fs::remove_file(path).unwrap();
        result.map(|world| world.seams).map_err(|e| e.to_string())
    }

    fn blob(data: Vec<u8>) -> SectionBlob {
        SectionBlob {
            section_id: SectionId::Seams as u32,
            version: 1,
            data,
        }
    }

    #[test]
    fn seam_absent_section_loads_no_seams() {
        let seams = load([], "postretro_seam_absent.prl").unwrap();
        assert!(seams.seams.is_empty());
    }

    #[test]
    fn seam_present_section_loads_records() {
        let section = SeamsSection {
            seams: vec![SeamRecord {
                source_entity: 3,
                name: "pair_door".to_owned(),
                target: "seam-pair-b".to_owned(),
                activation: SeamActivation::Touch,
                mode: SeamMode::Classic,
                enabled_on_spawn: true,
                brushes: vec![EnvironmentBrush {
                    aabb_min: [0.0; 3],
                    aabb_max: [1.0; 3],
                    planes: vec![
                        EnvironmentPlane {
                            normal: [1.0, 0.0, 0.0],
                            distance: 1.0
                        };
                        4
                    ],
                }],
                landmark_origin: [2.0, 0.0, -1.0],
                landmark_yaw: 1.5,
                on_arrive: "seam.arrived".to_owned(),
            }],
        };
        let seams = load([blob(section.to_bytes())], "postretro_seam_present.prl").unwrap();
        assert_eq!(seams, section);
    }

    #[test]
    fn seam_malformed_section_fails_load_naming_the_section() {
        // Version 1, one record, then nothing: truncated.
        let error = load(
            [blob(vec![1, 0, 1, 0, 0, 0])],
            "postretro_seam_malformed.prl",
        )
        .unwrap_err();
        assert!(error.contains("Seams"), "{error}");
    }
}
