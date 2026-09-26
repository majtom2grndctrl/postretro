//! Frame-bound compose planning, dispatch, and streamed residency reporting.

use std::time::Instant;

use postretro_render_cpu::mesh_instances;

use super::*;

impl ShResidencyState {
    #[expect(
        clippy::too_many_arguments,
        reason = "compose planning keeps the three passes' exact frame inputs explicit"
    )]
    pub(in crate::render) fn prepare_compose_frame(
        &mut self,
        region_sets: crate::render::ShSampleRegionSets<'_>,
        mesh_frame_plans: Option<&mesh_instances::MeshFramePlans>,
        include_viewmodels: bool,
        fog_draw_all: bool,
        records_compose: bool,
        force_full_resident: bool,
        indirect_active: bool,
        animated_direct_active: bool,
        frame_light_term_mask: LightTermMask,
        promotion_override: DirectShDebugOverride,
        animated_override: AnimatedDirectShDebugOverride,
        promoted_static_weights: &[f32],
        promoted_animated_states: &[PromotedBakedLightState],
    ) -> Result<(), ShResidencyDrainError> {
        let started = Instant::now();
        self.indirect_compose_diagnostics = ShComposePassDiagnostics::default();
        self.static_direct_compose_diagnostics = ShComposePassDiagnostics::default();
        self.animated_direct_compose_diagnostics = ShComposePassDiagnostics::default();

        self.compose_input_regions.clear();
        self.compose_input_regions
            .extend_from_slice(region_sets.visible_cells);
        self.compose_input_regions.extend(
            region_sets
                .fog_cells
                .iter()
                .map(|&(min, max)| crate::render::ShSampleRegion::new(min, max)),
        );
        self.compose_input_regions
            .extend_from_slice(region_sets.movers);
        if let Some(plans) = mesh_frame_plans {
            self.compose_input_regions.extend(
                mesh_instances::planned_forward_sample_bounds(plans, include_viewmodels)
                    .map(|bounds| crate::render::ShSampleRegion::new(bounds.min, bounds.max)),
            );
        }
        let mut region_rows = std::mem::take(&mut self.compose_region_rows);
        self.resolve_sample_region_rows(
            &self.compose_input_regions,
            fog_draw_all,
            &mut region_rows,
        )?;

        self.compose_indirect_resident_rows.clear();
        self.compose_indirect_resident_rows
            .extend(self.indirect_resident_rows.iter().copied());
        self.compose_direct_resident_rows.clear();
        self.compose_direct_resident_rows
            .extend(self.direct_promotion_resident_rows.iter().copied());
        if self.animated_direct_compose_required {
            self.compose_direct_resident_rows
                .extend(self.direct_animated_resident_rows.iter().copied());
        }
        self.compose_direct_resident_rows.sort_unstable();
        self.compose_direct_resident_rows.dedup();
        self.compose_animated_resident_rows.clear();
        if self.animated_direct_compose_required {
            self.compose_animated_resident_rows
                .extend(self.compose_direct_resident_rows.iter().copied());
        }
        self.prune_nonresident_dirty_rows();

        let mut residency_rows = std::mem::take(&mut self.compose_residency_rows);
        self.close_residency_rows_over_scaled_writers(
            self.indirect_dirty_rows.iter().copied(),
            &self.compose_indirect_resident_rows,
            &mut residency_rows,
        )?;
        self.compose_planner.mark_residency_rows(
            compose_plan::ComposePass::Indirect,
            residency_rows.iter().copied(),
        );

        self.close_residency_rows_over_scaled_writers(
            self.direct_promotion_dirty_rows
                .iter()
                .chain(&self.direct_animated_dirty_rows)
                .copied(),
            &self.compose_direct_resident_rows,
            &mut residency_rows,
        )?;
        self.compose_planner.mark_residency_rows(
            compose_plan::ComposePass::StaticDirect,
            residency_rows.iter().copied(),
        );
        if self.animated_direct_compose_required {
            self.compose_planner.mark_residency_rows(
                compose_plan::ComposePass::AnimatedDirect,
                residency_rows.iter().copied(),
            );
        }

        self.compose_animated_weights.clear();
        self.compose_animated_weights
            .extend((0..MAX_ANIMATED_BAKED_LIGHTS).map(|index| {
                1.0 - animated_baked_promotion_weight(index, promoted_animated_states.get(index))
            }));

        self.compose_indirect_contributing_rows.clear();
        self.compose_indirect_contributing_rows
            .extend(self.indirect_delta_row_refs.keys().copied());
        self.compose_static_contributing_rows.clear();
        self.compose_static_contributing_rows
            .extend(self.direct_promotion_row_refs.keys().copied());
        self.compose_animated_contributing_rows.clear();
        self.compose_animated_contributing_rows
            .extend(self.direct_animated_row_refs.keys().copied());
        let mut frame_plan = self.compose_frame_plan.take().unwrap_or_default();
        self.compose_planner.plan_frame_into(
            compose_plan::ComposePlannerFrame {
                records_compose,
                force_full_resident,
                gated_rows: &region_rows,
                indirect_rows: compose_plan::ComposePassRows {
                    resident: &self.compose_indirect_resident_rows,
                    contributing: &self.compose_indirect_contributing_rows,
                },
                static_direct_rows: compose_plan::ComposePassRows {
                    resident: &self.compose_direct_resident_rows,
                    contributing: &self.compose_static_contributing_rows,
                },
                animated_direct_rows: compose_plan::ComposePassRows {
                    resident: &self.compose_animated_resident_rows,
                    contributing: &self.compose_animated_contributing_rows,
                },
                indirect_active,
                animated_direct_active,
                effective_static_weights: promoted_static_weights,
                effective_animated_weights: &self.compose_animated_weights,
                controls: compose_plan::ComposeControlSnapshot {
                    light_term_mask: frame_light_term_mask,
                    promotion_override,
                    animated_override,
                },
            },
            &mut frame_plan,
        );
        self.indirect_compose_diagnostics = lag_diagnostics(self.compose_planner.lagging_rows(
            compose_plan::ComposePass::Indirect,
            &self.compose_indirect_resident_rows,
        ));
        self.static_direct_compose_diagnostics =
            lag_diagnostics(self.compose_planner.lagging_rows(
                compose_plan::ComposePass::StaticDirect,
                &self.compose_direct_resident_rows,
            ));
        self.animated_direct_compose_diagnostics =
            lag_diagnostics(self.compose_planner.lagging_rows(
                compose_plan::ComposePass::AnimatedDirect,
                &self.compose_animated_resident_rows,
            ));
        self.compose_frame_plan = Some(frame_plan);
        self.compose_region_rows = region_rows;
        self.compose_residency_rows = residency_rows;
        self.compose_planning_cpu_micros =
            u64::try_from(started.elapsed().as_micros()).unwrap_or(u64::MAX);
        Ok(())
    }

    pub(super) fn prune_nonresident_dirty_rows(&mut self) {
        let indirect_resident_rows = &self.indirect_resident_rows;
        self.indirect_dirty_rows
            .retain(|row| indirect_resident_rows.contains(row));
        let direct_resident_rows = &self.compose_direct_resident_rows;
        self.direct_promotion_dirty_rows
            .retain(|row| direct_resident_rows.binary_search(row).is_ok());
        let animated_resident_rows = &self.compose_animated_resident_rows;
        self.direct_animated_dirty_rows
            .retain(|row| animated_resident_rows.binary_search(row).is_ok());
        let indirect_delta_row_refs = &self.indirect_delta_row_refs;
        let direct_promotion_row_refs = &self.direct_promotion_row_refs;
        let direct_animated_row_refs = &self.direct_animated_row_refs;
        self.dirty_rows.retain(|&(section, row)| match section {
            INDIRECT_DELTA_ID => indirect_delta_row_refs.contains_key(&row),
            DIRECT_DELTA_ID => direct_promotion_row_refs.contains_key(&row),
            ANIMATED_DIRECT_DELTA_ID => direct_animated_row_refs.contains_key(&row),
            _ => true,
        });
    }

    #[expect(
        clippy::too_many_arguments,
        reason = "the frame boundary must pass the queue, encoder, uniform binding, direct-compose state, and separate promotion/animated timing-pass inputs together"
    )]
    pub(in crate::render) fn dispatch_direct_compose<'a>(
        &mut self,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        uniform_bind_group: &wgpu::BindGroup,
        _active: bool,
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
        let Some(frame_plan) = self.compose_frame_plan.take() else {
            return Err(ShResidencyDrainError::GpuCapacity {
                reason: "streamed direct compose dispatched before frame planning",
            });
        };
        let result: Result<(), ShResidencyDrainError> = (|| {
            let promotion_plan = &frame_plan.static_direct;
            let animated_plan = &frame_plan.animated_direct;
            let mut encoded_any = false;

            if !promotion_plan.rows().is_empty() {
                let dispatches = self
                    .gpu
                    .as_mut()
                    .ok_or(ShResidencyDrainError::GpuCapacity {
                        reason:
                            "streamed direct compose dispatch requested before GPU initialization",
                    })?
                    .dispatch_direct_promotion(
                        queue,
                        encoder,
                        frame_light_term_mask,
                        promotion_override,
                        promotion_plan.rows(),
                        promotion_timestamp_writes,
                    )?;
                self.compose_planner.commit_pass(promotion_plan);
                self.static_direct_compose_diagnostics = pass_diagnostics(
                    promotion_plan,
                    dispatches,
                    self.compose_planner.lagging_rows(
                        compose_plan::ComposePass::StaticDirect,
                        &self.compose_direct_resident_rows,
                    ),
                );
                self.direct_promotion_dirty_rows.clear();
                // Planning Pass A already made the matching Pass-B work durable.
                // Clearing both residency dirty sets here prevents a failed Pass B
                // from needlessly reseeding and rewriting committed Pass A.
                self.direct_animated_dirty_rows.clear();
                self.dirty_rows
                    .retain(|(section, _)| *section != DIRECT_DELTA_ID);
                encoded_any = true;
            }

            if !animated_plan.rows().is_empty() {
                let dispatches = self
                    .gpu
                    .as_mut()
                    .ok_or(ShResidencyDrainError::GpuCapacity {
                        reason:
                            "streamed animated direct compose dispatched before GPU initialization",
                    })?
                    .dispatch_direct_animated(
                        queue,
                        encoder,
                        uniform_bind_group,
                        animated_override,
                        promoted_animated_states,
                        animated_plan.rows(),
                        animated_timestamp_writes,
                    )?;
                self.compose_planner.commit_pass(animated_plan);
                self.animated_direct_compose_diagnostics = pass_diagnostics(
                    animated_plan,
                    dispatches,
                    self.compose_planner.lagging_rows(
                        compose_plan::ComposePass::AnimatedDirect,
                        &self.compose_animated_resident_rows,
                    ),
                );
                self.direct_animated_dirty_rows.clear();
                self.dirty_rows
                    .retain(|(section, _)| *section != ANIMATED_DIRECT_DELTA_ID);
                encoded_any = true;
            }

            if encoded_any {
                self.direct_compose_epoch = self
                    .direct_compose_epoch
                    .checked_add(1)
                    .ok_or(ShResidencyDrainError::SlotOverflow)?;
            }
            Ok(())
        })();
        self.compose_frame_plan = Some(frame_plan);
        result
    }

    /// Encode only coalesced affinity-row work after the one pre-compose
    /// residency drain. Sampling remains on the previous mirror until the
    /// next drain promotes these completed compose writes.
    pub(in crate::render) fn dispatch_indirect_compose<'a>(
        &mut self,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        uniform_bind_group: &wgpu::BindGroup,
        _active: bool,
        _frame_light_term_mask: LightTermMask,
        timestamp_writes: Option<wgpu::ComputePassTimestampWrites<'a>>,
    ) -> Result<(), ShResidencyDrainError> {
        let Some(frame_plan) = self.compose_frame_plan.take() else {
            return Err(ShResidencyDrainError::GpuCapacity {
                reason: "streamed indirect compose dispatched before frame planning",
            });
        };
        let plan = &frame_plan.indirect;
        if plan.rows().is_empty() {
            self.compose_frame_plan = Some(frame_plan);
            return Ok(());
        }
        let result: Result<(), ShResidencyDrainError> = (|| {
            let gpu = self
                .gpu
                .as_mut()
                .ok_or(ShResidencyDrainError::GpuCapacity {
                    reason: "streamed compose dispatch requested before GPU initialization",
                })?;
            let dispatches = gpu.dispatch_indirect_compose(
                queue,
                encoder,
                uniform_bind_group,
                plan.rows(),
                timestamp_writes,
            )?;
            self.compose_planner.commit_pass(plan);
            self.indirect_compose_diagnostics = pass_diagnostics(
                plan,
                dispatches,
                self.compose_planner.lagging_rows(
                    compose_plan::ComposePass::Indirect,
                    &self.compose_indirect_resident_rows,
                ),
            );
            self.indirect_compose_epoch = self
                .indirect_compose_epoch
                .checked_add(1)
                .ok_or(ShResidencyDrainError::SlotOverflow)?;
            self.indirect_dirty_rows.clear();
            self.dirty_rows
                .retain(|(section, _)| *section != INDIRECT_DELTA_ID);
            Ok(())
        })();
        self.compose_frame_plan = Some(frame_plan);
        result
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
            install_cpu_max_steady_drain_micros: self.install_cpu.max_steady_drain_micros,
            pool_growth_events: pool_growth.events,
            pool_growth_bytes: pool_growth.bytes,
            pool_growth_cpu_micros: pool_growth.cpu_micros,
            indirect_compose: self.indirect_compose_diagnostics,
            static_direct_compose: self.static_direct_compose_diagnostics,
            animated_direct_compose: self.animated_direct_compose_diagnostics,
            compose_planning_cpu_micros: self.compose_planning_cpu_micros,
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

fn pass_diagnostics(
    plan: &compose_plan::ComposePassPlan,
    dispatches: usize,
    remaining_lag: usize,
) -> ShComposePassDiagnostics {
    ShComposePassDiagnostics {
        rows_composed: u64::try_from(plan.rows().len()).unwrap_or(u64::MAX),
        dispatches: u64::try_from(dispatches).unwrap_or(u64::MAX),
        lagged_rows_composed: u64::try_from(plan.lagged_rows()).unwrap_or(u64::MAX),
        resident_rows_still_lagging: u64::try_from(remaining_lag).unwrap_or(u64::MAX),
    }
}

fn lag_diagnostics(remaining_lag: usize) -> ShComposePassDiagnostics {
    ShComposePassDiagnostics {
        resident_rows_still_lagging: u64::try_from(remaining_lag).unwrap_or(u64::MAX),
        ..ShComposePassDiagnostics::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compose_counters_match_successful_plan_and_dispatches() {
        let plan = compose_plan::ComposePassPlan::test_plan(
            compose_plan::ComposePass::Indirect,
            vec![1, 3, 8],
            2,
        );
        assert_eq!(
            pass_diagnostics(&plan, 1, 4),
            ShComposePassDiagnostics {
                rows_composed: 3,
                dispatches: 1,
                lagged_rows_composed: 2,
                resident_rows_still_lagging: 4,
            }
        );
    }

    #[test]
    fn empty_dispatch_diagnostics_preserve_resident_lag() {
        assert_eq!(
            lag_diagnostics(3),
            ShComposePassDiagnostics {
                resident_rows_still_lagging: 3,
                ..ShComposePassDiagnostics::default()
            }
        );
    }
}
