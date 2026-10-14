//! Metadata retained or buffered by the fused walk, apart from texel payloads.
//! See: context/lib/development_guide.md §1.4 (full resource lifetime).

use std::mem::size_of;

use crate::light_namespaces::{AlphaLightsNs, StaticBakedEntry, StaticBakedLights};
use crate::lightmap_bake::PreparedAtlas;
use crate::map_data::ShadowType;

/// Counts read without allocating, before the gate. Keep them independent of
/// cache mode: uncached and memo-hit builds must have the same admission bound.
#[derive(Debug, Clone, Copy, Default)]
pub(in crate::pipeline) struct MetadataCounts {
    pub(in crate::pipeline) static_lights: usize,
    pub(in crate::pipeline) layer_lights: usize,
    pub(in crate::pipeline) spec_lights: usize,
    pub(in crate::pipeline) eligible_lights: usize,
    pub(in crate::pipeline) primitives: usize,
    largest_light_input: usize,
}

impl MetadataCounts {
    pub(in crate::pipeline) fn new(
        static_lights: &StaticBakedLights<'_>,
        alpha_lights: &AlphaLightsNs<'_>,
        primitives: usize,
    ) -> Self {
        Self {
            static_lights: static_lights.len(),
            layer_lights: static_lights
                .entries()
                .iter()
                .filter(|entry| entry.light.shadow_type != ShadowType::Sdf)
                .count(),
            spec_lights: crate::shadowmask_bake::static_spec_light_count(alpha_lights),
            eligible_lights: static_lights
                .entries()
                .iter()
                .filter(|entry| {
                    entry.light.shadow_type == ShadowType::StaticLightMap && !entry.light.bake_only
                })
                .count(),
            primitives,
            // Postcard's fixed scalar fields fit inside MapLight's storage;
            // strings add their bytes and at most ten prefix bytes apiece.
            largest_light_input: static_lights
                .entries()
                .iter()
                .map(|entry| {
                    size_of::<crate::map_data::MapLight>()
                        .saturating_add(entry.light.carrier.len())
                        .saturating_add(10)
                        .saturating_add(entry.light.tags.iter().fold(0usize, |sum, tag| {
                            sum.saturating_add(tag.len()).saturating_add(10)
                        }))
                })
                .max()
                .unwrap_or(0),
        }
    }
}

/// Conservative std hash-table storage, including a resize's old table.
/// A 7/8-full, power-of-two table uses <16/7 buckets per live entry; its
/// preceding half-size table raises that to <24/7. Four buckets per entry
/// cover both, small-table rounding, and one control byte per bucket. The
/// fixed allowance covers both trailing control groups and alignment padding.
/// This deliberately prices the existing cache live-entry journal even on an
/// uncached bake; it is not the accepted warm memo payload-decode overhead.
pub(super) fn hash_table_bytes(entries: u64, entry_bytes: usize) -> u64 {
    if entries == 0 {
        return 0;
    }
    entries
        .saturating_mul(4)
        .saturating_mul(entry_bytes as u64 + 1)
        .saturating_add(128)
}

pub(super) fn predicted_metadata(
    prepared: &PreparedAtlas,
    counts: MetadataCounts,
    window: usize,
) -> (u64, u64, u64) {
    let lights = counts.layer_lights as u64;
    let faces = prepared.charts.len() as u64;
    let layers = u64::from(prepared.layer_count);
    let blocks = prepared.layout.blocks.len() as u64;
    let workers = (window.max(1) as u64).min(lights);
    let prefixes = lights
        .saturating_add(workers)
        .saturating_mul(size_of::<blake3::Hasher>() as u64);
    // Layer-light references and the selected-light vector may each grow to
    // twice their length. Key workers gather/sort primitive references; three
    // copies allow Vec growth and the stable sort's scratch, per bounded worker.
    let light_lists = lights
        .saturating_mul((2 * size_of::<&StaticBakedEntry<'_>>() + 2 * size_of::<usize>()) as u64);
    let key_scratch = workers
        .saturating_mul(counts.primitives as u64)
        .saturating_mul((3 * size_of::<usize>()) as u64)
        .saturating_add(
            workers
                .saturating_mul(counts.largest_light_input as u64)
                .saturating_mul(3),
        );
    // Each layer's chart Vec (including minimum capacity and growth), chart
    // bounds, and window task/slot metadata. Charts occupy at least one texel,
    // but pricing their descriptors separately avoids relying on pixel slack.
    let chart_lists = layers
        .saturating_mul((size_of::<Vec<usize>>() + 4 * size_of::<usize>()) as u64)
        .saturating_add(faces.saturating_mul(
            (3 * size_of::<usize>() + size_of::<Option<(glam::Vec3, glam::Vec3)>>()) as u64,
        ));
    let task_metadata = faces
        .saturating_mul(window as u64)
        .saturating_mul((size_of::<Option<Vec<crate::lightmap_layer::LayerTexel>>>() + 192) as u64);
    // Section builders/indices keep records beside block payloads. Include
    // their output descriptors and the streamed index scratch for both sections.
    let section_metadata = blocks.saturating_mul(512).saturating_add(256);
    // Each admitted partition may be writing through the cache's 64 KiB
    // BufWriter beside its serialized payload; section writers also use it.
    let io_scratch = (window as u64).saturating_add(2).saturating_mul(64 * 1024);
    let stage = prefixes
        .saturating_add(light_lists)
        .saturating_add(key_scratch)
        .saturating_add(chart_lists)
        .saturating_add(task_metadata)
        .saturating_add(section_metadata)
        .saturating_add(io_scratch);
    // Every Cartesian pair is still touched, including empty partitions. The
    // stage itself stores no such matrix, but StageCache retains one key/length
    // record per touched entry until final journal publication.
    let cache_entries = layers.saturating_mul(lights).saturating_add(2);
    let cache = hash_table_bytes(cache_entries, size_of::<([u8; 32], u64)>())
        // StageCache's outer B-tree holds three stage maps. Final publication
        // also flattens the per-stage maps and serializes key/length records.
        .saturating_add(cache_entries.saturating_mul(3 * 41 + 40 + 16))
        .saturating_add(3 * 1024);
    let ownership = crate::shadowmask_bake::metadata_bytes(
        prepared,
        counts.spec_lights,
        counts.eligible_lights,
    );
    (stage, cache, ownership)
}
