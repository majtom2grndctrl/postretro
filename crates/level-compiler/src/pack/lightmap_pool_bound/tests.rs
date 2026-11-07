//! Hand-computed bound fixtures, the cell input's seam, and the Build Summary.
//!
//! A full block fills a 2048² layer and a quarter block is 1024², so every
//! expected layer count below can be read off a fixture by eye. Layers price at
//! 6 MiB (BC6H 4 + direction 2) and 4 MiB per mask layer.

use std::time::Duration;

use log::Level;
use postretro_level_format::cell_residency_set::{DEFAULT_LEAD, ResidencyEntry};
use postretro_level_format::cells::{CELL_FLAG_DRAWABLE, CELL_FLAG_SOLID, CellRecord};
use postretro_level_format::cluster_directory::{
    CLUSTER_HINT_FLAG_PINNED, ClusterHintRecord, ClusterRecord,
};
use postretro_level_format::lightmap::{
    IRRADIANCE_FORMAT_BC6H, LIGHTMAP_POOL_LAYER_EDGE, LightmapBlock, LightmapBlockPayload,
    LightmapHeader, LightmapMode,
};
use postretro_level_format::lightmap_pool::{
    MASK_POOL_LAYER_BYTES, PerPool, PoolKind, all_resident_pool_bytes, lightmap_pool_layer_bytes,
    streamed_pool_bytes,
};
use postretro_level_format::shadowmask_atlas::group_plane_len;
use postretro_level_loader::{LightmapBlockClass, LightmapTarget};
use postretro_render_cpu::lightmap_pool::{
    DrainRequest, LightmapPoolModel, StaticPool, StreamedPoolSizing, plan_static_pool,
    plan_streaming_pool,
};
use postretro_test_log_capture::LogCapture;

use super::*;

const FULL: (u16, u16) = (2048, 2048);
const QUARTER: (u16, u16) = (1024, 1024);
const MIB: u64 = 1024 * 1024;

const NO_PLANES: [bool; 2] = [false, false];
const PLANE_A: [bool; 2] = [true, false];
const PLANE_B: [bool; 2] = [false, true];
const BOTH_PLANES: [bool; 2] = [true, true];

/// One block of a fixture level.
#[derive(Clone, Copy)]
struct Block {
    cell: u32,
    extent: (u16, u16),
    planes: [bool; 2],
}

fn block(cell: u32, extent: (u16, u16), planes: [bool; 2]) -> Block {
    Block {
        cell,
        extent,
        planes,
    }
}

/// Only extents and cells: nothing here reads a block's texels.
fn lightmap(blocks: &[Block]) -> LightmapSection {
    LightmapSection {
        direction_texel_scale: 2,
        irradiance_format: IRRADIANCE_FORMAT_BC6H,
        mode: LightmapMode::Shadowed,
        blocks: blocks
            .iter()
            .map(|block| LightmapBlock {
                cell_id: block.cell,
                width: block.extent.0,
                height: block.extent.1,
                irradiance: Vec::new(),
                direction: Vec::new(),
            })
            .collect(),
    }
}

/// A stored plane only has to be non-empty.
fn shadowmask(blocks: &[Block]) -> ShadowmaskAtlasSection {
    ShadowmaskAtlasSection {
        owners: Vec::new(),
        blocks: blocks
            .iter()
            .map(|block| block.planes.map(|stored| vec![1; usize::from(stored)]))
            .collect(),
    }
}

fn cell_records(flags: &[u32]) -> CellsSection {
    CellsSection {
        cells: flags
            .iter()
            .map(|&flags| CellRecord {
                bounds_min: [0.0; 3],
                bounds_max: [2.0, 4.0, 6.0],
                flags,
                face_start: 0,
                face_count: 0,
                portal_ref_start: 0,
                portal_ref_count: 0,
            })
            .collect(),
        portal_refs: Vec::new(),
    }
}

fn open_cells(count: usize) -> CellsSection {
    cell_records(&vec![CELL_FLAG_DRAWABLE; count])
}

/// One cluster per cell.
fn directory(cell_count: u32, pinned: &[u32]) -> ClusterDirectorySection {
    ClusterDirectorySection {
        runtime_cell_count: cell_count,
        primitive_limit: 64,
        cell_limit: 32,
        clusters: (0..cell_count)
            .map(|cell| ClusterRecord {
                bounds_min: [0.0; 3],
                bounds_max: [1.0; 3],
                member_start: cell,
                member_count: 1,
                range_start: 0,
                range_count: 0,
                primitive_count: 0,
                flags: 0,
            })
            .collect(),
        members: (0..cell_count).collect(),
        ranges: Vec::new(),
        resources: Vec::new(),
        cluster_hints: pinned
            .iter()
            .map(|&cluster_id| ClusterHintRecord {
                cluster_id,
                flags: CLUSTER_HINT_FLAG_PINNED,
                priority: 0,
            })
            .collect(),
        seam_portal_ids: Vec::new(),
        mandatory_bound: Vec::new(),
    }
}

/// Each camera cell's `(cell, lead)` pairs, sorted as the section stores them.
fn residency(max_lead: u32, sets: &[&[(u32, u32)]]) -> CellResidencySetSection {
    let mut offsets = vec![0];
    let mut entries = Vec::new();
    for set in sets {
        let mut set: Vec<_> = set
            .iter()
            .map(|&(cell_id, lead)| ResidencyEntry { lead, cell_id })
            .collect();
        set.sort_unstable();
        entries.extend(set);
        offsets.push(entries.len() as u32);
    }
    CellResidencySetSection {
        max_lead,
        offsets,
        entries,
    }
}

/// Everything a bound reads, owned by the test.
struct BoundLevel {
    lightmap: LightmapSection,
    shadowmask: ShadowmaskAtlasSection,
    cells: CellsSection,
    directory: ClusterDirectorySection,
    residency: CellResidencySetSection,
}

impl BoundLevel {
    fn new(blocks: &[Block], pinned: &[u32], max_lead: u32, sets: &[&[(u32, u32)]]) -> Self {
        Self {
            lightmap: lightmap(blocks),
            shadowmask: shadowmask(blocks),
            cells: open_cells(sets.len()),
            directory: directory(sets.len() as u32, pinned),
            residency: residency(max_lead, sets),
        }
    }

    fn resolve(&self) -> LightmapPoolBound {
        resolve_lightmap_pool_bound(self.inputs(Some(&self.residency))).unwrap()
    }

    /// The same level had its portals failed the loader's test.
    fn resolve_without_id_51(&self) -> LightmapPoolBound {
        resolve_lightmap_pool_bound(self.inputs(None)).unwrap()
    }

    fn inputs<'a>(
        &'a self,
        residency: Option<&'a CellResidencySetSection>,
    ) -> LightmapPoolBoundInputs<'a> {
        LightmapPoolBoundInputs {
            lightmap: &self.lightmap,
            shadowmask: Some(&self.shadowmask),
            cells: &self.cells,
            directory: &self.directory,
            residency,
        }
    }

    /// `worst_cell_demand` through the production input, plus `added` cells.
    fn worst_with(&self, added: &[(u32, u32)]) -> WorstDemand {
        let input = BoundCellInput::new(&self.residency, &self.directory).unwrap();
        worst_cell_demand(
            &camera_cells(&self.cells),
            |cell, set| {
                input.cells_of(cell, set);
                set.extend(
                    added
                        .iter()
                        .filter(|(camera, _)| *camera == cell)
                        .map(|&(_, cell)| cell),
                );
            },
            &CellBlocks::new(&self.lightmap, self.cells.cells.len()).unwrap(),
            &pool_demand(&self.lightmap, Some(&self.shadowmask)).unwrap(),
            LayerPrices::of(&self.lightmap).unwrap(),
        )
        .unwrap()
    }
}

fn section(lightmap: u32, mask: u32) -> Option<LightmapPoolBoundSection> {
    Some(LightmapPoolBoundSection {
        worst_lightmap_layers: lightmap,
        worst_mask_layers: mask,
    })
}

fn layers(lightmap: u32, mask: u32) -> PoolLayers {
    PoolLayers { lightmap, mask }
}

/// Cell 2 is four quarters, sharing one lightmap layer; cell 4 is pinned; cell
/// 5 holds two layers of lightmap and two mask planes.
///
/// | camera | set | lightmap | mask |
/// |---|---|---|---|
/// | 0 | cells 0, 1, 2, pin 4 | 3 full + 1 of quarters = 4 | 2 + 0 + 0 + 1 = 3 |
/// | 3 | cells 3, 0, 4 (also the pin) | 3 | 1 + 2 + 1 = 4 |
/// | 6 | cells 1, 3, pin 4 | 3 | 2 |
///
/// Cameras 1, 2, 4 and 5 have an empty set: the pin alone, 1 and 1.
fn hand_level() -> BoundLevel {
    let blocks = [
        block(0, FULL, BOTH_PLANES),
        block(1, FULL, NO_PLANES),
        block(2, QUARTER, NO_PLANES),
        block(2, QUARTER, NO_PLANES),
        block(2, QUARTER, NO_PLANES),
        block(2, QUARTER, NO_PLANES),
        block(3, FULL, PLANE_A),
        block(4, FULL, PLANE_B),
        block(5, FULL, BOTH_PLANES),
        block(5, FULL, NO_PLANES),
    ];
    let past = DEFAULT_LEAD + 1;
    BoundLevel::new(
        &blocks,
        &[4],
        DEFAULT_LEAD * 2,
        &[
            &[(0, 0), (1, DEFAULT_LEAD), (2, DEFAULT_LEAD), (5, past)],
            &[],
            &[],
            &[(3, 0), (0, DEFAULT_LEAD), (4, 5), (5, past)],
            &[],
            &[],
            &[(1, 0), (3, 0)],
        ],
    )
}

#[test]
fn bound_matches_hand_computed_fixture() {
    let level = hand_level();

    // Lightmap's worst is camera 0 and mask's is camera 3. Counting cell 5,
    // just past the lead, would read 6; dropping cells 1 and 2, exactly at it,
    // would read 2; counting the pin twice in camera 3 would read mask 5.
    assert_eq!(level.resolve().section, section(4, 4));
    let worst = level.worst_with(&[]);
    assert_eq!(worst.layers, layers(4, 4));
    assert_eq!(
        worst.peak,
        Some(PeakCell {
            cell: 0,
            bytes: (4 * 6 + 3 * 4) * MIB
        }),
        "camera 0 at 36 MiB beats camera 3 at 34 MiB"
    );
}

#[test]
fn an_absent_plane_demands_no_mask_layer_and_a_present_one_a_whole_placement() {
    let blocks = [
        block(0, QUARTER, BOTH_PLANES),
        block(0, QUARTER, BOTH_PLANES),
        block(0, QUARTER, PLANE_A),
        block(0, QUARTER, NO_PLANES),
    ];
    let level = BoundLevel::new(&blocks, &[], DEFAULT_LEAD, &[&[(0, 0)]]);
    assert_eq!(
        level.resolve().section,
        section(1, 2),
        "five planes at four per layer, in one lightmap layer"
    );
}

// A level with id 42 and no present plane has no mask pool.
#[test]
fn a_level_with_id_42_and_no_present_plane_bounds_no_mask_layers() {
    let level = BoundLevel::new(
        &[block(0, FULL, NO_PLANES)],
        &[],
        DEFAULT_LEAD,
        &[&[(0, 0)]],
    );
    assert_eq!(level.resolve().section, section(1, 0));
}

#[test]
fn a_level_without_id_42_bounds_no_mask_layers() {
    let level = BoundLevel::new(
        &[block(0, FULL, NO_PLANES)],
        &[],
        DEFAULT_LEAD,
        &[&[(0, 0)]],
    );
    let bound = resolve_lightmap_pool_bound(LightmapPoolBoundInputs {
        shadowmask: None,
        ..level.inputs(Some(&level.residency))
    })
    .unwrap();
    assert_eq!(bound.section, section(1, 0));
}

#[test]
fn the_baked_maximum_holds_where_it_is_below_the_default_lead() {
    let short = DEFAULT_LEAD / 2;
    let blocks = [block(0, FULL, NO_PLANES), block(1, FULL, NO_PLANES)];
    let level = BoundLevel::new(&blocks, &[], short, &[&[(0, 0), (1, short)], &[]]);
    let bound = level.resolve();
    assert_eq!(bound.section, section(2, 0));
    let line = &bound.report.lines()[0];
    assert!(line.contains("at 8 m lead"), "{line}");
}

#[test]
fn solid_and_exterior_cells_are_no_camera_cells() {
    let mut level = BoundLevel::new(
        &[block(0, FULL, NO_PLANES), block(1, FULL, NO_PLANES)],
        &[],
        DEFAULT_LEAD,
        &[&[(1, 0)], &[(0, 0), (1, 0)]],
    );
    level.cells = cell_records(&[CELL_FLAG_DRAWABLE, CELL_FLAG_SOLID]);
    assert_eq!(level.resolve().section, section(1, 0));
}

#[test]
fn a_level_of_no_camera_cell_stores_a_zero_bound_and_names_no_cell() {
    let mut level = BoundLevel::new(&[block(0, FULL, NO_PLANES)], &[], DEFAULT_LEAD, &[&[]]);
    level.cells = cell_records(&[CELL_FLAG_SOLID]);
    let bound = level.resolve();
    assert_eq!(bound.section, section(0, 0));
    let line = &bound.report.lines()[0];
    assert!(line.contains("worst case of 0 and 0"), "{line}");
    assert!(!line.contains("peaks"), "{line}");
}

// Camera cells whose sets hold no block cost 0 bytes, and a 0-byte cell is
// no peak worth naming.
#[test]
fn a_level_whose_camera_cells_all_pack_to_no_layers_names_no_peak_cell() {
    let level = BoundLevel::new(
        &[block(1, FULL, BOTH_PLANES)],
        &[],
        DEFAULT_LEAD,
        &[&[], &[]],
    );
    let worst = level.worst_with(&[]);
    assert_eq!(worst.layers, layers(0, 0));
    assert_eq!(worst.peak, None);
    let bound = level.resolve();
    assert_eq!(bound.section, section(0, 0));
    let line = &bound.report.lines()[0];
    assert!(line.contains("worst case of 0 and 0"), "{line}");
    assert!(!line.contains("peaks"), "{line}");
}

#[test]
fn added_cell_raises_the_bound_through_the_cell_input() {
    let level = hand_level();
    let before = level.worst_with(&[]);

    // Camera 6 goes from 3 and 2 to 5 and 4: past the lightmap worst, level
    // with the mask worst, and the most bytes at 46 MiB.
    let raised = level.worst_with(&[(6, 5)]);
    assert_eq!(raised.layers, layers(5, 4));
    assert_eq!(
        raised.peak,
        Some(PeakCell {
            cell: 6,
            bytes: (5 * 6 + 4 * 4) * MIB
        })
    );

    // Camera 3 gains a layer but only reaches the worst figures, so the
    // stored bound stands.
    assert_eq!(level.worst_with(&[(3, 1)]).layers, before.layers);

    let stored = |worst: WorstDemand| {
        LightmapPoolBoundSection {
            worst_lightmap_layers: worst.layers.lightmap,
            worst_mask_layers: worst.layers.mask,
        }
        .to_bytes()
    };
    assert_eq!(stored(raised).len(), stored(before).len(), "id 58's layout");
}

#[test]
fn a_cell_named_twice_in_a_set_counts_its_blocks_once() {
    let level = hand_level();
    assert_eq!(level.worst_with(&[(3, 3), (3, 0)]), level.worst_with(&[]));
}

// Mirror cells stay out of the bound: planar runs only at the top reflection
// tiers, so counting them would charge the default tier for an opt-in feature.
#[test]
fn bound_cell_input_reads_no_water_plane_data() {
    const FORBIDDEN: [&str; 7] = [
        "water",
        "planar",
        "mirror",
        "fluid",
        "reflection",
        "environment",
        "surface",
    ];
    let sources = [
        ("cell_input.rs", include_str!("cell_input.rs")),
        (
            "lightmap_pool_bound.rs",
            include_str!("../lightmap_pool_bound.rs"),
        ),
    ];
    for (name, source) in sources {
        let code = source
            .lines()
            .filter(|line| !line.trim_start().starts_with("//"))
            .collect::<Vec<_>>()
            .join("\n")
            .to_lowercase();
        assert!(code.contains("fn "), "{name} was read");
        for token in FORBIDDEN {
            assert!(!code.contains(token), "{name} names `{token}`");
        }
    }
    // Reads only what the set is defined from.
    let imports: Vec<_> = sources[0]
        .1
        .lines()
        .filter(|line| line.starts_with("use postretro_level_format"))
        .collect();
    assert_eq!(
        imports,
        [
            "use postretro_level_format::cell_residency_set::{CellResidencySetSection, DEFAULT_LEAD};",
            "use postretro_level_format::cluster_directory::ClusterDirectorySection;",
        ]
    );
}

/// `targeted`'s layers as the runtime counts them, across the dev-dependency
/// edge: the demand a loaded level reads from its id 22 and id 42 indices, and
/// the streamed pool model's own pack of its mandatory targets.
fn runtime_layers(level: &BoundLevel, targeted: &[u32]) -> PoolLayers {
    let demand = PoolDemand::of_indices(&level.lightmap.index(), Some(&level.shadowmask.index()));
    let mut model = LightmapPoolModel::new(demand, LIGHTMAP_POOL_LAYER_EDGE, PerPool::both(1))
        .expect("every fixture block fits a layer");
    let targets: Vec<LightmapTarget> = targeted
        .iter()
        .map(|&block| LightmapTarget {
            block,
            class: LightmapBlockClass::Mandatory,
            lead: 0,
        })
        .collect();
    model.plan_drain(DrainRequest {
        target_reset: Some(&targets),
        ..DrainRequest::default()
    });
    model.targeted_layers()
}

#[test]
fn compiler_and_runtime_count_the_same_layers() {
    // The first three take two layers tallest first and one in block-id order,
    // so a count in any other order disagrees here.
    let blocks: Vec<Block> = [(1792, 512), (896, 1408), (768, 640)]
        .into_iter()
        .chain((0..60u16).map(|n| (128 + (n * 37) % 900 / 4 * 4, 64 + (n * 53) % 1200 / 4 * 4)))
        .enumerate()
        .map(|(index, extent)| {
            let planes = [NO_PLANES, PLANE_A, PLANE_B, BOTH_PLANES][index % 4];
            block(0, extent, planes)
        })
        .collect();
    let level = BoundLevel::new(&blocks, &[], DEFAULT_LEAD, &[&[(0, 0)]]);

    let every_block: Vec<u32> = (0..blocks.len() as u32).collect();
    let expected = runtime_layers(&level, &every_block);
    assert!(expected.lightmap > 1 && expected.mask > 1, "{expected:?}");
    assert_eq!(
        level.resolve().section,
        section(expected.lightmap, expected.mask)
    );

    let demand = pool_demand(&level.lightmap, Some(&level.shadowmask)).unwrap();
    assert_eq!(all_resident_layers(&demand).unwrap(), expected);

    let expected = runtime_layers(&level, &[0, 1, 2]);
    assert_eq!(expected.lightmap, 2);
    assert_eq!(
        layers_of(&demand, &[0, 1, 2], &mut PackScratch::default()).unwrap(),
        expected
    );
}

fn lines_of(report: &LightmapPoolReport) -> String {
    report.lines().join("\n")
}

fn priced(bound: PoolLayers, all_resident: PoolLayers, peak: u32) -> LightmapPoolReport {
    let worst = WorstDemand {
        layers: bound,
        peak: Some(PeakCell {
            cell: peak,
            bytes: 1,
        }),
    };
    LightmapPoolReport::streamed(
        DEFAULT_LEAD,
        &worst,
        all_resident,
        test_prices(),
        &open_cells(8),
        Duration::ZERO,
    )
}

fn test_prices() -> LayerPrices {
    LayerPrices::of(&lightmap(&[block(0, FULL, NO_PLANES)])).unwrap()
}

#[test]
fn build_summary_reports_bound_and_warns_past_threshold() {
    let capture = LogCapture::start();

    // Streamed, at the threshold exactly: 32 × 6 + 48 × 4 = 384 MiB.
    let at = priced(layers(31, 47), layers(31, 47), 3);
    let text = lines_of(&at);
    assert_eq!(
        text,
        "Lightmap pool memory: 384.0 MiB at 16 m lead (how far ahead of the camera lightmaps \
         load), 31 lightmap and 47 shadow mask layers, sized from a worst case of 31 and 47; \
         demand peaks in cell 3, near (1.0, 2.0, 3.0) engine metres"
    );
    at.log();
    capture.assert_not_logged(Level::Warn, "Lightmap pool memory");

    // One mask layer over.
    let over = priced(layers(31, 48), layers(31, 48), 3);
    let text = lines_of(&over);
    assert!(
        text.starts_with("Lightmap pool memory: 388.0 MiB"),
        "{text}"
    );
    assert!(
        text.contains(
            "  warning: Lightmap pool memory 388.0 MiB is above the 384.0 MiB budget, so this \
             level may not fit a 2 GB GPU. Add walls or doors near (1.0, 2.0, 3.0) engine metres, \
             where lightmap demand peaks"
        ),
        "{text}"
    );
    over.log();
    capture.assert_logged_once(Level::Warn, "Lightmap pool memory 388.0 MiB");

    // No id 51, at the threshold: 64 × 6, no spare, no cell.
    capture.clear();
    let at = LightmapPoolReport::all_resident(layers(64, 0), test_prices(), Duration::ZERO);
    let text = lines_of(&at);
    assert_eq!(
        text,
        "Lightmap pool memory: 384.0 MiB, 64 lightmap and 0 shadow mask layers, the whole map: \
         without usable portals every lightmap stays loaded"
    );
    at.log();
    capture.assert_not_logged(Level::Warn, "Lightmap pool memory");

    let over = LightmapPoolReport::all_resident(layers(64, 1), test_prices(), Duration::ZERO);
    let text = lines_of(&over);
    assert!(text.contains("above the 384.0 MiB budget"), "{text}");
    assert!(text.contains("Seal the level"), "{text}");
    assert!(!text.contains("cell"), "no worst cell: {text}");
    over.log();
    capture.assert_logged_once(Level::Warn, "Lightmap pool memory 388.0 MiB");

    // No blocks.
    assert_eq!(
        lines_of(&LightmapPoolReport::default()),
        "Lightmap pool memory: none (this level has no baked lightmaps)"
    );
}

// Cell 3's mask layers and cell 5's lightmap layers price the same, so the tie
// runs across pools, whichever cell is offered first.
#[test]
fn build_summary_names_the_lowest_cell_on_a_byte_tie() {
    let prices = test_prices();
    for offers in [
        [(3, layers(0, 3)), (5, layers(2, 0))],
        [(5, layers(2, 0)), (3, layers(0, 3))],
    ] {
        let mut worst = WorstDemand::default();
        for (cell, layers) in offers {
            worst.offer(cell, layers, prices);
        }
        assert_eq!(worst.layers, layers(2, 3));
        assert_eq!(
            worst.peak,
            Some(PeakCell {
                cell: 3,
                bytes: 12 * MIB
            })
        );
    }
}

#[test]
fn a_tie_between_camera_cells_reports_the_lowest_id() {
    let blocks = [block(1, FULL, NO_PLANES), block(2, FULL, NO_PLANES)];
    let level = BoundLevel::new(
        &blocks,
        &[],
        DEFAULT_LEAD,
        &[&[], &[(2, 0)], &[(1, 0)], &[(2, 0)]],
    );
    let line = &level.resolve().report.lines()[0];
    assert!(line.contains("demand peaks in cell 1, near"), "{line}");
}

/// `count` full blocks, one per cell, each with `planes`; camera 0 sees the
/// first `seen` cells, and every other camera nothing.
fn full_block_level(count: u32, seen: u32, planes: [bool; 2]) -> BoundLevel {
    let blocks: Vec<_> = (0..count).map(|cell| block(cell, FULL, planes)).collect();
    let first: Vec<_> = (0..seen).map(|cell| (cell, 0)).collect();
    let mut sets = vec![first.as_slice()];
    sets.resize(count as usize, &[]);
    BoundLevel::new(&blocks, &[], DEFAULT_LEAD, &sets)
}

fn summary_bytes(bound: &LightmapPoolBound) -> String {
    let line = bound.report.lines().remove(0);
    let (bytes, _) = line
        .trim_start_matches("Lightmap pool memory: ")
        .split_once(" MiB")
        .unwrap();
    bytes.to_string()
}

/// What the runtime allocates first for `level` given `bound`, in the Build
/// Summary's MiB: render-cpu's streamed plan from the stored section, or with
/// none the all-resident plan the renderer uploads, each priced per layer.
fn runtime_first_generation_bytes(level: &BoundLevel, bound: &LightmapPoolBound) -> String {
    const MAX_DIMENSION: u32 = 8192;
    const MAX_LAYERS: u32 = 2048;
    let index = level.lightmap.index();
    let masks = level.shadowmask.index();
    let layer_bytes = PerPool {
        lightmap: lightmap_pool_layer_bytes(&index.header).unwrap(),
        mask: MASK_POOL_LAYER_BYTES,
    };
    let bytes: u64 = match bound.section {
        Some(section) => {
            let plan = plan_streaming_pool(
                Some(&index),
                Some(&masks),
                [0; 32],
                StreamedPoolSizing::from_bound(section.layers()),
                MAX_DIMENSION,
                MAX_LAYERS,
            );
            let StaticPool::Streaming(plan) = plan else {
                panic!("a level with id 58 streams: {plan:?}");
            };
            PoolKind::ALL
                .iter()
                .map(|&kind| {
                    streamed_pool_bytes(plan.first_generation_layers[kind], layer_bytes[kind])
                })
                .sum()
        }
        None => {
            let payloads: Vec<_> = level
                .lightmap
                .blocks
                .iter()
                .zip(&level.shadowmask.blocks)
                .map(|(block, planes)| sized_payload(&index.header, block, planes))
                .collect();
            let plan = plan_static_pool(
                Some(&index),
                Some(&masks),
                &payloads,
                MAX_DIMENSION,
                MAX_LAYERS,
            );
            let StaticPool::Blocks(plan) = plan else {
                panic!("a level without id 58 is all-resident: {plan:?}");
            };
            let mask_pool = plan.masks.as_ref().expect("the device holds the mask pool");
            all_resident_pool_bytes(plan.pool.layer_count, layer_bytes.lightmap)
                + all_resident_pool_bytes(mask_pool.layer_count, layer_bytes.mask)
        }
    };
    format!("{:.1}", bytes as f64 / MIB as f64)
}

/// Zeroed bytes of the lengths `block` and its stored planes need: the
/// all-resident plan checks lengths before it places anything.
fn sized_payload(
    header: &LightmapHeader,
    block: &LightmapBlock,
    planes: &[Vec<u8>; 2],
) -> LightmapBlockPayload {
    let (width, height) = (u32::from(block.width), u32::from(block.height));
    let zeroed = |len: Option<u64>| vec![0; len.unwrap() as usize];
    LightmapBlockPayload {
        irradiance: zeroed(header.irradiance_len(width, height)),
        direction: zeroed(header.direction_len(width, height)),
        shadowmask: Some(planes.each_ref().map(|plane| match plane.is_empty() {
            true => Vec::new(),
            false => zeroed(group_plane_len(width, height)),
        })),
    }
}

/// The summary's bytes equal `hand` MiB and what the runtime allocates first.
fn assert_summary_bytes(level: &BoundLevel, bound: &LightmapPoolBound, hand: u64) {
    let summary = summary_bytes(bound);
    assert_eq!(summary, format!("{hand}.0"));
    assert_eq!(summary, runtime_first_generation_bytes(level, bound));
}

#[test]
fn summary_bytes_equal_first_generation_bytes() {
    // Below the cap: 8 × 5/4 = 10 of 20 lightmap layers and, with both planes,
    // 16 × 5/4 = 20 of 40 mask layers. Each pool adds its spare.
    let level = full_block_level(20, 8, BOTH_PLANES);
    assert_summary_bytes(&level, &level.resolve(), 11 * 6 + 21 * 4);

    // At the cap: 10 × 5/4 rounds to 13, the whole level's 13 layers, and
    // plane A alone gives the mask pool the same figures. Spares stay.
    let level = full_block_level(13, 10, PLANE_A);
    assert_summary_bytes(&level, &level.resolve(), 14 * 6 + 14 * 4);

    // No id 51: the whole level, with no spare in either pool.
    assert_summary_bytes(&level, &level.resolve_without_id_51(), 13 * 6 + 13 * 4);

    // Id 42 with no present plane: no mask pool, so no mask spare either way.
    let level = full_block_level(20, 8, NO_PLANES);
    assert_summary_bytes(&level, &level.resolve(), 11 * 6);
    assert_summary_bytes(&level, &level.resolve_without_id_51(), 20 * 6);
}

#[test]
fn a_level_without_blocks_stores_nothing_and_reports_no_pool() {
    let level = BoundLevel::new(&[], &[], DEFAULT_LEAD, &[&[(0, 0)]]);
    let bound = level.resolve();
    assert_eq!(bound.section, None);
    assert_eq!(bound.report, LightmapPoolReport::default());
}

#[test]
fn a_level_without_id_51_stores_no_bound() {
    assert_eq!(hand_level().resolve_without_id_51().section, None);
}

// The runtime's block map refuses this, so the compiler must not bound it.
#[test]
fn a_cell_with_split_block_runs_fails_by_name() {
    let blocks = [
        block(0, FULL, NO_PLANES),
        block(1, FULL, NO_PLANES),
        block(0, FULL, NO_PLANES),
    ];
    let level = BoundLevel::new(&blocks, &[], DEFAULT_LEAD, &[&[(0, 0)], &[(1, 0)]]);
    let error = resolve_lightmap_pool_bound(level.inputs(Some(&level.residency)))
        .err()
        .expect("split runs are refused")
        .to_string();
    assert!(error.contains("Lightmap pool bound"), "{error}");
    assert!(error.contains("not contiguous"), "{error}");
}
