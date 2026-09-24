//! CPU-only ownership and drain-boundary types for streamed SH clusters.
//! See: context/lib/build_pipeline.md §PRL Compilation.

use std::sync::Arc;

use postretro_level_format::cluster_sh_payloads::DecodedClusterShPayload;

use super::manifest::ShStreamManifest;
use super::{PrlLoadError, stream_error};
/// Developer/test selection resolved after a valid id-49/id-50 pair is found.
///
/// The app consults this only for a world that is already in
/// [`ShStorage::Streaming`] mode, so a legacy world continues to ignore the
/// environment variable exactly as the loader does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShStreamingMode {
    Off,
    SyncProof,
    Async,
}

/// The SH-body ownership mode for a loaded level. Legacy keeps its original
/// decoded section bodies on `LevelWorld`; streaming retains only this immutable
/// manifest and obtains selected cluster bodies positionally.
#[derive(Debug)]
pub enum ShStorage {
    Legacy,
    Streaming(Arc<ShStreamManifest>),
}

impl ShStorage {
    pub fn manifest(&self) -> Option<&Arc<ShStreamManifest>> {
        match self {
            Self::Legacy => None,
            Self::Streaming(manifest) => Some(manifest),
        }
    }

    pub fn is_streaming(&self) -> bool {
        matches!(self, Self::Streaming(_))
    }
}
/// CPU-only prepared chunk. The planner owns these values until the renderer
/// reports an outcome; no wgpu types cross this boundary.
#[derive(Debug)]
pub struct PreparedShCluster {
    pub generation: u64,
    pub content_tag: [u8; 32],
    pub chunk: DecodedClusterShPayload,
}

/// Bounded renderer handoff. The controller owns policy and permits; the
/// renderer owns only validation, installation, and the returned outcome.
#[derive(Debug, Default)]
pub struct ShDrainBatch {
    pub generation: u64,
    pub content_tag: [u8; 32],
    pub target_reset: Option<Vec<u64>>,
    pub target_add: Vec<u32>,
    pub target_remove: Vec<u32>,
    pub evictions: Vec<u32>,
    pub ready: Vec<PreparedShCluster>,
}

impl ShDrainBatch {
    /// Validate the complete loader-to-renderer handoff against the immutable
    /// manifest identity before either side mutates generation state.
    pub fn validate_contract(
        &self,
        cluster_count: u32,
        content_tag: [u8; 32],
    ) -> Result<(), PrlLoadError> {
        if self.generation == 0 {
            return Err(stream_error("drain batch generation must be nonzero"));
        }
        if self.content_tag != content_tag {
            return Err(stream_error(
                "drain batch content tag does not match manifest",
            ));
        }
        if self.ready.len() > 2 {
            return Err(stream_error(
                "drain batch exceeds the two-ready-cluster cap",
            ));
        }
        let words = usize::try_from(cluster_count.div_ceil(64))
            .map_err(|_| stream_error("cluster bitset word count exceeds usize"))?;
        if let Some(reset) = &self.target_reset {
            if reset.len() != words {
                return Err(stream_error(
                    "target reset bitset length disagrees with cluster count",
                ));
            }
            if let (Some(last), remainder) = (reset.last(), cluster_count % 64)
                && remainder != 0
                && (*last >> remainder) != 0
            {
                return Err(stream_error(
                    "target reset bitset names an out-of-range cluster",
                ));
            }
        }
        validate_sorted_cluster_ids(&self.target_add, cluster_count, "target-add")?;
        validate_sorted_cluster_ids(&self.target_remove, cluster_count, "target-remove")?;
        if sorted_lists_intersect(&self.target_add, &self.target_remove) {
            return Err(stream_error(
                "target-add and target-remove must not name the same cluster",
            ));
        }
        // The planner may list a dependent before a lower-ID owner. The
        // renderer computes its own dependency-safe release sequence, so
        // eviction IDs must be unique and in range, but not numerically sorted.
        validate_unique_cluster_ids(&self.evictions, cluster_count, "evictions")?;
        let mut ready_ids = std::collections::BTreeSet::new();
        for prepared in &self.ready {
            if prepared.generation != self.generation || prepared.content_tag != self.content_tag {
                return Err(stream_error(
                    "ready cluster identity does not match drain batch",
                ));
            }
            if prepared.chunk.cluster_id >= cluster_count {
                return Err(stream_error(
                    "ready cluster id exceeds manifest cluster count",
                ));
            }
            if !ready_ids.insert(prepared.chunk.cluster_id) {
                return Err(stream_error(
                    "drain batch contains more than one ready chunk for a cluster",
                ));
            }
        }
        Ok(())
    }
}

/// Renderer-to-controller ownership return. Deferred chunks remain owned so
/// the controller can keep their permit and ready-byte accounting exactly once.
#[derive(Debug, Default)]
pub struct ShDrainOutcome {
    pub accepted: Vec<u32>,
    pub dropped: Vec<u32>,
    pub deferred: Vec<PreparedShCluster>,
    /// Clusters actually released by the renderer, not merely requested for
    /// eviction. An installed owner may remain pinned by a dependent.
    pub evicted: Vec<u32>,
}

pub(super) fn validate_sorted_cluster_ids(
    ids: &[u32],
    cluster_count: u32,
    label: &'static str,
) -> Result<(), PrlLoadError> {
    if ids.iter().any(|&id| id >= cluster_count) {
        return Err(stream_error(format!(
            "{label} contains an out-of-range cluster id"
        )));
    }
    if ids.windows(2).any(|pair| pair[0] >= pair[1]) {
        return Err(stream_error(format!(
            "{label} must be sorted and deduplicated"
        )));
    }
    Ok(())
}

fn validate_unique_cluster_ids(
    ids: &[u32],
    cluster_count: u32,
    label: &'static str,
) -> Result<(), PrlLoadError> {
    let mut seen = std::collections::BTreeSet::new();
    for &id in ids {
        if id >= cluster_count {
            return Err(stream_error(format!(
                "{label} contains an out-of-range cluster id"
            )));
        }
        if !seen.insert(id) {
            return Err(stream_error(format!(
                "{label} contains a duplicate cluster id"
            )));
        }
    }
    Ok(())
}

pub(super) fn sorted_lists_intersect(left: &[u32], right: &[u32]) -> bool {
    let mut left_index = 0;
    let mut right_index = 0;
    while left_index < left.len() && right_index < right.len() {
        match left[left_index].cmp(&right[right_index]) {
            std::cmp::Ordering::Less => left_index += 1,
            std::cmp::Ordering::Greater => right_index += 1,
            std::cmp::Ordering::Equal => return true,
        }
    }
    false
}
