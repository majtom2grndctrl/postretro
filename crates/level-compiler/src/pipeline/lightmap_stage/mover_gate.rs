//! The mover light gate over the fused walk's partitions, behind its per-light texel memo.
//! See: context/lib/build_pipeline.md §Compiler pipeline (step 12), §Build Cache

use std::sync::Mutex;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::time::{Duration, Instant};

use glam::Vec3;

use crate::cache::{CacheKey, StageCache};
use crate::light_namespaces::StaticBakedEntry;
use crate::lightmap_layer::{LAYER_FORMAT_VERSION, LayerKeyContext, LightmapLayer, SharedAtlas};
use crate::mover_light_gate::{
    CandidateMover, GateMemo, GateTally, MoverPoses, MoverTexels, TEXEL_MEMO, fold_gate_light,
    gate_key_hasher, gate_partition, light_candidates,
};

/// Gates each partition a pass bakes or loads, on the worker that produced
/// it, as the texel-step detector does. The consumer never holds a permit or
/// lock the gate needs, since an item's gate runs before the item is ready,
/// and worker order never reaches the tally: rows fold commutatively.
///
/// The memo is per layer light: one entry holds the light's static rows over
/// every bake layer. Its key is the layer payload's format version and the
/// light's layer hashes, which fold the light, its geometry slice, every
/// chart's frame and placement and the cell-block layout (all the gate reads
/// of a partition and its charts), then each mover pose the light's
/// influence reaches. Moving one terminus re-gates only the lights that
/// reach it, at either its old or new pose; a light that misses re-reads its
/// own partitions, once each.
pub(super) struct MoverGateDetector<'a> {
    poses: &'a MoverPoses,
    shared: &'a SharedAtlas<'a>,
    layer_lights: &'a [&'a StaticBakedEntry<'a>],
    /// By position in `layer_lights`.
    candidates: Vec<Vec<CandidateMover>>,
    /// By position: the light's memo key, when the stage caches and the light
    /// may meet a mover.
    keys: Vec<Option<CacheKey>>,
    /// By position: the memo served the light, so no partition of it gates.
    served: Vec<bool>,
    /// Rows the memo served.
    memo_rows: GateTally,
    /// Rows gated this build.
    gated: Mutex<GateTally>,
    /// By position: partitions of a wanted light inspected this build. A
    /// light's memo is written only once all its layers were.
    inspected: Vec<AtomicU32>,
    /// Gate time summed over workers.
    nanos: AtomicU64,
}

impl<'a> MoverGateDetector<'a> {
    /// `None` when no mover is analysed, so the walk pays nothing.
    pub(super) fn new(
        poses: Option<&'a MoverPoses>,
        shared: &'a SharedAtlas<'a>,
        layer_lights: &'a [&'a StaticBakedEntry<'a>],
    ) -> Option<Self> {
        let poses = poses.filter(|poses| !poses.analysed.is_empty())?;
        let candidates: Vec<_> = layer_lights
            .iter()
            .map(|entry| light_candidates(entry.light, poses))
            .collect();
        Some(Self {
            poses,
            shared,
            layer_lights,
            keys: candidates.iter().map(|_| None).collect(),
            served: vec![false; candidates.len()],
            inspected: candidates.iter().map(|_| AtomicU32::new(0)).collect(),
            candidates,
            memo_rows: GateTally::default(),
            gated: Mutex::new(GateTally::default()),
            nanos: AtomicU64::new(0),
        })
    }

    /// Key every light that may meet a mover and, when `read`, take its rows
    /// from the memo. Reach verify mode passes `read` false: it reads no
    /// cached lightmap work, so every light gates its re-walked partitions.
    /// On the caller's thread, outside any permit.
    pub(super) fn consult_memo(
        &mut self,
        cache: &StageCache,
        read: bool,
        layer_prefixes: &[blake3::Hasher],
        layer_count: u32,
    ) {
        let memo = GateMemo::new(cache, TEXEL_MEMO);
        let (mut probed, mut hits) = (0, 0);
        for (light, candidates) in self.candidates.iter().enumerate() {
            if candidates.is_empty() {
                continue;
            }
            let entry = self.layer_lights[light];
            let mut hasher =
                texel_key_hasher(LAYER_FORMAT_VERSION, &layer_prefixes[light], layer_count);
            fold_gate_light(&mut hasher, entry.light, self.poses, candidates);
            let key = memo.key(&hasher);
            if read {
                probed += 1;
                if let Some(rows) = memo.get(&key, self.poses, candidates) {
                    self.memo_rows.add_light_rows(entry.source_index, &rows);
                    self.served[light] = true;
                    hits += 1;
                }
            }
            self.keys[light] = Some(key);
        }
        memo.log_outcome(probed, hits);
    }

    /// Whether light position `light` gates this build: it may meet a posed
    /// mover and the memo did not serve it.
    pub(super) fn wants(&self, light: usize) -> bool {
        !self.candidates[light].is_empty() && !self.served[light]
    }

    /// Count one partition of a wanted light, empty or not.
    pub(super) fn note_inspected(&self, light: usize) {
        self.inspected[light].fetch_add(1, Ordering::Relaxed);
    }

    /// Gate `partition`, of light position `light`, over `charts` of its
    /// layer with their padded texel bounds.
    pub(super) fn detect(
        &self,
        light: usize,
        partition: &LightmapLayer,
        charts: impl Iterator<Item = (usize, Option<(Vec3, Vec3)>)>,
    ) {
        let started = Instant::now();
        let movers = gate_partition(
            self.poses,
            self.layer_lights[light].light,
            &self.candidates[light],
            self.shared,
            partition,
            charts,
        );
        if !movers.is_empty() {
            self.gated
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .add_partition(self.poses, self.layer_lights[light].source_index, &movers);
        }
        self.nanos
            .fetch_add(started.elapsed().as_nanos() as u64, Ordering::Relaxed);
    }

    /// Store each light gated this build, then return every row, served and
    /// gated, and the gate's time summed over workers. On the caller's
    /// thread, outside any permit.
    pub(super) fn finish(
        self,
        cache: Option<&StageCache>,
        layer_count: u32,
    ) -> (GateTally, Duration) {
        let gated = self
            .gated
            .into_inner()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some(cache) = cache {
            let memo = GateMemo::new(cache, TEXEL_MEMO);
            let mut by_light = gated.rows_by_light();
            for (light, key) in self.keys.iter().enumerate() {
                let Some(key) = key.as_ref().filter(|_| !self.served[light]) else {
                    continue;
                };
                // A light some pass left unwalked has partial rows; caching
                // them would serve the gap to every later build.
                let inspected = self.inspected[light].load(Ordering::Relaxed);
                debug_assert_eq!(inspected, layer_count, "a wanted light walks every layer");
                if inspected != layer_count {
                    continue;
                }
                let source_index = self.layer_lights[light].source_index;
                memo.put(key, &by_light.remove(&source_index).unwrap_or_default());
            }
        }
        let mut tally = self.memo_rows;
        tally.merge(&gated);
        (
            tally,
            Duration::from_nanos(self.nanos.load(Ordering::Relaxed)),
        )
    }
}

/// The start of a light's texel memo key: the gate epoch, the layer
/// payload's `format_version`, and the light's hash on each of `layer_count`
/// layers from its `layer_prefix`. A layer hash digests inputs only (the
/// layer cache adds the version to its own key), so without the version a
/// payload change would serve rows gated from the old payload.
fn texel_key_hasher(
    format_version: u32,
    layer_prefix: &blake3::Hasher,
    layer_count: u32,
) -> blake3::Hasher {
    let mut hasher = gate_key_hasher();
    hasher.update(&format_version.to_le_bytes());
    hasher.update(&layer_count.to_le_bytes());
    for layer in 0..layer_count {
        hasher.update(&LayerKeyContext::layer_hash(layer_prefix, layer));
    }
    hasher
}

/// A bound on the gate's bytes over `lights` layer lights, `movers`
/// analysed movers, `workers` concurrent partitions and `layer_charts`
/// charts on the most crowded bake layer. Retained: per light its candidate
/// list, key, flags and counter; per (light, mover) a candidate and both
/// termini's rows in the served and gated tallies, the per-light copy the
/// memo writes and the merged tally, each row allowed twice for B-tree node
/// slack. Transient: per (partition in flight, mover), its tally, its live
/// flags and four block lists. A partition holds one layer, so a list holds
/// at most one entry per chart of that layer before its dedup, allowed twice
/// for Vec growth.
pub(super) fn predicted_gate_bytes(
    lights: u64,
    movers: u64,
    workers: u64,
    layer_charts: u64,
) -> u64 {
    use std::mem::size_of;
    if movers == 0 {
        return 0;
    }
    let per_light = (size_of::<Vec<CandidateMover>>()
        + size_of::<Option<CacheKey>>()
        + size_of::<bool>()
        + size_of::<AtomicU32>()) as u64;
    let per_pair = (size_of::<CandidateMover>() + 2 * 4 * 2 * GateTally::ROW_BYTES) as u64;
    let tally = (size_of::<MoverTexels>() + size_of::<[bool; 2]>()) as u64;
    let block_lists = layer_charts.saturating_mul(2 * 4 * size_of::<u32>() as u64);
    let in_flight = workers
        .saturating_mul(movers)
        .saturating_mul(tally.saturating_add(block_lists));
    lights
        .saturating_mul(per_light)
        .saturating_add(lights.saturating_mul(movers).saturating_mul(per_pair))
        .saturating_add(in_flight)
}

#[cfg(test)]
mod tests {
    use super::*;

    // A layer payload format bump re-keys every texel memo entry, as it
    // re-keys the layers, so a warm build never serves rows gated from a
    // payload of the old format.
    #[test]
    fn the_texel_memo_key_moves_with_the_layer_format_version() {
        let prefix = blake3::Hasher::new();
        let key = |version| *texel_key_hasher(version, &prefix, 2).finalize().as_bytes();
        assert_ne!(key(LAYER_FORMAT_VERSION), key(LAYER_FORMAT_VERSION + 1));
    }
}
