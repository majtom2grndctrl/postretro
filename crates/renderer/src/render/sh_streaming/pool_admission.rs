//! Making room for mandatory work: evict, then grow, then refuse.
//! See: context/lib/rendering_pipeline.md §4 (Bounded SH pool)

use std::collections::BTreeMap;

use super::gpu::{DenseGrowthInputs, SparseGrowth};
use super::pool_demand::PoolDemand;
use super::*;

/// One drain's capacity policy, as the controller named it. The renderer knows
/// what a free is worth and the controller knows what may go, so neither has
/// to estimate the other's figures.
#[derive(Debug, Default)]
pub(super) struct PoolAdmission {
    mandatory: Vec<u32>,
    /// Consumed across the drain's installs: one pass over the list at most.
    victims: std::vec::IntoIter<u32>,
}

impl PoolAdmission {
    pub(super) fn new(mandatory: Vec<u32>, victims: Vec<u32>) -> Self {
        Self {
            mandatory,
            victims: victims.into_iter(),
        }
    }

    fn is_mandatory(&self, cluster_id: u32) -> bool {
        self.mandatory.binary_search(&cluster_id).is_ok()
    }
}

impl ShResidencyState {
    /// Install `prepared`, making room if it is mandatory. A cluster stages at
    /// most twice: the shortfall is measured without staging, so the second
    /// attempt is known to fit. Optional work leaves `PoolShortfall` for the
    /// caller, having evicted and grown nothing.
    pub(super) fn install_admitted(
        &mut self,
        mut gpu: Option<&mut InstallGpu<'_>>,
        prepared: &PreparedShCluster,
        admission: &mut PoolAdmission,
        outcome: &mut ShDrainOutcome,
    ) -> Result<(), ShResidencyDrainError> {
        match self.install(gpu.as_deref_mut(), prepared) {
            Err(ShResidencyDrainError::PoolShortfall) => {}
            result => return result,
        }
        let cluster_id = prepared.chunk.cluster_id;
        if !admission.is_mandatory(cluster_id) {
            return Err(ShResidencyDrainError::PoolShortfall);
        }
        let gpu = gpu.ok_or(ShResidencyDrainError::GpuCapacity {
            reason: "streamed capacity admission requires device handles",
        })?;
        let demand = self.chunk_demand(cluster_id, &prepared.chunk)?;
        // Work behind a retiring generation waits for it, evicting nothing and
        // starting no growth, however many frames that takes. Its generation
        // was sized to the mandatory tier, so it usually needs neither.
        if let Some(family) = self.retirement_wait(&demand)? {
            return Err(ShResidencyDrainError::GpuRetirementPressure { family });
        }
        if !self.evict_until_placed(gpu, &demand, admission, outcome)? {
            self.grow_to_mandatory_demand(gpu, &demand, &admission.mandatory)?;
        }
        match self.install(Some(gpu), prepared) {
            Err(ShResidencyDrainError::PoolShortfall) => Err(ShResidencyDrainError::GpuCapacity {
                reason: "streamed pools still lack room after admission",
            }),
            result => result,
        }
    }

    fn retirement_wait(
        &self,
        demand: &PoolDemand,
    ) -> Result<Option<&'static str>, ShResidencyDrainError> {
        let Some(pools) = self.gpu.as_ref() else {
            return Ok(None);
        };
        let placement = self.place(demand)?;
        pools.retirement_wait(
            placement.dense_slots,
            &placement.sparse,
            &self.source_metadata,
        )
    }

    /// Walk the victims until `demand` places. Returns whether it does.
    ///
    /// Every zero this stages is submitted here, before the caller can stage a
    /// retry that reuses the freed ranges: the queue-order rule of `drain`.
    fn evict_until_placed(
        &mut self,
        gpu: &mut InstallGpu<'_>,
        demand: &PoolDemand,
        admission: &mut PoolAdmission,
        outcome: &mut ShDrainOutcome,
    ) -> Result<bool, ShResidencyDrainError> {
        let mut uploads = self.begin_uploads(0);
        let walked = self.walk_victims(&mut uploads, demand, admission, outcome);
        self.submit_uploads(uploads, gpu.device, gpu.queue);
        walked
    }

    fn walk_victims(
        &mut self,
        uploads: &mut StagedUploads,
        demand: &PoolDemand,
        admission: &mut PoolAdmission,
        outcome: &mut ShDrainOutcome,
    ) -> Result<bool, ShResidencyDrainError> {
        let mut placed = self.holds(&self.place(demand)?);
        while !placed {
            let Some(victim) = admission.victims.next() else {
                return Ok(false);
            };
            if !self.installed.contains_key(&victim) {
                continue;
            }
            // A victim sharing everything it names frees nothing, yet it goes
            // all the same: it is what keeps the next victim's storage live.
            let freed = self.evict(uploads, victim)?;
            outcome.evicted.push(victim);
            if freed {
                placed = self.holds(&self.place(demand)?);
            }
        }
        Ok(true)
    }

    /// Every family's capacity is the whole map and nothing has ever left any
    /// pool, so first-fit from empty has left no hole. Eviction has no work
    /// here, and without eviction this stays true. A level reaching the cap
    /// any other way reports false and keeps evicting, because victims are
    /// what recover fragmentation.
    pub(super) fn pool_holds_whole_map(&self) -> bool {
        self.gpu
            .as_ref()
            .is_some_and(StreamingGpuPools::holds_whole_map)
            && !self.dense_slots.has_released()
            && !self.sparse_pools.values().any(SparsePool::has_released)
    }

    /// A pool at the whole map holds every node and row if first-fit lets it,
    /// so this refusal means fragmentation stranded free slots. Nothing moves
    /// live storage, so the cluster stays dark until eviction opens a range
    /// that fits. The controller retries one hysteresis window after each
    /// refusal, for as long as the cluster stays a target.
    pub(super) fn warn_whole_map_refusal(&mut self, family: &str, cluster_id: u32) {
        if !std::mem::replace(&mut self.whole_map_refusal_warned, true) {
            log::warn!(
                "[SH streaming] fragmentation refused cluster {cluster_id} at the SH {family} \
                 pool's whole-map cap; surfaces it lights stay black"
            );
        }
    }

    /// Grow once, to the whole targeted mandatory demand: a second growth
    /// would wait behind this one's retirement, and doubling is what overshot
    /// the live set. The device limit and whole map clamp the size, and refuse
    /// only what `demand` itself cannot do without.
    fn grow_to_mandatory_demand(
        &mut self,
        gpu: &mut InstallGpu<'_>,
        demand: &PoolDemand,
        mandatory: &[u32],
    ) -> Result<(), ShResidencyDrainError> {
        let needed = self.place(demand)?;
        let wanted = self.place(&self.mandatory_demand(mandatory)?)?;
        let probe_occlusion_enabled = self.probe_occlusion_enabled();
        let Some(pools) = self.gpu.as_mut() else {
            return Ok(());
        };
        let layers = pools.shape.layers;
        pools.grow_dense(
            needed.dense_slots,
            wanted.dense_slots,
            DenseGrowthInputs {
                device: gpu.device,
                queue: gpu.queue,
                base: &self.base_metadata,
                sources: &self.source_metadata,
                probe_occlusion_enabled,
                sh: gpu.sh,
                selection_weights: gpu.selection_weights,
            },
        )?;
        if pools.shape.layers != layers {
            log::warn!(
                "[SH streaming] mandatory demand outgrew the SH dense pool: {layers} -> {} layers",
                pools.shape.layers
            );
        }
        let growth: BTreeMap<u32, SparseGrowth> = needed
            .sparse
            .iter()
            .map(|(&section_id, &needed)| {
                let wanted = wanted.sparse.get(&section_id).copied().unwrap_or(needed);
                (section_id, SparseGrowth { needed, wanted })
            })
            .collect();
        let capacity = |pools: &StreamingGpuPools, section_id: u32| {
            (
                pools.sparse_entry_capacity(section_id),
                pools.sparse_tile_f16_capacity(section_id),
            )
        };
        let before: BTreeMap<u32, (u32, u32)> = growth
            .keys()
            .map(|&section_id| (section_id, capacity(pools, section_id)))
            .collect();
        pools.grow_sparse(
            gpu.device,
            gpu.queue,
            &self.base_metadata,
            &self.source_metadata,
            &growth,
            gpu.sh,
            gpu.selection_weights,
        )?;
        for (section_id, before) in before {
            let after = capacity(pools, section_id);
            if after != before {
                log::warn!(
                    "[SH streaming] mandatory demand outgrew the SH sparse id-{section_id} pool: \
                     {before:?} -> {after:?} (entries, f16 tiles)"
                );
            }
        }
        Ok(())
    }
}
