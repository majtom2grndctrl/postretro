//! The level's one cell-demand stage: lead L over the baked id-51 reach, and
//! the frame's visibility path, as every streamed resource reads them.
//! See: context/lib/rendering_pipeline.md §4 (Lightmap cell-block residency)

use postretro_level_format::cell_residency_set::CellResidencySetSection;
use postretro_level_format::cell_visibility::CELL_VISIBILITY_DISTANCE_FIXED_POINT_SCALE;
use postretro_visibility::{VisibilityPath, VisibleCells};

/// Default lead L in metres.
pub(crate) const DEFAULT_LEAD_METRES: u32 = 16;
/// Id-51 leads are in id-46 fixed-point distance units.
pub(crate) const LEAD_UNITS_PER_METRE: u32 = CELL_VISIBILITY_DISTANCE_FIXED_POINT_SCALE;
/// The default lead L in fixed point, for fixtures whose sets reach past it.
#[cfg(test)]
pub(crate) const DEFAULT_LEAD: u32 = DEFAULT_LEAD_METRES * LEAD_UNITS_PER_METRE;

/// Owns lead L for the level. One stage per level, held at level scope, so
/// SH and lightmap read one L by construction, whether the lightmap streams,
/// is declined, or the level streams SH alone. L is a developer lever, not a
/// player setting; a change takes effect at the next frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct CellDemand {
    /// Lead L, fixed point; never past `max_lead`.
    lead: u32,
    /// The level's baked maximum lead (id-51 header).
    max_lead: u32,
}

impl CellDemand {
    pub(crate) fn new(max_lead: u32) -> Self {
        Self {
            lead: (DEFAULT_LEAD_METRES * LEAD_UNITS_PER_METRE).min(max_lead),
            max_lead,
        }
    }

    /// Lead L in id-46 fixed-point units.
    pub(crate) fn lead(&self) -> u32 {
        self.lead
    }

    /// Lead L in metres, for log lines, the capture report and the slider.
    #[cfg_attr(not(feature = "dev-tools"), allow(dead_code))]
    pub(crate) fn lead_metres(&self) -> f32 {
        to_metres(self.lead)
    }

    /// The baked maximum lead in metres: the lead slider's upper end.
    #[cfg_attr(not(feature = "dev-tools"), allow(dead_code))]
    pub(crate) fn max_lead_metres(&self) -> f32 {
        to_metres(self.max_lead)
    }

    /// Clamped to the baked maximum: the set holds no entry past it.
    #[cfg(any(test, feature = "dev-tools"))]
    pub(crate) fn set_lead(&mut self, lead: u32) {
        self.lead = lead.min(self.max_lead);
    }

    #[cfg(any(test, feature = "dev-tools"))]
    pub(crate) fn set_lead_metres(&mut self, metres: f32) {
        let units = (metres.max(0.0) * LEAD_UNITS_PER_METRE as f32).round();
        self.set_lead(if units >= u32::MAX as f32 {
            u32::MAX
        } else {
            units as u32
        });
    }

    /// This frame's demand view over `residency_set`, the same level's id 51.
    pub(crate) fn frame<'a>(
        &self,
        residency_set: &'a CellResidencySetSection,
        camera_cell: u32,
        path: VisibilityPath,
        visible_cells: &'a VisibleCells,
    ) -> DemandFrame<'a> {
        DemandFrame {
            residency_set,
            camera_cell,
            path,
            visible_cells,
            lead: self.lead,
        }
    }
}

pub(crate) fn to_metres(lead: u32) -> f32 {
    lead as f32 / LEAD_UNITS_PER_METRE as f32
}

/// How a visibility path shapes the baked part of this frame's demand.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PathDemand {
    /// The camera cell's baked set; drawn cells are demanded too.
    PortalWalk,
    /// The camera cell's baked set. In play, a frustum-culled path's drawn
    /// cells are not demanded by the reach: a frustum-all fallback can draw
    /// the whole map.
    CameraSet,
    /// Solid or exterior camera cell: as `CameraSet` when the cell has a
    /// baked set; otherwise keep current demand.
    CameraSetOrHold,
    /// Empty world: no residency-set lookup, no change.
    Hold,
}

impl PathDemand {
    pub(crate) fn of(path: VisibilityPath) -> Self {
        match path {
            VisibilityPath::PrlPortal { .. } => Self::PortalWalk,
            VisibilityPath::PortalStepLimitFallback { .. } | VisibilityPath::NoPortalsFallback => {
                Self::CameraSet
            }
            VisibilityPath::SolidCellFallback | VisibilityPath::ExteriorCellFallback => {
                Self::CameraSetOrHold
            }
            VisibilityPath::EmptyWorldFallback => Self::Hold,
        }
    }
}

/// One frame's cell demand: the camera cell's id-51 entries split at the
/// stage's lead L, the path that classifies them, and the drawn cells. Each
/// resource maps these cells to its own units.
#[derive(Debug, Clone, Copy)]
pub(crate) struct DemandFrame<'a> {
    /// The level's id-51 set; the same level every resource was built for.
    pub(crate) residency_set: &'a CellResidencySetSection,
    pub(crate) camera_cell: u32,
    pub(crate) path: VisibilityPath,
    pub(crate) visible_cells: &'a VisibleCells,
    /// The stage's lead L, fixed point.
    pub(crate) lead: u32,
}

impl DemandFrame<'_> {
    pub(crate) fn path_demand(&self) -> PathDemand {
        PathDemand::of(self.path)
    }

    pub(crate) fn is_portal_walk(&self) -> bool {
        matches!(self.path, VisibilityPath::PrlPortal { .. })
    }

    /// Whether the frame draws cells whose visible misses count: every path
    /// but the empty world, which draws no cell.
    pub(crate) fn draws_cells(&self) -> bool {
        !matches!(self.path, VisibilityPath::EmptyWorldFallback)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lead_defaults_to_sixteen_metres_within_the_baked_maximum() {
        let demand = CellDemand::new(32 * LEAD_UNITS_PER_METRE);
        assert_eq!(demand.lead(), 16 * LEAD_UNITS_PER_METRE);
        let short = CellDemand::new(8 * LEAD_UNITS_PER_METRE);
        assert_eq!(short.lead(), 8 * LEAD_UNITS_PER_METRE, "clamped to max");
    }

    #[test]
    fn lead_lever_clamps_to_the_baked_maximum() {
        let mut demand = CellDemand::new(32 * LEAD_UNITS_PER_METRE);
        demand.set_lead_metres(40.0);
        assert_eq!(demand.lead(), 32 * LEAD_UNITS_PER_METRE);
        assert_eq!(demand.max_lead_metres(), 32.0);
        demand.set_lead_metres(-3.0);
        assert_eq!(demand.lead(), 0);
        demand.set_lead_metres(2.5);
        assert_eq!(demand.lead(), 2560);
    }
}
