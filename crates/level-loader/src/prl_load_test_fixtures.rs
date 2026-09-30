// Minimal on-disk PRL fixtures shared by the loader's test modules.
// See: context/lib/testing_guide.md §4 (Test fixtures)

use postretro_level_format::bvh::BvhSection;
use postretro_level_format::cell_locator::{
    CellLocatorChild as FormatCellLocatorChild, CellLocatorNodeRecord, CellLocatorSection,
};
use postretro_level_format::cells::{CellRecord, CellsSection};
use postretro_level_format::fog_volumes::FogVolumesSection;
use postretro_level_format::geometry::{GEOMETRY_CONTAINER_VERSION, GeometrySection};
use postretro_level_format::portals::{PortalRecord, PortalsSection};
use postretro_level_format::sh_volume::OctahedralShVolumeSection;
use postretro_level_format::texture_cache_keys::TextureCacheKeysSection;
use postretro_level_format::{SectionBlob, SectionId, write_prl};

/// A loadable two-cell PRL with empty geometry plus `additional_sections`,
/// written to the temp dir under `name`.
pub(crate) fn write_prl_load_fixture(
    additional_sections: impl IntoIterator<Item = SectionBlob>,
    name: &str,
) -> std::path::PathBuf {
    write_prl_load_fixture_with_geometry(
        geometry_blob(GeometrySection {
            vertices: Vec::new(),
            indices: Vec::new(),
            faces: Vec::new(),
        }),
        additional_sections,
        name,
    )
}

/// A Geometry (id 17) blob at the current container version.
pub(crate) fn geometry_blob(section: GeometrySection) -> SectionBlob {
    SectionBlob {
        section_id: SectionId::Geometry as u32,
        version: GEOMETRY_CONTAINER_VERSION,
        data: section.to_bytes(),
    }
}

/// [`write_prl_load_fixture`] with a caller-built Geometry (id 17) blob.
pub(crate) fn write_prl_load_fixture_with_geometry(
    geometry: SectionBlob,
    additional_sections: impl IntoIterator<Item = SectionBlob>,
    name: &str,
) -> std::path::PathBuf {
    let cell = |x: f32| CellRecord {
        bounds_min: [x, 0.0, 0.0],
        bounds_max: [x + 1.0, 1.0, 1.0],
        flags: 0,
        face_start: 0,
        face_count: 0,
        portal_ref_start: 0,
        portal_ref_count: 0,
    };
    let cells = CellsSection {
        cells: vec![cell(0.0), cell(2.0)],
        portal_refs: Vec::new(),
    };
    let locator = CellLocatorSection {
        root: FormatCellLocatorChild::Node(0),
        nodes: vec![CellLocatorNodeRecord {
            plane_normal: [1.0, 0.0, 0.0],
            plane_distance: 1.5,
            front: FormatCellLocatorChild::Cell(0),
            back: FormatCellLocatorChild::Cell(1),
        }],
    };
    write_fixture(geometry, cells, locator, additional_sections, name)
}

/// [`write_prl_load_fixture`] whose two cells share portal 0 through the
/// x = 1 face, so the level has a usable portal graph (id 15 included).
pub(crate) fn write_portal_prl_load_fixture(
    additional_sections: impl IntoIterator<Item = SectionBlob>,
    name: &str,
) -> std::path::PathBuf {
    let cell = |x: f32, refs_at: u32| CellRecord {
        bounds_min: [x, 0.0, 0.0],
        bounds_max: [x + 1.0, 1.0, 1.0],
        flags: 0,
        face_start: 0,
        face_count: 0,
        portal_ref_start: refs_at,
        portal_ref_count: 1,
    };
    let cells = CellsSection {
        cells: vec![cell(0.0, 0), cell(1.0, 1)],
        portal_refs: vec![0, 0],
    };
    let portals = PortalsSection {
        portals: vec![PortalRecord {
            vertex_start: 0,
            vertex_count: 4,
            front_leaf: 0,
            back_leaf: 1,
        }],
        vertices: vec![
            [1.0, 0.0, 0.0],
            [1.0, 1.0, 0.0],
            [1.0, 1.0, 1.0],
            [1.0, 0.0, 1.0],
        ],
    };
    let locator = CellLocatorSection {
        root: FormatCellLocatorChild::Node(0),
        nodes: vec![CellLocatorNodeRecord {
            plane_normal: [1.0, 0.0, 0.0],
            plane_distance: 1.0,
            front: FormatCellLocatorChild::Cell(1),
            back: FormatCellLocatorChild::Cell(0),
        }],
    };
    let portals = SectionBlob {
        section_id: SectionId::Portals as u32,
        version: 1,
        data: portals.to_bytes(),
    };
    write_fixture(
        geometry_blob(GeometrySection {
            vertices: Vec::new(),
            indices: Vec::new(),
            faces: Vec::new(),
        }),
        cells,
        locator,
        std::iter::once(portals).chain(additional_sections),
        name,
    )
}

fn write_fixture(
    geometry: SectionBlob,
    cells: CellsSection,
    locator: CellLocatorSection,
    additional_sections: impl IntoIterator<Item = SectionBlob>,
    name: &str,
) -> std::path::PathBuf {
    let mut sections = vec![
        geometry,
        SectionBlob {
            section_id: SectionId::Bvh as u32,
            version: 1,
            data: BvhSection {
                nodes: Vec::new(),
                leaves: Vec::new(),
                root_node_index: 0,
            }
            .to_bytes(),
        },
        SectionBlob {
            section_id: SectionId::Cells as u32,
            version: 1,
            data: cells.to_bytes(),
        },
        SectionBlob {
            section_id: SectionId::CellLocator as u32,
            version: 1,
            data: locator.to_bytes(),
        },
        SectionBlob {
            section_id: SectionId::OctahedralShVolume as u32,
            version: 1,
            data: OctahedralShVolumeSection::placeholder().to_bytes(),
        },
        SectionBlob {
            section_id: SectionId::TextureCacheKeys as u32,
            version: 1,
            data: TextureCacheKeysSection::default().to_bytes(),
        },
        SectionBlob {
            section_id: SectionId::FogVolumes as u32,
            version: 1,
            data: FogVolumesSection::default().to_bytes(),
        },
    ];
    sections.extend(additional_sections);

    let path = std::env::temp_dir().join(name);
    let mut file = std::fs::File::create(&path).unwrap();
    write_prl(&mut file, &sections).unwrap();
    path
}
