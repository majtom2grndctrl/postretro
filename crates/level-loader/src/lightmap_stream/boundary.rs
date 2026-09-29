//! Controller-to-renderer handoff for streamed lightmap cell blocks.
//! See: context/lib/rendering_pipeline.md §4 (Lightmap cell-block residency)

use postretro_level_format::lightmap::LightmapBlockPayload;

use super::lightmap_stream_error;
use crate::prl::PrlLoadError;

/// Developer/test selection. `AllResident` loads every block at install: the
/// parity baseline and the fallback when streaming is unavailable, mirroring
/// SH's `off`. `Stream` is the default for a level with a usable id-51 set.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LightmapStreamingMode {
    AllResident,
    Stream,
}

/// Why a block is targeted. Declaration order is refusal order: the renderer
/// may refuse or evict only `Band` blocks under cap pressure. `Mandatory`
/// (within lead L of the camera cell, or a pinned cluster's cell) and
/// `Visible` (drawn this frame but outside the baked set) are never refused;
/// the pool grows past its cap to hold them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum LightmapBlockClass {
    Mandatory,
    Visible,
    Band,
}

/// One block's current target state. `lead` is the smallest id-51 lead (id-46
/// fixed point) that makes the block's cell mandatory from the camera cell,
/// or 0 for visible and pinned blocks. Under pressure the renderer evicts
/// `Band` blocks farthest lead first.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LightmapTarget {
    pub block: u32,
    pub class: LightmapBlockClass,
    pub lead: u32,
}

/// A read lightmap/shadowmask pair, owned by the controller until the renderer
/// reports an outcome. The pair installs whole or not at all.
#[derive(Debug)]
pub struct PreparedLightmapBlock {
    pub generation: u64,
    pub content_tag: [u8; 32],
    pub block: u32,
    pub payload: LightmapBlockPayload,
}

/// Bounded renderer handoff, built once per drain by the controller.
///
/// The controller owns demand, reads and the shared drain budget (which
/// admitted `ready`). The renderer owns placement: allocation, band eviction
/// under the cap, in-place repack through the spare layer, growth, and
/// retirement. It reports what actually happened in [`LightmapDrainOutcome`].
///
/// `target_set` and `target_remove` are deltas against the targets the
/// renderer last accepted; `target_reset` replaces them wholesale (first drain
/// of a generation, or after a reset). A block leaving every target is freed
/// at this drain.
#[derive(Debug, Default)]
pub struct LightmapDrainBatch {
    pub generation: u64,
    pub content_tag: [u8; 32],
    /// Pool cap in layers (the dev-tools slider). Band blocks must fit under
    /// it; mandatory and visible blocks may grow the pool past it.
    pub pool_cap_layers: u32,
    pub target_reset: Option<Vec<LightmapTarget>>,
    /// New or reclassified targets, sorted by block id, unique.
    pub target_set: Vec<LightmapTarget>,
    /// Blocks leaving every target, sorted by block id, unique.
    pub target_remove: Vec<u32>,
    /// Pairs admitted by the shared drain budget this drain.
    pub ready: Vec<PreparedLightmapBlock>,
}

impl LightmapDrainBatch {
    /// Validate identity and structure against the level's block count before
    /// either side mutates state. Counts and sizes are controller policy.
    pub fn validate_contract(
        &self,
        block_count: u32,
        content_tag: [u8; 32],
    ) -> Result<(), PrlLoadError> {
        if self.generation == 0 {
            return Err(lightmap_stream_error(
                "drain batch generation must be nonzero",
            ));
        }
        if self.content_tag != content_tag {
            return Err(lightmap_stream_error(
                "drain batch content tag does not match the level",
            ));
        }
        if let Some(reset) = &self.target_reset {
            validate_sorted_targets(reset, block_count, "target reset")?;
        }
        validate_sorted_targets(&self.target_set, block_count, "target-set")?;
        validate_sorted_blocks(&self.target_remove, block_count, "target-remove")?;
        let set_blocks: Vec<u32> = self.target_set.iter().map(|t| t.block).collect();
        if sorted_lists_intersect(&set_blocks, &self.target_remove) {
            return Err(lightmap_stream_error(
                "target-set and target-remove must not name the same block",
            ));
        }
        let mut ready = std::collections::BTreeSet::new();
        for prepared in &self.ready {
            if prepared.generation != self.generation || prepared.content_tag != self.content_tag {
                return Err(lightmap_stream_error(
                    "ready block identity does not match drain batch",
                ));
            }
            if prepared.block >= block_count {
                return Err(lightmap_stream_error(
                    "ready block id exceeds the level's block count",
                ));
            }
            if !ready.insert(prepared.block) {
                return Err(lightmap_stream_error(
                    "drain batch contains more than one ready pair for a block",
                ));
            }
        }
        Ok(())
    }
}

/// Pool shape after a drain, for controller prefetch gating and diagnostics.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct LightmapPoolReport {
    /// Layers of the active pool generation.
    pub layers: u32,
    /// Texels free under the cap: the room band prefetch may still fill.
    /// The controller requests band reads only against this headroom; the
    /// renderer's refusal stays authoritative.
    pub band_headroom_texels: u64,
    /// Whether this drain repacked in place, or grew a new generation.
    pub repacked: bool,
    pub grew: bool,
    /// Whether a retiring generation is still awaiting submitted-work-done.
    pub retiring: bool,
}

/// Renderer-to-controller ownership return.
#[derive(Debug, Default)]
pub struct LightmapDrainOutcome {
    /// Pairs installed this drain; sampleable from this drain on.
    pub installed: Vec<u32>,
    /// Band pairs that did not fit under the cap. Their payloads are dropped;
    /// the controller must not re-request them until the pool frees room.
    pub refused: Vec<u32>,
    /// Pairs the renderer could not place yet (a mandatory block waiting on a
    /// retiring pool: a counted transient miss). Returned owned.
    pub deferred: Vec<PreparedLightmapBlock>,
    /// Blocks actually released: removed targets, and band blocks evicted for
    /// cap pressure or to make room for mandatory work.
    pub evicted: Vec<u32>,
    pub pool: LightmapPoolReport,
}

fn sorted_lists_intersect(left: &[u32], right: &[u32]) -> bool {
    let (mut l, mut r) = (0, 0);
    while l < left.len() && r < right.len() {
        match left[l].cmp(&right[r]) {
            std::cmp::Ordering::Less => l += 1,
            std::cmp::Ordering::Greater => r += 1,
            std::cmp::Ordering::Equal => return true,
        }
    }
    false
}

fn validate_sorted_targets(
    targets: &[LightmapTarget],
    block_count: u32,
    label: &'static str,
) -> Result<(), PrlLoadError> {
    if targets.iter().any(|t| t.block >= block_count) {
        return Err(lightmap_stream_error(format!(
            "{label} contains an out-of-range block id"
        )));
    }
    if targets
        .windows(2)
        .any(|pair| pair[0].block >= pair[1].block)
    {
        return Err(lightmap_stream_error(format!(
            "{label} must be sorted by block id and deduplicated"
        )));
    }
    Ok(())
}

fn validate_sorted_blocks(
    blocks: &[u32],
    block_count: u32,
    label: &'static str,
) -> Result<(), PrlLoadError> {
    if blocks.iter().any(|&b| b >= block_count) {
        return Err(lightmap_stream_error(format!(
            "{label} contains an out-of-range block id"
        )));
    }
    if blocks.windows(2).any(|pair| pair[0] >= pair[1]) {
        return Err(lightmap_stream_error(format!(
            "{label} must be sorted and deduplicated"
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn target(block: u32, class: LightmapBlockClass) -> LightmapTarget {
        LightmapTarget {
            block,
            class,
            lead: 0,
        }
    }

    fn batch() -> LightmapDrainBatch {
        LightmapDrainBatch {
            generation: 3,
            content_tag: [7; 32],
            pool_cap_layers: 4,
            target_set: vec![
                target(1, LightmapBlockClass::Mandatory),
                target(4, LightmapBlockClass::Band),
            ],
            target_remove: vec![2],
            ..Default::default()
        }
    }

    #[test]
    fn lightmap_drain_contract_accepts_sorted_disjoint_deltas() {
        batch().validate_contract(8, [7; 32]).unwrap();
    }

    #[test]
    fn lightmap_drain_contract_rejects_identity_range_order_and_overlap() {
        let mut b = batch();
        b.generation = 0;
        assert!(b.validate_contract(8, [7; 32]).is_err());
        assert!(batch().validate_contract(8, [8; 32]).is_err());
        assert!(batch().validate_contract(4, [7; 32]).is_err());

        let mut unsorted = batch();
        unsorted.target_set.reverse();
        assert!(unsorted.validate_contract(8, [7; 32]).is_err());

        let mut overlap = batch();
        overlap.target_remove = vec![4];
        assert!(overlap.validate_contract(8, [7; 32]).is_err());

        let mut twice = batch();
        for _ in 0..2 {
            twice.ready.push(PreparedLightmapBlock {
                generation: 3,
                content_tag: [7; 32],
                block: 1,
                payload: LightmapBlockPayload::default(),
            });
        }
        assert!(twice.validate_contract(8, [7; 32]).is_err());
    }

    #[test]
    fn block_classes_order_refusable_band_last() {
        assert!(LightmapBlockClass::Mandatory < LightmapBlockClass::Visible);
        assert!(LightmapBlockClass::Visible < LightmapBlockClass::Band);
    }
}
