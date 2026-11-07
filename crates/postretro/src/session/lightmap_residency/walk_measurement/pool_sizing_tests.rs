//! The harness's pools against the runtime's: first generation, each pool's
//! retirement, and a walk within the bound.
//! See: context/lib/rendering_pipeline.md §4

use std::sync::Arc;

use postretro_level_format::cell_residency_set::CellResidencySetSection;
use postretro_level_format::lightmap::{
    DIRECTION_TEXEL_BYTES, IRRADIANCE_FORMAT_BC6H, LightmapBlock, LightmapMode, LightmapSection,
};
use postretro_level_format::lightmap_pool_bound::LightmapPoolBoundSection;
use postretro_level_format::shadowmask_atlas::{ShadowmaskAtlasSection, group_plane_len};
use postretro_level_format::{SectionBlob, SectionId};
use postretro_level_loader::LevelWorld;
use postretro_render_cpu::lightmap_pool::{PerPool, PoolKind};
use postretro_visibility::VisibleCells;

use super::frame_driver::{FrameDriver, FrameInput, FrameOutcome};
use super::levers::{DEFAULT_LEVERS, Levers};
use super::modes::ReadsMode;
use super::pool_mirror::{PoolMirror, PoolStats, RetireClock};
use super::{LightmapStreamingSession, Rig};
use crate::level_load::test_support::offscreen_renderer_or_skip;
use crate::lightmap_streaming::prl_test_fixture::{FIXTURE_CELLS, StreamedLightmapPrl};
use crate::lightmap_streaming::source::LightmapBlockSource;
use crate::lightmap_streaming::test_fixtures::{
    BlockSpec, PORTAL, TestBlockSource, corridor_blocks, corridor_set,
};
use crate::session::level_streaming::LevelStreaming;
use crate::streaming::cell_demand::CellDemand;

const MIB: u64 = 1024 * 1024;

/// Past half a layer on a side, so no two blocks share a layer and a pool's
/// layers count its items.
const LAYER_FILLING_EXTENT: u16 = 1100;
/// Planes of the fixture's five blocks, in block order: four in all.
const PLANES: [[bool; 2]; FIXTURE_CELLS as usize] = [
    [true, true],
    [true, false],
    [false, true],
    [false, false],
    [false, false],
];

/// Id 22 at the shipped formats, every block filling a layer.
fn layer_filling_lightmap() -> LightmapSection {
    let edge = usize::from(LAYER_FILLING_EXTENT);
    LightmapSection {
        direction_texel_scale: 2,
        irradiance_format: IRRADIANCE_FORMAT_BC6H,
        mode: LightmapMode::Shadowed,
        blocks: (0..FIXTURE_CELLS)
            .map(|cell| LightmapBlock {
                cell_id: cell,
                width: LAYER_FILLING_EXTENT,
                height: LAYER_FILLING_EXTENT,
                irradiance: vec![1; (edge / 4) * (edge / 4) * 16],
                direction: vec![1; (edge / 2) * (edge / 2) * DIRECTION_TEXEL_BYTES],
            })
            .collect(),
    }
}

fn masks() -> ShadowmaskAtlasSection {
    let extent = u32::from(LAYER_FILLING_EXTENT);
    let plane = group_plane_len(extent, extent).unwrap() as usize;
    let stored = |present: bool| if present { vec![1; plane] } else { Vec::new() };
    ShadowmaskAtlasSection {
        owners: Vec::new(),
        blocks: PLANES
            .iter()
            .map(|&[a, b]| [stored(a), stored(b)])
            .collect(),
    }
}

/// The streamed fixture with id 42 and blocks that each fill a layer, its id 58
/// holding `bound`: five lightmap layers and four mask layers all-resident.
fn sized_level(bound: LightmapPoolBoundSection) -> StreamedLightmapPrl {
    StreamedLightmapPrl::write_edited(|sections| {
        let blob = |section: SectionId, data| SectionBlob {
            section_id: section as u32,
            version: 1,
            data,
        };
        sections.retain(|section| {
            section.section_id != SectionId::Lightmap as u32
                && section.section_id != SectionId::LightmapPoolBound as u32
        });
        sections.push(blob(SectionId::LightmapPoolBound, bound.to_bytes()));
        sections.push(blob(
            SectionId::Lightmap,
            layer_filling_lightmap().to_bytes(),
        ));
        sections.push(blob(SectionId::ShadowmaskAtlas, masks().to_bytes()));
    })
}

/// The harness's first layers and bytes, by pool, from a rig at `levers`.
fn harness_first_generation(world: &LevelWorld, levers: Levers) -> PerPool<(u32, u64)> {
    let rig = Rig::new(world, levers, ReadsMode::CompletedReads);
    let pools = &rig.driver.mirror().stats.pools;
    PerPool::from_fn(|kind| (pools[kind].first_layers, pools[kind].first_bytes))
}

/// The first generation the renderer allocates for `level`, from its own
/// counters, or `None` without an adapter.
fn runtime_first_generation(
    test: &str,
    level: &StreamedLightmapPrl,
) -> Option<PerPool<(u32, u64)>> {
    let mut renderer = offscreen_renderer_or_skip(test)?;
    let mut world = level.load();
    let payloads = world.take_gpu_lighting_payloads();
    let geometry = crate::render::level_world_to_geometry(&world, &[]);
    renderer
        .install_level_geometry(&geometry, payloads)
        .expect("the fixture installs");
    let counters = renderer
        .lightmap_stream_counters()
        .expect("the fixture streams its lightmap");
    Some(PerPool::from_fn(|kind| {
        (counters.pool_layers[kind], counters.active_pool_bytes[kind])
    }))
}

// Pool sizing. The figures are worked by hand from the bound, the 5/4 margin and the
// shipped layer prices; the renderer's own counters confirm them where an
// adapter exists. Spares count, and a pool with nothing to hold has neither
// layers nor a spare.
#[test]
fn walk_harness_first_generation_matches_runtime() {
    const TEST: &str = "walk_harness_first_generation_matches_runtime";
    const LIGHTMAP_LAYER: u64 = 6 * MIB;
    const MASK_LAYER: u64 = 4 * MIB;
    let cases = [
        (
            "below the all-resident layers",
            sized_level(LightmapPoolBoundSection {
                worst_lightmap_layers: 3,
                worst_mask_layers: 2,
            }),
            PerPool {
                lightmap: (4, 5 * LIGHTMAP_LAYER),
                mask: (3, 4 * MASK_LAYER),
            },
        ),
        (
            "at the all-resident layers",
            sized_level(LightmapPoolBoundSection {
                worst_lightmap_layers: 5,
                worst_mask_layers: 4,
            }),
            PerPool {
                lightmap: (5, 6 * LIGHTMAP_LAYER),
                mask: (4, 5 * MASK_LAYER),
            },
        ),
    ];
    for (name, level, expected) in cases {
        let world = level.load();
        assert_eq!(
            harness_first_generation(&world, DEFAULT_LEVERS),
            expected,
            "{name}"
        );
        if let Some(runtime) = runtime_first_generation(TEST, &level) {
            assert_eq!(runtime, expected, "{name}: the renderer's allocation");
        }
    }

    let no_masks = StreamedLightmapPrl::write();
    let world = no_masks.load();
    let harness = harness_first_generation(&world, DEFAULT_LEVERS);
    assert_eq!(harness.mask, (0, 0), "no id 42, no mask pool");
    assert_eq!(harness.lightmap.0, 1);
    if let Some(runtime) = runtime_first_generation(TEST, &no_masks) {
        assert_eq!(runtime, harness, "the renderer's allocation with no id 42");
    }
}

// A harness sets a pool's size directly: one pool's figure leaves the other on
// id 58, and may stand past what the level needs all-resident.
#[test]
fn a_pool_size_sets_that_pools_first_generation_alone() {
    let level = sized_level(LightmapPoolBoundSection {
        worst_lightmap_layers: 3,
        worst_mask_layers: 2,
    });
    let world = level.load();
    let layers = |lightmap, mask| {
        let levers = Levers {
            pool_layers: PerPool { lightmap, mask },
            ..DEFAULT_LEVERS
        };
        harness_first_generation(&world, levers).map(|(layers, _)| layers)
    };
    assert_eq!(
        layers(Some(7), None),
        PerPool {
            lightmap: 7,
            mask: 3
        }
    );
    assert_eq!(
        layers(None, Some(9)),
        PerPool {
            lightmap: 4,
            mask: 9
        }
    );
    assert_eq!(
        layers(Some(2), Some(1)),
        PerPool {
            lightmap: 2,
            mask: 1
        }
    );
}

const CORRIDOR_LAYER_EDGE: u32 = 64;
/// Camera 3's mandatory blocks 1–5 are the corridor's worst: five lightmap
/// layers, and two planes each in the mask pool.
const CORRIDOR_BOUND: PerPool<u32> = PerPool {
    lightmap: 5,
    mask: 10,
};
const CORRIDOR_CAMERAS: u32 = 7;

/// The corridor level, every block holding both planes and filling a 64²
/// layer, streamed through a real issuer into the pool mirror.
struct MaskedCorridor {
    driver: FrameDriver,
    set: CellResidencySetSection,
}

impl MaskedCorridor {
    fn new(first_generation: PerPool<u32>) -> Self {
        Self::over(corridor_blocks(64, true), first_generation)
    }

    fn over(blocks: Vec<BlockSpec>, first_generation: PerPool<u32>) -> Self {
        let source = TestBlockSource::new(blocks);
        let set = corridor_set();
        let session = LightmapStreamingSession::with_source(
            Arc::clone(&source) as Arc<dyn LightmapBlockSource>,
            &set,
            None,
        )
        .unwrap();
        let mirror = PoolMirror::for_test_source(
            source.as_ref(),
            CORRIDOR_LAYER_EDGE,
            first_generation,
            RetireClock::Frames,
        );
        let mut level = LevelStreaming::default();
        level.install_cell_demand(CellDemand::new(set.max_lead));
        level.install_lightmap(session);
        let cell_blocks = super::baked_set::cell_block_ranges(0..7, 8);
        Self {
            driver: FrameDriver::new(level, mirror, ReadsMode::CompletedReads, cell_blocks),
            set,
        }
    }

    fn frame(&mut self, camera_cell: u32, drawn: &[u32]) -> FrameOutcome {
        let visible = VisibleCells::Culled(drawn.to_vec());
        self.driver.frame(
            FrameInput {
                visible_cells: &visible,
                sh_visible_cells: &visible,
                camera_cell,
                path: PORTAL,
                seconds: 0.0,
            },
            &self.set,
        )
    }

    /// Up the corridor and back, a few frames at each camera cell, drawing only
    /// the camera cell itself: every demand is mandatory.
    fn walk(&mut self) {
        let there = 0..CORRIDOR_CAMERAS;
        for camera in there.clone().chain(there.rev()) {
            for _ in 0..4 {
                self.frame(camera, &[camera]);
            }
        }
    }

    fn stats(&self) -> &PoolStats {
        &self.driver.mirror().stats
    }

    fn growths(&self) -> PerPool<u64> {
        PerPool::from_fn(|kind| self.stats().pools[kind].growths)
    }
}

// Never growing within the bound, through the harness: the model's `walk_within_bound_never_grows` holds
// on the real controller, issuer and mirror, in both pools. One layer short of
// the bound, that pool alone grows, so the walk does press on its bound.
#[test]
fn walk_harness_at_the_bound_with_no_margin_never_grows() {
    let mut at_the_bound = MaskedCorridor::new(CORRIDOR_BOUND);
    at_the_bound.walk();
    assert_eq!(
        at_the_bound.growths(),
        PerPool {
            lightmap: 0,
            mask: 0
        }
    );
    assert_eq!(at_the_bound.stats().deferred, 0);
    for kind in PoolKind::ALL {
        assert_eq!(
            at_the_bound.stats().pools[kind].peak_occupied_layers,
            CORRIDOR_BOUND[kind],
            "the walk fills the {} pool",
            kind.name()
        );
    }

    let short = PerPool {
        lightmap: CORRIDOR_BOUND.lightmap - 1,
        ..CORRIDOR_BOUND
    };
    let mut short_of_it = MaskedCorridor::new(short);
    short_of_it.walk();
    assert_eq!(
        short_of_it.growths(),
        PerPool {
            lightmap: 1,
            mask: 0
        }
    );
}

// A mask growth blocks later mask growth until its own release, and the
// lightmap pool neither grows nor retires for it.
#[test]
fn a_mask_growth_blocks_only_mask_growth_until_its_own_release() {
    let mut corridor = MaskedCorridor::new(PerPool {
        lightmap: 8,
        mask: 1,
    });
    let first = corridor.frame(0, &[0]);
    assert!(
        first.grew,
        "camera 0's six planes grow a one-layer mask pool"
    );
    assert_eq!(
        corridor.growths(),
        PerPool {
            lightmap: 0,
            mask: 1
        }
    );

    // Camera 3's ten planes pass the eight the growth reserved.
    let second = corridor.frame(3, &[3, 4, 5]);
    assert!(second.retiring_at_start && !second.grew);
    assert_eq!(second.deferred, 1, "block 5 waits on the mask pool");

    let mut frames = 0;
    while corridor.growths().mask < 2 {
        let frame = corridor.frame(3, &[3, 4, 5]);
        frames += 1;
        assert!(frames < 4, "the mask pool's release lets it grow again");
        assert_eq!(frame.grew, corridor.growths().mask == 2);
    }
    assert_eq!(corridor.growths().lightmap, 0);
    assert_eq!(corridor.stats().pools.lightmap.peak_layers, 8);
}

// The other way: the lightmap pool's retiring generation holds back a pair
// that needs lightmap growth, and not the mask growth of a pair with lightmap
// room.
#[test]
fn a_lightmap_generation_retiring_leaves_the_mask_pool_free_to_grow() {
    let mut corridor = MaskedCorridor::new(PerPool {
        lightmap: 1,
        mask: 6,
    });
    corridor.frame(0, &[0]);
    assert_eq!(
        corridor.growths(),
        PerPool {
            lightmap: 1,
            mask: 0
        }
    );

    // Block 0 leaves. Blocks 3 and 4 take the lightmap room, and need a mask
    // layer past the six, while block 5 has no lightmap room to land in.
    let second = corridor.frame(3, &[3, 4, 5]);
    assert!(second.retiring_at_start);
    assert_eq!(
        corridor.growths(),
        PerPool {
            lightmap: 1,
            mask: 1
        },
        "the mask pool grew behind the lightmap pool's retiring generation"
    );
    assert_eq!(second.deferred, 1, "block 5 waits on the lightmap pool");
}

// Copy and repack stats are per pool: a tight mask pool repacks and compacts
// while a roomy lightmap pool never does, and only the mask pool's figures move.
#[test]
fn repacks_count_against_the_pool_that_ran_them() {
    const HEIGHTS: [u16; 7] = [12, 52, 16, 40, 20, 48, 12];
    let blocks = corridor_blocks(64, true)
        .into_iter()
        .zip(HEIGHTS)
        .map(|(block, height)| BlockSpec {
            height,
            planes: [true, false],
            ..block
        })
        .collect();
    let mut corridor = MaskedCorridor::over(
        blocks,
        PerPool {
            lightmap: 8,
            mask: 3,
        },
    );
    corridor.walk();
    let pools = &corridor.stats().pools;
    assert_eq!(
        (pools.lightmap.repacks, pools.lightmap.repack_copies),
        (0, 0)
    );
    assert_eq!(pools.lightmap.compactions, 0);
    assert_eq!((pools.mask.repacks, pools.mask.compactions), (2, 1));
    assert!(pools.mask.compaction_copies > 0, "{:?}", pools.mask);
    assert!(pools.mask.repack_copies >= pools.mask.compaction_copies);
    assert_eq!(pools.mask.growths, 0, "the pool held the walk by repacking");
}
