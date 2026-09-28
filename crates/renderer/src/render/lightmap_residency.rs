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
/// unloads a level). Rows come from the textures actually bound, so a
/// rejected atlas never reaches the meter; its placeholder does.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct LightmapResidencyReport {
    pub allocations: Vec<ResidencyAllocation>,
    pub total_bytes: u64,
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
        format!(
            "[Renderer] Lightmap residency: {rows}; total {} B ({:.2} MiB)",
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
    }
}
