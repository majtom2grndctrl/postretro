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

/// Renderer-to-controller ownership return. Deferred chunks remain owned so
/// the controller can keep their permit and ready-byte accounting exactly once.
#[derive(Debug, Default)]
pub struct ShDrainOutcome {
    pub accepted: Vec<u32>,
    pub dropped: Vec<u32>,
    pub deferred: Vec<PreparedShCluster>,
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
