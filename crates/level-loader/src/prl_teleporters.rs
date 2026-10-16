//! Optional teleporter section: malformed bytes fail the level load.
use crate::prl::PrlLoadError;
use crate::prl_loader::section_validation_from_error;
use postretro_level_format::teleporters::TeleportersSection;

pub(crate) fn decode_teleporters(
    data: Option<Vec<u8>>,
) -> Result<TeleportersSection, PrlLoadError> {
    match data {
        None => Ok(TeleportersSection::default()),
        Some(data) => TeleportersSection::from_bytes(&data)
            .map_err(|error| section_validation_from_error("Teleporters", error)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{load_prl, prl_load_test_fixtures::write_prl_load_fixture};
    use postretro_level_format::{SectionBlob, SectionId};

    #[test]
    fn teleport_absent_section_loads_empty() {
        let path = write_prl_load_fixture([], "postretro_teleport_absent.prl");
        let result = load_prl(path.to_str().unwrap());
        std::fs::remove_file(path).unwrap();
        assert!(result.unwrap().teleporters.teleporters.is_empty());
    }
    #[test]
    fn teleport_present_section_loads_records() {
        use postretro_level_format::environment_volumes::{EnvironmentBrush, EnvironmentPlane};
        use postretro_level_format::teleporters::{TeleportMode, TeleporterRecord};
        let section = TeleportersSection {
            teleporters: vec![TeleporterRecord {
                source_entity: 9,
                mode: TeleportMode::Classic,
                enabled_on_spawn: false,
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
                destination_origin: [5.0, 6.0, 7.0],
                destination_yaw: 0.8,
                landmark_origin: [0.0; 3],
                landmark_yaw: 0.0,
                arrival_sound: "sfx/teleport".to_owned(),
                on_teleport: "gate.used".to_owned(),
            }],
        };
        let path = write_prl_load_fixture(
            [SectionBlob {
                section_id: SectionId::Teleporters as u32,
                version: 1,
                data: section.to_bytes(),
            }],
            "postretro_teleport_empty.prl",
        );
        let result = load_prl(path.to_str().unwrap());
        std::fs::remove_file(path).unwrap();
        assert_eq!(result.unwrap().teleporters, section);
    }
    #[test]
    fn teleport_malformed_section_fails_load() {
        let path = write_prl_load_fixture(
            [SectionBlob {
                section_id: SectionId::Teleporters as u32,
                version: 1,
                data: vec![1, 0, 1, 0, 0, 0],
            }],
            "postretro_teleport_malformed.prl",
        );
        let result = load_prl(path.to_str().unwrap());
        std::fs::remove_file(path).unwrap();
        assert!(result.err().unwrap().to_string().contains("Teleporters"));
    }
}
