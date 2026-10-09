//! A small on-disk PRL whose lightmap streams, loaded through the real loader.
//! See: context/lib/testing_guide.md

use std::path::PathBuf;

use postretro_level_format::bvh::BvhSection;
use postretro_level_format::cell_locator::{
    CellLocatorChild, CellLocatorNodeRecord, CellLocatorSection,
};
use postretro_level_format::cell_residency_set::{CellResidencySetSection, ResidencyEntry};
use postretro_level_format::cells::{CellRecord, CellsSection};
use postretro_level_format::fog_volumes::FogVolumesSection;
use postretro_level_format::geometry::{GEOMETRY_CONTAINER_VERSION, GeometrySection};
use postretro_level_format::lightmap::{
    DIRECTION_TEXEL_BYTES, IRRADIANCE_FORMAT_RGBA16F, IRRADIANCE_TEXEL_BYTES,
    LIGHTMAP_POOL_LAYER_EDGE, LightmapBlock, LightmapMode, LightmapSection,
};
use postretro_level_format::portals::{PortalRecord, PortalsSection};
use postretro_level_format::sh_volume::OctahedralShVolumeSection;
use postretro_level_format::texture_cache_keys::TextureCacheKeysSection;
use postretro_level_format::{SectionBlob, SectionId, write_prl};
use postretro_level_loader::{LevelWorld, LightmapStreamManifest};
use postretro_render_cpu::lightmap_pool::LightmapPoolModel;

use super::levers::LEAD_UNITS_PER_METRE;

/// Cells in a row along +x, one metre each, joined by a portal at every
/// shared face. Cell c owns block c.
pub(crate) const FIXTURE_CELLS: u32 = 5;
/// Every block's extent: a multiple of the level's 4-texel slot alignment.
const BLOCK_EXTENT: (u16, u16) = (8, 4);
/// Leads of the neighbours one and two cells away. At the default 16 m lead
/// a camera cell's mandatory set is itself and its neighbours; the cells two
/// away are its prefetch band.
const NEAR_LEAD_METRES: u32 = 4;
const BAND_LEAD_METRES: u32 = 24;

/// A streamed-lightmap PRL in a temp directory, removed on drop. It carries
/// cells, portals, a cell locator, id 51 and id 22 (no id 42: the full load
/// keeps id 42 only beside a usable shadow-light selection).
pub(crate) struct StreamedLightmapPrl {
    _dir: tempfile::TempDir,
    path: PathBuf,
}

impl StreamedLightmapPrl {
    pub(crate) fn write() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("streamed-lightmap.prl");
        let mut file = std::fs::File::create(&path).unwrap();
        write_prl(&mut file, &sections()).unwrap();
        Self { _dir: dir, path }
    }

    /// Loads with the process's modes: `POSTRETRO_LIGHTMAP_STREAMING` unset
    /// (or `stream`) streams the lightmap.
    pub(crate) fn load(&self) -> LevelWorld {
        let world = postretro_level_loader::load_prl(self.path.to_str().unwrap())
            .expect("the streamed-lightmap fixture loads");
        assert!(
            world.lightmap_stream_manifest().is_some(),
            "the fixture streams its lightmap"
        );
        world
    }
}

/// The renderer's placement policy over `manifest`'s blocks at the default
/// cap, standing in for the GPU pool.
pub(crate) fn manifest_pool_model(manifest: &LightmapStreamManifest) -> LightmapPoolModel {
    let index = manifest.lightmap_index();
    let extents = index
        .records
        .iter()
        .map(|record| (u32::from(record.width), u32::from(record.height)))
        .collect();
    LightmapPoolModel::new(
        extents,
        index.header.block_alignment(),
        LIGHTMAP_POOL_LAYER_EDGE,
        postretro_renderer::DEFAULT_LIGHTMAP_POOL_CAP_LAYERS,
    )
    .expect("fixture blocks fit a pool layer")
}

fn sections() -> Vec<SectionBlob> {
    let blob = |section_id: SectionId, version, data| SectionBlob {
        section_id: section_id as u32,
        version,
        data,
    };
    vec![
        blob(
            SectionId::Geometry,
            GEOMETRY_CONTAINER_VERSION,
            GeometrySection {
                vertices: Vec::new(),
                indices: Vec::new(),
                faces: Vec::new(),
            }
            .to_bytes(),
        ),
        blob(
            SectionId::Bvh,
            1,
            BvhSection {
                nodes: Vec::new(),
                leaves: Vec::new(),
                root_node_index: 0,
            }
            .to_bytes(),
        ),
        blob(SectionId::Cells, 1, cells().to_bytes()),
        blob(SectionId::CellLocator, 1, locator().to_bytes()),
        blob(SectionId::Portals, 1, portals().to_bytes()),
        blob(
            SectionId::OctahedralShVolume,
            1,
            OctahedralShVolumeSection::placeholder().to_bytes(),
        ),
        blob(
            SectionId::TextureCacheKeys,
            1,
            TextureCacheKeysSection::default().to_bytes(),
        ),
        blob(
            SectionId::FogVolumes,
            1,
            FogVolumesSection::default().to_bytes(),
        ),
        blob(SectionId::CellResidencySet, 1, residency_set().to_bytes()),
        blob(SectionId::Lightmap, 1, lightmap().to_bytes()),
    ]
}

/// Cell c spans x in [c, c + 1]; it references the portals on its faces.
fn cells() -> CellsSection {
    let last = FIXTURE_CELLS - 1;
    let mut portal_refs = Vec::new();
    let cells = (0..FIXTURE_CELLS)
        .map(|cell| {
            let start = portal_refs.len() as u32;
            if cell > 0 {
                portal_refs.push(cell - 1);
            }
            if cell < last {
                portal_refs.push(cell);
            }
            CellRecord {
                bounds_min: [cell as f32, 0.0, 0.0],
                bounds_max: [cell as f32 + 1.0, 1.0, 1.0],
                flags: 0,
                face_start: 0,
                face_count: 0,
                portal_ref_start: start,
                portal_ref_count: portal_refs.len() as u32 - start,
            }
        })
        .collect();
    CellsSection { cells, portal_refs }
}

/// Portal p is the unit square at x = p + 1, between cells p and p + 1.
fn portals() -> PortalsSection {
    let mut vertices = Vec::new();
    let portals = (0..FIXTURE_CELLS - 1)
        .map(|portal| {
            let x = portal as f32 + 1.0;
            let vertex_start = vertices.len() as u32;
            vertices.extend([[x, 0.0, 0.0], [x, 1.0, 0.0], [x, 1.0, 1.0], [x, 0.0, 1.0]]);
            PortalRecord {
                vertex_start,
                vertex_count: 4,
                front_leaf: portal,
                back_leaf: portal + 1,
            }
        })
        .collect();
    PortalsSection { portals, vertices }
}

/// A chain of x-planes: node n splits cell n (behind x = n + 1) from the
/// cells past it.
fn locator() -> CellLocatorSection {
    let last = FIXTURE_CELLS - 1;
    let nodes = (0..last)
        .map(|node| CellLocatorNodeRecord {
            plane_normal: [1.0, 0.0, 0.0],
            plane_distance: node as f32 + 1.0,
            front: if node + 1 == last {
                CellLocatorChild::Cell(last)
            } else {
                CellLocatorChild::Node(node + 1)
            },
            back: CellLocatorChild::Cell(node),
        })
        .collect();
    CellLocatorSection {
        root: CellLocatorChild::Node(0),
        nodes,
    }
}

/// From camera cell c: c at lead 0, its neighbours at 4 m, the cells two
/// away at 24 m.
fn residency_set() -> CellResidencySetSection {
    let mut offsets = vec![0];
    let mut entries = Vec::new();
    for camera in 0..FIXTURE_CELLS {
        let mut range: Vec<ResidencyEntry> = (0..FIXTURE_CELLS)
            .filter_map(|cell| {
                let lead = match camera.abs_diff(cell) {
                    0 => 0,
                    1 => NEAR_LEAD_METRES,
                    2 => BAND_LEAD_METRES,
                    _ => return None,
                };
                Some(ResidencyEntry {
                    lead: lead * LEAD_UNITS_PER_METRE,
                    cell_id: cell,
                })
            })
            .collect();
        range.sort();
        entries.extend(range);
        offsets.push(entries.len() as u32);
    }
    CellResidencySetSection {
        max_lead: 32 * LEAD_UNITS_PER_METRE,
        offsets,
        entries,
    }
}

/// One block per cell, with bytes distinct per block.
fn lightmap() -> LightmapSection {
    let (width, height) = BLOCK_EXTENT;
    let texels = usize::from(width) * usize::from(height);
    LightmapSection {
        direction_texel_scale: 2,
        irradiance_format: IRRADIANCE_FORMAT_RGBA16F,
        mode: LightmapMode::Shadowed,
        blocks: (0..FIXTURE_CELLS)
            .map(|cell| LightmapBlock {
                cell_id: cell,
                width,
                height,
                irradiance: vec![cell as u8 + 1; texels * IRRADIANCE_TEXEL_BYTES],
                direction: vec![200 - cell as u8; texels / 4 * DIRECTION_TEXEL_BYTES],
            })
            .collect(),
    }
}
