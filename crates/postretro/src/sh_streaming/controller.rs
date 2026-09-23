//! Pure cluster-target policy and the synchronous SH streaming proof gate.
//! See: context/plans/in-progress/sh-probe-streaming--cluster-residency/index.md

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use postretro_level_loader::{
    PreparedShCluster, PrlLoadError, ShDrainBatch, ShDrainOutcome, ShStreamManifest,
};
use thiserror::Error;

use super::budget::{FixedGpuCharges, ShGpuBudgetInputs, ShResidencyAccounting};
use super::generation::{GenerationClock, ProcessGenerationClock};
use super::topology::PlannerTopology;

#[path = "lifecycle.rs"]
mod lifecycle;
#[path = "targeting.rs"]
mod targeting;

pub(crate) const PREFETCH_HOPS: u8 = 2;
pub(crate) const HYSTERESIS_SECONDS: f64 = 2.0;
pub(crate) const MAX_STREAM_PERMITS: usize = 4;
pub(crate) const MAX_INSTALLS_PER_DRAIN: usize = 2;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ClusterResidencyState {
    Absent,
    Queued,
    Ready,
    InstalledUncomposed,
    Sampleable,
    Failed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ShDrainAdmission {
    Ready,
    DroppedStale,
    DroppedNotTargeted,
    DroppedDuplicate,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ShClusterRequest {
    pub(crate) generation: u64,
    pub(crate) content_tag: [u8; 32],
    pub(crate) cluster_id: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SyncReadResult {
    NoTargetReady,
    Prepared(u32),
}

#[derive(Debug, Error)]
pub(crate) enum ShResidencyControllerError {
    #[error("SH streaming residency generation is exhausted")]
    GenerationExhausted,
    #[error("SH streaming planner has no retained manifest for synchronous reads")]
    MissingManifest,
    #[error("SH streaming planner time must be finite, nonnegative, and monotonic")]
    InvalidMonotonicTime,
    #[error("SH streaming planner topology is invalid: {0}")]
    InvalidTopology(String),
    #[error("SH streaming owner dependency graph contains a cycle at cluster {0}")]
    OwnerCycle(u32),
    #[error("SH streaming accounting overflow while charging {0}")]
    AccountingOverflow(&'static str),
    #[error("SH streaming accounting underflow while releasing {0}")]
    AccountingUnderflow(&'static str),
    #[error("SH streaming drain outcome is invalid: {0}")]
    InvalidDrainOutcome(String),
    #[error("SH streaming synchronous chunk read failed: {0}")]
    SourceRead(#[source] PrlLoadError),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum TargetClass {
    Visible,
    Prefetch,
    Hysteresis,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct ShEvictionKey {
    pub(crate) last_visible_time: f64,
    pub(crate) last_target_time: f64,
    pub(crate) cluster_id: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct FailureIdentity {
    generation: u64,
    content_tag: [u8; 32],
    cluster_id: u32,
    chunk_hash: [u8; 32],
}

#[derive(Debug, Clone, Copy)]
struct FailureState {
    identity: FailureIdentity,
    retry_spent: bool,
    left_target: bool,
    left_target_horizon_revision: Option<u64>,
}

#[derive(Debug, Clone)]
struct ClusterState {
    state: ClusterResidencyState,
    last_visible_time: Option<f64>,
    last_target_time: Option<f64>,
    hysteresis_started_at: Option<f64>,
    class: Option<TargetClass>,
    suppressed: bool,
    failure: Option<FailureState>,
}

impl Default for ClusterState {
    fn default() -> Self {
        Self {
            state: ClusterResidencyState::Absent,
            last_visible_time: None,
            last_target_time: None,
            hysteresis_started_at: None,
            class: None,
            suppressed: false,
            failure: None,
        }
    }
}

#[derive(Debug)]
struct ReadyCluster {
    prepared: PreparedShCluster,
    byte_charge: u64,
}

/// Session-local SH policy. It has no renderer or wgpu types: a renderer sees
/// only the loader-owned `ShDrainBatch`, then returns `ShDrainOutcome`.
#[derive(Debug)]
pub(crate) struct ShResidencyController {
    manifest: Option<Arc<ShStreamManifest>>,
    topology: PlannerTopology,
    generation: u64,
    content_tag: [u8; 32],
    states: Vec<ClusterState>,
    targets: BTreeSet<u32>,
    last_horizon: BTreeSet<u32>,
    horizon_revision: u64,
    last_time: Option<f64>,
    needs_target_reset: bool,
    sent_targets: BTreeSet<u32>,
    ready: BTreeMap<u32, ReadyCluster>,
    in_drain: BTreeMap<u32, u64>,
    permits_in_use: usize,
    accounting: ShResidencyAccounting,
}

impl ShResidencyController {
    pub(crate) fn new(
        manifest: Arc<ShStreamManifest>,
        gpu_budget: ShGpuBudgetInputs,
    ) -> Result<Self, ShResidencyControllerError> {
        Self::with_clock(manifest, gpu_budget, &ProcessGenerationClock)
    }

    pub(crate) fn with_clock(
        manifest: Arc<ShStreamManifest>,
        gpu_budget: ShGpuBudgetInputs,
        clock: &impl GenerationClock,
    ) -> Result<Self, ShResidencyControllerError> {
        let topology = PlannerTopology::from_manifest(&manifest)?;
        Self::from_parts(Some(manifest), topology, clock, gpu_budget)
    }

    fn from_parts(
        manifest: Option<Arc<ShStreamManifest>>,
        topology: PlannerTopology,
        clock: &impl GenerationClock,
        gpu_budget: ShGpuBudgetInputs,
    ) -> Result<Self, ShResidencyControllerError> {
        let generation = clock
            .take_generation()
            .filter(|&generation| generation != 0)
            .ok_or(ShResidencyControllerError::GenerationExhausted)?;
        let cluster_count = topology.cluster_count();
        if topology.owners.len() != cluster_count
            || topology.requested_resident_bytes.len() != cluster_count
            || topology.chunk_hashes.len() != cluster_count
        {
            return Err(ShResidencyControllerError::InvalidTopology(
                "planner topology lengths disagree".into(),
            ));
        }
        let content_tag = manifest
            .as_ref()
            .map_or([0; 32], |manifest| manifest.content_tag());
        Ok(Self {
            manifest,
            topology,
            generation,
            content_tag,
            states: vec![ClusterState::default(); cluster_count],
            targets: BTreeSet::new(),
            last_horizon: BTreeSet::new(),
            horizon_revision: 0,
            last_time: None,
            needs_target_reset: true,
            sent_targets: BTreeSet::new(),
            ready: BTreeMap::new(),
            in_drain: BTreeMap::new(),
            permits_in_use: 0,
            accounting: ShResidencyAccounting::new(gpu_budget)?,
        })
    }

    pub(crate) fn generation(&self) -> u64 {
        self.generation
    }

    pub(crate) fn content_tag(&self) -> [u8; 32] {
        self.content_tag
    }

    pub(crate) fn accounting(&self) -> ShResidencyAccounting {
        self.accounting
    }

    /// Task 9 updates actual fixed metadata, whole-resident scatter, and pool
    /// capacity after the renderer allocates or grows its physical resources.
    pub(crate) fn update_gpu_charges(
        &mut self,
        fixed_gpu: FixedGpuCharges,
    ) -> Result<(), ShResidencyControllerError> {
        self.accounting.update_fixed_gpu(fixed_gpu)
    }

    pub(crate) fn state(&self, cluster_id: u32) -> Option<ClusterResidencyState> {
        self.states
            .get(cluster_id as usize)
            .map(|state| state.state)
    }

    pub(crate) fn is_targeted(&self, cluster_id: u32) -> bool {
        self.targets.contains(&cluster_id)
    }

    /// True only when every current visible/prefetch/owner target has crossed
    /// the renderer-confirmed one-frame compose boundary. Static capture uses
    /// this to know when its deterministic preload is complete.
    #[cfg_attr(not(feature = "capture"), allow(dead_code))]
    pub(crate) fn all_targets_sampleable(&self) -> bool {
        self.targets.iter().all(|&cluster_id| {
            self.states
                .get(cluster_id as usize)
                .is_some_and(|state| state.state == ClusterResidencyState::Sampleable)
        })
    }

    pub(crate) fn permits_in_use(&self) -> usize {
        self.permits_in_use
    }

    fn ready_for_install(&self, cluster_id: u32) -> bool {
        self.targets.contains(&cluster_id)
            && self.topology.owners[cluster_id as usize]
                .iter()
                .all(|&owner| {
                    self.states[owner as usize].state == ClusterResidencyState::Sampleable
                })
    }

    fn mark_failed(&mut self, cluster_id: u32) -> Result<(), ShResidencyControllerError> {
        let identity = FailureIdentity {
            generation: self.generation,
            content_tag: self.content_tag,
            cluster_id,
            chunk_hash: self.topology.chunk_hashes[cluster_id as usize],
        };
        self.release_permit()?;
        let state = &mut self.states[cluster_id as usize];
        let retry_spent = state
            .failure
            .is_some_and(|failure| failure.identity == identity && failure.retry_spent);
        state.state = ClusterResidencyState::Failed;
        state.failure = Some(FailureState {
            identity,
            retry_spent,
            left_target: false,
            left_target_horizon_revision: None,
        });
        Ok(())
    }

    fn release_ready_charge(&mut self, bytes: u64) -> Result<(), ShResidencyControllerError> {
        self.accounting.cpu.ready.remove(bytes, "ready bytes")
    }

    fn release_permit(&mut self) -> Result<(), ShResidencyControllerError> {
        self.permits_in_use = self.permits_in_use.checked_sub(1).ok_or(
            ShResidencyControllerError::AccountingUnderflow("stream permits"),
        )?;
        Ok(())
    }

    #[cfg(test)]
    fn for_test(
        topology: PlannerTopology,
        clock: &impl GenerationClock,
    ) -> Result<Self, ShResidencyControllerError> {
        Self::from_parts(None, topology, clock, ShGpuBudgetInputs::default())
    }
}

fn target_bitset(
    cluster_count: usize,
    targets: &BTreeSet<u32>,
) -> Result<Vec<u64>, ShResidencyControllerError> {
    let words = cluster_count.div_ceil(64);
    let mut bitset = vec![0; words];
    for &cluster_id in targets {
        let index = usize::try_from(cluster_id).map_err(|_| {
            ShResidencyControllerError::InvalidTopology("cluster id exceeds usize".into())
        })?;
        let word = bitset.get_mut(index / 64).ok_or_else(|| {
            ShResidencyControllerError::InvalidTopology("target cluster is out of range".into())
        })?;
        *word |= 1u64 << (index % 64);
    }
    Ok(bitset)
}

fn validate_outcome_lists(outcome: &ShDrainOutcome) -> Result<(), ShResidencyControllerError> {
    let mut ids = BTreeSet::new();
    for &cluster_id in outcome.accepted.iter().chain(&outcome.dropped) {
        if !ids.insert(cluster_id) {
            return Err(ShResidencyControllerError::InvalidDrainOutcome(
                "accepted and dropped ids overlap or repeat".into(),
            ));
        }
    }
    for prepared in &outcome.deferred {
        if !ids.insert(prepared.chunk.cluster_id) {
            return Err(ShResidencyControllerError::InvalidDrainOutcome(
                "deferred id overlaps another outcome".into(),
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
#[path = "controller_tests.rs"]
mod tests;
