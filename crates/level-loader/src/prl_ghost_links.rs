//! Optional ghost-link section: absent means no links, malformed bytes fail the load.
//! See: context/lib/build_pipeline.md §PRL section IDs (GhostLinks, id 56)
use crate::prl::PrlLoadError;
use crate::prl_loader::section_validation_from_error;
use postretro_level_format::ghost_links::GhostLinksSection;

pub(crate) fn decode_ghost_links(data: Option<Vec<u8>>) -> Result<GhostLinksSection, PrlLoadError> {
    match data {
        None => Ok(GhostLinksSection::default()),
        Some(data) => GhostLinksSection::from_bytes(&data)
            .map_err(|error| section_validation_from_error("GhostLinks", error)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{load_prl, prl_load_test_fixtures::write_prl_load_fixture};
    use postretro_level_format::environment_volumes::{EnvironmentBrush, EnvironmentPlane};
    use postretro_level_format::ghost_links::{GhostLinkRecord, GhostMembers};
    use postretro_level_format::{SectionBlob, SectionId};

    fn load(
        blobs: impl IntoIterator<Item = SectionBlob>,
        name: &str,
    ) -> Result<GhostLinksSection, String> {
        let path = write_prl_load_fixture(blobs, name);
        let result = load_prl(path.to_str().unwrap());
        std::fs::remove_file(path).unwrap();
        result
            .map(|world| world.ghost_links)
            .map_err(|e| e.to_string())
    }

    #[test]
    fn ghost_link_absent_section_loads_no_links() {
        let links = load([], "postretro_ghost_link_absent.prl").unwrap();
        assert!(links.links.is_empty());
    }

    #[test]
    fn ghost_link_present_section_loads_records() {
        let section = GhostLinksSection {
            links: vec![GhostLinkRecord {
                source_entity: 4,
                enabled_on_spawn: true,
                members: GhostMembers::PLAYERS | GhostMembers::PROJECTILES,
                style: "echo".to_owned(),
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
                landmark_origin: [1.0, 0.0, 0.0],
                landmark_yaw: 0.0,
                target_origin: [9.0, 0.0, 0.0],
                target_yaw: 0.5,
                ghost_tags: vec!["barrier".to_owned()],
            }],
        };
        let links = load(
            [SectionBlob {
                section_id: SectionId::GhostLinks as u32,
                version: 1,
                data: section.to_bytes(),
            }],
            "postretro_ghost_link_present.prl",
        )
        .unwrap();
        assert_eq!(links, section);
    }

    #[test]
    fn ghost_link_malformed_section_fails_load() {
        // A present section with zero links is malformed: absence means none.
        let error = load(
            [SectionBlob {
                section_id: SectionId::GhostLinks as u32,
                version: 1,
                data: vec![1, 0, 0, 0, 0, 0],
            }],
            "postretro_ghost_link_malformed.prl",
        )
        .unwrap_err();
        assert!(error.contains("GhostLinks"), "{error}");
    }
}
