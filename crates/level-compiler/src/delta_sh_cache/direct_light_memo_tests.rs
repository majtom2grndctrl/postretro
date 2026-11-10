// The direct delta bakes' per-light memo: key contents, entry order, and
// corrupt-entry handling, over a synthetic grid and closure bake.
// See: context/lib/build_pipeline.md §Build Cache

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use glam::DVec3;
use postretro_level_format::geometry::{FaceMeta, GeometrySection, Vertex};
use postretro_level_format::texture_names::TextureNamesSection;

use super::direct_light_memo::{
    DirectDeltaLight, DirectDeltaMemo, DirectDeltaStage, bake_or_load_direct_delta_subblocks,
};
use super::{DeltaShCacheTally, bake_delta_subblocks_uncached};
use crate::bake_control::BakeControl;
use crate::cache::StageCache;
use crate::geometry::{FaceIndexRange, GeometryResult};
use crate::governor::Governor;
use crate::map_data::{FalloffModel, LightType, MapLight, ShadowType};
use crate::reporter::StageProgress;
use crate::sh_bake::ProbeGridLayout;
use crate::texel_footprint::ProbeEarlyOut;

const STAGE: DirectDeltaStage = DirectDeltaStage {
    id: "direct_light_memo_test",
    version: 1,
};

fn geometry() -> GeometryResult {
    let vertex = |position| {
        Vertex::new(
            position,
            [0.0, 0.0],
            [0.0, 1.0, 0.0],
            [1.0, 0.0, 0.0],
            true,
            [0.0, 0.0],
            0,
        )
    };
    GeometryResult {
        geometry: GeometrySection {
            vertices: vec![
                vertex([0.0, 0.0, 0.0]),
                vertex([1.0, 0.0, 0.0]),
                vertex([0.0, 0.0, 1.0]),
            ],
            indices: vec![0, 1, 2],
            faces: vec![FaceMeta {
                leaf_index: 0,
                texture_index: 0,
            }],
        },
        texture_names: TextureNamesSection {
            names: vec!["floor".to_owned()],
        },
        face_index_ranges: vec![FaceIndexRange {
            index_offset: 0,
            index_count: 3,
        }],
    }
}

/// Five probes along x over two affinity cells: cell 0 holds probes 0–3
/// (probe 2 invalid), cell 1 holds probe 4.
fn layout() -> ProbeGridLayout {
    ProbeGridLayout {
        world_min: DVec3::ZERO,
        dims: [5, 1, 1],
        cell_size: [1.0; 3],
        far_sentinel: 8.0,
        probe_positions: (0..5).map(|x| DVec3::X * f64::from(x)).collect(),
        validity: vec![1, 1, 0, 1, 1],
    }
}

const AFFINITY_DIMS: [u32; 3] = [2, 1, 1];
/// Halves per probe tile in the synthetic sub-blocks.
const STRIDE: usize = 2;
const SUBBLOCK: usize = 64 * STRIDE;

/// Whether sub-block slot `slot` of `cell` is a valid probe in [`layout`].
fn valid_slot(cell: u32, slot: usize) -> bool {
    match cell {
        0 => matches!(slot, 0 | 1 | 3),
        _ => slot == 0,
    }
}

/// The synthetic bake of one sub-block: each valid slot names its light,
/// cell and slot; every other slot holds `invalid` (zero, as the real bakes
/// write).
fn subblock(light: u32, cell: u32, invalid: u16) -> Vec<u16> {
    (0..64)
        .flat_map(|slot| {
            if valid_slot(cell, slot) {
                [light as u16 * 100 + cell as u16, slot as u16 + 7]
            } else {
                [invalid; STRIDE]
            }
        })
        .collect()
}

fn light(origin_x: f64) -> MapLight {
    MapLight {
        origin: DVec3::new(origin_x, 1.0, 0.0),
        light_type: LightType::Point,
        carrier: String::new(),
        intensity: 1.0,
        color: [1.0; 3],
        falloff_model: FalloffModel::Linear,
        falloff_range: 8.0,
        light_size: 0.0,
        angular_diameter: 0.0,
        cone_angle_inner: None,
        cone_angle_outer: None,
        cone_direction: None,
        animation: None,
        bake_only: false,
        is_dynamic: false,
        casts_entity_shadows: false,
        is_animated: false,
        tags: Vec::new(),
        shadow_type: ShadowType::StaticLightMap,
    }
}

fn memo_light(light: &MapLight, seed_axis: u64) -> DirectDeltaLight<'_> {
    DirectDeltaLight {
        light,
        seed_axis,
        early_out: ProbeEarlyOut::Agreed,
    }
}

fn control(progress: &StageProgress) -> BakeControl {
    BakeControl::new(Arc::new(Governor::new(2, false)), progress)
}

fn key_of(
    cache: &StageCache,
    stage: DirectDeltaStage,
    geometry: &GeometryResult,
    layout: &ProbeGridLayout,
    light: &DirectDeltaLight<'_>,
    cells: &[u32],
) -> String {
    DirectDeltaMemo::new(cache, stage, geometry, layout, [1, 1, 1])
        .key(light, cells)
        .as_filename()
}

#[test]
fn the_key_folds_what_the_bake_reads_and_not_texture() {
    let dir = tempfile::tempdir().expect("cache dir");
    let cache = StageCache::new(dir.path()).expect("cache");
    let (geometry, layout, base_light) = (geometry(), layout(), light(0.0));
    let key =
        |stage: DirectDeltaStage,
         geometry: &GeometryResult,
         layout: &ProbeGridLayout,
         light: &DirectDeltaLight<'_>,
         cells: &[u32]| { key_of(&cache, stage, geometry, layout, light, cells) };
    let base = key(STAGE, &geometry, &layout, &memo_light(&base_light, 3), &[0]);

    let mut retextured = geometry.clone();
    retextured.geometry.faces[0].texture_index = 1;
    retextured.texture_names.names.push("wall".to_owned());
    assert_eq!(
        key(
            STAGE,
            &retextured,
            &layout,
            &memo_light(&base_light, 3),
            &[0]
        ),
        base,
        "a retexture keeps the key"
    );

    let mut moved = geometry.clone();
    moved.geometry.vertices[2].position[1] = 0.5;
    let mut invalid = self::layout();
    invalid.validity[1] = 0;
    let recoloured = MapLight {
        color: [1.0, 0.5, 0.5],
        ..base_light.clone()
    };
    let bumped = DirectDeltaStage {
        version: STAGE.version + 1,
        ..STAGE
    };
    let embedded = DirectDeltaLight {
        early_out: ProbeEarlyOut::Off,
        ..memo_light(&base_light, 3)
    };
    for (what, other) in [
        (
            "an occluder edit",
            key(STAGE, &moved, &layout, &memo_light(&base_light, 3), &[0]),
        ),
        (
            "a probe validity edit",
            key(
                STAGE,
                &geometry,
                &invalid,
                &memo_light(&base_light, 3),
                &[0],
            ),
        ),
        (
            "a light edit",
            key(STAGE, &geometry, &layout, &memo_light(&recoloured, 3), &[0]),
        ),
        (
            "a seed shift",
            key(STAGE, &geometry, &layout, &memo_light(&base_light, 4), &[0]),
        ),
        (
            "an embedded-emitter flag",
            key(STAGE, &geometry, &layout, &embedded, &[0]),
        ),
        (
            "a reach edit",
            key(
                STAGE,
                &geometry,
                &layout,
                &memo_light(&base_light, 3),
                &[0, 1],
            ),
        ),
        (
            "a stage version bump",
            key(
                bumped,
                &geometry,
                &layout,
                &memo_light(&base_light, 3),
                &[0],
            ),
        ),
    ] {
        assert_ne!(other, base, "{what} re-keys the light");
    }
}

/// Two lights over three cells; light 1 reaches two. The closure's block
/// names its light and cell, so order is visible in the output.
const LIGHTS: [u32; 3] = [1, 0, 1];
const CELLS: [u32; 3] = [0, 1, 1];

fn bake_with(
    cache: &StageCache,
    lights: &[MapLight],
    calls: &AtomicUsize,
    progress: &StageProgress,
) -> super::DeltaShCachedSubblocks {
    bake_with_invalid(cache, lights, calls, progress, 0)
}

fn bake_with_invalid(
    cache: &StageCache,
    lights: &[MapLight],
    calls: &AtomicUsize,
    progress: &StageProgress,
    invalid: u16,
) -> super::DeltaShCachedSubblocks {
    let (geometry, layout) = (geometry(), layout());
    let memo = DirectDeltaMemo::new(cache, STAGE, &geometry, &layout, AFFINITY_DIMS);
    let memo_lights: Vec<_> = lights
        .iter()
        .enumerate()
        .map(|(index, light)| memo_light(light, index as u64))
        .collect();
    bake_or_load_direct_delta_subblocks(
        Some(&memo),
        &memo_lights,
        &LIGHTS,
        &CELLS,
        SUBBLOCK,
        &control(progress),
        |light, cell| {
            calls.fetch_add(1, Ordering::Relaxed);
            // Light 0's invalid slots take `invalid`; light 1's stay zero.
            subblock(light, cell, if light == 0 { invalid } else { 0 })
        },
    )
}

#[test]
fn warm_serves_cold_bytes_in_csr_order_and_counts_entries() {
    let dir = tempfile::tempdir().expect("cache dir");
    let cache = StageCache::new(dir.path()).expect("cache");
    let mut lights = vec![light(0.0), light(1.0)];
    let calls = AtomicUsize::new(0);

    let cold_progress = StageProgress::indeterminate();
    let cold = bake_delta_subblocks_uncached(
        &LIGHTS,
        &CELLS,
        SUBBLOCK,
        &control(&cold_progress),
        |l, c| subblock(l, c, 0),
    );
    let expected: Vec<u16> = [(1, 0), (0, 1), (1, 1)]
        .into_iter()
        .flat_map(|(light, cell)| subblock(light, cell, 0))
        .collect();
    assert_eq!(cold.subblocks, expected);

    let progress = StageProgress::indeterminate();
    let first = bake_with(&cache, &lights, &calls, &progress);
    assert_eq!(first.subblocks, cold.subblocks);
    assert_eq!(first.tally, DeltaShCacheTally { hits: 0, misses: 3 });
    assert_eq!(progress.completed(), 3);

    let progress = StageProgress::indeterminate();
    let warm = bake_with(&cache, &lights, &calls, &progress);
    assert_eq!(warm.subblocks, cold.subblocks);
    assert_eq!(warm.tally, DeltaShCacheTally { hits: 3, misses: 0 });
    assert_eq!(
        progress.completed(),
        3,
        "a hit advances every entry it serves"
    );
    assert_eq!(
        calls.load(Ordering::Relaxed),
        3,
        "a warm build bakes nothing"
    );

    lights[1].falloff_range = 6.0;
    let edited = bake_with(&cache, &lights, &calls, &progress);
    assert_eq!(
        edited.tally,
        DeltaShCacheTally { hits: 1, misses: 2 },
        "only the edited light's two entries re-bake"
    );
    assert_eq!(calls.load(Ordering::Relaxed), 5);
}

#[test]
fn a_corrupt_entry_rebakes_that_light() {
    let dir = tempfile::tempdir().expect("cache dir");
    let cache = StageCache::new(dir.path()).expect("cache");
    let lights = vec![light(0.0), light(1.0)];
    let calls = AtomicUsize::new(0);
    let progress = StageProgress::indeterminate();
    let first = bake_with(&cache, &lights, &calls, &progress);

    let (geometry, layout) = (geometry(), layout());
    let memo = DirectDeltaMemo::new(&cache, STAGE, &geometry, &layout, AFFINITY_DIMS);
    cache.put(&memo.key(&memo_light(&lights[1], 1), &[0, 1]), &[1, 2, 3]);
    let rebuilt = bake_with(&cache, &lights, &calls, &progress);
    assert_eq!(rebuilt.subblocks, first.subblocks);
    assert_eq!(rebuilt.tally, DeltaShCacheTally { hits: 1, misses: 2 });
}

// A light's entry holds its tiles at valid probes only, so an
// invalid probe's zero tile costs nothing. Light 1 reaches cell 0 (three
// valid probes of four in the grid) and cell 1 (one): four tiles of two
// halves, 16 bytes, where the dense sub-blocks would be 512.
#[test]
fn an_entry_holds_tiles_at_valid_probes_only() {
    let dir = tempfile::tempdir().expect("cache dir");
    let cache = StageCache::new(dir.path()).expect("cache");
    let lights = vec![light(0.0), light(1.0)];
    let calls = AtomicUsize::new(0);
    bake_with(&cache, &lights, &calls, &StageProgress::indeterminate());
    let (geometry, layout) = (geometry(), layout());
    let memo = DirectDeltaMemo::new(&cache, STAGE, &geometry, &layout, AFFINITY_DIMS);
    let entry = cache
        .get(&memo.key(&memo_light(&lights[1], 1), &[0, 1]))
        .expect("light 1 is memoized");
    assert_eq!(entry.len(), 4 * STRIDE * 2);
    let entry = cache
        .get(&memo.key(&memo_light(&lights[0], 0), &[1]))
        .expect("light 0 is memoized");
    assert_eq!(entry.len(), STRIDE * 2);
}

// A sub-block holding a non-zero half at an invalid probe could not be
// restored from valid probes alone, so its light is never memoized: it
// re-bakes warm, and the output still equals cold.
#[test]
fn a_light_with_a_non_zero_invalid_tile_is_not_memoized() {
    let dir = tempfile::tempdir().expect("cache dir");
    let cache = StageCache::new(dir.path()).expect("cache");
    let lights = vec![light(0.0), light(1.0)];
    let calls = AtomicUsize::new(0);
    let progress = StageProgress::indeterminate();
    let cold = bake_with_invalid(&cache, &lights, &calls, &progress, 9);
    let warm = bake_with_invalid(&cache, &lights, &calls, &progress, 9);
    assert_eq!(warm.subblocks, cold.subblocks);
    assert_eq!(
        warm.tally,
        DeltaShCacheTally { hits: 2, misses: 1 },
        "light 0's one entry re-bakes; light 1's two hit"
    );
}
