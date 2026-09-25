//! Pure cluster-target policy and the synchronous SH streaming proof gate.
//! See: context/lib/rendering_pipeline.md §4

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use postretro_level_loader::{
    CellVisibility, PreparedShCluster, PrlLoadError, ShDrainBatch, ShDrainOutcome, ShStreamManifest,
};
use thiserror::Error;

use super::budget::{FixedGpuCharges, ShGpuBudgetInputs, ShResidencyAccounting};
use super::generation::{GenerationClock, ProcessGenerationClock};
use super::topology::PlannerTopology;
use super::warm_set::{WarmSet, WarmSource};

#[path = "lifecycle.rs"]
mod lifecycle;
#[path = "pressure.rs"]
mod pressure;
#[path = "targeting.rs"]
mod targeting;

/// Cluster-hop radius of the warm-set fallback used without usable id 46.
pub(crate) const PREFETCH_HOPS: u8 = 2;
/// Warm-set size cap in distinct clusters, including the camera's own.
pub(crate) const WARM_SET_CLUSTERS: usize = 8;
/// Bounds one warm walk, which runs only when the camera changes cell.
pub(crate) const WARM_WALK_MAX_SETTLED_CELLS: usize = 4096;
pub(crate) const HYSTERESIS_SECONDS: f64 = 2.0;
/// A permit covers one cluster from request until install, drop, or failure.
pub(crate) const MAX_STREAM_PERMITS: usize = 8;
/// Decoded bytes handed to the renderer per drain. The first eligible cluster
/// is always admitted, so one oversized chunk can still install.
pub(crate) const MAX_INSTALL_DECODED_BYTES_PER_DRAIN: u64 = 8 * 1024 * 1024;

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
    pub(crate) chunk_hash: [u8; 32],
    /// The cluster's class was `Visible` or `Pinned` (owner closure included)
    /// when requested. Mandatory reads are served before optional ones.
    pub(crate) mandatory: bool,
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
    Pinned,
    SeamWarm,
    Prefetch,
    Hysteresis,
}

impl TargetClass {
    const fn is_pressure_eligible(self) -> bool {
        matches!(self, Self::SeamWarm | Self::Prefetch)
    }
}

/// The class/priority pair propagated through owner closure. Priority only
/// belongs to cold optional work; protected classes intentionally carry zero
/// so authored policy cannot reorder visible, pinned, or retained work.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct TargetDirective {
    class: TargetClass,
    effective_priority: u32,
}

impl TargetDirective {
    const fn new(class: TargetClass, authored_priority: u32) -> Self {
        Self {
            class,
            effective_priority: if class.is_pressure_eligible() {
                authored_priority
            } else {
                0
            },
        }
    }

    fn supersedes(self, current: Self) -> bool {
        self.class < current.class
            || (self.class == current.class
                && self.class.is_pressure_eligible()
                && self.effective_priority > current.effective_priority)
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct ShEvictionKey {
    pub(crate) last_visible_time: f64,
    pub(crate) last_target_time: f64,
    pub(crate) cluster_id: u32,
}

/// Cumulative since controller creation, except `last_drain_decoded_bytes`
/// (a gauge) and `max_drain_decoded_bytes` (a high-water mark).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct ShResidencyCounters {
    pub(crate) misses: u64,
    pub(crate) installs: u64,
    pub(crate) evictions: u64,
    pub(crate) retries: u64,
    /// Completed reads that admission dropped as stale, not targeted, or
    /// duplicate, and their decoded bytes.
    pub(crate) discarded_reads: u64,
    pub(crate) discarded_read_bytes: u64,
    pub(crate) cancelled_requests: u64,
    /// Decoded bytes handed to the renderer in drain batches. A chunk the
    /// renderer defers is counted again when it is handed over again.
    pub(crate) decoded_bytes_installed: u64,
    /// Decoded bytes of the most recent drain that handed over any cluster.
    pub(crate) last_drain_decoded_bytes: u64,
    pub(crate) max_drain_decoded_bytes: u64,
    /// Drains that left eligible ready work behind because of the byte budget.
    pub(crate) budget_limited_drains: u64,
}

/// Plain controller-owned state for the live SH report. GPU pool figures stay
/// renderer-owned; this half owns permits, CPU payload phases, and policy
/// counters so neither side has to borrow the other's lifetime state.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct ShResidencyControllerSnapshot {
    pub(crate) cpu: super::budget::CpuPhaseLedger,
    pub(crate) permits_in_use: usize,
    pub(crate) target_clusters: usize,
    pub(crate) warm_clusters: usize,
    pub(crate) absent_clusters: usize,
    pub(crate) queued_clusters: usize,
    pub(crate) ready_clusters: usize,
    pub(crate) installed_uncomposed_clusters: usize,
    pub(crate) sampleable_clusters: usize,
    pub(crate) failed_clusters: usize,
    pub(crate) non_evictable_overshoot_bytes: u64,
    pub(crate) counters: ShResidencyCounters,
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
    effective_priority: u32,
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
            effective_priority: 0,
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
    warm_source: WarmSource,
    /// Cached for `warm.camera_cell()`; recomputed only when that cell changes.
    warm: WarmSet,
    generation: u64,
    content_tag: [u8; 32],
    states: Vec<ClusterState>,
    targets: BTreeSet<u32>,
    last_horizon: BTreeSet<u32>,
    last_seam_warm: BTreeSet<u32>,
    horizon_revision: u64,
    last_time: Option<f64>,
    needs_target_reset: bool,
    sent_targets: BTreeSet<u32>,
    ready: BTreeMap<u32, ReadyCluster>,
    in_drain: BTreeMap<u32, u64>,
    in_drain_evictions: BTreeSet<u32>,
    permits_in_use: usize,
    accounting: ShResidencyAccounting,
    non_evictable_overshoot_bytes: u64,
    overshoot_reported: bool,
    prior_visible_misses: BTreeSet<u32>,
    counters: ShResidencyCounters,
}

impl ShResidencyController {
    /// `cell_visibility` is the same level's loaded id-46 section; the
    /// controller builds its own per-cell adjacency from it.
    pub(crate) fn new(
        manifest: Arc<ShStreamManifest>,
        gpu_budget: ShGpuBudgetInputs,
        cell_visibility: Option<&CellVisibility>,
    ) -> Result<Self, ShResidencyControllerError> {
        Self::with_clock(
            manifest,
            gpu_budget,
            cell_visibility,
            &ProcessGenerationClock,
        )
    }

    pub(crate) fn with_clock(
        manifest: Arc<ShStreamManifest>,
        gpu_budget: ShGpuBudgetInputs,
        cell_visibility: Option<&CellVisibility>,
        clock: &impl GenerationClock,
    ) -> Result<Self, ShResidencyControllerError> {
        let topology = PlannerTopology::from_manifest(&manifest)?;
        let warm_source =
            WarmSource::from_cell_visibility(cell_visibility, topology.cell_to_cluster.len());
        Self::from_parts(Some(manifest), topology, warm_source, clock, gpu_budget)
    }

    fn from_parts(
        manifest: Option<Arc<ShStreamManifest>>,
        topology: PlannerTopology,
        warm_source: WarmSource,
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
            || topology.encoded_chunk_bytes.len() != cluster_count
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
            warm_source,
            warm: WarmSet::default(),
            generation,
            content_tag,
            states: vec![ClusterState::default(); cluster_count],
            targets: BTreeSet::new(),
            last_horizon: BTreeSet::new(),
            last_seam_warm: BTreeSet::new(),
            horizon_revision: 0,
            last_time: None,
            needs_target_reset: true,
            sent_targets: BTreeSet::new(),
            ready: BTreeMap::new(),
            in_drain: BTreeMap::new(),
            in_drain_evictions: BTreeSet::new(),
            permits_in_use: 0,
            accounting: ShResidencyAccounting::new(gpu_budget)?,
            non_evictable_overshoot_bytes: 0,
            overshoot_reported: false,
            prior_visible_misses: BTreeSet::new(),
            counters: ShResidencyCounters::default(),
        })
    }

    #[cfg(test)]
    pub(crate) fn generation(&self) -> u64 {
        self.generation
    }

    #[cfg(test)]
    pub(crate) fn content_tag(&self) -> [u8; 32] {
        self.content_tag
    }

    #[cfg(test)]
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

    #[cfg(test)]
    pub(crate) fn state(&self, cluster_id: u32) -> Option<ClusterResidencyState> {
        self.states
            .get(cluster_id as usize)
            .map(|state| state.state)
    }

    #[cfg(test)]
    pub(crate) fn is_targeted(&self, cluster_id: u32) -> bool {
        self.targets.contains(&cluster_id)
    }

    /// The current target set, published to the I/O issuer for pre-read
    /// cancellation.
    pub(crate) fn targets(&self) -> &BTreeSet<u32> {
        &self.targets
    }

    /// True only when every current visible/prefetch/owner target has crossed
    /// the renderer-confirmed one-frame compose boundary. Static capture uses
    /// this to know when its deterministic preload is complete.
    #[cfg(any(test, feature = "capture"))]
    pub(crate) fn all_targets_sampleable(&self) -> bool {
        self.targets.iter().all(|&cluster_id| {
            self.states
                .get(cluster_id as usize)
                .is_some_and(|state| state.state == ClusterResidencyState::Sampleable)
        })
    }

    #[cfg(test)]
    pub(crate) fn permits_in_use(&self) -> usize {
        self.permits_in_use
    }

    #[cfg(test)]
    pub(crate) fn counters(&self) -> ShResidencyCounters {
        self.counters
    }

    #[cfg(test)]
    pub(crate) fn non_evictable_overshoot_bytes(&self) -> u64 {
        self.non_evictable_overshoot_bytes
    }

    pub(crate) fn report_snapshot(&self) -> ShResidencyControllerSnapshot {
        let mut snapshot = ShResidencyControllerSnapshot {
            cpu: self.accounting.cpu,
            permits_in_use: self.permits_in_use,
            target_clusters: self.targets.len(),
            warm_clusters: self.warm.len(),
            non_evictable_overshoot_bytes: self.non_evictable_overshoot_bytes,
            counters: self.counters,
            ..ShResidencyControllerSnapshot::default()
        };
        for state in &self.states {
            match state.state {
                ClusterResidencyState::Absent => snapshot.absent_clusters += 1,
                ClusterResidencyState::Queued => snapshot.queued_clusters += 1,
                ClusterResidencyState::Ready => snapshot.ready_clusters += 1,
                ClusterResidencyState::InstalledUncomposed => {
                    snapshot.installed_uncomposed_clusters += 1
                }
                ClusterResidencyState::Sampleable => snapshot.sampleable_clusters += 1,
                ClusterResidencyState::Failed => snapshot.failed_clusters += 1,
            }
        }
        snapshot
    }

    fn increment_counter(
        counter: &mut u64,
        label: &'static str,
    ) -> Result<(), ShResidencyControllerError> {
        Self::add_to_counter(counter, 1, label)
    }

    fn add_to_counter(
        counter: &mut u64,
        amount: u64,
        label: &'static str,
    ) -> Result<(), ShResidencyControllerError> {
        *counter = counter
            .checked_add(amount)
            .ok_or(ShResidencyControllerError::AccountingOverflow(label))?;
        Ok(())
    }

    /// Rank of a cluster in the current warm set; non-warm clusters rank last.
    fn warm_rank(&self, cluster_id: u32) -> u32 {
        self.warm.rank(cluster_id)
    }

    fn ready_for_install(&self, cluster_id: u32) -> bool {
        self.targets.contains(&cluster_id)
            && self.topology.owners[cluster_id as usize]
                .iter()
                .all(|&owner| {
                    self.states[owner as usize].state == ClusterResidencyState::Sampleable
                })
    }

    fn mark_failed(&mut self, cluster_id: u32) -> Result<bool, ShResidencyControllerError> {
        let identity = FailureIdentity {
            generation: self.generation,
            content_tag: self.content_tag,
            cluster_id,
            chunk_hash: self.topology.chunk_hashes[cluster_id as usize],
        };
        self.release_permit()?;
        let state = &mut self.states[cluster_id as usize];
        let same_identity = state
            .failure
            .is_some_and(|failure| failure.identity == identity);
        let retry_spent = same_identity && state.failure.is_some_and(|failure| failure.retry_spent);
        // The retained failure survives its one permitted leave/re-enter
        // retry, so the retry cannot emit the same identity's warning again.
        let should_warn = !same_identity;
        state.state = ClusterResidencyState::Failed;
        state.failure = Some(FailureState {
            identity,
            retry_spent,
            left_target: false,
            left_target_horizon_revision: None,
        });
        Ok(should_warn)
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

    /// Test controllers have no id 46, so they use the cluster-hop fallback.
    #[cfg(test)]
    fn for_test(
        topology: PlannerTopology,
        clock: &impl GenerationClock,
    ) -> Result<Self, ShResidencyControllerError> {
        Self::for_test_with_budget(topology, clock, ShGpuBudgetInputs::default())
    }

    #[cfg(test)]
    fn for_test_with_budget(
        topology: PlannerTopology,
        clock: &impl GenerationClock,
        gpu_budget: ShGpuBudgetInputs,
    ) -> Result<Self, ShResidencyControllerError> {
        let warm_source = WarmSource::resolve(
            None::<(usize, &[postretro_level_loader::CoupledCellPair])>,
            topology.cell_to_cluster.len(),
        );
        Self::from_parts(None, topology, warm_source, clock, gpu_budget)
    }

    /// A test controller whose warm walk runs over the given id-46 pairs.
    #[cfg(test)]
    fn for_test_with_cell_pairs(
        topology: PlannerTopology,
        pairs: &[postretro_level_loader::CoupledCellPair],
        gpu_budget: ShGpuBudgetInputs,
    ) -> Result<Self, ShResidencyControllerError> {
        let cell_count = topology.cell_to_cluster.len();
        let warm_source = WarmSource::resolve(Some((cell_count, pairs)), cell_count);
        Self::from_parts(
            None,
            topology,
            warm_source,
            &super::generation::FixedGenerationClock::new(1),
            gpu_budget,
        )
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
    for &cluster_id in outcome
        .accepted
        .iter()
        .chain(&outcome.dropped)
        .chain(&outcome.evicted)
    {
        if !ids.insert(cluster_id) {
            return Err(ShResidencyControllerError::InvalidDrainOutcome(
                "accepted, dropped, and evicted ids must not overlap or repeat".into(),
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
#[cfg(test)]
#[path = "controller_warm_and_budget_tests.rs"]
mod warm_and_budget_tests;
