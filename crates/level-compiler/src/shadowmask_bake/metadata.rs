//! Ownership and estimate metadata charged before the fused walk allocates it.
//! See: context/lib/development_guide.md §1.4.

use std::mem::size_of;

use crate::lightmap_bake::PreparedAtlas;

use super::eligibility::EligibleLight;
use super::ownership::{FaceChannels, OwnerDrop};

pub(crate) fn metadata_bytes(
    prepared: &PreparedAtlas,
    spec_lights: usize,
    eligible_lights: usize,
) -> u64 {
    let faces = prepared.charts.len() as u64;
    let cuts = prepared.cut_parents.len() as u64;
    let lights = eligible_lights as u64;
    // Eligibility's source/spec map exists while the eligible Vec is built.
    // The later label map and eligible-by-source map replace it; charge their
    // sum, including hash-table growth and the eligible Vec's capacity slack.
    let hash_table = |n: u64, bytes: usize| {
        if n == 0 {
            0
        } else {
            n.saturating_mul(4)
                .saturating_mul(bytes as u64 + 1)
                .saturating_add(128)
        }
    };
    let light_metadata = hash_table(spec_lights as u64, size_of::<(usize, (usize, u32))>())
        .saturating_add(hash_table(lights, size_of::<(u16, usize)>()))
        .saturating_add(hash_table(lights, size_of::<(usize, EligibleLight<'_>)>()))
        .saturating_add(lights.saturating_mul((2 * size_of::<EligibleLight<'_>>()) as u64));
    if eligible_lights == 0 {
        return light_metadata;
    }
    // Per-face owners/channels/cut lookup, transient sum/held B-trees, and
    // per-block ownership/carry/encoded descriptors. The B-tree allowances
    // exceed a whole minimally occupied node per entry, including edges.
    let state = faces
        .saturating_mul(size_of::<FaceChannels>() as u64 + 1024)
        .saturating_add((prepared.layout.blocks.len() as u64).saturating_mul(256))
        .saturating_add(cuts.saturating_mul(size_of::<FaceChannels>() as u64 + 256));
    // Cut estimate: reached pairs and their ordered results coexist. Pair Vec
    // growth may hold old+new buffers (three lengths); results have exact size.
    // Later, cut exact-count B-trees hold at most one entry per eligible light.
    let cut_pairs = cuts.saturating_mul(lights);
    let estimate_pairs = cut_pairs
        .saturating_mul((3 * size_of::<(usize, usize)>() + size_of::<(u32, f64, usize)>()) as u64);
    let exact_counts = cut_pairs.saturating_mul(256);
    let sample_count = prepared
        .cut_parents
        .iter()
        .flat_map(|cut| cut.faces.clone())
        .map(|face| {
            let chart = &prepared.charts[face];
            u64::from(
                chart
                    .width_texels
                    .div_ceil(super::estimate::ESTIMATE_STRIDE),
            ) * u64::from(
                chart
                    .height_texels
                    .div_ceil(super::estimate::ESTIMATE_STRIDE),
            )
        })
        .fold(0u64, u64::saturating_add);
    let samples = sample_count
        .saturating_mul((3 * size_of::<(usize, i32, i32)>()) as u64)
        .saturating_add(cuts.saturating_mul(size_of::<Vec<(usize, i32, i32)>>() as u64));
    // Each (uncut face or cut parent, light) can report at most one drop.
    // The Vec's growth peak and its final serialized 24-byte records are
    // conservatively co-priced; persistence never duplicates mask payloads.
    let drops = faces
        .saturating_add(cuts)
        .saturating_mul(lights)
        .saturating_mul((3 * size_of::<OwnerDrop>() + 24) as u64)
        .saturating_add(8);
    light_metadata
        .saturating_add(state)
        .saturating_add(estimate_pairs.max(exact_counts))
        .saturating_add(samples)
        .saturating_add(drops)
}
