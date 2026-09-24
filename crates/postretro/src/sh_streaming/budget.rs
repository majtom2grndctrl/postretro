//! Checked residency accounting with separate GPU and host-phase ledgers.
//! See: context/plans/in-progress/sh-probe-streaming--cluster-residency/index.md

use super::controller::ShResidencyControllerError;

pub(crate) const DEFAULT_GPU_FLOOR_BYTES: u64 = 256 * 1024 * 1024;

/// Inputs whose byte sizes only the renderer can determine. The controller
/// deliberately records them as separate GPU charges rather than treating
/// logical resident payload as a second allocation.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct FixedGpuCharges {
    pub(crate) fixed_metadata_bytes: u64,
    pub(crate) whole_resident_scatter_bytes: u64,
    pub(crate) active_pool_capacity_bytes: u64,
}

/// Renderer-derived minimum capacity for each streamable physical-pool group.
/// `None` means the family is absent. The coupled id-34/id-35 group is one
/// physical allocation and therefore one floor term.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct StreamedPoolMinima {
    pub(crate) dense_group_bytes: Option<u64>,
    pub(crate) indirect_delta_bytes: Option<u64>,
    pub(crate) direct_delta_bytes: Option<u64>,
    pub(crate) animated_direct_delta_bytes: Option<u64>,
}

impl StreamedPoolMinima {
    fn checked_sum(self) -> Result<u64, ShResidencyControllerError> {
        [
            self.dense_group_bytes,
            self.indirect_delta_bytes,
            self.direct_delta_bytes,
            self.animated_direct_delta_bytes,
        ]
        .into_iter()
        .flatten()
        .try_fold(0u64, |total, bytes| {
            total
                .checked_add(bytes)
                .ok_or(ShResidencyControllerError::AccountingOverflow(
                    "streamed pool minima",
                ))
        })
    }
}

/// The app asks the renderer for these explicit physical figures; the planner
/// never guesses them from aggregate id-50 chunk bytes.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct ShGpuBudgetInputs {
    pub(crate) fixed: FixedGpuCharges,
    pub(crate) pool_minima: StreamedPoolMinima,
    /// The renderer's exact initial physical floor after atlas-layer and sparse
    /// alignment rounding. This may be below the requested 256 MiB when the
    /// complete level cannot fill that budget. The controller retains family
    /// minima for admission but must not reconstruct this physical total.
    pub(crate) renderer_effective_floor_bytes: Option<u64>,
}

/// A current/high-water host phase. Payload ownership moves between phases,
/// so the controller uses this type for every transition instead of adding the
/// phases together into a misleading residency total.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct BytePhase {
    pub(crate) current_bytes: u64,
    pub(crate) high_water_bytes: u64,
}

impl BytePhase {
    pub(crate) fn add(
        &mut self,
        bytes: u64,
        label: &'static str,
    ) -> Result<(), ShResidencyControllerError> {
        self.current_bytes = self
            .current_bytes
            .checked_add(bytes)
            .ok_or(ShResidencyControllerError::AccountingOverflow(label))?;
        self.high_water_bytes = self.high_water_bytes.max(self.current_bytes);
        Ok(())
    }

    pub(crate) fn remove(
        &mut self,
        bytes: u64,
        label: &'static str,
    ) -> Result<(), ShResidencyControllerError> {
        self.current_bytes = self
            .current_bytes
            .checked_sub(bytes)
            .ok_or(ShResidencyControllerError::AccountingUnderflow(label))?;
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct CpuPhaseLedger {
    pub(crate) encoded: BytePhase,
    pub(crate) decoding: BytePhase,
    pub(crate) ready: BytePhase,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ShResidencyAccounting {
    pub(crate) fixed_gpu: FixedGpuCharges,
    pub(crate) pool_minima: StreamedPoolMinima,
    renderer_effective_floor_bytes: Option<u64>,
    pub(crate) logical_occupancy_bytes: u64,
    pub(crate) cpu: CpuPhaseLedger,
}

impl ShResidencyAccounting {
    pub(crate) fn new(inputs: ShGpuBudgetInputs) -> Result<Self, ShResidencyControllerError> {
        let pool_minima_bytes = inputs.pool_minima.checked_sum()?;
        let mandatory_floor_bytes = inputs
            .fixed
            .fixed_metadata_bytes
            .checked_add(inputs.fixed.whole_resident_scatter_bytes)
            .and_then(|bytes| bytes.checked_add(pool_minima_bytes))
            .ok_or(ShResidencyControllerError::AccountingOverflow(
                "effective GPU floor",
            ))?;
        // The 256 MiB default is a requested allocation, not a mandatory
        // physical charge: a small level can cap every family below it.
        if inputs
            .renderer_effective_floor_bytes
            .is_some_and(|renderer_floor| renderer_floor < mandatory_floor_bytes)
        {
            return Err(ShResidencyControllerError::AccountingUnderflow(
                "renderer effective GPU floor",
            ));
        }
        let _ = inputs
            .fixed
            .fixed_metadata_bytes
            .checked_add(inputs.fixed.whole_resident_scatter_bytes)
            .and_then(|bytes| bytes.checked_add(inputs.fixed.active_pool_capacity_bytes))
            .ok_or(ShResidencyControllerError::AccountingOverflow(
                "requested GPU bytes",
            ))?;
        Ok(Self {
            fixed_gpu: inputs.fixed,
            pool_minima: inputs.pool_minima,
            renderer_effective_floor_bytes: inputs.renderer_effective_floor_bytes,
            logical_occupancy_bytes: 0,
            cpu: CpuPhaseLedger::default(),
        })
    }

    pub(crate) fn effective_floor_bytes(&self) -> Result<u64, ShResidencyControllerError> {
        if let Some(renderer_floor) = self.renderer_effective_floor_bytes {
            return Ok(renderer_floor);
        }
        let pool_minima_bytes = self.pool_minima.checked_sum()?;
        let mandatory = self
            .fixed_gpu
            .fixed_metadata_bytes
            .checked_add(self.fixed_gpu.whole_resident_scatter_bytes)
            .and_then(|bytes| bytes.checked_add(pool_minima_bytes))
            .ok_or(ShResidencyControllerError::AccountingOverflow(
                "effective GPU floor",
            ))?;
        Ok(DEFAULT_GPU_FLOOR_BYTES.max(mandatory))
    }

    #[cfg(test)]
    pub(crate) fn requested_gpu_bytes(&self) -> Result<u64, ShResidencyControllerError> {
        self.fixed_gpu
            .fixed_metadata_bytes
            .checked_add(self.fixed_gpu.whole_resident_scatter_bytes)
            .and_then(|bytes| bytes.checked_add(self.fixed_gpu.active_pool_capacity_bytes))
            .ok_or(ShResidencyControllerError::AccountingOverflow(
                "requested GPU bytes",
            ))
    }

    /// Nominal space for cluster payloads inside the renderer's effective
    /// floor. This is a policy comparator, not a second GPU allocation charge:
    /// logical occupancy remains a sub-ledger of the active physical pools.
    pub(crate) fn nominal_cluster_bytes(&self) -> Result<u64, ShResidencyControllerError> {
        self.effective_floor_bytes()?
            .checked_sub(self.fixed_gpu.fixed_metadata_bytes)
            .and_then(|bytes| bytes.checked_sub(self.fixed_gpu.whole_resident_scatter_bytes))
            .ok_or(ShResidencyControllerError::AccountingUnderflow(
                "nominal cluster budget",
            ))
    }

    pub(crate) fn remove_logical(&mut self, bytes: u64) -> Result<(), ShResidencyControllerError> {
        self.logical_occupancy_bytes = self.logical_occupancy_bytes.checked_sub(bytes).ok_or(
            ShResidencyControllerError::AccountingUnderflow("logical occupancy"),
        )?;
        Ok(())
    }

    pub(crate) fn add_logical(&mut self, bytes: u64) -> Result<(), ShResidencyControllerError> {
        self.can_add_logical(bytes)?;
        self.logical_occupancy_bytes += bytes;
        Ok(())
    }

    pub(crate) fn can_add_logical(&self, bytes: u64) -> Result<(), ShResidencyControllerError> {
        self.logical_occupancy_bytes.checked_add(bytes).ok_or(
            ShResidencyControllerError::AccountingOverflow("logical occupancy"),
        )?;
        Ok(())
    }

    pub(crate) fn update_fixed_gpu(
        &mut self,
        fixed_gpu: FixedGpuCharges,
    ) -> Result<(), ShResidencyControllerError> {
        let inputs = ShGpuBudgetInputs {
            fixed: fixed_gpu,
            pool_minima: self.pool_minima,
            renderer_effective_floor_bytes: self.renderer_effective_floor_bytes,
        };
        let _ = Self::new(inputs)?;
        self.fixed_gpu = fixed_gpu;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn effective_floor_sums_family_minima_without_double_counting_logical_or_cpu_bytes() {
        let mib = 1024 * 1024;
        let inputs = ShGpuBudgetInputs {
            fixed: FixedGpuCharges {
                fixed_metadata_bytes: 16 * mib,
                whole_resident_scatter_bytes: 20 * mib,
                active_pool_capacity_bytes: 400 * mib,
            },
            pool_minima: StreamedPoolMinima {
                dense_group_bytes: Some(120 * mib),
                indirect_delta_bytes: Some(80 * mib),
                direct_delta_bytes: Some(40 * mib),
                animated_direct_delta_bytes: Some(16 * mib),
            },
            ..ShGpuBudgetInputs::default()
        };
        let mut accounting = ShResidencyAccounting::new(inputs).unwrap();
        assert_eq!(accounting.effective_floor_bytes().unwrap(), 292 * mib);
        assert_eq!(accounting.requested_gpu_bytes().unwrap(), 436 * mib);

        accounting.add_logical(90 * mib).unwrap();
        accounting.cpu.encoded.add(7 * mib, "encoded").unwrap();
        accounting.cpu.encoded.remove(7 * mib, "encoded").unwrap();
        accounting.cpu.decoding.add(9 * mib, "decoding").unwrap();
        accounting.cpu.decoding.remove(9 * mib, "decoding").unwrap();
        accounting.cpu.ready.add(11 * mib, "ready").unwrap();

        assert_eq!(accounting.logical_occupancy_bytes, 90 * mib);
        assert_eq!(accounting.requested_gpu_bytes().unwrap(), 436 * mib);
        assert_eq!(accounting.cpu.encoded.current_bytes, 0);
        assert_eq!(accounting.cpu.encoded.high_water_bytes, 7 * mib);
        assert_eq!(accounting.cpu.decoding.current_bytes, 0);
        assert_eq!(accounting.cpu.decoding.high_water_bytes, 9 * mib);
        assert_eq!(accounting.cpu.ready.current_bytes, 11 * mib);
        assert_eq!(accounting.cpu.ready.high_water_bytes, 11 * mib);
    }

    #[test]
    fn exact_renderer_floor_preserves_physical_rounding() {
        let inputs = ShGpuBudgetInputs {
            fixed: FixedGpuCharges {
                fixed_metadata_bytes: 10,
                whole_resident_scatter_bytes: 20,
                active_pool_capacity_bytes: 30,
            },
            pool_minima: StreamedPoolMinima {
                dense_group_bytes: Some(4),
                indirect_delta_bytes: Some(5),
                direct_delta_bytes: None,
                animated_direct_delta_bytes: None,
            },
            // The renderer owns atlas-layer padding, so the app must retain
            // this exact result rather than recomputing 256 MiB/minima alone.
            renderer_effective_floor_bytes: Some(DEFAULT_GPU_FLOOR_BYTES + 512),
        };

        let accounting = ShResidencyAccounting::new(inputs).unwrap();
        assert_eq!(
            accounting.effective_floor_bytes().unwrap(),
            DEFAULT_GPU_FLOOR_BYTES + 512
        );
    }

    #[test]
    fn exact_renderer_floor_cannot_understate_known_minima() {
        let inputs = ShGpuBudgetInputs {
            fixed: FixedGpuCharges {
                fixed_metadata_bytes: 10,
                whole_resident_scatter_bytes: 20,
                active_pool_capacity_bytes: 30,
            },
            pool_minima: StreamedPoolMinima {
                dense_group_bytes: Some(4),
                indirect_delta_bytes: Some(5),
                direct_delta_bytes: None,
                animated_direct_delta_bytes: None,
            },
            renderer_effective_floor_bytes: Some(38),
        };

        assert!(matches!(
            ShResidencyAccounting::new(inputs),
            Err(ShResidencyControllerError::AccountingUnderflow(
                "renderer effective GPU floor"
            ))
        ));
    }

    #[test]
    fn checked_budget_inputs_and_ledger_transitions_reject_overflow() {
        let overflowing_minima = ShGpuBudgetInputs {
            fixed: FixedGpuCharges::default(),
            pool_minima: StreamedPoolMinima {
                dense_group_bytes: Some(u64::MAX),
                indirect_delta_bytes: Some(1),
                ..StreamedPoolMinima::default()
            },
            ..ShGpuBudgetInputs::default()
        };
        assert!(matches!(
            ShResidencyAccounting::new(overflowing_minima),
            Err(ShResidencyControllerError::AccountingOverflow(_))
        ));

        let mut accounting = ShResidencyAccounting::new(ShGpuBudgetInputs::default()).unwrap();
        let prior_charges = accounting.fixed_gpu;
        assert!(matches!(
            accounting.update_fixed_gpu(FixedGpuCharges {
                fixed_metadata_bytes: 1,
                whole_resident_scatter_bytes: 0,
                active_pool_capacity_bytes: u64::MAX,
            }),
            Err(ShResidencyControllerError::AccountingOverflow(
                "requested GPU bytes"
            ))
        ));
        assert_eq!(accounting.fixed_gpu, prior_charges);
        accounting.add_logical(u64::MAX).unwrap();
        assert!(matches!(
            accounting.add_logical(1),
            Err(ShResidencyControllerError::AccountingOverflow(_))
        ));
        assert!(matches!(
            accounting.cpu.ready.remove(1, "ready"),
            Err(ShResidencyControllerError::AccountingUnderflow(_))
        ));
    }
}
