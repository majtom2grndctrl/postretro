//! Shared per-entry cache for sparse delta-SH bakes.
//!
//! Each delta section uses the same CSR shape: one independently-baked dense
//! sub-block for every `(affinity cell, light)` entry. This module owns the
//! cache boundary around those sub-blocks; callers still assemble and process
//! their sections after this raw pre-drop payload is returned.

use rayon::prelude::*;

use crate::bake_control::BakeControl;
use crate::cache::{CacheKey, StageCache};
use crate::map_data::MapLight;

/// Cache hit/miss counts at the delta bake's `(cell, light)` granularity.
///
/// A cache-disabled bake deliberately reports every entry as a miss: it has
/// neither read nor written the stage cache.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct DeltaShCacheTally {
    pub(crate) hits: usize,
    pub(crate) misses: usize,
}

/// Raw dense sub-block payloads in the input CSR-entry order, plus their cache
/// accounting. The caller owns section reassembly and every downstream delta
/// pass (drop, coarsening, compaction, and payload cap).
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct DeltaShCachedSubblocks {
    pub(crate) subblocks: Vec<u16>,
    pub(crate) tally: DeltaShCacheTally,
}

/// The cache-key inputs shared by all three delta-SH bakes.
pub(crate) struct DeltaShCacheInputs<'a> {
    pub(crate) stage_id: &'a str,
    pub(crate) stage_version: u32,
    pub(crate) geometry_hash: [u8; 32],
    pub(crate) affinity_dims: [u32; 3],
    pub(crate) affinity_lights: &'a [u32],
    pub(crate) csr_entry_cells: &'a [u32],
    pub(crate) valid_probe_masks: &'a [u64],
    pub(crate) probe_spacing: f32,
    /// Seed axis by keyed light index. Indirect delta supplies zero because its
    /// stable cell/probe position is already part of the folded coordinates.
    pub(crate) light_seed_axes: &'a [u64],
    pub(crate) expected_subblock_f16_len: usize,
}

/// Build one `(affinity cell, light)` key.
///
/// This is `pub(crate)` so every delta bake and its version-contract tests use
/// one explicit fold order. The stage id/version live in [`CacheKey`] itself;
/// callers must provide distinct ids because the raw f16 payload shapes match
/// across the three delta sections.
pub(crate) fn delta_sh_entry_cache_key(
    stage_id: &str,
    stage_version: u32,
    geometry_hash: &[u8; 32],
    affinity_dims: [u32; 3],
    cell: u32,
    probe_spacing: f32,
    valid_probe_mask: u64,
    seed_axis: u64,
    light: &MapLight,
) -> CacheKey {
    assert!(
        affinity_dims.iter().all(|&dimension| dimension > 0),
        "delta cache requires non-zero affinity dimensions"
    );

    let (cell_x, cell_y, cell_z) = affinity_cell_coord(cell, affinity_dims);
    let encoded_light = postcard::to_allocvec(light).expect("postcard serialize delta MapLight");
    let light_len = u64::try_from(encoded_light.len()).expect("MapLight encoding length fits u64");

    let mut hasher = blake3::Hasher::new();
    hasher.update(geometry_hash);
    for dimension in affinity_dims {
        hasher.update(&dimension.to_le_bytes());
    }
    for coordinate in [cell_x, cell_y, cell_z] {
        hasher.update(&coordinate.to_le_bytes());
    }
    hasher.update(&probe_spacing.to_le_bytes());
    hasher.update(&valid_probe_mask.to_le_bytes());
    hasher.update(&seed_axis.to_le_bytes());
    hasher.update(&light_len.to_le_bytes());
    hasher.update(&encoded_light);

    CacheKey::new(stage_id, stage_version, hasher.finalize().as_bytes())
}

/// Bake or load every CSR entry while preserving the input's exact entry order.
///
/// `key_lights` and `light_seed_axes` are indexed by `affinity_lights`:
/// unitizing callers pass normalized unit-radiance copies and a zero seed axis,
/// while direct callers pass authored lights and their source/static indices.
/// The closure receives `(light_index, cell)` and returns one dense f16 block.
pub(crate) fn bake_or_load_delta_subblocks<F>(
    inputs: &DeltaShCacheInputs<'_>,
    key_lights: &[MapLight],
    cache: Option<&StageCache>,
    control: &BakeControl,
    bake_subblock: F,
) -> DeltaShCachedSubblocks
where
    F: Fn(u32, u32) -> Vec<u16> + Sync,
{
    assert_eq!(
        inputs.affinity_lights.len(),
        inputs.csr_entry_cells.len(),
        "delta cache CSR lights and cells must stay entry-parallel"
    );
    assert_eq!(
        key_lights.len(),
        inputs.light_seed_axes.len(),
        "delta cache keyed lights and seed axes must stay index-parallel"
    );

    let entries: Vec<(Vec<u16>, bool)> = inputs
        .affinity_lights
        .par_iter()
        .zip(inputs.csr_entry_cells.par_iter())
        .map(|(&light_index, &cell)| {
            let light = key_lights
                .get(light_index as usize)
                .expect("delta cache light index must be in the keyed light table");
            let seed_axis = *inputs
                .light_seed_axes
                .get(light_index as usize)
                .expect("delta cache light index must have a seed axis");
            let valid_probe_mask = *inputs
                .valid_probe_masks
                .get(cell as usize)
                .expect("delta cache cell must have a probe-validity mask");
            let key = delta_sh_entry_cache_key(
                inputs.stage_id,
                inputs.stage_version,
                &inputs.geometry_hash,
                inputs.affinity_dims,
                cell,
                inputs.probe_spacing,
                valid_probe_mask,
                seed_axis,
                light,
            );

            let loaded = cache.and_then(|stage_cache| {
                stage_cache
                    .get(&key)
                    .and_then(|bytes| decode_subblock(&bytes, inputs.expected_subblock_f16_len))
            });
            let (subblock, hit) = match loaded {
                Some(subblock) => {
                    // A whole warm rebuild can be all hits. Preserve its pause
                    // responsiveness without consuming a bake-work permit.
                    control.governor().checkpoint();
                    (subblock, true)
                }
                None => {
                    // Keep the permit around ray work only. Cache I/O happens
                    // outside it, and a disabled cache bypasses I/O entirely.
                    let subblock = {
                        let _permit = control.governor().enter();
                        bake_subblock(light_index, cell)
                    };
                    assert_eq!(
                        subblock.len(),
                        inputs.expected_subblock_f16_len,
                        "delta sub-block bake must produce the stage's fixed dense payload length"
                    );
                    if let Some(stage_cache) = cache {
                        stage_cache.put(&key, &encode_subblock(&subblock));
                    }
                    (subblock, false)
                }
            };
            control.advance(1);
            (subblock, hit)
        })
        .collect();

    let tally = DeltaShCacheTally {
        hits: entries.iter().filter(|(_, hit)| *hit).count(),
        misses: entries.iter().filter(|(_, hit)| !*hit).count(),
    };
    let subblocks = entries
        .into_iter()
        .flat_map(|(subblock, _)| subblock)
        .collect();

    DeltaShCachedSubblocks { subblocks, tally }
}

fn affinity_cell_coord(cell: u32, affinity_dims: [u32; 3]) -> (u32, u32, u32) {
    let cell_x = cell % affinity_dims[0];
    let cell_y = (cell / affinity_dims[0]) % affinity_dims[1];
    let cell_z = cell / (affinity_dims[0] * affinity_dims[1]);
    (cell_x, cell_y, cell_z)
}

fn encode_subblock(subblock: &[u16]) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(subblock.len() * std::mem::size_of::<u16>());
    for value in subblock {
        bytes.extend_from_slice(&value.to_le_bytes());
    }
    bytes
}

fn decode_subblock(bytes: &[u8], expected_f16_len: usize) -> Option<Vec<u16>> {
    if bytes.len() != expected_f16_len * std::mem::size_of::<u16>() {
        return None;
    }
    Some(
        bytes
            .chunks_exact(std::mem::size_of::<u16>())
            .map(|chunk| u16::from_le_bytes([chunk[0], chunk[1]]))
            .collect(),
    )
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::PathBuf;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};

    use glam::DVec3;

    use super::*;
    use crate::governor::Governor;
    use crate::map_data::{FalloffModel, LightType, ShadowType};
    use crate::reporter::StageProgress;

    const TEST_STAGE_ID: &str = "delta_sh_cache_test";
    const TEST_STAGE_VERSION: u32 = 1;

    fn fresh_temp_dir(label: &str) -> PathBuf {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock after unix epoch")
            .as_nanos();
        std::env::temp_dir().join(format!(
            "postretro_delta_sh_cache_{label}_{stamp}_{}",
            std::process::id()
        ))
    }

    fn light(origin_x: f64) -> MapLight {
        MapLight {
            origin: DVec3::new(origin_x, 0.0, 0.0),
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

    fn inputs<'a>(
        lights: &'a [u32],
        cells: &'a [u32],
        seed_axes: &'a [u64],
    ) -> DeltaShCacheInputs<'a> {
        DeltaShCacheInputs {
            stage_id: TEST_STAGE_ID,
            stage_version: TEST_STAGE_VERSION,
            geometry_hash: [9; 32],
            affinity_dims: [2, 1, 1],
            affinity_lights: lights,
            csr_entry_cells: cells,
            valid_probe_masks: &[u64::MAX, u64::MAX],
            probe_spacing: 1.0,
            light_seed_axes: seed_axes,
            expected_subblock_f16_len: 2,
        }
    }

    fn control() -> BakeControl {
        let progress = StageProgress::indeterminate();
        BakeControl::new(Arc::new(Governor::new(2, false)), &progress)
    }

    #[test]
    fn cache_key_changes_when_stage_version_changes() {
        let light = light(0.0);
        let key_a = delta_sh_entry_cache_key(
            TEST_STAGE_ID,
            1,
            &[9; 32],
            [2, 1, 1],
            0,
            1.0,
            u64::MAX,
            0,
            &light,
        );
        let key_b = delta_sh_entry_cache_key(
            TEST_STAGE_ID,
            2,
            &[9; 32],
            [2, 1, 1],
            0,
            1.0,
            u64::MAX,
            0,
            &light,
        );

        assert_ne!(key_a.as_filename(), key_b.as_filename());
    }

    #[test]
    fn cache_is_per_entry_and_none_bakes_without_cache_io() {
        let dir = fresh_temp_dir("per_entry");
        let cache = StageCache::new(&dir).expect("create cache");
        let mut light_table = vec![light(0.0), light(4.0)];
        let lights = [0, 1];
        let cells = [0, 1];
        let seed_axes = [0, 0];
        let calls = AtomicUsize::new(0);

        let first = bake_or_load_delta_subblocks(
            &inputs(&lights, &cells, &seed_axes),
            &light_table,
            Some(&cache),
            &control(),
            |light_index, cell| {
                calls.fetch_add(1, Ordering::Relaxed);
                vec![light_index as u16, cell as u16]
            },
        );
        assert_eq!(first.tally, DeltaShCacheTally { hits: 0, misses: 2 });

        let second = bake_or_load_delta_subblocks(
            &inputs(&lights, &cells, &seed_axes),
            &light_table,
            Some(&cache),
            &control(),
            |light_index, cell| {
                calls.fetch_add(1, Ordering::Relaxed);
                vec![light_index as u16, cell as u16]
            },
        );
        assert_eq!(second.tally, DeltaShCacheTally { hits: 2, misses: 0 });
        assert_eq!(first.subblocks, second.subblocks);
        assert_eq!(calls.load(Ordering::Relaxed), 2);

        light_table[0].falloff_range = 12.0;
        let edited = bake_or_load_delta_subblocks(
            &inputs(&lights, &cells, &seed_axes),
            &light_table,
            Some(&cache),
            &control(),
            |light_index, cell| {
                calls.fetch_add(1, Ordering::Relaxed);
                vec![light_index as u16, cell as u16]
            },
        );
        assert_eq!(edited.tally, DeltaShCacheTally { hits: 1, misses: 1 });
        assert_eq!(calls.load(Ordering::Relaxed), 3);

        let uncached = bake_or_load_delta_subblocks(
            &inputs(&lights, &cells, &seed_axes),
            &light_table,
            None,
            &control(),
            |light_index, cell| {
                calls.fetch_add(1, Ordering::Relaxed);
                vec![light_index as u16, cell as u16]
            },
        );
        assert_eq!(uncached.tally, DeltaShCacheTally { hits: 0, misses: 2 });
        assert_eq!(calls.load(Ordering::Relaxed), 5);

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn corrupt_payload_rebakes_the_entry() {
        let dir = fresh_temp_dir("corrupt");
        let cache = StageCache::new(&dir).expect("create cache");
        let light_table = vec![light(0.0)];
        let lights = [0];
        let cells = [0];
        let seed_axes = [0];
        let calls = AtomicUsize::new(0);
        let cache_inputs = inputs(&lights, &cells, &seed_axes);

        let first = bake_or_load_delta_subblocks(
            &cache_inputs,
            &light_table,
            Some(&cache),
            &control(),
            |_, _| {
                calls.fetch_add(1, Ordering::Relaxed);
                vec![42, 42]
            },
        );
        assert_eq!(first.tally, DeltaShCacheTally { hits: 0, misses: 1 });

        let key = delta_sh_entry_cache_key(
            TEST_STAGE_ID,
            TEST_STAGE_VERSION,
            &[9; 32],
            [2, 1, 1],
            0,
            1.0,
            u64::MAX,
            seed_axes[0],
            &light_table[0],
        );
        cache.put(&key, &encode_subblock(&[7]));

        let rebuilt = bake_or_load_delta_subblocks(
            &cache_inputs,
            &light_table,
            Some(&cache),
            &control(),
            |_, _| {
                calls.fetch_add(1, Ordering::Relaxed);
                vec![42, 42]
            },
        );
        assert_eq!(rebuilt.tally, DeltaShCacheTally { hits: 0, misses: 1 });
        assert_eq!(calls.load(Ordering::Relaxed), 2);

        let _ = fs::remove_dir_all(&dir);
    }
}
