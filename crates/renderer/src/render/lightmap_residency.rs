// Lightmap-family GPU residency meter, one row per group-4 lightmap texture.
// See: context/lib/rendering_pipeline.md §7.8

use super::residency::{ResidencyAllocation, allocation_total_bytes};

pub const LIGHTMAP_STATIC_IRRADIANCE: &str = "static_irradiance";
pub const LIGHTMAP_STATIC_DIRECTION: &str = "static_direction";
pub const LIGHTMAP_SHADOWMASK: &str = "shadowmask";
pub const LIGHTMAP_ANIMATED_IRRADIANCE: &str = "animated_irradiance";
pub const LIGHTMAP_ANIMATED_DIRECTION: &str = "animated_direction";

/// Resident bytes of every lightmap-family texture bound for the installed
/// level, rebuilt by each level install (including the empty install that
/// unloads a level) and by each streamed-pool growth or retirement release.
/// Rows come from the textures actually bound, so a rejected atlas never
/// reaches the meter; its placeholder does.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct LightmapResidencyReport {
    pub allocations: Vec<ResidencyAllocation>,
    pub total_bytes: u64,
    /// Bytes a streamed pool's grown-out generation holds until its
    /// submitted work is done. Counted apart from `total_bytes`, like SH's
    /// retirement capacity: the rows are the bound, active generation.
    pub retiring_bytes: u64,
}

impl LightmapResidencyReport {
    pub(super) fn new(
        static_rows: [ResidencyAllocation; 3],
        animated_rows: [ResidencyAllocation; 2],
    ) -> Self {
        let allocations: Vec<ResidencyAllocation> =
            static_rows.into_iter().chain(animated_rows).collect();
        let total_bytes = allocation_total_bytes(&allocations);
        Self {
            allocations,
            total_bytes,
            retiring_bytes: 0,
        }
    }

    /// The same animated rows with new static rows and retiring bytes, after
    /// a streamed pool grew or released its retiring generation.
    pub(super) fn with_static_rows(
        &self,
        static_rows: [ResidencyAllocation; 3],
        retiring_bytes: u64,
    ) -> Self {
        let animated_rows: [ResidencyAllocation; 2] = self.allocations[3..5]
            .to_vec()
            .try_into()
            .expect("the report keeps two animated rows after the three static ones");
        Self {
            retiring_bytes,
            ..Self::new(static_rows, animated_rows)
        }
    }

    /// Bytes of the row named `name`, one of the `LIGHTMAP_*` names.
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
        let retiring = if self.retiring_bytes == 0 {
            String::new()
        } else {
            format!("; retiring {} B", self.retiring_bytes)
        };
        format!(
            "[Renderer] Lightmap residency: {rows}; total {} B ({:.2} MiB){retiring}",
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
            sources: vec![ResidencySource::Derived],
            bytes,
            state: ResidencyAllocationState::Dummy,
            shape: ResidencyAllocationShape::Texture {
                format: "Rgba16Float",
                dimension: "D2",
                extent: [1, 1, 1],
            },
        }
    }

    #[test]
    fn report_keeps_the_five_family_rows_in_order_and_totals_them() {
        let report = LightmapResidencyReport::new(
            [
                row(LIGHTMAP_STATIC_IRRADIANCE, 1),
                row(LIGHTMAP_STATIC_DIRECTION, 2),
                row(LIGHTMAP_SHADOWMASK, 4),
            ],
            [
                row(LIGHTMAP_ANIMATED_IRRADIANCE, 8),
                row(LIGHTMAP_ANIMATED_DIRECTION, 16),
            ],
        );
        let names: Vec<_> = report.allocations.iter().map(|row| row.name).collect();
        assert_eq!(
            names,
            [
                LIGHTMAP_STATIC_IRRADIANCE,
                LIGHTMAP_STATIC_DIRECTION,
                LIGHTMAP_SHADOWMASK,
                LIGHTMAP_ANIMATED_IRRADIANCE,
                LIGHTMAP_ANIMATED_DIRECTION,
            ]
        );
        assert_eq!(report.total_bytes, 31);
        assert_eq!(report.bytes(LIGHTMAP_ANIMATED_DIRECTION), Some(16));
        let line = report.log_line();
        for name in names {
            assert!(line.contains(name), "{line}");
        }
        assert!(line.contains("animated_irradiance 8 B"), "{line}");
        assert!(!line.contains("retiring"), "{line}");
    }

    // A streamed pool's growth replaces the static rows and reports its
    // retiring generation apart from the bound total.
    #[test]
    fn static_rows_refresh_keeps_the_animated_rows_and_counts_retiring_bytes_apart() {
        let report = LightmapResidencyReport::new(
            [
                row(LIGHTMAP_STATIC_IRRADIANCE, 1),
                row(LIGHTMAP_STATIC_DIRECTION, 2),
                row(LIGHTMAP_SHADOWMASK, 4),
            ],
            [
                row(LIGHTMAP_ANIMATED_IRRADIANCE, 8),
                row(LIGHTMAP_ANIMATED_DIRECTION, 16),
            ],
        );
        let grown = report.with_static_rows(
            [
                row(LIGHTMAP_STATIC_IRRADIANCE, 100),
                row(LIGHTMAP_STATIC_DIRECTION, 200),
                row(LIGHTMAP_SHADOWMASK, 400),
            ],
            7,
        );
        assert_eq!(grown.bytes(LIGHTMAP_STATIC_IRRADIANCE), Some(100));
        assert_eq!(grown.bytes(LIGHTMAP_ANIMATED_DIRECTION), Some(16));
        assert_eq!(
            grown.total_bytes, 724,
            "the retiring set is not in the total"
        );
        assert_eq!(grown.retiring_bytes, 7);
        assert!(
            grown.log_line().contains("retiring 7 B"),
            "{}",
            grown.log_line()
        );
    }
}
