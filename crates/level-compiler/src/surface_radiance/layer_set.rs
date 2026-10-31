// The bake's per-light layers: one per distinct light content, cached or shaded as each finishes.
// See: context/lib/build_pipeline.md §Build Cache

use std::sync::OnceLock;

use rayon::prelude::*;

use super::layer::{LayerKeys, LightDigest, SparseLayer, layer_progress_units, shade_light_layer};
use super::patches::PatchSet;
use super::reach::PatchReachIndex;
use crate::bake_control::BakeControl;
use crate::cache::StageCache;
use crate::map_data::MapLight;
use crate::sh_bake::RaytracingCtx;
use crate::texel_footprint::EmbeddedEmitters;

/// One distinct layer the bake needs and how many static lights share it.
#[derive(Clone, Debug)]
pub(crate) struct PlannedLayer {
    pub(crate) digest: LightDigest,
    /// The effective embedded-emitter flag every sharing light carries.
    pub(crate) embedded: bool,
    /// Position in the static-light slice of a light with this content:
    /// which one does not matter, since shading reads only that content.
    pub(crate) light: usize,
    /// Patches in the light's reach, as the SH base gate counted them.
    pub(crate) reach: u64,
    /// Static lights of this content and flag: how often the sum adds it.
    pub(crate) occurrences: u32,
}

/// Every layer one SH base bake sums, ordered by (digest, flag): an order
/// that follows content alone, so reordering entities changes neither which
/// layers exist nor the order the sum adds them. Lights of equal content
/// and flag share one layer. A light whose reach holds no patch has none.
pub(crate) struct LayerPlan {
    pub(crate) layers: Vec<PlannedLayer>,
    #[cfg(test)]
    _held: crate::live_bytes::Held,
}

impl LayerPlan {
    /// `light_reach[i]` and `embedded`'s key `i` both describe `lights[i]`,
    /// the static-light slice.
    pub(crate) fn new(
        lights: &[&MapLight],
        light_reach: &[u64],
        embedded: &EmbeddedEmitters,
    ) -> Self {
        assert_eq!(
            lights.len(),
            light_reach.len(),
            "one reach per static light"
        );
        let mut occurrences: Vec<PlannedLayer> = lights
            .iter()
            .zip(light_reach)
            .enumerate()
            .filter(|&(_, (_, &reach))| reach > 0)
            .map(|(light, (map_light, &reach))| PlannedLayer {
                digest: LightDigest::of(map_light),
                embedded: embedded.is_embedded(light),
                light,
                reach,
                occurrences: 1,
            })
            .collect();
        #[cfg(test)]
        let _occurrences = crate::live_bytes::hold_vec("per-light metadata", &occurrences);
        occurrences.sort_by_key(|planned| (planned.digest, planned.embedded));
        let mut layers: Vec<PlannedLayer> = Vec::with_capacity(occurrences.len());
        #[cfg(test)]
        let _held = crate::live_bytes::hold_vec("per-light metadata", &layers);
        for planned in occurrences {
            match layers.last_mut() {
                Some(last)
                    if (last.digest, last.embedded) == (planned.digest, planned.embedded) =>
                {
                    debug_assert_eq!(last.reach, planned.reach, "equal content, equal reach");
                    last.occurrences += 1;
                }
                _ => layers.push(planned),
            }
        }
        Self {
            layers,
            #[cfg(test)]
            _held,
        }
    }

    /// Progress units shading every layer advances: what a fully cold bake
    /// publishes for this phase. A cache hit advances its layer's share.
    pub(crate) fn progress_units(&self) -> usize {
        self.layers
            .iter()
            .map(|planned| layer_progress_units(planned.reach as usize))
            .sum()
    }
}

/// The build cache as the layers read and write it.
pub(crate) struct LayerCache<'a> {
    pub(crate) cache: &'a StageCache,
    pub(crate) keys: LayerKeys,
}

impl LayerCache<'_> {
    /// The cached layer, or `None` on a miss. An entry that does not decode
    /// for this patch set is a miss and re-shades. Runs outside any permit.
    fn load(&self, planned: &PlannedLayer, patch_count: usize) -> Option<SparseLayer> {
        note_cache_io();
        let key = self.keys.key(planned.digest, planned.embedded);
        let layer = self.cache.get(&key).and_then(|bytes| {
            #[cfg(test)]
            let _payload = crate::live_bytes::hold_vec("patch layer cache payload", &bytes);
            let layer = SparseLayer::from_bytes(&bytes, patch_count);
            if layer.is_none() {
                log::warn!("[cache] corrupt sh_patch_layer entry, re-shading");
            }
            layer
        });
        log::info!(
            "[cache] sh_patch_layer {}",
            if layer.is_some() { "hit" } else { "miss" }
        );
        layer
    }

    /// Write a layer the moment it is shaded, before the sum, so a build
    /// stopped later keeps it. Runs outside any permit.
    fn store(&self, planned: &PlannedLayer, layer: &SparseLayer, patch_count: usize) {
        note_cache_io();
        let key = self.keys.key(planned.digest, planned.embedded);
        #[cfg(test)]
        let _stream = crate::live_bytes::hold(
            "patch layer cache stream buffer",
            StageCache::stream_buffer_bytes(layer.byte_len() as u64),
        );
        self.cache
            .put_streamed(&key, layer.byte_len() as u64, |out| {
                layer.write_to(patch_count, out)
            });
    }
}

/// Count one layer cache read or write, and whether this thread holds a
/// governor permit while making it.
fn note_cache_io() {
    #[cfg(test)]
    {
        let under_permit = crate::governor::permits_held_on_this_thread() > 0;
        crate::bake_counters::bump(|counts| {
            counts.layer_cache_io += 1;
            counts.layer_cache_io_under_permit += u64::from(under_permit);
        });
    }
}

/// Every planned layer, index-parallel to `plan.layers`: read from `cache`
/// when it holds the layer, else shaded over the light's reach under
/// governor permits and written back as soon as it finishes. Cache reads
/// and writes take no permit. Layers resolve in parallel; each is a pure
/// function of its keyed inputs, so the result does not depend on worker
/// count, completion order or cache state.
pub(crate) fn resolve_layers(
    ctx: &RaytracingCtx<'_>,
    patches: &PatchSet,
    lights: &[&MapLight],
    plan: &LayerPlan,
    cache: Option<&LayerCache<'_>>,
    control: &BakeControl,
) -> Vec<SparseLayer> {
    // Built by the first miss; a build whose layers all hit never sorts the
    // patches.
    let reach_index: OnceLock<PatchReachIndex<'_>> = OnceLock::new();
    plan.layers
        .par_iter()
        .map(|planned| {
            if let Some(layer) = cache.and_then(|cache| cache.load(planned, patches.len())) {
                control.advance(layer_progress_units(planned.reach as usize));
                #[cfg(test)]
                crate::bake_counters::bump(|counts| counts.layers_cached += 1);
                return layer;
            }
            let light = lights[planned.light];
            let in_reach = reach_index
                .get_or_init(|| PatchReachIndex::new(patches.patches()))
                .in_reach(light);
            #[cfg(test)]
            let in_reach_held = crate::live_bytes::hold_vec("in-reach lists", &in_reach);
            // AC 31: a layer shades exactly the patches the gate priced in
            // its reach, so the gate's patch set is the set the bake shades.
            #[cfg(any(test, debug_assertions))]
            assert_eq!(
                in_reach.len() as u64,
                planned.reach,
                "the bake shades the patches the SH base gate priced"
            );
            let layer = shade_light_layer(
                ctx,
                patches,
                &in_reach,
                light,
                planned.digest.seed_axis(),
                planned.embedded,
                control,
            );
            drop(in_reach);
            #[cfg(test)]
            drop(in_reach_held);
            #[cfg(test)]
            crate::bake_counters::bump(|counts| counts.layers_shaded += 1);
            if let Some(cache) = cache {
                cache.store(planned, &layer, patches.len());
            }
            layer
        })
        .collect()
}
