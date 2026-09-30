//! SH's half of the level-scope drain: target deltas, ready offers, admitted installs.
//! See: context/lib/rendering_pipeline.md §4

use super::*;
use crate::streaming::drain_budget::{DrainItem, DrainRank};
use crate::streaming::request::StreamResource;
use crate::streaming::shared_drain::SharedDrain;

impl ShResidencyController {
    /// Emits a decoded-byte-bounded set of renderer-ready chunks plus an
    /// initial reset or sorted target deltas for the sync-proof path. This path
    /// deliberately never emits evictions or changes targets for budget
    /// pressure: the sync-proof capture path stays a stable no-eviction
    /// baseline.
    #[cfg(any(test, feature = "capture"))]
    pub(crate) fn take_drain_batch(&mut self) -> Result<ShDrainBatch, ShResidencyControllerError> {
        self.take_drain_batch_alone(false)
    }

    /// Async-only drain policy. Departed residents leave before pressure can
    /// suppress cold prefetch, and every eviction remains only a request until
    /// the renderer confirms that no installed dependent pins its owner.
    #[cfg(any(test, feature = "capture"))]
    pub(crate) fn take_async_drain_batch(
        &mut self,
    ) -> Result<ShDrainBatch, ShResidencyControllerError> {
        self.take_drain_batch_alone(true)
    }

    /// One async frame's controller work after completions are admitted: the
    /// drain batch first, then new read requests. The drain's budget policy
    /// may suppress optional targets, so requests taken before it could name
    /// a cluster this same frame has already dropped.
    #[cfg(test)]
    pub(crate) fn take_async_drain_batch_and_requests(
        &mut self,
    ) -> Result<(ShDrainBatch, Vec<ShClusterRequest>), ShResidencyControllerError> {
        let batch = self.take_async_drain_batch()?;
        let requests = self.take_requests()?;
        Ok((batch, requests))
    }

    /// Every request the controller's permits allow now. Call after the
    /// drain, whose budget policy may have suppressed optional targets.
    pub(crate) fn take_requests(
        &mut self,
    ) -> Result<Vec<ShClusterRequest>, ShResidencyControllerError> {
        let mut requests = Vec::new();
        while let Some(request) = self.take_next_request()? {
            requests.push(request);
        }
        Ok(requests)
    }

    /// A drain with SH as the only resource: the shared admission over SH's
    /// ready clusters alone.
    #[cfg(any(test, feature = "capture"))]
    fn take_drain_batch_alone(
        &mut self,
        eviction_enabled: bool,
    ) -> Result<ShDrainBatch, ShResidencyControllerError> {
        let mut drain = SharedDrain::default();
        let mut batch = self.begin_drain(eviction_enabled, &mut drain)?;
        drain
            .admit()
            .map_err(|_| ShResidencyControllerError::AccountingOverflow("drain decoded bytes"))?;
        self.finish_drain(&mut batch, &drain)?;
        Ok(batch)
    }

    /// First half of one drain: budget policy (async), target deltas,
    /// departed-ready cleanup and evictions into the returned batch, and every
    /// installable ready cluster offered to `drain`. The level-scope owner
    /// admits the merged list once, then calls [`Self::finish_drain`].
    pub(crate) fn begin_drain(
        &mut self,
        eviction_enabled: bool,
        drain: &mut SharedDrain,
    ) -> Result<ShDrainBatch, ShResidencyControllerError> {
        if !self.in_drain.is_empty() || !self.in_drain_evictions.is_empty() {
            return Err(ShResidencyControllerError::InvalidDrainOutcome(
                "previous drain outcome was not applied before preparing another batch".into(),
            ));
        }
        if eviction_enabled {
            self.apply_budget_policy()?;
        }
        let mut batch = ShDrainBatch {
            generation: self.generation,
            content_tag: self.content_tag,
            ..ShDrainBatch::default()
        };
        if self.needs_target_reset {
            batch.target_reset = Some(target_bitset(self.topology.cluster_count(), &self.targets)?);
            self.needs_target_reset = false;
            self.sent_targets = self.targets.clone();
        } else {
            batch.target_add = self
                .targets
                .difference(&self.sent_targets)
                .copied()
                .collect();
            batch.target_remove = self
                .sent_targets
                .difference(&self.targets)
                .copied()
                .collect();
            self.sent_targets = self.targets.clone();
        }
        self.drop_departed_ready(&self.targets.clone())?;

        if eviction_enabled {
            batch.evictions = self.departed_eviction_order();
        }
        self.in_drain_evictions = batch.evictions.iter().copied().collect();
        for (&cluster_id, ready) in &self.ready {
            if !self.ready_for_install(cluster_id) {
                continue;
            }
            let state = &self.states[cluster_id as usize];
            drain.offer(DrainItem {
                rank: DrainRank::sh(
                    state.class.unwrap_or(TargetClass::Hysteresis).drain_class(),
                    state.effective_priority,
                    cluster_id,
                ),
                bytes: ready.byte_charge,
            });
        }
        Ok(batch)
    }

    /// Second half of one drain: SH's admitted clusters move into the batch
    /// in admission order, and the drain counters record SH's share.
    pub(crate) fn finish_drain(
        &mut self,
        batch: &mut ShDrainBatch,
        drain: &SharedDrain,
    ) -> Result<(), ShResidencyControllerError> {
        let share = drain.admission(StreamResource::Sh);
        let mut counters = self.counters;
        Self::add_to_counter(
            &mut counters.decoded_bytes_installed,
            share.bytes,
            "decoded bytes installed",
        )?;
        if share.admitted > 0 {
            counters.last_drain_decoded_bytes = share.bytes;
            counters.max_drain_decoded_bytes = counters.max_drain_decoded_bytes.max(share.bytes);
        }
        if share.left_behind {
            Self::increment_counter(&mut counters.budget_limited_drains, "budget-limited drains")?;
        }
        self.counters = counters;
        for cluster_id in drain.admitted_keys(StreamResource::Sh) {
            let ready = self
                .ready
                .remove(&cluster_id)
                .expect("admitted SH key came from the ready map");
            self.in_drain.insert(cluster_id, ready.byte_charge);
            batch.ready.push(ready.prepared);
        }
        Ok(())
    }
}
