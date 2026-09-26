// CPU-side visibility and light-reachability inputs for one render frame.
// See: context/lib/rendering_pipeline.md §§1–2

use glam::{Mat4, Vec3};
use postretro_level_loader::LevelWorld;
use postretro_renderer::ShSampleRegion;
use postretro_visibility::{
    CameraCullVisibility, VisibilityPath, VisibilityStats, VisibleCells, determine_visible_cells,
};

/// Plain render inputs derived from one visibility walk. The app and static
/// capture share this seam so a future SH-storage choice reaches the renderer
/// after visibility has been determined, without changing the frame order.
pub(crate) struct VisibleRenderPreparation {
    pub(crate) visible_cells: VisibleCells,
    pub(crate) fog_reachable: Vec<u32>,
    pub(crate) light_reachable_cell_mask: Vec<bool>,
    pub(crate) reachable_cell_aabbs: Vec<(Vec3, Vec3)>,
    /// Drawable-cell world bounds. Kept separate from fog reach so the empty
    /// fog list can retain its DrawAll sentinel at the renderer boundary.
    pub(crate) visible_cell_aabbs: Vec<ShSampleRegion>,
    pub(crate) stats: VisibilityStats,
}

impl VisibleRenderPreparation {
    /// Determine the drawable and wider light/fog-reachable sets for a loaded
    /// level. `scratch` remains caller-owned to preserve the windowed frame's
    /// allocation behavior.
    pub(crate) fn for_level(
        world: &LevelWorld,
        eye: Vec3,
        view_proj: Mat4,
        blocked_portals: &[bool],
        capture_portal_walk: bool,
        scratch: &mut Vec<u32>,
    ) -> Self {
        let (visibility, _) = determine_visible_cells(
            eye,
            view_proj,
            world,
            blocked_portals,
            capture_portal_walk,
            scratch,
        );
        let fog_reachable = visibility.fog_reachable;
        let (light_reachable_cell_mask, reachable_cell_aabbs) =
            light_reachability_inputs(world, &fog_reachable);
        let visible_cell_aabbs = visible_cell_regions(world, &visibility.visible_cells);

        Self {
            visible_cells: visibility.visible_cells,
            fog_reachable,
            light_reachable_cell_mask,
            reachable_cell_aabbs,
            visible_cell_aabbs,
            stats: visibility.stats,
        }
    }

    /// The world-free frontend path draws no level geometry and therefore has
    /// no camera cell or portal-reachability inputs.
    pub(crate) fn empty_world() -> Self {
        Self {
            visible_cells: VisibleCells::DrawAll,
            fog_reachable: Vec::new(),
            light_reachable_cell_mask: Vec::new(),
            reachable_cell_aabbs: Vec::new(),
            visible_cell_aabbs: Vec::new(),
            stats: VisibilityStats {
                camera_cell: 0,
                total_faces: 0,
                drawn_faces: 0,
                path: VisibilityPath::EmptyWorldFallback,
            },
        }
    }

    pub(crate) fn camera_cull(&self) -> CameraCullVisibility<'_> {
        CameraCullVisibility {
            cells: &self.visible_cells,
            path: self.stats.path,
        }
    }
}

fn visible_cell_regions(world: &LevelWorld, visible: &VisibleCells) -> Vec<ShSampleRegion> {
    match visible {
        VisibleCells::DrawAll => world
            .cells
            .iter()
            .map(|cell| ShSampleRegion::new(cell.bounds_min, cell.bounds_max))
            .collect(),
        VisibleCells::Culled(cells) => cells
            .iter()
            .filter_map(|&cell| world.cells.get(cell as usize))
            .map(|cell| ShSampleRegion::new(cell.bounds_min, cell.bounds_max))
            .collect(),
    }
}

/// Dynamic lights follow the wider portal-reachable fog set, not only the
/// drawable cells. An empty slice is the DrawAll fallback sentinel, so both
/// downstream inputs stay empty and every cell-assigned light remains eligible.
fn light_reachability_inputs(
    world: &LevelWorld,
    fog_reachable: &[u32],
) -> (Vec<bool>, Vec<(Vec3, Vec3)>) {
    if fog_reachable.is_empty() {
        return (Vec::new(), Vec::new());
    }

    let mut light_reachable_cell_mask = vec![false; world.cell_count()];
    let mut reachable_cell_aabbs = Vec::with_capacity(fog_reachable.len());
    for &cell_id in fog_reachable {
        let index = cell_id as usize;
        if index < light_reachable_cell_mask.len() {
            light_reachable_cell_mask[index] = true;
        }
        if let Some(cell) = world.cells.get(index) {
            reachable_cell_aabbs.push((cell.bounds_min, cell.bounds_max));
        }
    }

    (light_reachable_cell_mask, reachable_cell_aabbs)
}
