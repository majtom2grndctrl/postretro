// Pre-base-SH delta working-set projection and admission.
// See: context/lib/build_pipeline.md §Build Cache

use std::collections::BTreeMap;
use std::fmt;

pub(super) const DELTA_WORKING_SET_DENSE_AND_COMPACTION_FACTOR: u64 = 2;
// Reserve another cumulative-delta-sized share for the co-resident id 34/id 35
// originals and clones. Base-dominated maps remain outside this delta gate, but
// a delta-dominated admission no longer spends the entire budget on deltas.
pub(super) const DELTA_WORKING_SET_BASE_COPY_HEADROOM_FACTOR: u64 = 1;
pub(super) const DELTA_WORKING_SET_ANALYSIS_CLONE_FACTOR: u64 = 1;
pub(super) const DELTA_WORKING_SET_COPY_FACTOR: u64 =
    DELTA_WORKING_SET_DENSE_AND_COMPACTION_FACTOR + DELTA_WORKING_SET_BASE_COPY_HEADROOM_FACTOR;
pub(super) const DELTA_WORKING_SET_ANALYZE_COPY_FACTOR: u64 =
    DELTA_WORKING_SET_COPY_FACTOR + DELTA_WORKING_SET_ANALYSIS_CLONE_FACTOR;
pub(super) const DELTA_WORKING_SET_HISTOGRAM_ROWS: usize = 10;

/// CSR-only inputs for one delta bake's dense working-set projection.
///
/// `static_indices` is present only for id 41, whose CSR light indices address
/// the EntityShadowLights selection rather than the global static-light list.
#[derive(Clone, Copy)]
pub(super) struct DeltaCsrProjectionInput<'a> {
    pub(super) label: &'static str,
    pub(super) affinity_lights: &'a [u32],
    pub(super) static_indices: Option<&'a [u64]>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct DeltaWorkingSetHistogramRow {
    pub(super) light_index: u32,
    pub(super) static_index: Option<u64>,
    pub(super) csr_entry_count: u64,
    pub(super) dense_bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct DeltaWorkingSetBakeProjection {
    pub(super) label: &'static str,
    pub(super) dense_bytes: u64,
    pub(super) histogram: Vec<DeltaWorkingSetHistogramRow>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct DeltaWorkingSetProjection {
    pub(super) bakes: Vec<DeltaWorkingSetBakeProjection>,
    pub(super) cumulative_dense_bytes: u64,
    pub(super) copy_chain_factor: u64,
    pub(super) estimated_peak_bytes: u64,
    pub(super) budget_bytes: u64,
}

#[derive(Debug)]
pub(super) enum DeltaWorkingSetGateError {
    ArithmeticOverflow { calculation: &'static str },
    BudgetExceeded(DeltaWorkingSetProjection),
}

impl fmt::Display for DeltaWorkingSetGateError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ArithmeticOverflow { calculation } => write!(
                formatter,
                "SH delta working-set gate refused before dense baking: {calculation} overflowed"
            ),
            Self::BudgetExceeded(projection) => {
                write!(
                    formatter,
                    "SH delta working-set gate refused before dense baking: estimated peak {} bytes exceeds budget {} bytes ({} cumulative dense bytes × copy-chain factor {})",
                    projection.estimated_peak_bytes,
                    projection.budget_bytes,
                    projection.cumulative_dense_bytes,
                    projection.copy_chain_factor,
                )?;
                for bake in &projection.bakes {
                    write!(
                        formatter,
                        "; {} dense {} bytes",
                        bake.label, bake.dense_bytes
                    )?;
                    if bake.histogram.is_empty() {
                        write!(formatter, ", no CSR entries")?;
                        continue;
                    }
                    write!(formatter, ", top lights [")?;
                    for (index, row) in bake.histogram.iter().enumerate() {
                        if index != 0 {
                            write!(formatter, ", ")?;
                        }
                        write!(
                            formatter,
                            "light {}{}: {} CSR entries, {} dense bytes",
                            row.light_index,
                            row.static_index
                                .map(|static_index| format!(" (static_index {static_index})"))
                                .unwrap_or_default(),
                            row.csr_entry_count,
                            row.dense_bytes,
                        )?;
                    }
                    write!(formatter, "]")?;
                }
                Ok(())
            }
        }
    }
}

impl std::error::Error for DeltaWorkingSetGateError {}

pub(super) fn delta_working_set_copy_chain_factor(retains_analysis_clone: bool) -> u64 {
    if retains_analysis_clone {
        DELTA_WORKING_SET_ANALYZE_COPY_FACTOR
    } else {
        DELTA_WORKING_SET_COPY_FACTOR
    }
}

pub(super) fn retains_sh_analyze_dense_deltas(sh_analyze: bool, sh_coarsening_enabled: bool) -> bool {
    sh_analyze && sh_coarsening_enabled
}

/// Project and gate the three delta bakes before any dense f16 payload exists.
///
/// The cumulative estimate deliberately treats all three dense payloads as
/// co-resident through compaction. The copy-chain factor covers the dense and
/// compaction buffers, reserves headroom for the co-resident base-volume
/// originals and clones, and adds the `--sh-analyze` clone when retained.
pub(super) fn gate_delta_working_set(
    bakes: [DeltaCsrProjectionInput<'_>; 3],
    subblock_f16_len: usize,
    budget_bytes: u64,
    copy_chain_factor: u64,
) -> Result<DeltaWorkingSetProjection, DeltaWorkingSetGateError> {
    let subblock_f16_len = u64::try_from(subblock_f16_len).map_err(|_| {
        DeltaWorkingSetGateError::ArithmeticOverflow {
            calculation: "delta sub-block f16 length conversion",
        }
    })?;
    let dense_bytes_per_entry =
        subblock_f16_len
            .checked_mul(2)
            .ok_or(DeltaWorkingSetGateError::ArithmeticOverflow {
                calculation: "delta sub-block byte length",
            })?;

    let mut cumulative_dense_bytes = 0u64;
    let mut projected_bakes = Vec::with_capacity(bakes.len());
    for bake in bakes {
        let entry_count = u64::try_from(bake.affinity_lights.len()).map_err(|_| {
            DeltaWorkingSetGateError::ArithmeticOverflow {
                calculation: "delta CSR entry count conversion",
            }
        })?;
        let dense_bytes = entry_count.checked_mul(dense_bytes_per_entry).ok_or(
            DeltaWorkingSetGateError::ArithmeticOverflow {
                calculation: "delta dense payload projection",
            },
        )?;
        cumulative_dense_bytes = cumulative_dense_bytes.checked_add(dense_bytes).ok_or(
            DeltaWorkingSetGateError::ArithmeticOverflow {
                calculation: "cumulative delta dense payload projection",
            },
        )?;

        let mut counts = BTreeMap::<u32, u64>::new();
        for &light_index in bake.affinity_lights {
            let count = counts.entry(light_index).or_default();
            *count = count
                .checked_add(1)
                .ok_or(DeltaWorkingSetGateError::ArithmeticOverflow {
                    calculation: "per-light delta CSR occurrence count",
                })?;
        }
        let mut histogram: Vec<_> = counts
            .into_iter()
            .map(|(light_index, csr_entry_count)| {
                let dense_bytes = csr_entry_count.checked_mul(dense_bytes_per_entry).ok_or(
                    DeltaWorkingSetGateError::ArithmeticOverflow {
                        calculation: "per-light dense payload projection",
                    },
                )?;
                Ok(DeltaWorkingSetHistogramRow {
                    light_index,
                    static_index: bake
                        .static_indices
                        .and_then(|static_indices| static_indices.get(light_index as usize))
                        .copied(),
                    csr_entry_count,
                    dense_bytes,
                })
            })
            .collect::<Result<Vec<_>, DeltaWorkingSetGateError>>()?;
        histogram.sort_unstable_by(|left, right| {
            right
                .dense_bytes
                .cmp(&left.dense_bytes)
                .then_with(|| left.light_index.cmp(&right.light_index))
        });
        histogram.truncate(DELTA_WORKING_SET_HISTOGRAM_ROWS);
        projected_bakes.push(DeltaWorkingSetBakeProjection {
            label: bake.label,
            dense_bytes,
            histogram,
        });
    }

    let estimated_peak_bytes = cumulative_dense_bytes
        .checked_mul(copy_chain_factor)
        .ok_or(DeltaWorkingSetGateError::ArithmeticOverflow {
            calculation: "delta working-set copy-chain projection",
        })?;
    let projection = DeltaWorkingSetProjection {
        bakes: projected_bakes,
        cumulative_dense_bytes,
        copy_chain_factor,
        estimated_peak_bytes,
        budget_bytes,
    };
    if estimated_peak_bytes > budget_bytes {
        Err(DeltaWorkingSetGateError::BudgetExceeded(projection))
    } else {
        Ok(projection)
    }
}

pub(super) fn log_delta_working_set_projection(projection: &DeltaWorkingSetProjection, verbose: bool) {
    log::info!(
        "[Compiler] SH delta working-set gate: estimated peak {} bytes, budget {} bytes, admitted",
        projection.estimated_peak_bytes,
        projection.budget_bytes,
    );
    if verbose {
        for bake in &projection.bakes {
            for row in &bake.histogram {
                match row.static_index {
                    Some(static_index) => log::info!(
                        "[Compiler] SH delta working-set histogram: {} selection slot {} (static_index {}), {} CSR entries, {} dense bytes",
                        bake.label,
                        row.light_index,
                        static_index,
                        row.csr_entry_count,
                        row.dense_bytes,
                    ),
                    None => log::info!(
                        "[Compiler] SH delta working-set histogram: {} light {}: {} CSR entries, {} dense bytes",
                        bake.label,
                        row.light_index,
                        row.csr_entry_count,
                        row.dense_bytes,
                    ),
                }
            }
        }
    }
}

