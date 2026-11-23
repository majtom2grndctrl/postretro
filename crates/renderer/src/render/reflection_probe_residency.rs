// Reflection probe GPU residency meter: one row per probe record of the
// active level set.
// See: context/lib/resource_management.md §5.3

use super::residency::{ResidencyAllocation, allocation_total_bytes};

pub const REFLECTION_PROBE_COLOUR: &str = "probe_colour";
pub const REFLECTION_PROBE_DISTANCE: &str = "probe_distance";
pub const REFLECTION_PROBE_SHARE_TABLE: &str = "probe_share_table";
pub const REFLECTION_PROBE_SUMS: &str = "probe_sums";

/// Resident bytes of the active level's reflection probe records: the colour
/// array with every mip, the distance array, the share table and the
/// live-lighting sums, read back from what the level bound. A level with no
/// probes, and no active level, report [`Self::EMPTY`]. The fluid install's
/// dummies are not probes and never appear here.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ReflectionProbeResidencyReport {
    pub allocations: Vec<ResidencyAllocation>,
    pub total_bytes: u64,
}

impl ReflectionProbeResidencyReport {
    /// No rows and no bytes: no probes resident.
    pub const EMPTY: Self = Self {
        allocations: Vec::new(),
        total_bytes: 0,
    };

    pub(super) fn new(rows: [ResidencyAllocation; 4]) -> Self {
        let allocations = rows.to_vec();
        let total_bytes = allocation_total_bytes(&allocations);
        Self {
            allocations,
            total_bytes,
        }
    }

    /// Bytes of the row named `name`, one of the `REFLECTION_PROBE_*` names.
    pub fn bytes(&self, name: &str) -> Option<u64> {
        self.allocations
            .iter()
            .find(|row| row.name == name)
            .map(|row| row.bytes)
    }

    pub(super) fn log_line(&self) -> String {
        let rows = self
            .allocations
            .iter()
            .map(|row| format!("{} {} B", row.name, row.bytes))
            .collect::<Vec<_>>()
            .join(", ");
        format!(
            "[Renderer] Reflection probe residency: {rows}; total {} B ({:.2} MiB)",
            self.total_bytes,
            self.total_bytes as f64 / (1024.0 * 1024.0),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::render::residency::{
        ResidencyAllocationShape, ResidencyAllocationState, ResidencySource,
    };

    fn row(name: &'static str, bytes: u64) -> ResidencyAllocation {
        ResidencyAllocation {
            name,
            sources: vec![ResidencySource::Section(54)],
            bytes,
            state: ResidencyAllocationState::Data,
            shape: ResidencyAllocationShape::Buffer {
                binding_bytes: bytes,
            },
        }
    }

    #[test]
    fn probe_report_keeps_its_four_rows_in_order_and_totals_them() {
        let report = ReflectionProbeResidencyReport::new([
            row(REFLECTION_PROBE_COLOUR, 1),
            row(REFLECTION_PROBE_DISTANCE, 2),
            row(REFLECTION_PROBE_SHARE_TABLE, 4),
            row(REFLECTION_PROBE_SUMS, 8),
        ]);
        let names: Vec<_> = report.allocations.iter().map(|row| row.name).collect();
        assert_eq!(
            names,
            [
                REFLECTION_PROBE_COLOUR,
                REFLECTION_PROBE_DISTANCE,
                REFLECTION_PROBE_SHARE_TABLE,
                REFLECTION_PROBE_SUMS,
            ]
        );
        assert_eq!(report.total_bytes, 15);
        assert_eq!(report.bytes(REFLECTION_PROBE_SUMS), Some(8));
        assert_eq!(report.bytes("probe_index"), None);
        assert_eq!(
            report.log_line(),
            "[Renderer] Reflection probe residency: probe_colour 1 B, probe_distance 2 B, \
             probe_share_table 4 B, probe_sums 8 B; total 15 B (0.00 MiB)"
        );
    }
}
