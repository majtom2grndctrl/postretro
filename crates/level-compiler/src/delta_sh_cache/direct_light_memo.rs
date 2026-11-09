//! The direct delta bakes' per-light memo (ids 41 and 45): one cache entry
//! per light holding every CSR sub-block it reaches, keyed on the occluding
//! world, the probe grid and the light as its section bakes it.
//! See: context/lib/build_pipeline.md §Build Cache

use std::io::{self, Write};

use postretro_level_format::delta_sh_volumes::PROBES_PER_CELL;
use postretro_level_format::octahedral::{
    DEFAULT_IRRADIANCE_TILE_BORDER, DEFAULT_IRRADIANCE_TILE_DIMENSION,
};
use rayon::prelude::*;

use super::{DeltaShCacheTally, DeltaShCachedSubblocks, bake_delta_subblocks_uncached};
use crate::bake_content_keys::{fold_occluding_geometry, fold_probe_grid_layout};
use crate::bake_control::BakeControl;
use crate::cache::{CacheKey, StageCache};
use crate::direct_sh_bake::DIRECT_SH_STAGE_VERSION;
use crate::geometry::GeometryResult;
use crate::lightmap_bake::{DEFAULT_AREA_SAMPLE_COUNT, fold_soft_visibility_constants};
use crate::map_data::MapLight;
use crate::sh_bake::ProbeGridLayout;
use crate::texel_footprint::ProbeEarlyOut;

/// One direct delta section's memo stage. The two sections share a payload
/// shape, so their ids must differ.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct DirectDeltaStage {
    pub(crate) id: &'static str,
    /// Bump when that section's sub-block bake or this memo's payload
    /// changes; the tracer epoch, the soft-visibility constants, the sample
    /// count and id 35's stage version (the probe-direct radiance both
    /// share) fold on their own.
    pub(crate) version: u32,
}

/// One light as its section bakes it. The CSR position is never keyed: the
/// payload holds only the light's own sub-blocks, so a selection or slot
/// change elsewhere leaves this light's entry valid.
pub(crate) struct DirectDeltaLight<'a> {
    /// Exactly what the sub-block bake reads: id 41's authored light, id
    /// 45's unit-radiance copy.
    pub(crate) light: &'a MapLight,
    /// The global light index the soft-visibility seed reads.
    pub(crate) seed_axis: u64,
    pub(crate) early_out: ProbeEarlyOut,
}

/// The per-light memo over one stage cache. Its prefix holds what every
/// light's key in one bake shares: the occluding world (vertex positions and
/// the index buffer, so a retexture hits), the probe grid with its validity,
/// the affinity grid, the tracer epoch, the soft-visibility constants, the
/// probe sample count, id 35's stage version and the tile shape.
pub(crate) struct DirectDeltaMemo<'c> {
    cache: &'c StageCache,
    stage: DirectDeltaStage,
    prefix: blake3::Hasher,
}

impl<'c> DirectDeltaMemo<'c> {
    pub(crate) fn new(
        cache: &'c StageCache,
        stage: DirectDeltaStage,
        geometry: &GeometryResult,
        layout: &ProbeGridLayout,
        affinity_dims: [u32; 3],
    ) -> Self {
        let mut prefix = blake3::Hasher::new();
        prefix.update(&crate::ray_traversal::TRACER_EPOCH.to_le_bytes());
        fold_soft_visibility_constants(&mut prefix);
        prefix.update(&DEFAULT_AREA_SAMPLE_COUNT.to_le_bytes());
        prefix.update(&DIRECT_SH_STAGE_VERSION.to_le_bytes());
        prefix.update(&DEFAULT_IRRADIANCE_TILE_DIMENSION.to_le_bytes());
        prefix.update(&DEFAULT_IRRADIANCE_TILE_BORDER.to_le_bytes());
        prefix.update(&(PROBES_PER_CELL as u32).to_le_bytes());
        fold_occluding_geometry(&mut prefix, geometry);
        fold_probe_grid_layout(&mut prefix, layout);
        for dimension in affinity_dims {
            prefix.update(&dimension.to_le_bytes());
        }
        Self {
            cache,
            stage,
            prefix,
        }
    }

    /// The prefix, then the light's seed axis, early-out and encoded light,
    /// then the affinity cells it reaches in CSR order. The cells are the
    /// reach cull's answer, so the reach inputs (BSP, portals, policy) need
    /// no fold of their own.
    pub(crate) fn key(&self, light: &DirectDeltaLight<'_>, cells: &[u32]) -> CacheKey {
        let mut hasher = self.prefix.clone();
        hasher.update(&light.seed_axis.to_le_bytes());
        hasher.update(&[light.early_out.key_byte()]);
        let encoded = super::encoded_key_light(light.light);
        hasher.update(&(encoded.len() as u64).to_le_bytes());
        hasher.update(&encoded);
        hasher.update(&(cells.len() as u64).to_le_bytes());
        for cell in cells {
            hasher.update(&cell.to_le_bytes());
        }
        CacheKey::new(
            self.stage.id,
            self.stage.version,
            hasher.finalize().as_bytes(),
        )
    }

    /// Store a light's sub-blocks as little-endian halves, streamed so no
    /// second copy of the payload is held. Runs outside any governor permit.
    fn put(&self, key: &CacheKey, halves: &[u16]) {
        self.cache
            .put_streamed(key, (halves.len() * 2) as u64, |writer| {
                write_halves(writer, halves)
            });
    }

    /// One line per build: `hit` when every light hit, else `miss` with the
    /// share that re-bakes.
    fn log_outcome(&self, probed: usize, hits: usize) {
        if probed == 0 {
            return;
        }
        if hits == probed {
            log::info!("[cache] {} hit ({probed} light(s))", self.stage.id);
        } else {
            log::info!(
                "[cache] {} miss ({} of {probed} light(s) re-bake)",
                self.stage.id,
                probed - hits
            );
        }
    }
}

fn write_halves(writer: &mut dyn Write, halves: &[u16]) -> io::Result<()> {
    const CHUNK_HALVES: usize = 2048;
    let mut buffer = [0u8; CHUNK_HALVES * 2];
    for chunk in halves.chunks(CHUNK_HALVES) {
        for (bytes, half) in buffer.as_chunks_mut::<2>().0.iter_mut().zip(chunk) {
            *bytes = half.to_le_bytes();
        }
        writer.write_all(&buffer[..chunk.len() * 2])?;
    }
    Ok(())
}

/// One light's sub-blocks, in its cells' CSR order.
enum LightPayload {
    /// Read from the cache: little-endian halves, decoded as they are
    /// copied out, so a hit holds one copy.
    Cached(Vec<u8>),
    Baked(Vec<u16>),
}

impl LightPayload {
    fn copy_subblock(&self, subblock: usize, len: usize, out: &mut Vec<u16>) {
        let start = subblock * len;
        match self {
            Self::Baked(halves) => out.extend_from_slice(&halves[start..start + len]),
            Self::Cached(bytes) => out.extend(
                bytes[start * 2..(start + len) * 2]
                    .as_chunks::<2>()
                    .0
                    .iter()
                    .map(|&half| u16::from_le_bytes(half)),
            ),
        }
    }
}

/// Bake or load every CSR entry of a direct delta section, preserving the
/// input's entry order. `lights` is indexed by the CSR light index; the
/// closure receives `(light_index, cell)` and returns one dense block of
/// `expected_subblock_f16_len` halves.
///
/// With no memo, this is [`bake_delta_subblocks_uncached`]. With one, each
/// light that reaches a cell is probed once: a hit serves all its entries
/// and traces nothing, a miss bakes all of them (one permit and one progress
/// step per entry) and stores them as one entry. The tally counts entries,
/// as the indirect bake's does. Resident memory matches the uncached bake:
/// one copy of each light's sub-blocks, then the assembled section.
pub(crate) fn bake_or_load_direct_delta_subblocks<F>(
    memo: Option<&DirectDeltaMemo<'_>>,
    lights: &[DirectDeltaLight<'_>],
    affinity_lights: &[u32],
    csr_entry_cells: &[u32],
    expected_subblock_f16_len: usize,
    control: &BakeControl,
    bake_subblock: F,
) -> DeltaShCachedSubblocks
where
    F: Fn(u32, u32) -> Vec<u16> + Sync,
{
    let Some(memo) = memo else {
        return bake_delta_subblocks_uncached(
            affinity_lights,
            csr_entry_cells,
            expected_subblock_f16_len,
            control,
            bake_subblock,
        );
    };
    assert_eq!(
        affinity_lights.len(),
        csr_entry_cells.len(),
        "direct delta CSR lights and cells must stay entry-parallel"
    );

    // The CSR walks cells in ascending order, so each light's cells come out
    // ascending: the order its payload stores them in.
    let mut light_cells: Vec<Vec<u32>> = vec![Vec::new(); lights.len()];
    for (&light_index, &cell) in affinity_lights.iter().zip(csr_entry_cells) {
        light_cells
            .get_mut(light_index as usize)
            .expect("direct delta CSR light index must be in the keyed light table")
            .push(cell);
    }

    let len = expected_subblock_f16_len;
    let payloads: Vec<Option<(LightPayload, bool)>> = light_cells
        .par_iter()
        .enumerate()
        .map(|(light_index, cells)| {
            if cells.is_empty() {
                return None;
            }
            let key = memo.key(&lights[light_index], cells);
            if let Some(bytes) = memo.cache.get(&key) {
                if bytes.len() == cells.len() * len * 2 {
                    // A whole-light hit checkpoints without a bake permit.
                    control.governor().checkpoint();
                    control.advance(cells.len());
                    return Some((LightPayload::Cached(bytes), true));
                }
                log::warn!("[cache] corrupt {} entry, re-baking", memo.stage.id);
            }
            let mut halves = vec![0u16; cells.len() * len];
            halves
                .par_chunks_mut(len)
                .zip(cells.par_iter())
                .for_each(|(slot, &cell)| {
                    // Permit guards ray work, not cache I/O.
                    let subblock = {
                        let _permit = control.governor().enter();
                        bake_subblock(light_index as u32, cell)
                    };
                    assert_eq!(
                        subblock.len(),
                        len,
                        "delta sub-block bake must produce the stage's fixed dense payload length"
                    );
                    slot.copy_from_slice(&subblock);
                    control.advance(1);
                });
            memo.put(&key, &halves);
            Some((LightPayload::Baked(halves), false))
        })
        .collect();

    let probed = payloads.iter().flatten().count();
    let hit_lights = payloads.iter().flatten().filter(|(_, hit)| *hit).count();
    memo.log_outcome(probed, hit_lights);

    let mut subblocks = Vec::with_capacity(affinity_lights.len() * len);
    let mut next_subblock = vec![0usize; lights.len()];
    let mut tally = DeltaShCacheTally::default();
    for &light_index in affinity_lights {
        let (payload, hit) = payloads[light_index as usize]
            .as_ref()
            .expect("every CSR light was probed");
        let subblock = &mut next_subblock[light_index as usize];
        payload.copy_subblock(*subblock, len, &mut subblocks);
        *subblock += 1;
        if *hit {
            tally.hits += 1;
        } else {
            tally.misses += 1;
        }
    }

    DeltaShCachedSubblocks { subblocks, tally }
}
