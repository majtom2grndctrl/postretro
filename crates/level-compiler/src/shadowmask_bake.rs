// Per-face owner shadowmask bake for owner-eligible static lights, fused into
// the lightmap walk.
// Governing context: context/lib/build_pipeline.md §PRL (ShadowmaskAtlas ownership)

use std::collections::HashMap;
use std::time::{Duration, Instant};

use postretro_level_format::lightmap::LIGHTMAP_POOL_LAYER_EDGE;
use postretro_level_format::shadowmask_atlas::{
    SHADOWMASK_GROUP_COUNT, ShadowmaskAtlasSection, ShadowmaskOwnerBounds,
};

use crate::bake_control::BakeControl;
use crate::cache::{CacheKey, StageCache};
use crate::lightmap_bake;
use crate::lightmap_layer::{self, LightmapLayer, SharedAtlas};

mod eligibility;
mod encode;
mod fill;
mod memo;
mod ownership;

pub(crate) use eligibility::{
    EligibleLight, MAX_STATIC_SPEC_LIGHTS, owner_eligible_lights, static_spec_light_count,
};
use fill::*;
pub(crate) use memo::shadowmask_atlas_input_hash;
use memo::{cache_shadowmask_section, read_shadowmask_memo};
pub(crate) use ownership::{OwnerReport, log_owner_report};

pub const SHADOWMASK_ATLAS_STAGE_ID: &str = "shadowmask_atlas";

/// Bump when the cached `ShadowmaskAtlas` bytes can change without a layer input
/// hash change: owner admission or ranking, raw-visibility quantization,
/// payload encoding (BC5 groups, per-block BC4 mode choice), memo entry
/// layout (the drop-report prefix), or `ShadowmaskAtlasSection::to_bytes`
/// payload semantics.
///
/// v5: `SMB6`, one pair of BC5 group planes per lightmap cell block.
///
/// v6: raw visibility follows the chart-local soft-visibility seeds.
///
/// v7: `SMB7` per-face owners ranked in the walk; the entry carries the drop
/// report.
pub const SHADOWMASK_ATLAS_STAGE_VERSION: u32 = 7;

/// A pool layer holds the two mask groups side by side, so it is
/// `SHADOWMASK_GROUP_COUNT` pool edges wide and must fit the pinned device
/// texture dimension. The oversize-face cut and cell packing bound every block
/// by the pool layer edge, so the doubled pool layer fits by construction.
pub(crate) const MAX_SHADOWMASK_TEXTURE_WIDTH: u32 = lightmap_bake::MAX_ATLAS_DIMENSION;

const _: () =
    assert!(LIGHTMAP_POOL_LAYER_EDGE * SHADOWMASK_GROUP_COUNT <= MAX_SHADOWMASK_TEXTURE_WIDTH);

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub(crate) enum ShadowmaskBakeError {
    #[error(
        "shadowmask atlas dimensions {width}x{height} are not multiples of 4; \
         BC5 encodes 4x4 blocks and would truncate the payload"
    )]
    MisalignedAtlas { width: u32, height: u32 },
    #[error(
        "the level has {count} static spec lights, more than the {max} a shadowmask owner \
         index can name; make some of them dynamic or remove them"
    )]
    TooManyStaticSpecLights { count: usize, max: usize },
}

/// Fails before any lightmap bake work when the level's compact static
/// spec-light count overflows the u16 owner index.
pub(crate) fn check_owner_index_capacity(
    static_spec_light_count: usize,
) -> Result<(), ShadowmaskBakeError> {
    if static_spec_light_count > MAX_STATIC_SPEC_LIGHTS {
        return Err(ShadowmaskBakeError::TooManyStaticSpecLights {
            count: static_spec_light_count,
            max: MAX_STATIC_SPEC_LIGHTS,
        });
    }
    Ok(())
}

/// Fill checks the cooperative pause gate at this cadence without consuming a
/// governor permit; chart work remains the only governed parallel level.
const SHADOWMASK_FILL_CHECKPOINT_TEXELS: usize = 1024;

/// Shadowmask state prepared before the fused lightmap walk. A memo hit, or a
/// level with nothing to own, carries its finished outcome and requests no
/// partitions. A miss owns an empty fill that ranks each eligible light's
/// partition while the lightmap fold still has it resident.
pub(crate) struct FusedShadowmaskPlan<'a> {
    outcome: Option<(Option<ShadowmaskAtlasSection>, OwnerReport)>,
    fill: Option<OwnershipFill<'a>>,
    eligible_by_source: HashMap<usize, EligibleLight<'a>>,
    cache_write: Option<(&'a StageCache, CacheKey)>,
    control: &'a BakeControl,
    work_elapsed: Duration,
}

impl FusedShadowmaskPlan<'_> {
    pub(crate) fn needs_source(&self, source_index: usize) -> bool {
        self.fill.is_some() && self.eligible_by_source.contains_key(&source_index)
    }

    pub(crate) fn consume_partition(
        &mut self,
        source_index: usize,
        partition: &LightmapLayer,
        shared: &SharedAtlas<'_>,
    ) {
        let Some(fill) = self.fill.as_mut() else {
            return;
        };
        let Some(light) = self.eligible_by_source.get(&source_index) else {
            return;
        };
        let started = Instant::now();
        fill.consume(light, partition, shared);
        self.control.advance(1);
        self.work_elapsed += started.elapsed();
    }

    pub(crate) fn finish(mut self, shared: &SharedAtlas<'_>) -> FusedShadowmaskOutput {
        let started = Instant::now();
        let (section, report) = match (self.outcome.take(), self.fill.take()) {
            (Some(outcome), _) => outcome,
            (None, Some(fill)) => {
                let (section, report) = fill.finish(shared);
                if let Some((cache, key)) = self.cache_write.take() {
                    cache_shadowmask_section(cache, &key, section.as_ref(), &report);
                }
                self.control.advance(1);
                (section, report)
            }
            (None, None) => (None, OwnerReport::default()),
        };
        self.work_elapsed += started.elapsed();
        FusedShadowmaskOutput {
            section,
            report,
            elapsed: self.work_elapsed,
        }
    }
}

pub(crate) struct FusedShadowmaskOutput {
    pub(crate) section: Option<ShadowmaskAtlasSection>,
    pub(crate) report: OwnerReport,
    pub(crate) elapsed: Duration,
}

/// Probe the whole shadowmask memo and, on a miss, set up the empty owner fill
/// the fused walk ranks partitions into. No visibility ray is traced here.
///
/// `eligible_layer_hashes` holds each eligible light's `layer_count` layer
/// hashes in ascending layer order, light-major, and is empty without a cache.
pub(crate) fn prepare_fused_shadowmask<'a>(
    eligible: Vec<EligibleLight<'a>>,
    eligible_layer_hashes: &[[u8; 32]],
    static_spec_light_count: usize,
    layer_count: u32,
    shared: &SharedAtlas<'_>,
    cache: Option<&'a StageCache>,
    control: &'a BakeControl,
) -> Result<FusedShadowmaskPlan<'a>, ShadowmaskBakeError> {
    let started = Instant::now();
    let finished = |outcome, eligible_by_source| FusedShadowmaskPlan {
        outcome: Some(outcome),
        fill: None,
        eligible_by_source,
        cache_write: None,
        control,
        work_elapsed: started.elapsed(),
    };
    // No eligible light, or no placed chart, means nothing to own. The empty-
    // geometry atlas is 1×1 and would otherwise fail the alignment check.
    if eligible.is_empty() || shared.placements.is_empty() {
        return Ok(finished((None, OwnerReport::default()), HashMap::new()));
    }
    if !shared.atlas_width.is_multiple_of(4) || !shared.atlas_height.is_multiple_of(4) {
        return Err(ShadowmaskBakeError::MisalignedAtlas {
            width: shared.atlas_width,
            height: shared.atlas_height,
        });
    }
    let bounds = ShadowmaskOwnerBounds {
        face_count: shared.charts.len() as u32,
        static_spec_light_count: static_spec_light_count as u32,
    };
    let section_key = cache.map(|_| {
        let input_hash = shadowmask_atlas_input_hash(
            &eligible,
            eligible_layer_hashes,
            shared.charts.len(),
            shared.atlas_width,
            shared.atlas_height,
            layer_count,
            &[],
        );
        CacheKey::new(
            SHADOWMASK_ATLAS_STAGE_ID,
            SHADOWMASK_ATLAS_STAGE_VERSION,
            &input_hash,
        )
    });
    // One unit per eligible partition the walk ranks, plus the encode.
    let total = eligible
        .len()
        .saturating_mul(layer_count as usize)
        .saturating_add(1);
    control.publish_total(total);
    let eligible_by_source: HashMap<usize, EligibleLight<'a>> = eligible
        .iter()
        .map(|light| (light.source_index, *light))
        .collect();

    if let (Some(cache), Some(key)) = (cache, section_key.as_ref()) {
        if let Some(memo) = read_shadowmask_memo(cache, key, shared, bounds) {
            log::info!("[cache] shadowmask_atlas hit");
            // The memo stands in for the eligible partitions it summarizes;
            // the next prune must keep them as the fallback for a light edit.
            for hash in eligible_layer_hashes {
                cache.mark_used(&CacheKey::new(
                    "lightmap_layer",
                    lightmap_layer::LAYER_FORMAT_VERSION,
                    hash,
                ));
            }
            control.governor().checkpoint();
            control.advance(total);
            return Ok(finished((memo.section, memo.report), eligible_by_source));
        }
        log::info!("[cache] shadowmask_atlas miss");
    }

    Ok(FusedShadowmaskPlan {
        outcome: None,
        fill: Some(OwnershipFill::new(shared, layer_count, Some(control))),
        eligible_by_source,
        cache_write: cache.zip(section_key),
        control,
        work_elapsed: started.elapsed(),
    })
}

#[cfg(test)]
thread_local! {
    static LAST_RAW_FILL: std::cell::RefCell<Option<Vec<u8>>> = const {
        std::cell::RefCell::new(None)
    };
    static RAW_FILL_LIVE_BYTES: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    static RAW_FILL_LIVE_BYTES_AT_CACHE_WRITE: std::cell::Cell<Option<usize>> = const {
        std::cell::Cell::new(None)
    };
    static SHADOWMASK_OUTPUT_ALLOCATION_COUNT: std::cell::Cell<usize> = const {
        std::cell::Cell::new(0)
    };
    static SHADOWMASK_STREAMED_CACHE_WRITE_COUNT: std::cell::Cell<usize> = const {
        std::cell::Cell::new(0)
    };
}

#[cfg(test)]
fn take_raw_fill_live_bytes_at_cache_write() -> Option<usize> {
    RAW_FILL_LIVE_BYTES_AT_CACHE_WRITE.with(std::cell::Cell::take)
}

/// Tests assert exact fill values on the raw masks the encoder consumed; BC4
/// reproduces only block endpoints exactly. The copy lives outside
/// `RawFillBuffer`, so the raw-fill residency counters exclude it.
#[cfg(test)]
fn record_raw_fill(raw: &[u8]) {
    LAST_RAW_FILL.with(|last| *last.borrow_mut() = Some(raw.to_vec()));
}

#[cfg(test)]
fn take_last_raw_fill() -> Vec<u8> {
    LAST_RAW_FILL
        .with(|last| last.borrow_mut().take())
        .expect("a shadowmask fill must have finished on this thread")
}

/// Allocates the pre-encode raw fill, initialized occluded: an owned channel
/// reads zero wherever its owner wrote nothing, padding included.
fn allocate_shadowmask_raw_fill(data_len: usize) -> RawFillBuffer {
    #[cfg(test)]
    SHADOWMASK_OUTPUT_ALLOCATION_COUNT.with(|count| count.set(count.get() + 1));
    RawFillBuffer::new(vec![0; data_len])
}

#[cfg(test)]
fn raw_fill_live_bytes_add(bytes: usize) {
    RAW_FILL_LIVE_BYTES.with(|live| live.set(live.get() + bytes));
}

#[cfg(test)]
fn raw_fill_live_bytes_sub(bytes: usize) {
    RAW_FILL_LIVE_BYTES.with(|live| live.set(live.get() - bytes));
}

#[cfg(test)]
fn raw_fill_live_bytes() -> usize {
    RAW_FILL_LIVE_BYTES.with(std::cell::Cell::get)
}

#[cfg(test)]
pub(crate) fn reset_shadowmask_output_allocation_count() {
    SHADOWMASK_OUTPUT_ALLOCATION_COUNT.with(|count| count.set(0));
}

/// Section payload allocations on this thread: a raw fill, or the encoded
/// payload of an empty section, which has no raw fill.
#[cfg(test)]
pub(crate) fn shadowmask_output_allocation_count() -> usize {
    SHADOWMASK_OUTPUT_ALLOCATION_COUNT.with(std::cell::Cell::get)
}

#[cfg(test)]
fn reset_shadowmask_streamed_cache_write_count() {
    SHADOWMASK_STREAMED_CACHE_WRITE_COUNT.with(|count| count.set(0));
}

#[cfg(test)]
fn shadowmask_streamed_cache_write_count() -> usize {
    SHADOWMASK_STREAMED_CACHE_WRITE_COUNT.with(std::cell::Cell::get)
}

#[cfg(test)]
mod tests;
