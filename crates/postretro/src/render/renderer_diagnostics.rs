// Owns renderer diagnostics queries and dev-tools overlay emission.
// Governing context: context/lib/rendering_pipeline.md

use super::*;

#[cfg(any(feature = "dev-tools", test))]
fn stable_bvh_cell_color(cell_id: u32) -> [u8; 4] {
    const PALETTE: [[u8; 4]; 12] = [
        [244, 114, 182, 255],
        [96, 165, 250, 255],
        [52, 211, 153, 255],
        [251, 191, 36, 255],
        [167, 139, 250, 255],
        [248, 113, 113, 255],
        [45, 212, 191, 255],
        [250, 204, 21, 255],
        [129, 140, 248, 255],
        [74, 222, 128, 255],
        [251, 146, 60, 255],
        [56, 189, 248, 255],
    ];

    PALETTE[(cell_id as usize) % PALETTE.len()]
}

#[cfg(any(feature = "dev-tools", test))]
fn bvh_overlay_color(leaf: &crate::geometry::BvhLeaf, color_mode: BvhOverlayColorMode) -> [u8; 4] {
    match color_mode {
        BvhOverlayColorMode::CellId => stable_bvh_cell_color(leaf.cell_id),
    }
}

#[cfg(any(feature = "dev-tools", test))]
fn bvh_leaf_aabb(leaf: &crate::geometry::BvhLeaf) -> (Vec3, Vec3) {
    (
        Vec3::from_array(leaf.aabb_min),
        Vec3::from_array(leaf.aabb_max),
    )
}

#[cfg(any(feature = "dev-tools", test))]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum CellOverlayKind {
    DrawableVisible,
    DrawableHidden,
    DrawableDrawAllFallback,
    Empty,
}

#[cfg(any(feature = "dev-tools", test))]
const COLOR_CELL_VISIBLE: [u8; 4] = [56, 230, 92, 230];
#[cfg(any(feature = "dev-tools", test))]
const COLOR_CELL_HIDDEN: [u8; 4] = [64, 156, 255, 170];
#[cfg(any(feature = "dev-tools", test))]
const COLOR_CELL_DRAW_ALL_FALLBACK: [u8; 4] = [255, 190, 72, 230];
#[cfg(any(feature = "dev-tools", test))]
const COLOR_CELL_EMPTY: [u8; 4] = [170, 180, 190, 90];
#[cfg(feature = "dev-tools")]
const COLOR_PORTAL_EDGE: [u8; 4] = [255, 214, 92, 255];

#[cfg(any(feature = "dev-tools", test))]
fn cell_overlay_kind(
    leaf_index: usize,
    leaf: &crate::prl::LeafData,
    visible_cells: &crate::visibility::VisibleCells,
) -> Option<CellOverlayKind> {
    if leaf.is_solid {
        return None;
    }

    if leaf.face_count == 0 {
        return Some(CellOverlayKind::Empty);
    }

    match visible_cells {
        crate::visibility::VisibleCells::Culled(cells) => {
            if cells.contains(&(leaf_index as u32)) {
                Some(CellOverlayKind::DrawableVisible)
            } else {
                Some(CellOverlayKind::DrawableHidden)
            }
        }
        crate::visibility::VisibleCells::DrawAll => Some(CellOverlayKind::DrawableDrawAllFallback),
    }
}

#[cfg(any(feature = "dev-tools", test))]
fn cell_overlay_color(
    leaf_index: usize,
    leaf: &crate::prl::LeafData,
    visible_cells: &crate::visibility::VisibleCells,
) -> Option<[u8; 4]> {
    match cell_overlay_kind(leaf_index, leaf, visible_cells)? {
        CellOverlayKind::DrawableVisible => Some(COLOR_CELL_VISIBLE),
        CellOverlayKind::DrawableHidden => Some(COLOR_CELL_HIDDEN),
        CellOverlayKind::DrawableDrawAllFallback => Some(COLOR_CELL_DRAW_ALL_FALLBACK),
        CellOverlayKind::Empty => Some(COLOR_CELL_EMPTY),
    }
}

#[cfg(any(feature = "dev-tools", test))]
fn portal_edges(portal: &crate::prl::PortalData) -> Vec<(Vec3, Vec3)> {
    if portal.polygon.len() < 2 {
        return Vec::new();
    }

    let mut edges = Vec::with_capacity(portal.polygon.len());
    for i in 0..portal.polygon.len() {
        let start = portal.polygon[i];
        let end = portal.polygon[(i + 1) % portal.polygon.len()];
        edges.push((start, end));
    }
    edges
}

#[cfg(any(feature = "dev-tools", test))]
pub(crate) fn select_bvh_overlay_leaf_indices(
    leaves: &[crate::geometry::BvhLeaf],
    budget: BvhOverlayBudget,
    visible_cells: Option<&[bool]>,
) -> Vec<usize> {
    let budget = budget.sanitized();
    if budget.max_boxes == 0 {
        return Vec::new();
    }

    leaves
        .iter()
        .enumerate()
        .filter(|(leaf_index, leaf)| {
            if leaf_index % budget.stride != 0 {
                return false;
            }
            if !budget.visible_cells_only {
                return true;
            }
            visible_cells
                .map(|cells| cells.get(leaf.cell_id as usize).copied().unwrap_or(false))
                .unwrap_or(true)
        })
        .map(|(leaf_index, _)| leaf_index)
        .take(budget.max_boxes)
        .collect()
}

#[cfg(feature = "dev-tools")]
pub(super) fn emit_bvh_overlay(
    leaves: &[crate::geometry::BvhLeaf],
    state: BvhOverlayState,
    visible_cells: Option<&[bool]>,
    lines: &mut super::debug_lines::DebugLineRenderer,
) {
    if !state.visible || leaves.is_empty() {
        return;
    }

    let selected_leaf_indices =
        select_bvh_overlay_leaf_indices(leaves, state.budget, visible_cells);
    for leaf_index in selected_leaf_indices {
        let Some(leaf) = leaves.get(leaf_index) else {
            continue;
        };
        let (min, max) = bvh_leaf_aabb(leaf);
        let color = bvh_overlay_color(leaf, state.color_mode);
        match state.depth_mode {
            BvhOverlayDepthMode::DepthTested => lines.push_aabb(min, max, color),
            BvhOverlayDepthMode::XRayAlwaysOnTop => lines.push_aabb_overlay(min, max, color),
        }
    }
}

#[cfg(feature = "dev-tools")]
pub(super) fn emit_cell_overlay(
    world: &crate::prl::LevelWorld,
    visible_cells: &crate::visibility::VisibleCells,
    state: CellOverlayState,
    lines: &mut super::debug_lines::DebugLineRenderer,
) {
    if !state.visible || world.leaves.is_empty() {
        return;
    }

    for (leaf_index, leaf) in world.leaves.iter().enumerate() {
        let Some(color) = cell_overlay_color(leaf_index, leaf, visible_cells) else {
            continue;
        };
        match state.depth_mode {
            BvhOverlayDepthMode::DepthTested => {
                lines.push_aabb(leaf.bounds_min, leaf.bounds_max, color);
            }
            BvhOverlayDepthMode::XRayAlwaysOnTop => {
                lines.push_aabb_overlay(leaf.bounds_min, leaf.bounds_max, color);
            }
        }
    }
}

#[cfg(feature = "dev-tools")]
pub(super) fn emit_portal_overlay(
    world: &crate::prl::LevelWorld,
    state: PortalOverlayState,
    lines: &mut super::debug_lines::DebugLineRenderer,
) {
    if !state.visible || world.portals.is_empty() {
        return;
    }

    for portal in &world.portals {
        for (start, end) in portal_edges(portal) {
            match state.depth_mode {
                BvhOverlayDepthMode::DepthTested => {
                    lines.push_line(start, end, COLOR_PORTAL_EDGE);
                }
                BvhOverlayDepthMode::XRayAlwaysOnTop => {
                    lines.push_line_overlay(start, end, COLOR_PORTAL_EDGE);
                }
            }
        }
    }
}

impl Renderer {
    /// `true` when the loaded map carries a baked SH volume. The diagnostic
    /// panel queries this to render either live controls or a disabled-state label.
    #[cfg(feature = "dev-tools")]
    pub fn has_sh_volume(&self) -> bool {
        self.sh_volume_resources.present
    }

    /// `true` when the loaded map carries a baked SDF static-occluder atlas.
    /// The SDF shadow pass gates its dispatch on this; the SDF visibility
    /// applies to the per-light `sdf`-tagged diffuse/specular forward loops,
    /// not to `lm_irr`. Legacy PRLs report `false` and the renderer degrades
    /// cleanly to `main`-equivalent lighting.
    #[allow(dead_code)]
    pub fn has_sdf_atlas(&self) -> bool {
        self.sdf_atlas_resources.present
    }

    /// Borrow the SDF atlas resources. The SDF shadow pass consumes the
    /// bind group + layout here; no other pass should bind these — forward
    /// gets only an upsampled shadow-factor texture in group 5.
    #[allow(dead_code)]
    pub fn sdf_atlas_resources(&self) -> &SdfAtlasResources {
        &self.sdf_atlas_resources
    }

    /// Lightmap bake mode read from the PRL (Shadowed = visibility baked in).
    /// Under the disjoint-direct design, `sdf` lights are excluded from
    /// `lm_irr` at bake time, so the forward pass never multiplies SDF
    /// visibility into the static-lightmap term; this accessor is retained
    /// only for legacy-PRL compatibility.
    #[allow(dead_code)]
    pub fn lightmap_mode(&self) -> crate::prl::LightmapMode {
        self.lightmap_mode
    }

    /// Per-animated-light delta-volume metadata for the SH diagnostic overlay.
    /// Empty when the map has no delta SH volumes.
    #[cfg(feature = "dev-tools")]
    pub fn sh_delta_volumes(&self) -> &[sh_volume::DeltaVolumeMeta] {
        &self.sh_delta_volumes_meta
    }

    /// Emits SH diagnostic line segments into the renderer's per-frame debug-line
    /// buffer. Called from the frame loop between egui UI build and
    /// `render_frame_indirect`. The caller is responsible for clearing the
    /// debug-line buffer before this call (via `clear_debug_lines`) so the
    /// emit path stays purely additive and other debug-line producers can
    /// coexist; this also keeps the buffer bounded across early-return frames
    /// (Timeout/Occluded/Outdated) where `render_frame_indirect` skips its
    /// debug-line render pass.
    ///
    /// `visible_leaf_mask` is the same portal-reachable leaf mask passed to
    /// `render_frame_indirect`; the cells overlay colors each cell by the
    /// frame-visibility of the leaf its center sits in.
    #[cfg(feature = "dev-tools")]
    pub fn emit_sh_diagnostics(
        &mut self,
        state: &sh_diagnostics::ShDiagnosticsState,
        camera_pos: Vec3,
        world: &crate::prl::LevelWorld,
        visible_leaf_mask: &[bool],
    ) {
        // Drive the live atlas readback only while the irradiance overlay is
        // actually drawn — every other frame it costs nothing.
        let want_live_irradiance = state.show_markers
            && state.marker_mode == sh_diagnostics::MarkerMode::Irradiance
            && self.sh_volume_resources.present;
        self.sh_probe_readback.set_wanted(want_live_irradiance);

        sh_diagnostics::emit(
            state,
            &self.sh_volume_resources,
            &self.sh_delta_volumes_meta,
            camera_pos,
            world,
            visible_leaf_mask,
            &mut self.debug_lines,
        );
    }

    /// Emit navmesh diagnostic debug lines (region rectangles + portal edges)
    /// from the runtime nav graph. No-op while the overlay is toggled off.
    /// Must run after `clear_debug_lines` and before the frame's debug-line
    /// pass, mirroring `emit_sh_diagnostics`.
    #[cfg(feature = "dev-tools")]
    pub fn emit_nav_diagnostics(&mut self, graph: &crate::nav::NavGraph) {
        if !self.show_navmesh {
            return;
        }
        nav_diagnostics::emit(graph, &mut self.debug_lines);
    }

    /// Emit agent path/corridor diagnostic debug lines: the corridor from the
    /// agent's `position` through its remaining funnel waypoints (from `cursor`),
    /// plus a per-waypoint cross marker sized to the capsule `radius`. Gated by
    /// the same navmesh overlay toggle (`Alt+Shift+N`) so the path draws
    /// alongside the region/portal overlay. Must run after `clear_debug_lines`
    /// and before the frame's debug-line pass, mirroring `emit_nav_diagnostics`.
    ///
    /// Keeps all wgpu renderer-side (Renderer-owns-GPU): the call site hands in
    /// plain agent geometry, never a debug-line / wgpu handle. The render-private
    /// `nav_diagnostics::emit_agent_path` (it is `pub(super)`) is reached only
    /// through this wrapper.
    #[cfg(feature = "dev-tools")]
    pub fn emit_agent_path_overlay(
        &mut self,
        position: Vec3,
        path: &[Vec3],
        cursor: usize,
        radius: f32,
    ) {
        if !self.show_navmesh {
            return;
        }
        nav_diagnostics::emit_agent_path(position, path, cursor, radius, &mut self.debug_lines);
    }

    /// Draw an "ugly-but-honest" wireframe capsule at each replicated remote
    /// entity position (M15 Phase 1 netcode visibility aid). `centers` are the
    /// pawn `Transform.position`s collected client-side by
    /// `netcode::remote_entity_positions`; `radius`/`half_height` size the
    /// capsule to the standing player volume. Cyan so it reads clearly against
    /// the world. No-op when `centers` is empty (single-player and host).
    ///
    /// Routed through the **overlay (always-on-top)** debug-line path
    /// (`push_capsule_overlay`), NOT the depth-tested `push_capsule`: the marker's
    /// whole job is "where is the other player," so it must stay visible when the
    /// remote pawn moves behind world geometry. The depth-tested path culls the
    /// capsule's fragments against the opaque world depth buffer, which made the
    /// capsule vanish whenever a wall came between the client camera and the host
    /// pawn — the "gets lost after moving around" symptom.
    ///
    /// Renderer-owns-GPU: the call site hands in plain `Vec3` positions and
    /// capsule dims — never a debug-line / wgpu handle. Must run after
    /// `clear_debug_lines` and before the frame's debug-line pass, mirroring the
    /// other per-frame overlay emitters.
    #[cfg(feature = "dev-tools")]
    pub fn emit_remote_entity_markers(&mut self, centers: &[Vec3], radius: f32, half_height: f32) {
        const REMOTE_ENTITY_COLOR: [u8; 4] = [0, 255, 255, 255]; // cyan
        for &center in centers {
            self.debug_lines
                .push_capsule_overlay(center, radius, half_height, REMOTE_ENTITY_COLOR);
        }
    }

    /// Flip the navmesh overlay on/off. Bound to `Alt+Shift+N`.
    #[cfg(feature = "dev-tools")]
    pub fn toggle_navmesh_overlay(&mut self) -> bool {
        self.show_navmesh = !self.show_navmesh;
        log::info!(
            "[Renderer] Navmesh overlay: {}",
            if self.show_navmesh { "on" } else { "off" },
        );
        self.show_navmesh
    }

    pub fn toggle_wireframe(&mut self) -> bool {
        let next_mode =
            if self.world_wireframe_mode == WorldWireframeMode::CullStatusTrianglesAlwaysOnTop {
                WorldWireframeMode::Off
            } else {
                WorldWireframeMode::CullStatusTrianglesAlwaysOnTop
            };
        self.set_world_wireframe_mode(next_mode);
        log::info!(
            "[Renderer] Wireframe overlay: {}",
            self.world_wireframe_mode.label(),
        );
        self.wireframe_enabled
    }

    #[cfg_attr(not(feature = "dev-tools"), allow(dead_code))]
    pub fn world_wireframe_mode(&self) -> WorldWireframeMode {
        self.world_wireframe_mode
    }

    pub fn set_world_wireframe_mode(&mut self, mode: WorldWireframeMode) {
        self.world_wireframe_mode = mode;
        self.wireframe_enabled = mode != WorldWireframeMode::Off;
    }

    #[cfg(feature = "dev-tools")]
    pub fn bvh_overlay_state(&self) -> BvhOverlayState {
        self.bvh_overlay
    }

    #[cfg(feature = "dev-tools")]
    pub fn set_bvh_overlay_visible(&mut self, visible: bool) {
        self.bvh_overlay.visible = visible;
    }

    #[cfg(feature = "dev-tools")]
    pub fn set_bvh_overlay_color_mode(&mut self, mode: BvhOverlayColorMode) {
        self.bvh_overlay.color_mode = mode;
    }

    #[cfg(feature = "dev-tools")]
    pub fn set_bvh_overlay_depth_mode(&mut self, mode: BvhOverlayDepthMode) {
        self.bvh_overlay.depth_mode = mode;
    }

    #[cfg(feature = "dev-tools")]
    pub fn set_bvh_overlay_budget(&mut self, budget: BvhOverlayBudget) {
        self.bvh_overlay.budget = budget.sanitized();
    }

    #[cfg(feature = "dev-tools")]
    pub fn cell_overlay_state(&self) -> CellOverlayState {
        self.cell_overlay
    }

    #[cfg(feature = "dev-tools")]
    pub fn set_cell_overlay_visible(&mut self, visible: bool) {
        self.cell_overlay.visible = visible;
    }

    #[cfg(feature = "dev-tools")]
    pub fn set_cell_overlay_depth_mode(&mut self, mode: BvhOverlayDepthMode) {
        self.cell_overlay.depth_mode = mode;
    }

    /// Last frame's CPU-derived camera-cull diagnostics: which path ran
    /// (candidate vs tree walk), candidate/total/submitted leaf counts.
    /// Surfaced in the Spatial diagnostics tab. Diagnostic only.
    #[cfg(feature = "dev-tools")]
    pub fn camera_cull_diagnostics(&self) -> CameraCullDiagnostics {
        self.camera_cull_diagnostics
    }

    #[cfg(feature = "dev-tools")]
    pub fn portal_overlay_state(&self) -> PortalOverlayState {
        self.portal_overlay
    }

    #[cfg(feature = "dev-tools")]
    pub fn set_portal_overlay_visible(&mut self, visible: bool) {
        self.portal_overlay.visible = visible;
    }

    #[cfg(feature = "dev-tools")]
    pub fn set_portal_overlay_depth_mode(&mut self, mode: BvhOverlayDepthMode) {
        self.portal_overlay.depth_mode = mode;
    }

    #[cfg(feature = "dev-tools")]
    #[allow(dead_code)]
    pub fn bvh_overlay_leaf_indices(&self, visible_cells: Option<&[bool]>) -> Vec<usize> {
        if !self.bvh_overlay.visible {
            return Vec::new();
        }
        select_bvh_overlay_leaf_indices(&self.bvh_leaves, self.bvh_overlay.budget, visible_cells)
    }

    /// Emit compiled BVH leaf AABBs into the per-frame debug-line buffer.
    /// The caller must clear debug lines before this method and call it before
    /// `render_frame_indirect`, matching the SH/nav diagnostic emitters. This
    /// intentionally uses only renderer-owned CPU copies of the loaded BVH
    /// leaves; it does not read back GPU cull status.
    #[cfg(feature = "dev-tools")]
    pub fn emit_bvh_overlay_diagnostics(&mut self, visible_cells: Option<&[bool]>) {
        emit_bvh_overlay(
            &self.bvh_leaves,
            self.bvh_overlay,
            visible_cells,
            &mut self.debug_lines,
        );
    }

    /// Emit BSP cell bounds colored from the current frame's drawable
    /// `VisibleCells`. This intentionally ignores fog/light reachable masks:
    /// `Culled` is the exact visible drawable set, and `DrawAll` uses a distinct
    /// fallback color so it does not read as a successful portal traversal.
    #[cfg(feature = "dev-tools")]
    pub fn emit_cell_overlay_diagnostics(
        &mut self,
        world: &crate::prl::LevelWorld,
        visible_cells: &crate::visibility::VisibleCells,
    ) {
        emit_cell_overlay(
            world,
            visible_cells,
            self.cell_overlay,
            &mut self.debug_lines,
        );
    }

    /// Emit decoded PRL portal polygon edges into the per-frame debug-line
    /// buffer. Consumes only `LevelWorld` CPU data.
    #[cfg(feature = "dev-tools")]
    pub fn emit_portal_overlay_diagnostics(&mut self, world: &crate::prl::LevelWorld) {
        emit_portal_overlay(world, self.portal_overlay, &mut self.debug_lines);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn leaf(cell_id: u32) -> crate::geometry::BvhLeaf {
        crate::geometry::BvhLeaf {
            aabb_min: [0.0, 0.0, 0.0],
            material_bucket_id: 0,
            aabb_max: [1.0, 1.0, 1.0],
            index_offset: 0,
            index_count: 0,
            cell_id,
            chunk_range_start: 0,
            chunk_range_count: 0,
        }
    }

    fn bsp_leaf(is_solid: bool, face_count: u32) -> crate::prl::LeafData {
        crate::prl::LeafData {
            bounds_min: Vec3::ZERO,
            bounds_max: Vec3::ONE,
            face_start: 0,
            face_count,
            is_solid,
        }
    }

    #[test]
    fn bvh_overlay_selection_applies_stride_before_budget() {
        let leaves = [leaf(0), leaf(0), leaf(0), leaf(0), leaf(0), leaf(0)];
        let budget = BvhOverlayBudget {
            max_boxes: 2,
            stride: 2,
            visible_cells_only: false,
        };

        assert_eq!(
            select_bvh_overlay_leaf_indices(&leaves, budget, None),
            vec![0, 2],
        );
    }

    #[test]
    fn bvh_overlay_selection_filters_visible_cells_deterministically() {
        let leaves = [leaf(0), leaf(1), leaf(2), leaf(1), leaf(3)];
        let visible_cells = [false, true, false, true];
        let budget = BvhOverlayBudget {
            max_boxes: 8,
            stride: 1,
            visible_cells_only: true,
        };

        assert_eq!(
            select_bvh_overlay_leaf_indices(&leaves, budget, Some(&visible_cells)),
            vec![1, 3, 4],
        );
    }

    #[test]
    fn bvh_overlay_selection_treats_missing_mask_as_draw_all() {
        let leaves = [leaf(0), leaf(1), leaf(2)];
        let budget = BvhOverlayBudget {
            max_boxes: 8,
            stride: 1,
            visible_cells_only: true,
        };

        assert_eq!(
            select_bvh_overlay_leaf_indices(&leaves, budget, None),
            vec![0, 1, 2],
        );
    }

    #[test]
    fn bvh_overlay_budget_sanitizes_zero_stride() {
        let budget = BvhOverlayBudget {
            max_boxes: 3,
            stride: 0,
            visible_cells_only: false,
        };

        assert_eq!(budget.sanitized().stride, 1);
    }

    #[test]
    fn bvh_overlay_color_is_stable_by_cell_id() {
        let first = leaf(7);
        let second = leaf(7);
        let different = leaf(8);

        assert_eq!(
            bvh_overlay_color(&first, BvhOverlayColorMode::CellId),
            bvh_overlay_color(&second, BvhOverlayColorMode::CellId),
        );
        assert_ne!(
            bvh_overlay_color(&first, BvhOverlayColorMode::CellId),
            bvh_overlay_color(&different, BvhOverlayColorMode::CellId),
        );
    }

    #[test]
    fn bvh_leaf_aabb_converts_prl_arrays_to_renderer_vectors() {
        let leaf = crate::geometry::BvhLeaf {
            aabb_min: [-1.0, 2.0, 3.5],
            material_bucket_id: 0,
            aabb_max: [4.0, 5.25, 6.0],
            index_offset: 0,
            index_count: 0,
            cell_id: 0,
            chunk_range_start: 0,
            chunk_range_count: 0,
        };

        let (min, max) = bvh_leaf_aabb(&leaf);

        assert_eq!(min, Vec3::new(-1.0, 2.0, 3.5));
        assert_eq!(max, Vec3::new(4.0, 5.25, 6.0));
    }

    #[test]
    fn cell_overlay_color_uses_drawable_visible_cells() {
        let visible = crate::visibility::VisibleCells::Culled(vec![2]);
        let drawable = bsp_leaf(false, 1);

        assert_eq!(
            cell_overlay_color(2, &drawable, &visible),
            Some(COLOR_CELL_VISIBLE),
        );
        assert_eq!(
            cell_overlay_color(1, &drawable, &visible),
            Some(COLOR_CELL_HIDDEN),
        );
    }

    #[test]
    fn cell_overlay_draw_all_uses_distinct_fallback_color() {
        let drawable = bsp_leaf(false, 1);

        assert_eq!(
            cell_overlay_color(0, &drawable, &crate::visibility::VisibleCells::DrawAll),
            Some(COLOR_CELL_DRAW_ALL_FALLBACK),
        );
        assert_ne!(COLOR_CELL_DRAW_ALL_FALLBACK, COLOR_CELL_VISIBLE);
    }

    #[test]
    fn cell_overlay_skips_solid_and_marks_empty_non_solid_cells_neutral() {
        let visible = crate::visibility::VisibleCells::Culled(vec![0]);

        assert_eq!(cell_overlay_color(0, &bsp_leaf(true, 1), &visible), None);
        assert_eq!(
            cell_overlay_color(0, &bsp_leaf(false, 0), &visible),
            Some(COLOR_CELL_EMPTY),
        );
    }

    #[test]
    fn portal_edges_close_polygon_loop() {
        let portal = crate::prl::PortalData {
            polygon: vec![Vec3::ZERO, Vec3::X, Vec3::Y],
            front_leaf: 0,
            back_leaf: 1,
        };

        assert_eq!(
            portal_edges(&portal),
            vec![
                (Vec3::ZERO, Vec3::X),
                (Vec3::X, Vec3::Y),
                (Vec3::Y, Vec3::ZERO)
            ],
        );
    }
}
