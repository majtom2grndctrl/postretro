//! Frame-bound compose dispatch and streamed residency reporting.

use super::*;

fn coalesce_rows(rows: &BTreeSet<u32>) -> Vec<(u32, u32)> {
    let mut ranges: Vec<(u32, u32)> = Vec::new();
    for &row in rows {
        match ranges.last_mut() {
            Some((start, count)) if start.checked_add(*count) == Some(row) => {
                *count = count
                    .checked_add(1)
                    .expect("validated affinity row count must fit u32");
            }
            _ => ranges.push((row, 1)),
        }
    }
    ranges
}

/// Collect the two direct-compose dirty unions for this frame. Pass B may
/// own an id-45 row without owning an id-35/id-41 row itself. During a forced
/// resident refresh, seed Pass A for that row first so Pass B never samples
/// an uninitialized intermediate target.
fn direct_dispatch_rows(
    force_resident: bool,
    promotion_dirty: &BTreeSet<u32>,
    animated_dirty: &BTreeSet<u32>,
    promotion_resident: &BTreeSet<u32>,
    animated_resident: &BTreeSet<u32>,
) -> (BTreeSet<u32>, BTreeSet<u32>) {
    let mut promotion_rows = promotion_dirty.clone();
    let mut animated_rows = animated_dirty.clone();
    if force_resident {
        promotion_rows.extend(promotion_resident);
        animated_rows.extend(animated_resident);
        // Id-45-only owners have no id-35/id-41 contribution to Pass A's
        // normal dirty union. A forced recompose must nevertheless rebuild
        // their intermediate cell before Pass B samples it.
        promotion_rows.extend(&animated_rows);
    }
    (promotion_rows, animated_rows)
}

impl ShResidencyState {
    #[expect(
        clippy::too_many_arguments,
        reason = "the frame boundary must pass the queue, encoder, uniform binding, direct-compose state, and separate promotion/animated timing-pass inputs together"
    )]
    pub(in crate::render) fn dispatch_direct_compose<'a>(
        &mut self,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        uniform_bind_group: &wgpu::BindGroup,
        active: bool,
        frame_light_term_mask: LightTermMask,
        promotion_override: DirectShDebugOverride,
        animated_override: AnimatedDirectShDebugOverride,
        promoted_animated_states: &[PromotedBakedLightState],
        promotion_timestamp_writes: Option<wgpu::ComputePassTimestampWrites<'a>>,
        animated_timestamp_writes: Option<wgpu::ComputePassTimestampWrites<'a>>,
    ) -> Result<(), ShResidencyDrainError> {
        if !self.direct_compose_required {
            return Ok(());
        }
        let force_resident =
            active || self.direct_was_active || frame_light_term_mask != self.last_direct_mask;
        let (promotion_rows, animated_rows) = direct_dispatch_rows(
            force_resident,
            &self.direct_promotion_dirty_rows,
            &self.direct_animated_dirty_rows,
            &self.direct_promotion_resident_rows,
            &self.direct_animated_resident_rows,
        );
        if promotion_rows.is_empty() && animated_rows.is_empty() {
            self.direct_was_active = active;
            self.last_direct_mask = frame_light_term_mask;
            return Ok(());
        }
        let gpu = self
            .gpu
            .as_mut()
            .ok_or(ShResidencyDrainError::GpuCapacity {
                reason: "streamed direct compose dispatch requested before GPU initialization",
            })?;
        gpu.dispatch_direct_compose(
            queue,
            encoder,
            uniform_bind_group,
            frame_light_term_mask,
            promotion_override,
            animated_override,
            promoted_animated_states,
            &coalesce_rows(&promotion_rows),
            &coalesce_rows(&animated_rows),
            force_resident,
            promotion_timestamp_writes,
            animated_timestamp_writes,
        )?;
        self.direct_compose_epoch = self
            .direct_compose_epoch
            .checked_add(1)
            .ok_or(ShResidencyDrainError::SlotOverflow)?;
        self.direct_promotion_dirty_rows.clear();
        self.direct_animated_dirty_rows.clear();
        self.dirty_rows.retain(|(section, _)| {
            *section != DIRECT_DELTA_ID && *section != ANIMATED_DIRECT_DELTA_ID
        });
        self.direct_was_active = active;
        self.last_direct_mask = frame_light_term_mask;
        Ok(())
    }

    /// Encode only coalesced affinity-row work after the one pre-compose
    /// residency drain. Sampling remains on the previous mirror until the
    /// next drain promotes these completed compose writes.
    pub(in crate::render) fn dispatch_indirect_compose<'a>(
        &mut self,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        uniform_bind_group: &wgpu::BindGroup,
        active: bool,
        frame_light_term_mask: LightTermMask,
        timestamp_writes: Option<wgpu::ComputePassTimestampWrites<'a>>,
    ) -> Result<(), ShResidencyDrainError> {
        let force_resident =
            active || self.indirect_was_active || frame_light_term_mask != self.last_indirect_mask;
        if !should_dispatch(
            active,
            !self.indirect_dirty_rows.is_empty(),
            self.indirect_was_active,
            frame_light_term_mask,
            self.last_indirect_mask,
        ) {
            return Ok(());
        }
        let mut rows = self.indirect_dirty_rows.clone();
        if force_resident {
            rows.extend(&self.indirect_resident_rows);
        }
        if rows.is_empty() {
            return Ok(());
        }
        let ranges = coalesce_rows(&rows);
        let gpu = self
            .gpu
            .as_ref()
            .ok_or(ShResidencyDrainError::GpuCapacity {
                reason: "streamed compose dispatch requested before GPU initialization",
            })?;
        gpu.dispatch_indirect_compose(
            queue,
            encoder,
            uniform_bind_group,
            &ranges,
            timestamp_writes,
        )?;
        self.indirect_compose_epoch = self
            .indirect_compose_epoch
            .checked_add(1)
            .ok_or(ShResidencyDrainError::SlotOverflow)?;
        self.indirect_dirty_rows.clear();
        self.dirty_rows
            .retain(|(section, _)| *section != INDIRECT_DELTA_ID);
        self.indirect_was_active = active;
        self.last_indirect_mask = frame_light_term_mask;
        Ok(())
    }

    pub(in crate::render) fn snapshot(&self) -> ShResidencySnapshot {
        let sparse_capacity = |section_id| {
            self.gpu.as_ref().map_or_else(
                || {
                    self.sparse_pools
                        .get(&section_id)
                        .map_or(0, |pool| pool.entries.capacity)
                },
                |gpu| gpu.sparse_entry_capacity(section_id),
            )
        };
        let (
            fixed_metadata_bytes,
            active_capacity_bytes,
            retiring_capacity_bytes,
            dense_group_minimum_bytes,
            sparse_group_minimum_bytes,
        ) = self.gpu.as_ref().map_or_else(
            || (0, 0, 0, None, BTreeMap::new()),
            |gpu| {
                (
                    gpu.fixed_metadata_bytes,
                    gpu.active_capacity_bytes,
                    gpu.retiring_capacity_bytes(),
                    Some(gpu.dense_group_minimum_bytes),
                    gpu.sparse_group_minimum_bytes.clone(),
                )
            },
        );
        let whole_resident_scatter_bytes = self
            .gpu
            .as_ref()
            .map_or(0, |gpu| gpu.whole_resident_scatter_bytes);
        let pool_growth = self
            .gpu
            .as_ref()
            .map_or_else(PoolGrowthCounters::default, |gpu| gpu.growth);
        let logical_occupancy_bytes = self.logical_occupancy_bytes();
        ShResidencySnapshot {
            generation: self.generation,
            target_clusters: self.targets.len(),
            installed_clusters: self.installed.len(),
            sampleable_clusters: self.sampleable.len(),
            // The allocator high-water is an address watermark, not the
            // physical texture capacity. Report the actual active atlas so
            // the residency ledger cannot understate allocated GPU bytes.
            dense_slot_capacity: self
                .gpu
                .as_ref()
                .map_or(self.dense_slots.capacity, |gpu| gpu.shape.slots),
            dense_live_slots: self.node_slots.values().map(|range| range.len).sum(),
            indirect_sparse_entry_capacity: sparse_capacity(INDIRECT_DELTA_ID),
            direct_sparse_entry_capacity: sparse_capacity(DIRECT_DELTA_ID),
            animated_direct_sparse_entry_capacity: sparse_capacity(ANIMATED_DIRECT_DELTA_ID),
            dirty_affinity_rows: self
                .dirty_rows
                .iter()
                .map(|(_, row)| *row)
                .chain(self.indirect_dirty_rows.iter().copied())
                .chain(self.direct_promotion_dirty_rows.iter().copied())
                .chain(self.direct_animated_dirty_rows.iter().copied())
                .collect::<BTreeSet<_>>()
                .len(),
            fixed_metadata_bytes,
            active_capacity_bytes,
            effective_floor_bytes: self
                .gpu
                .as_ref()
                .map_or(0, StreamingGpuPools::effective_floor_bytes),
            dense_group_minimum_bytes,
            indirect_delta_minimum_bytes: sparse_group_minimum_bytes
                .get(&INDIRECT_DELTA_ID)
                .copied(),
            direct_delta_minimum_bytes: sparse_group_minimum_bytes.get(&DIRECT_DELTA_ID).copied(),
            animated_direct_delta_minimum_bytes: sparse_group_minimum_bytes
                .get(&ANIMATED_DIRECT_DELTA_ID)
                .copied(),
            logical_occupancy_bytes,
            retiring_capacity_bytes,
            replacement_peak_bytes: active_capacity_bytes
                .checked_add(retiring_capacity_bytes)
                .expect("validated streamed GPU capacities must not overflow u64"),
            whole_resident_scatter_bytes,
            install_cpu_total_micros: self.install_cpu.total_micros,
            install_cpu_max_drain_micros: self.install_cpu.max_drain_micros,
            install_cpu_last_drain_micros: self.install_cpu.last_drain_micros,
            pool_growth_events: pool_growth.events,
            pool_growth_bytes: pool_growth.bytes,
        }
    }

    pub(in crate::render) fn streaming_allocation_summary(
        &self,
    ) -> crate::render::sh_residency::ShStreamingAllocationSummary {
        let snapshot = self.snapshot();
        crate::render::sh_residency::ShStreamingAllocationSummary {
            fixed_metadata_bytes: snapshot.fixed_metadata_bytes,
            whole_resident_scatter_bytes: snapshot.whole_resident_scatter_bytes,
            active_capacity_bytes: snapshot.active_capacity_bytes,
            logical_occupancy_bytes: snapshot.logical_occupancy_bytes,
            retiring_capacity_bytes: snapshot.retiring_capacity_bytes,
            replacement_peak_bytes: snapshot.replacement_peak_bytes,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn forced_direct_refresh_seeds_pass_a_for_an_id45_only_owner() {
        let animated_only = BTreeSet::from([7]);
        let (promotion, animated) = direct_dispatch_rows(
            true,
            &BTreeSet::new(),
            &BTreeSet::new(),
            &BTreeSet::new(),
            &animated_only,
        );

        // Pass A produces the intermediate target, so its forced range must
        // cover every Pass-B-only resident id-45 row before Pass B reads it.
        assert_eq!(promotion, animated_only);
        assert_eq!(animated, BTreeSet::from([7]));
    }
}
