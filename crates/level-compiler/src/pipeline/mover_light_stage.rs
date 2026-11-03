//! The mover light gate's probe and animated passes behind their per-light memos, and the merge
//! of all three tiers' rows.
//! See: context/lib/build_pipeline.md §Compiler pipeline (step 12), §Build Cache

use std::sync::Arc;
use std::time::Instant;

use postretro_level_format::animated_light_chunks::AnimatedLightChunksSection;
use postretro_level_format::animated_light_weight_maps::AnimatedLightWeightMapsSection;

use crate::bake_content_keys::geometry_content_hash;
use crate::bake_control::BakeControl;
use crate::cache::{CacheKey, StageCache};
use crate::chart_raster::ChartPlacement;
use crate::governor::Governor;
use crate::light_namespaces::AnimatedBakedLights;
use crate::lightmap_bake::{BlockLayout, Chart};
use crate::lightmap_layer::{SharedAtlas, atlas_layout_fingerprint};
use crate::map_data::ShadowType;
use crate::mover_light_gate::{
    ANIMATED_MEMO, AnimatedScene, GateLight, GateLights, GateMemo, GateTally, MoverPoses,
    PROBE_MEMO, ProbeScene, fold_gate_light, gate_animated, gate_key_hasher, gate_probes,
    light_candidates,
};
use crate::reporter::StageProgress;
use crate::sh_bake::{self, ProbeGridLayout, ShBakeCtx, ShConfig};

/// Gate the analysed lights' probes over the SH probe grid, as the SH
/// family lays it out and traces it: before atlas preparation, on the
/// pre-cut world. `None` when no mover is analysed.
///
/// The memo is per analysed light. Its key holds what every light's probe
/// gate reads (the whole static world the trace sees, the grid's probes and
/// validity, the tracer epoch), then the light and each mover pose its
/// influence reaches.
pub(super) fn probe_gate(
    poses: &MoverPoses,
    lights: &GateLights<'_>,
    sh_ctx: &ShBakeCtx<'_>,
    sh_config: &ShConfig,
    cache: Option<&StageCache>,
    governor: &Arc<Governor>,
) -> Option<GateTally> {
    if poses.analysed.is_empty() {
        return None;
    }
    let started = Instant::now();
    let layout = sh_bake::probe_grid_layout(sh_ctx, sh_config);
    let mut tally = GateTally::default();
    let mut to_gate: Vec<GateLight<'_>> = lights.analysed.clone();
    let mut keys: Vec<(usize, CacheKey)> = Vec::new();
    if let Some(cache) = cache.filter(|_| !layout.is_empty()) {
        let memo = GateMemo::new(cache, PROBE_MEMO);
        // Hashing the whole world is the dear part, so only a light that
        // may meet a mover pays for it.
        let mut prefix: Option<blake3::Hasher> = None;
        let (mut probed, mut hits) = (0, 0);
        to_gate.clear();
        for gate_light in &lights.analysed {
            let candidates = light_candidates(gate_light.light, poses);
            if candidates.is_empty() {
                continue;
            }
            let mut hasher = prefix
                .get_or_insert_with(|| probe_key_prefix(sh_ctx, &layout))
                .clone();
            fold_gate_light(&mut hasher, gate_light.light, poses, &candidates);
            let key = memo.key(&hasher);
            probed += 1;
            match memo.get(&key, poses, &candidates) {
                Some(rows) => {
                    tally.add_light_rows(gate_light.source_index, &rows);
                    hits += 1;
                }
                None => {
                    to_gate.push(*gate_light);
                    keys.push((gate_light.source_index, key));
                }
            }
        }
        memo.log_outcome(probed, hits);
    }
    let control = BakeControl::new(Arc::clone(governor), &StageProgress::indeterminate());
    let probes = gate_probes(
        poses,
        &to_gate,
        &ProbeScene {
            layout: &layout,
            bvh: sh_ctx.bvh,
            primitives: sh_ctx.primitives,
            geometry: sh_ctx.geometry,
        },
        &control,
    );
    let mut gated = GateTally::default();
    for light in &probes {
        gated.add_probes(poses, light.source_index, &light.movers);
    }
    if let Some(cache) = cache {
        let memo = GateMemo::new(cache, PROBE_MEMO);
        let mut by_light = gated.rows_by_light();
        for (source_index, key) in &keys {
            memo.put(key, &by_light.remove(source_index).unwrap_or_default());
        }
    }
    tally.merge(&gated);
    log::debug!(
        "[Compiler] mover light gate: probe tier {:.1} ms over {} probes",
        started.elapsed().as_secs_f64() * 1000.0,
        layout.total_probes(),
    );
    Some(tally)
}

/// The gate epoch, the tracer epoch, the whole-map geometry the probe trace
/// reads (the BVH is built from it), and the probe grid: positions and
/// validity, which the geometry alone does not fix.
fn probe_key_prefix(sh_ctx: &ShBakeCtx<'_>, layout: &ProbeGridLayout) -> blake3::Hasher {
    let mut hasher = gate_key_hasher();
    hasher.update(&crate::ray_traversal::TRACER_EPOCH.to_le_bytes());
    hasher.update(&geometry_content_hash(sh_ctx.geometry));
    for component in layout.world_min.to_array() {
        hasher.update(&component.to_bits().to_le_bytes());
    }
    for (count, size) in layout.dims.iter().zip(&layout.cell_size) {
        hasher.update(&count.to_le_bytes());
        hasher.update(&size.to_bits().to_le_bytes());
    }
    hasher.update(&(layout.probe_positions.len() as u64).to_le_bytes());
    for position in &layout.probe_positions {
        for component in position.to_array() {
            hasher.update(&component.to_bits().to_le_bytes());
        }
    }
    hasher.update(&layout.validity);
    hasher
}

/// The finished animated sections the animated gate reads, and the static
/// atlas their chunks name charts and blocks in.
pub(super) struct AnimatedInputs<'a> {
    pub(super) lights: &'a AnimatedBakedLights<'a>,
    pub(super) chunks: Option<&'a AnimatedLightChunksSection>,
    pub(super) weight_maps: Option<&'a AnimatedLightWeightMapsSection>,
    pub(super) face_charts: &'a [Chart],
    pub(super) placements: &'a [ChartPlacement],
    pub(super) atlas_width: u32,
    pub(super) atlas_height: u32,
    pub(super) layout: &'a BlockLayout,
}

/// Gate the animated lights from their finished id-25 weights, then fold
/// the static, animated and probe rows into one tally. The fold order is
/// immaterial: rows key on (mover, terminus, light) and their counts add.
pub(super) fn merge_rows(
    poses: &MoverPoses,
    static_rows: Option<GateTally>,
    probes: Option<GateTally>,
    animated: AnimatedInputs<'_>,
    cache: Option<&StageCache>,
    governor: &Arc<Governor>,
) -> GateTally {
    let mut tally = static_rows.unwrap_or_default();
    if let (Some(chunks), Some(weight_maps)) = (animated.chunks, animated.weight_maps)
        && !poses.analysed.is_empty()
    {
        let started = Instant::now();
        tally.merge(&animated_rows(
            poses,
            &animated,
            chunks,
            weight_maps,
            cache,
            governor,
        ));
        log::debug!(
            "[Compiler] mover light gate: animated tier {:.1} ms",
            started.elapsed().as_secs_f64() * 1000.0
        );
    }
    if let Some(probes) = probes {
        tally.merge(&probes);
    }
    tally
}

/// The animated tier's rows, behind a per-light memo. Its key holds the
/// finished ids 24 and 25 (every weight, rect and block the gate reads) and
/// the static atlas their chunks resolve charts in, then the light's slot,
/// the light and each mover pose its influence reaches. Any weight edit
/// re-gates every animated light; a mover edit re-gates only the lights
/// reaching its old or new pose.
fn animated_rows(
    poses: &MoverPoses,
    animated: &AnimatedInputs<'_>,
    chunks: &AnimatedLightChunksSection,
    weight_maps: &AnimatedLightWeightMapsSection,
    cache: Option<&StageCache>,
    governor: &Arc<Governor>,
) -> GateTally {
    let entries = animated.lights.entries();
    let mut tally = GateTally::default();
    let mut wanted = vec![true; entries.len()];
    let mut keys: Vec<(usize, CacheKey)> = Vec::new();
    if let Some(cache) = cache {
        let memo = GateMemo::new(cache, ANIMATED_MEMO);
        let mut prefix: Option<blake3::Hasher> = None;
        let (mut probed, mut hits) = (0, 0);
        for (slot, entry) in entries.iter().enumerate() {
            let candidates = light_candidates(entry.light, poses);
            if entry.light.shadow_type == ShadowType::Sdf || candidates.is_empty() {
                wanted[slot] = false;
                continue;
            }
            let mut hasher = prefix
                .get_or_insert_with(|| animated_key_prefix(animated, chunks, weight_maps))
                .clone();
            hasher.update(&(slot as u32).to_le_bytes());
            fold_gate_light(&mut hasher, entry.light, poses, &candidates);
            let key = memo.key(&hasher);
            probed += 1;
            match memo.get(&key, poses, &candidates) {
                Some(rows) => {
                    tally.add_light_rows(entry.source_index, &rows);
                    wanted[slot] = false;
                    hits += 1;
                }
                None => keys.push((entry.source_index, key)),
            }
        }
        memo.log_outcome(probed, hits);
    }
    let control = BakeControl::new(Arc::clone(governor), &StageProgress::indeterminate());
    let lights = gate_animated(
        poses,
        animated.lights,
        &AnimatedScene {
            chunks,
            weight_maps,
            face_charts: animated.face_charts,
        },
        &wanted,
        &control,
    );
    let mut gated = GateTally::default();
    for light in &lights {
        gated.add_partition(poses, light.source_index, &light.movers);
    }
    if let Some(cache) = cache {
        let memo = GateMemo::new(cache, ANIMATED_MEMO);
        let mut by_light = gated.rows_by_light();
        for (source_index, key) in &keys {
            memo.put(key, &by_light.remove(source_index).unwrap_or_default());
        }
    }
    tally.merge(&gated);
    tally
}

/// The gate epoch, the tracer epoch (a texel's segment starts at the bake's
/// ray lift, which the weights alone do not fix), the finished ids 24 and
/// 25, and the static atlas layout: chart frames, placements and the
/// cell-block table.
fn animated_key_prefix(
    animated: &AnimatedInputs<'_>,
    chunks: &AnimatedLightChunksSection,
    weight_maps: &AnimatedLightWeightMapsSection,
) -> blake3::Hasher {
    let mut hasher = gate_key_hasher();
    hasher.update(&crate::ray_traversal::TRACER_EPOCH.to_le_bytes());
    for section in [chunks.to_bytes(), weight_maps.to_bytes()] {
        hasher.update(&(section.len() as u64).to_le_bytes());
        hasher.update(&section);
    }
    hasher.update(&atlas_layout_fingerprint(&SharedAtlas {
        charts: animated.face_charts,
        placements: animated.placements,
        atlas_width: animated.atlas_width,
        atlas_height: animated.atlas_height,
        layout: animated.layout,
    }));
    hasher
}
