// SH volume diagnostic overlay: emits debug-line segments visualizing baked SH
// irradiance volumes. Gated on `dev-tools`. See: context/lib/rendering_pipeline.md §11
//
use glam::Vec3;

use super::debug_lines::DebugLineRenderer;
use super::sh_volume::{DeltaVolumeMeta, ShVolumeResources};
use crate::prl::LevelWorld;

/// Coloring mode for per-probe markers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MarkerMode {
    /// Green for `validity != 0`, red for invalid probes.
    Validity,
    /// All probes drawn with the same neutral color.
    Uniform,
}

/// Panel-bound diagnostic state. Mirrors `DiagnosticsState::seeded` discipline:
/// `seeded` flips true on first panel open so the panel can pull live defaults
/// without snapping the world. `per_light_visible` resets on map load.
pub struct ShDiagnosticsState {
    pub show_base_aabb: bool,
    pub show_cells: bool,
    pub show_markers: bool,
    pub marker_mode: MarkerMode,
    pub marker_scale: f32,
    pub cell_radius: f32,
    pub per_light_visible: Vec<bool>,
    pub seeded: bool,
}

impl Default for ShDiagnosticsState {
    fn default() -> Self {
        // All toggles default off so overlay geometry only appears in response
        // to an explicit user action in the panel. Without this, opening a map
        // for the first time would render the base AABB before the user has
        // touched the inspector.
        Self {
            show_base_aabb: false,
            show_cells: false,
            show_markers: false,
            marker_mode: MarkerMode::Validity,
            marker_scale: 0.25,
            cell_radius: 12.0,
            per_light_visible: Vec::new(),
            seeded: false,
        }
    }
}

/// Probe storage is z-major: `idx = x + y*Nx + z*Nx*Ny`. Centralized here so
/// the SH bake layout and the diagnostic reader cannot drift apart silently.
fn probe_index(x: u32, y: u32, z: u32, dims: [u32; 3]) -> usize {
    let nx = dims[0] as usize;
    let ny = dims[1] as usize;
    (x as usize) + (y as usize) * nx + (z as usize) * nx * ny
}

/// Whether delta volume `index` is currently shown. Before the panel seeds
/// `per_light_visible`, missing entries default to visible so a freshly-loaded
/// level renders all delta volumes until the user toggles them off.
fn delta_volume_visible(state: &ShDiagnosticsState, index: usize) -> bool {
    state.per_light_visible.get(index).copied().unwrap_or(true)
}

const COLOR_BASE_AABB: [u8; 4] = [255, 220, 80, 255];
const COLOR_DELTA_AABB: [u8; 4] = [200, 120, 255, 255];
/// Cell whose center sits in a leaf that the portal-reachable set covers
/// for the current frame (i.e., visible per portal traversal / frustum).
const COLOR_CELL_VISIBLE: [u8; 4] = [0, 230, 60, 200];
/// Cell whose center sits in a leaf culled by portal traversal / frustum
/// for the current frame, or in a solid leaf with no portal reach.
const COLOR_CELL_CULLED: [u8; 4] = [0, 220, 220, 200];
const COLOR_PROBE_VALID: [u8; 4] = [60, 230, 80, 255];
const COLOR_PROBE_INVALID: [u8; 4] = [230, 60, 60, 255];
const COLOR_PROBE_UNIFORM: [u8; 4] = [230, 230, 230, 255];

/// Emit one frame of SH diagnostic line segments. Driven entirely by the
/// toggles in `state` — enabled overlays continue rendering after the debug
/// panel is dismissed, and only un-checking a toggle hides its geometry.
pub(super) fn emit(
    state: &ShDiagnosticsState,
    sh: &ShVolumeResources,
    delta_vols: &[DeltaVolumeMeta],
    camera_pos: Vec3,
    world: &LevelWorld,
    visible_leaf_mask: &[bool],
    lines: &mut DebugLineRenderer,
) {
    // The frame loop clears the debug-line buffer unconditionally before
    // calling `emit`, so this function is purely additive — it never owns
    // the buffer lifecycle and never clobbers segments produced by other
    // debug-line producers in the same frame.
    if !sh.present {
        return;
    }

    let dims = sh.grid_dimensions;
    let origin = Vec3::from(sh.grid_origin);
    let cell = Vec3::from(sh.cell_size);
    let extent = Vec3::new(
        cell.x * dims[0] as f32,
        cell.y * dims[1] as f32,
        cell.z * dims[2] as f32,
    );

    if state.show_base_aabb {
        lines.push_aabb(origin, origin + extent, COLOR_BASE_AABB);
    }

    if state.show_cells && state.cell_radius > 0.0 {
        emit_cells(
            state,
            dims,
            origin,
            cell,
            camera_pos,
            world,
            visible_leaf_mask,
            lines,
        );
    }

    if state.show_markers {
        emit_markers(state, sh, dims, origin, cell, lines);
    }

    for (i, meta) in delta_vols.iter().enumerate() {
        if !delta_volume_visible(state, i) {
            continue;
        }
        let d_origin = Vec3::from(meta.origin);
        let d_extent = Vec3::new(
            meta.cell_size[0] * meta.grid_dimensions[0] as f32,
            meta.cell_size[1] * meta.grid_dimensions[1] as f32,
            meta.cell_size[2] * meta.grid_dimensions[2] as f32,
        );
        lines.push_aabb(d_origin, d_origin + d_extent, COLOR_DELTA_AABB);
    }
}

fn emit_cells(
    state: &ShDiagnosticsState,
    dims: [u32; 3],
    origin: Vec3,
    cell: Vec3,
    camera_pos: Vec3,
    world: &LevelWorld,
    visible_leaf_mask: &[bool],
    lines: &mut DebugLineRenderer,
) {
    // Cell color reflects the portal-reachable leaf set built from
    // `fog_reachable` — not the frustum+portal cull used by the wireframe
    // overlay. Leaves reachable via portals are colored visible; all others
    // are colored culled. No frustum check is applied here. An empty mask
    // is the DrawAll sentinel — fallback paths (no portals, solid-leaf,
    // exterior camera, empty world) don't compute a portal set, so every
    // cell is treated as visible to avoid misleadingly cyan overlays.
    let r2 = state.cell_radius * state.cell_radius;
    let draw_all = visible_leaf_mask.is_empty();
    for z in 0..dims[2] {
        for y in 0..dims[1] {
            for x in 0..dims[0] {
                let cell_min =
                    origin + Vec3::new(x as f32 * cell.x, y as f32 * cell.y, z as f32 * cell.z);
                let cell_max = cell_min + cell;
                let center = (cell_min + cell_max) * 0.5;
                if (center - camera_pos).length_squared() > r2 {
                    continue;
                }
                let leaf_idx = world.find_leaf(center);
                let visible = if draw_all {
                    true
                } else {
                    visible_leaf_mask.get(leaf_idx).copied().unwrap_or(false)
                };
                let color = if visible {
                    COLOR_CELL_VISIBLE
                } else {
                    COLOR_CELL_CULLED
                };
                lines.push_aabb(cell_min, cell_max, color);
            }
        }
    }
}

fn emit_markers(
    state: &ShDiagnosticsState,
    sh: &ShVolumeResources,
    dims: [u32; 3],
    origin: Vec3,
    cell: Vec3,
    lines: &mut DebugLineRenderer,
) {
    for z in 0..dims[2] {
        for y in 0..dims[1] {
            for x in 0..dims[0] {
                let pos = origin
                    + Vec3::new(
                        (x as f32 + 0.5) * cell.x,
                        (y as f32 + 0.5) * cell.y,
                        (z as f32 + 0.5) * cell.z,
                    );
                let color = match state.marker_mode {
                    MarkerMode::Uniform => COLOR_PROBE_UNIFORM,
                    MarkerMode::Validity => {
                        let idx = probe_index(x, y, z, dims);
                        // Out-of-range entries (validity slice shorter than the
                        // probe count) are treated as invalid rather than
                        // panicking — keeps the overlay tolerant of partial
                        // bakes.
                        let valid = sh.validity.get(idx).copied().unwrap_or(0) != 0;
                        if valid {
                            COLOR_PROBE_VALID
                        } else {
                            COLOR_PROBE_INVALID
                        }
                    }
                };
                lines.push_marker(pos, state.marker_scale, color);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn marker_mode_defaults_to_validity() {
        let s = ShDiagnosticsState::default();
        assert_eq!(s.marker_mode, MarkerMode::Validity);
        assert_eq!(s.marker_scale, 0.25);
        assert_eq!(s.cell_radius, 12.0);
        assert!(!s.show_base_aabb);
        assert!(!s.show_cells);
        assert!(!s.show_markers);
        assert!(!s.seeded);
        assert!(s.per_light_visible.is_empty());
    }

    /// Probe storage layout is z-major: index = x + y*Nx + z*Nx*Ny. This
    /// asserts the contract on the actual `probe_index` helper used by
    /// `emit_markers`, so a layout change in the SH bake forces the test
    /// to be updated alongside the reader.
    #[test]
    fn probe_index_is_z_major() {
        let dims = [3u32, 4u32, 5u32];
        assert_eq!(probe_index(0, 0, 0, dims), 0);
        assert_eq!(probe_index(1, 0, 0, dims), 1);
        assert_eq!(probe_index(0, 1, 0, dims), 3);
        assert_eq!(probe_index(0, 0, 1, dims), 12);
        assert_eq!(probe_index(2, 3, 4, dims), 2 + 9 + 48);
    }

    /// Contract: before the panel seeds `per_light_visible`, every delta
    /// volume is treated as visible. After seeding, the per-index flag wins.
    #[test]
    fn delta_volume_visible_defaults_true_until_seeded() {
        let mut s = ShDiagnosticsState::default();
        // Unseeded: any index is visible.
        assert!(delta_volume_visible(&s, 0));
        assert!(delta_volume_visible(&s, 7));

        // Seeded: explicit flag is respected; out-of-range still defaults true.
        s.per_light_visible = vec![true, false, true];
        assert!(delta_volume_visible(&s, 0));
        assert!(!delta_volume_visible(&s, 1));
        assert!(delta_volume_visible(&s, 2));
        assert!(delta_volume_visible(&s, 3));
    }
}
