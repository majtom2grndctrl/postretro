// The direct half's per-light memo: its key, keyed on the occluding world
// alone, and its payload of one light's entries.
// See: context/lib/build_pipeline.md §Build Cache

use postretro_level_format::mover_probe_channel::{
    MOVER_IRRADIANCE_TILE_F16_COUNT, MOVER_VISIBILITY_BYTES_PER_PROBE,
};
use postretro_level_format::octahedral::{
    DEFAULT_IRRADIANCE_TILE_BORDER, DEFAULT_IRRADIANCE_TILE_DIMENSION,
};

use super::direct::{ChannelLight, DirectEntry, DirectScene, LightMover, ProbeCells};
use super::visibility::{INSIDE_BRUSH_RULE, InsideBrushRule};
use crate::bake_content_keys::{fold_occluding_geometry, fold_probe_grid_layout};
use crate::cache::{CacheKey, StageCache};
use crate::direct_sh_bake::DIRECT_SH_STAGE_VERSION;
use crate::lightmap_bake::{DEFAULT_AREA_SAMPLE_COUNT, fold_soft_visibility_constants};
use crate::mover_light_gate::{Terminus, gate_key_hasher};
use crate::partition::Aabb;

/// The memo's cache stage. Bump the version when the q bake, its own-tile
/// assembly or the payload changes; the gate epoch, tracer epoch, soft-
/// visibility constants and id 35's stage version fold on their own.
const STAGE_ID: &str = "mover_probe_direct";
/// 2: posed shadow segments meet a face within the static tracer's τ.
const STAGE_VERSION: u32 = 2;

/// What every light's key shares: the occluding world, the probe grid and
/// the reach decomposition's inputs. Only occlusion enters: vertex
/// positions, the index buffer, BSP solidity, exterior leaves and the
/// portal graph, never texture names or coordinates, so a retexture hits.
pub(crate) struct MemoPrefix(blake3::Hasher);

impl MemoPrefix {
    pub(crate) fn new(scene: &DirectScene<'_>) -> Self {
        let mut hasher = gate_key_hasher();
        hasher.update(&crate::ray_traversal::TRACER_EPOCH.to_le_bytes());
        fold_soft_visibility_constants(&mut hasher);
        hasher.update(&DEFAULT_AREA_SAMPLE_COUNT.to_le_bytes());
        hasher.update(&DIRECT_SH_STAGE_VERSION.to_le_bytes());
        hasher.update(&DEFAULT_IRRADIANCE_TILE_DIMENSION.to_le_bytes());
        hasher.update(&DEFAULT_IRRADIANCE_TILE_BORDER.to_le_bytes());
        hasher.update(&[match INSIDE_BRUSH_RULE {
            InsideBrushRule::SkipContainingBrush => 0,
            InsideBrushRule::Dark => 1,
        }]);

        fold_occluding_geometry(&mut hasher, scene.ctx.geometry);
        hasher.update(&crate::sdf_bake::solidity_bytes(scene.reach.tree));
        let mut exterior: Vec<usize> = scene.reach.exterior_leaves.iter().copied().collect();
        exterior.sort_unstable();
        hasher.update(&(exterior.len() as u64).to_le_bytes());
        for leaf in exterior {
            hasher.update(&(leaf as u64).to_le_bytes());
        }
        hasher.update(&(scene.reach.portals.len() as u64).to_le_bytes());
        for portal in scene.reach.portals {
            hasher.update(&(portal.front_leaf as u64).to_le_bytes());
            hasher.update(&(portal.back_leaf as u64).to_le_bytes());
        }
        hasher.update(&scene.reach.probe_spacing.to_bits().to_le_bytes());
        fold_probe_grid_layout(&mut hasher, scene.layout);
        Self(hasher)
    }
}

/// The per-light memo over one stage cache.
pub(crate) struct DirectMemo<'c> {
    cache: &'c StageCache,
    prefix: MemoPrefix,
}

impl<'c> DirectMemo<'c> {
    pub(crate) fn new(cache: &'c StageCache, prefix: MemoPrefix) -> Self {
        Self { cache, prefix }
    }

    /// The prefix, then the light as its section bakes it (namespace, seed
    /// axis, early-out, reach policy, the whole light) and each mover it may
    /// meet: its id and, per terminus, whether the light's reach touches the
    /// pose and the posed brushes where it does. A mover the light cannot
    /// meet is absent, so moving one mover re-bakes only the lights reaching
    /// its old or new pose. The light index is not folded: the payload never
    /// names it.
    pub(crate) fn key(&self, light: &ChannelLight<'_>, movers: &[LightMover]) -> CacheKey {
        let mut hasher = self.prefix.0.clone();
        hasher.update(&[light.namespace as u8]);
        hasher.update(&light.seed_axis.to_le_bytes());
        hasher.update(&[light.early_out.key_byte()]);
        hasher.update(format!("{:?}", light.policy).as_bytes());
        for light in [&light.baked, light.authored] {
            let encoded =
                postcard::to_allocvec(light).expect("postcard serialize MapLight for q memo key");
            hasher.update(&(encoded.len() as u32).to_le_bytes());
            hasher.update(&encoded);
        }
        hasher.update(&(movers.len() as u32).to_le_bytes());
        for mover in movers {
            hasher.update(&mover.mover_id.to_le_bytes());
            for terminus in Terminus::BOTH {
                let Some(occluder) = &mover.poses[terminus.index()] else {
                    hasher.update(&[0]);
                    continue;
                };
                hasher.update(&[1]);
                let pose = occluder.pose();
                fold_aabb(&mut hasher, &pose.bounds);
                hasher.update(&(pose.brushes.len() as u32).to_le_bytes());
                for brush in &pose.brushes {
                    fold_aabb(&mut hasher, &brush.bounds);
                    hasher.update(&(brush.planes.len() as u32).to_le_bytes());
                    for &(normal, distance) in &brush.planes {
                        for component in normal.to_array() {
                            hasher.update(&component.to_bits().to_le_bytes());
                        }
                        hasher.update(&distance.to_bits().to_le_bytes());
                    }
                }
            }
        }
        CacheKey::new(STAGE_ID, STAGE_VERSION, hasher.finalize().as_bytes())
    }

    /// The entries stored under `key`, or `None` on a miss. A payload that
    /// names a cell past the grid, a mover outside `movers`, entries out of
    /// order, or a probe count unlike the cell's is corrupt: a miss that
    /// re-bakes. Runs outside any governor permit.
    pub(crate) fn get(
        &self,
        key: &CacheKey,
        cells: &ProbeCells,
        movers: &[LightMover],
    ) -> Option<Vec<DirectEntry>> {
        let bytes = self.cache.get(key)?;
        let entries = decode(&bytes, cells, movers);
        if entries.is_none() {
            log::warn!("[cache] corrupt {STAGE_ID} entry, re-baking");
        }
        entries
    }

    /// Store `entries`, empty included, so an unchanged light hits next
    /// build. Runs outside any governor permit.
    pub(crate) fn put(&self, key: &CacheKey, entries: &[DirectEntry]) {
        self.cache.put(key, &encode(entries));
    }

    /// One line per build: `hit` when every probed light hit, else `miss`
    /// with the share that re-bakes.
    pub(crate) fn log_outcome(&self, probed: usize, hits: usize) {
        if probed == 0 {
            return;
        }
        if hits == probed {
            log::info!("[cache] {STAGE_ID} hit ({probed} light(s))");
        } else {
            log::info!(
                "[cache] {STAGE_ID} miss ({} of {probed} light(s) re-bake)",
                probed - hits
            );
        }
    }
}

fn fold_aabb(hasher: &mut blake3::Hasher, bounds: &Aabb) {
    for component in bounds
        .min
        .to_array()
        .into_iter()
        .chain(bounds.max.to_array())
    {
        hasher.update(&component.to_bits().to_le_bytes());
    }
}

/// `u32` entry count, then per entry `u32` cell, `u32` mover id, `u32`
/// probe count n, `2n` visibility bytes and `108n` little-endian halves.
pub(crate) fn encode(entries: &[DirectEntry]) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(&(entries.len() as u32).to_le_bytes());
    for entry in entries {
        let probes = entry.visibility.len() / MOVER_VISIBILITY_BYTES_PER_PROBE;
        out.extend_from_slice(&entry.cell.to_le_bytes());
        out.extend_from_slice(&entry.mover_id.to_le_bytes());
        out.extend_from_slice(&(probes as u32).to_le_bytes());
        out.extend_from_slice(&entry.visibility);
        for half in &entry.tiles {
            out.extend_from_slice(&half.to_le_bytes());
        }
    }
    out
}

/// The entries of a well-formed payload: each cell inside the grid with its
/// valid-probe count, each mover one of `movers`, strictly ascending by
/// (cell, mover), and no trailing bytes.
pub(crate) fn decode(
    bytes: &[u8],
    cells: &ProbeCells,
    movers: &[LightMover],
) -> Option<Vec<DirectEntry>> {
    let mut reader = Reader(bytes);
    let count = reader.u32()? as usize;
    let mut entries: Vec<DirectEntry> = Vec::with_capacity(count.min(bytes.len() / 12));
    for _ in 0..count {
        let cell = reader.u32()?;
        let mover_id = reader.u32()?;
        let probes = reader.u32()? as usize;
        let mask = *cells.valid_masks.get(cell as usize)?;
        let ordered = entries
            .last()
            .is_none_or(|last| (last.cell, last.mover_id) < (cell, mover_id));
        if !ordered
            || probes != mask.count_ones() as usize
            || !movers.iter().any(|mover| mover.mover_id == mover_id)
        {
            return None;
        }
        let visibility = reader
            .take(probes * MOVER_VISIBILITY_BYTES_PER_PROBE)?
            .to_vec();
        let tiles = reader
            .take(probes * MOVER_IRRADIANCE_TILE_F16_COUNT * 2)?
            .as_chunks::<2>()
            .0
            .iter()
            .map(|&half| u16::from_le_bytes(half))
            .collect();
        entries.push(DirectEntry {
            cell,
            mover_id,
            visibility,
            tiles,
        });
    }
    reader.0.is_empty().then_some(entries)
}

struct Reader<'b>(&'b [u8]);

impl<'b> Reader<'b> {
    fn take(&mut self, len: usize) -> Option<&'b [u8]> {
        if self.0.len() < len {
            return None;
        }
        let (head, rest) = self.0.split_at(len);
        self.0 = rest;
        Some(head)
    }

    fn u32(&mut self) -> Option<u32> {
        self.take(4)
            .map(|bytes| u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
    }
}
