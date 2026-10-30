// Debug-overlay and spatial diagnostics state: wireframe and BVH/cell/portal
// overlay modes, and the camera-cull and spatial-tab diagnostics.
// See: context/lib/rendering_pipeline.md §7.8

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[cfg_attr(not(feature = "dev-tools"), allow(dead_code))]
pub enum WorldWireframeMode {
    Off,
    CullStatusTrianglesAlwaysOnTop,
    VisibleTrianglesDepthTested,
}

#[cfg_attr(not(feature = "dev-tools"), allow(dead_code))]
impl WorldWireframeMode {
    pub const ALL_VARIANTS: [Self; 3] = [
        Self::Off,
        Self::CullStatusTrianglesAlwaysOnTop,
        Self::VisibleTrianglesDepthTested,
    ];

    pub const fn label(self) -> &'static str {
        match self {
            Self::Off => "Off",
            Self::CullStatusTrianglesAlwaysOnTop => "Cull-status triangles (all BVH leaves, x-ray)",
            Self::VisibleTrianglesDepthTested => "CPU-visible triangles (depth-tested)",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[cfg_attr(not(feature = "dev-tools"), allow(dead_code))]
pub enum BvhOverlayColorMode {
    CellId,
}

#[cfg_attr(not(feature = "dev-tools"), allow(dead_code))]
impl BvhOverlayColorMode {
    pub const ALL_VARIANTS: [Self; 1] = [Self::CellId];

    pub const fn label(self) -> &'static str {
        match self {
            Self::CellId => "Stable cell ID",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[cfg_attr(not(feature = "dev-tools"), allow(dead_code))]
pub enum BvhOverlayDepthMode {
    DepthTested,
    XRayAlwaysOnTop,
}

#[cfg_attr(not(feature = "dev-tools"), allow(dead_code))]
impl BvhOverlayDepthMode {
    pub const ALL_VARIANTS: [Self; 2] = [Self::DepthTested, Self::XRayAlwaysOnTop];

    pub const fn label(self) -> &'static str {
        match self {
            Self::DepthTested => "Depth-tested",
            Self::XRayAlwaysOnTop => "X-ray / always on top",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[cfg_attr(not(feature = "dev-tools"), allow(dead_code))]
pub struct BvhOverlayBudget {
    pub max_boxes: usize,
    pub stride: usize,
    pub visible_cells_only: bool,
}

impl Default for BvhOverlayBudget {
    fn default() -> Self {
        Self {
            max_boxes: 512,
            stride: 1,
            visible_cells_only: false,
        }
    }
}

#[cfg_attr(not(feature = "dev-tools"), allow(dead_code))]
impl BvhOverlayBudget {
    pub fn sanitized(self) -> Self {
        Self {
            max_boxes: self.max_boxes,
            stride: self.stride.max(1),
            visible_cells_only: self.visible_cells_only,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[cfg_attr(not(feature = "dev-tools"), allow(dead_code))]
pub struct BvhOverlayState {
    pub visible: bool,
    pub color_mode: BvhOverlayColorMode,
    pub depth_mode: BvhOverlayDepthMode,
    pub budget: BvhOverlayBudget,
}

impl Default for BvhOverlayState {
    fn default() -> Self {
        Self {
            visible: false,
            color_mode: BvhOverlayColorMode::CellId,
            depth_mode: BvhOverlayDepthMode::DepthTested,
            budget: BvhOverlayBudget::default(),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[cfg_attr(not(feature = "dev-tools"), allow(dead_code))]
pub struct CellOverlayState {
    pub visible: bool,
    pub depth_mode: BvhOverlayDepthMode,
}

impl Default for CellOverlayState {
    fn default() -> Self {
        Self {
            visible: false,
            depth_mode: BvhOverlayDepthMode::DepthTested,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[cfg_attr(not(feature = "dev-tools"), allow(dead_code))]
pub struct PortalOverlayState {
    pub visible: bool,
    pub depth_mode: BvhOverlayDepthMode,
}

impl Default for PortalOverlayState {
    fn default() -> Self {
        Self {
            visible: false,
            depth_mode: BvhOverlayDepthMode::DepthTested,
        }
    }
}

#[cfg(feature = "dev-tools")]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AgentOverlayState {
    pub enabled: bool,
    pub paths: bool,
    pub velocities: bool,
    pub destinations: bool,
    pub labels: bool,
}

#[cfg(feature = "dev-tools")]
impl Default for AgentOverlayState {
    fn default() -> Self {
        Self {
            enabled: false,
            paths: true,
            velocities: true,
            destinations: true,
            labels: true,
        }
    }
}

/// Which camera-cull path ran for a frame, surfaced to the Spatial diagnostics
/// tab. Diagnostic only — never gates behavior.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CameraCullPath {
    /// Visible-cell candidate cull (valid `CellDrawIndex` + `Culled` + portal or
    /// portal-step-limit provenance). `candidate_leaves` is the gathered count.
    Candidate { candidate_leaves: u32 },
    /// Whole-BVH tree walk (`DrawAll`, solid/exterior/no-portals `Culled`, or an
    /// out-of-range visible cell id).
    TreeWalk,
}

/// CPU-derived camera-cull diagnostics for the Spatial tab. Refreshed after
/// camera visibility is known and before debug UI renders. Exposes
/// candidate-vs-total leaves so a future optional indirect-compaction pass is a
/// measured decision, not a guess. Not a perf gate; reads no GPU buffers.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CameraCullDiagnostics {
    /// Which path the frame used.
    pub path: CameraCullPath,
    /// Total BVH leaves in the level (the tree walk's working set).
    pub total_leaves: u32,
    /// Leaves submitted this frame (passed the frustum predicate and, on the
    /// candidate path, were gathered from visible cells). CPU-derived for both
    /// cull paths so it matches the current Spatial diagnostics frame.
    pub submitted_leaves: u32,
}

impl Default for CameraCullDiagnostics {
    fn default() -> Self {
        Self {
            path: CameraCullPath::TreeWalk,
            total_leaves: 0,
            submitted_leaves: 0,
        }
    }
}

impl CameraCullDiagnostics {
    /// Candidate leaf count for the frame, or `None` on the tree-walk path
    /// (where there is no candidate gather).
    #[cfg(any(feature = "dev-tools", test))]
    pub fn candidate_leaves(&self) -> Option<u32> {
        match self.path {
            CameraCullPath::Candidate { candidate_leaves } => Some(candidate_leaves),
            CameraCullPath::TreeWalk => None,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[cfg_attr(not(feature = "dev-tools"), allow(dead_code))]
pub enum SpatialCellSetDiagnostics {
    DrawAll,
    Cells { count: u32 },
}

impl SpatialCellSetDiagnostics {
    #[cfg(any(feature = "dev-tools", test))]
    pub fn from_visible_cells(cells: &postretro_visibility::VisibleCells) -> Self {
        match cells {
            postretro_visibility::VisibleCells::Culled(cells) => Self::Cells {
                count: cells.len() as u32,
            },
            postretro_visibility::VisibleCells::DrawAll => Self::DrawAll,
        }
    }

    #[cfg(any(feature = "dev-tools", test))]
    pub fn from_cell_slice(cells: &[u32]) -> Self {
        Self::Cells {
            count: cells.len() as u32,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
#[cfg_attr(not(feature = "dev-tools"), allow(dead_code))]
pub enum LocatorDiagnostics {
    NoLevel,
    Trace(postretro_level_loader::CellLocatorTrace),
}

#[derive(Clone, Debug, PartialEq)]
#[cfg_attr(not(feature = "dev-tools"), allow(dead_code))]
pub struct SpatialDiagnostics {
    pub current_cell: Option<u32>,
    pub portal_drawable_cells: SpatialCellSetDiagnostics,
    pub fog_reachable_cells: SpatialCellSetDiagnostics,
    pub locator: LocatorDiagnostics,
}

impl Default for SpatialDiagnostics {
    fn default() -> Self {
        Self {
            current_cell: None,
            portal_drawable_cells: SpatialCellSetDiagnostics::DrawAll,
            fog_reachable_cells: SpatialCellSetDiagnostics::Cells { count: 0 },
            locator: LocatorDiagnostics::NoLevel,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn camera_cull_diagnostics_reports_candidate_leaves_per_path() {
        let candidate = CameraCullDiagnostics {
            path: CameraCullPath::Candidate {
                candidate_leaves: 7,
            },
            total_leaves: 100,
            submitted_leaves: 3,
        };
        assert_eq!(candidate.candidate_leaves(), Some(7));

        let tree_walk = CameraCullDiagnostics {
            path: CameraCullPath::TreeWalk,
            total_leaves: 100,
            submitted_leaves: 42,
        };
        assert_eq!(tree_walk.candidate_leaves(), None);
        // Default is the tree-walk path with zeroed counts.
        assert_eq!(CameraCullDiagnostics::default().candidate_leaves(), None);
    }

    #[test]
    fn spatial_cell_set_diagnostics_counts_culled_cells() {
        let cells = postretro_visibility::VisibleCells::Culled(vec![1, 2, 3]);
        assert_eq!(
            SpatialCellSetDiagnostics::from_visible_cells(&cells),
            SpatialCellSetDiagnostics::Cells { count: 3 }
        );
        assert_eq!(
            SpatialCellSetDiagnostics::from_visible_cells(
                &postretro_visibility::VisibleCells::DrawAll
            ),
            SpatialCellSetDiagnostics::DrawAll
        );
        assert_eq!(
            SpatialCellSetDiagnostics::from_cell_slice(&[4, 5]),
            SpatialCellSetDiagnostics::Cells { count: 2 }
        );
    }

    #[test]
    fn world_wireframe_modes_define_final_spatial_contract() {
        assert_eq!(
            WorldWireframeMode::ALL_VARIANTS,
            [
                WorldWireframeMode::Off,
                WorldWireframeMode::CullStatusTrianglesAlwaysOnTop,
                WorldWireframeMode::VisibleTrianglesDepthTested,
            ],
        );
        assert_eq!(WorldWireframeMode::Off.label(), "Off");
        assert_eq!(
            WorldWireframeMode::CullStatusTrianglesAlwaysOnTop.label(),
            "Cull-status triangles (all BVH leaves, x-ray)",
        );
        assert_eq!(
            WorldWireframeMode::VisibleTrianglesDepthTested.label(),
            "CPU-visible triangles (depth-tested)",
        );
    }

    #[test]
    fn spatial_overlay_defaults_are_off_depth_tested_and_cell_colored() {
        assert_eq!(
            BvhOverlayState::default(),
            BvhOverlayState {
                visible: false,
                color_mode: BvhOverlayColorMode::CellId,
                depth_mode: BvhOverlayDepthMode::DepthTested,
                budget: BvhOverlayBudget {
                    max_boxes: 512,
                    stride: 1,
                    visible_cells_only: false,
                },
            },
        );
        assert_eq!(
            CellOverlayState::default(),
            CellOverlayState {
                visible: false,
                depth_mode: BvhOverlayDepthMode::DepthTested,
            },
        );
        assert_eq!(
            PortalOverlayState::default(),
            PortalOverlayState {
                visible: false,
                depth_mode: BvhOverlayDepthMode::DepthTested,
            },
        );
    }
}
