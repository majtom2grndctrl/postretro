// Streamed-map counterpart to `sh_diagnostics`'s whole-loaded overlay path.
// See: context/lib/rendering_pipeline.md §"Cluster SH residency"
//
// A streamed map never populates `ShVolumeResources`'s id-34 mirrors — the
// compact atlas body is intentionally omitted. Grid geometry, validity,
// density level, and node scale come from the streaming session's own base
// metadata (`ShResidencyState::base_metadata`) instead. `MarkerMode::Residency`
// additionally colors each probe from the renderer's live residency mirrors
// (`ShResidencyState::probe_residency_classes`) rather than bake-time data —
// it only exists here because it has no meaning against a whole-loaded volume.
//
// Split out of `sh_diagnostics.rs` per context/lib/development_guide.md §2:
// that file was already over the split-before-adding threshold.

use glam::Vec3;
use postretro_level_loader::{LevelWorld, ShStreamBaseMetadata};

use super::debug_lines::DebugLineRenderer;
use super::sh_diagnostics::{
    COLOR_BASE_AABB, COLOR_PROBE_INVALID, COLOR_PROBE_UNIFORM, COLOR_PROBE_VALID,
    MarkerMode, ShDiagnosticsState, density_level_marker_color, emit_cells, probe_index,
};
use super::sh_streaming::{ProbeResidencyClass, ShResidencyState};

/// The shader can sample this probe's tile right now.
const COLOR_RESIDENCY_SAMPLEABLE: [u8; 4] = [50, 220, 90, 255];
/// Installed on GPU, awaiting the next compose→sample promotion.
const COLOR_RESIDENCY_INSTALLED: [u8; 4] = [235, 210, 40, 255];
/// Owner cluster is a current target but nothing has installed for this probe yet.
const COLOR_RESIDENCY_REQUESTED: [u8; 4] = [255, 150, 30, 255];
/// Valid probe with no residency activity at all — a sampling miss.
const COLOR_RESIDENCY_MISS: [u8; 4] = [225, 40, 40, 255];
/// Bake-time invalid probe (inside solid / off-grid).
const COLOR_RESIDENCY_INVALID: [u8; 4] = [110, 110, 110, 200];

/// Marker color for `MarkerMode::Residency`. Five clearly distinct hues so
/// mixed states in one grid stay legible at a glance in the 3D view.
fn residency_marker_color(class: ProbeResidencyClass) -> [u8; 4] {
    match class {
        ProbeResidencyClass::Sampleable => COLOR_RESIDENCY_SAMPLEABLE,
        ProbeResidencyClass::InstalledAwaitingCompose => COLOR_RESIDENCY_INSTALLED,
        ProbeResidencyClass::Requested => COLOR_RESIDENCY_REQUESTED,
        ProbeResidencyClass::Miss => COLOR_RESIDENCY_MISS,
        ProbeResidencyClass::Invalid => COLOR_RESIDENCY_INVALID,
    }
}

/// Classify one probe from the renderer's own residency mirrors. Order
/// matters: an invalid probe is never anything else, and a sampleable probe
/// always wins over "installed" or "requested" even if its owner is also a
/// live target. Pure and free of `ShResidencyState` so it is testable without
/// constructing renderer state; `ShResidencyState::probe_residency_classes`
/// (in `sh_streaming::probe_diagnostics`) maps this over every dense probe.
pub(super) fn classify_probe(
    validity: u8,
    sampled_word: u32,
    compose_word: u32,
    owner_is_target: bool,
) -> ProbeResidencyClass {
    if validity == 0 {
        return ProbeResidencyClass::Invalid;
    }
    if sampled_word != 0 {
        return ProbeResidencyClass::Sampleable;
    }
    if compose_word != 0 {
        return ProbeResidencyClass::InstalledAwaitingCompose;
    }
    if owner_is_target {
        return ProbeResidencyClass::Requested;
    }
    ProbeResidencyClass::Miss
}

/// Streamed grid origin, cell size, and world-space extent, read from the
/// session's base metadata. The streamed counterpart to the
/// `sh.grid_origin`/`sh.cell_size`/`sh.grid_dimensions` mirrors a whole-loaded
/// `ShVolumeResources` carries.
fn streamed_grid_geometry(base: &ShStreamBaseMetadata) -> (Vec3, Vec3, Vec3) {
    let dims = base.grid_dimensions;
    let origin = Vec3::from(base.grid_origin);
    let cell = Vec3::from(base.cell_size);
    let extent = Vec3::new(
        cell.x * dims[0] as f32,
        cell.y * dims[1] as f32,
        cell.z * dims[2] as f32,
    );
    (origin, cell, extent)
}

/// Emit AABB/cell/marker overlays for an active streaming session. Called
/// from `sh_diagnostics::emit` when `sh.present` is false and a streaming
/// session exists. Animated-light delta-volume AABBs stay out of scope here,
/// unchanged from the pre-streaming overlay (they only ever drew against a
/// whole-loaded SH volume).
pub(super) fn emit(
    state: &ShDiagnosticsState,
    residency: &ShResidencyState,
    camera_pos: Vec3,
    world: &LevelWorld,
    visible_cell_mask: &[bool],
    lines: &mut DebugLineRenderer,
) {
    let base = residency.base_metadata();
    let dims = base.grid_dimensions;
    let (origin, cell, extent) = streamed_grid_geometry(base);

    if state.show_base_aabb {
        lines.push_aabb_overlay(origin, origin + extent, COLOR_BASE_AABB);
    }

    if state.show_cells && state.cell_radius > 0.0 {
        emit_cells(
            state,
            dims,
            origin,
            cell,
            camera_pos,
            world,
            visible_cell_mask,
            lines,
        );
    }

    if state.show_markers && state.cell_radius > 0.0 {
        emit_markers(state, residency, base, dims, origin, cell, camera_pos, lines);
    }
}

#[allow(clippy::too_many_arguments)]
fn emit_markers(
    state: &ShDiagnosticsState,
    residency: &ShResidencyState,
    base: &ShStreamBaseMetadata,
    dims: [u32; 3],
    origin: Vec3,
    cell: Vec3,
    camera_pos: Vec3,
    lines: &mut DebugLineRenderer,
) {
    // Radius gate mirrors `sh_diagnostics::emit_markers` — without it, dense
    // probe grids blow past the debug-line segment cap and whole rooms
    // vanish from the overlay.
    let r2 = state.cell_radius * state.cell_radius;
    // Classified once per frame rather than per probe. `probe_residency_classes`
    // is only ever `Some` (computed) for `MarkerMode::Residency` — nothing
    // else in this function reads it, and every other mode skips the
    // per-probe residency walk entirely.
    let residency_classes =
        (state.marker_mode == MarkerMode::Residency).then(|| residency.probe_residency_classes());

    for z in 0..dims[2] {
        for y in 0..dims[1] {
            for x in 0..dims[0] {
                let pos =
                    origin + Vec3::new(x as f32 * cell.x, y as f32 * cell.y, z as f32 * cell.z);
                if (pos - camera_pos).length_squared() > r2 {
                    continue;
                }
                let idx = probe_index(x, y, z, dims);
                let Some(color) =
                    marker_color(state.marker_mode, base, residency_classes.as_deref(), idx)
                else {
                    continue;
                };
                lines.push_marker(pos, state.marker_scale, color);
            }
        }
    }
}

/// Resolve one streamed probe's marker color. `None` means "draw nothing":
/// used for `Irradiance`, which has no data source on a streamed map (the
/// live "total"-atlas readback is fixed at renderer init to the whole-loaded
/// atlas geometry and bind group — a streaming session owns a differently
/// sized atlas behind its own bind group, `ShResidencyState::bind_group`;
/// wiring it here would mean a second readback path, out of scope for this
/// overlay — the panel shows a label instead, see `debug_ui`), and for
/// `Residency` before a streaming session has classified anything. An
/// out-of-range probe index otherwise degrades to the invalid color, the same
/// tolerance `sh_diagnostics::emit_markers` applies to a partial mirror.
fn marker_color(
    mode: MarkerMode,
    base: &ShStreamBaseMetadata,
    residency_classes: Option<&[ProbeResidencyClass]>,
    idx: usize,
) -> Option<[u8; 4]> {
    match mode {
        MarkerMode::Uniform => Some(COLOR_PROBE_UNIFORM),
        MarkerMode::Validity => {
            let valid = base.probes.get(idx).is_some_and(|probe| probe.validity != 0);
            Some(if valid {
                COLOR_PROBE_VALID
            } else {
                COLOR_PROBE_INVALID
            })
        }
        MarkerMode::DensityLevel => {
            let (level, scale) = base
                .probes
                .get(idx)
                .map(|probe| (probe.density_level, probe.node_scale))
                .unwrap_or_default();
            Some(density_level_marker_color(level, scale))
        }
        MarkerMode::Irradiance => None,
        MarkerMode::Residency => {
            let class = residency_classes?
                .get(idx)
                .copied()
                .unwrap_or(ProbeResidencyClass::Invalid);
            Some(residency_marker_color(class))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use postretro_level_format::sh_volume::OctahedralShProbe;

    fn base_with_probes(probes: Vec<OctahedralShProbe>) -> ShStreamBaseMetadata {
        ShStreamBaseMetadata {
            grid_origin: [1.0, 2.0, 3.0],
            cell_size: [0.5, 0.25, 2.0],
            grid_dimensions: [probes.len() as u32, 1, 1],
            probe_stride: 8,
            tile_dimension: 4,
            tile_border: 1,
            atlas_dimensions: [8, 8],
            layer_count: 1,
            tiles_per_layer: 1,
            atlas_tiles_per_row: 1,
            irradiance_format: 0,
            probes,
            animation_descriptors: Vec::new(),
            slot_for_map_light: Vec::new(),
        }
    }

    #[test]
    fn classify_probe_covers_every_class_in_priority_order() {
        // Invalid wins regardless of any mirror state.
        assert_eq!(
            classify_probe(0, 0xFFFF, 0xFFFF, true),
            ProbeResidencyClass::Invalid
        );
        // Sampleable wins over installed/requested for a valid probe.
        assert_eq!(
            classify_probe(1, 0x1, 0x1, true),
            ProbeResidencyClass::Sampleable
        );
        // Installed-awaiting-compose: compose word set, sampled word not.
        assert_eq!(
            classify_probe(1, 0, 0x1, true),
            ProbeResidencyClass::InstalledAwaitingCompose
        );
        // Requested: owner is a live target, nothing installed yet.
        assert_eq!(
            classify_probe(1, 0, 0, true),
            ProbeResidencyClass::Requested
        );
        // Miss: valid, no residency activity, owner not targeted.
        assert_eq!(classify_probe(1, 0, 0, false), ProbeResidencyClass::Miss);
    }

    #[test]
    fn residency_marker_colors_are_pairwise_distinct() {
        let classes = [
            ProbeResidencyClass::Sampleable,
            ProbeResidencyClass::InstalledAwaitingCompose,
            ProbeResidencyClass::Requested,
            ProbeResidencyClass::Miss,
            ProbeResidencyClass::Invalid,
        ];
        for (i, &a) in classes.iter().enumerate() {
            for &b in &classes[i + 1..] {
                assert_ne!(
                    residency_marker_color(a),
                    residency_marker_color(b),
                    "{a:?} and {b:?} must render as distinct colors"
                );
            }
        }
    }

    #[test]
    fn streamed_grid_geometry_reads_base_metadata() {
        let base = base_with_probes(vec![OctahedralShProbe::default(); 4]);
        let (origin, cell, extent) = streamed_grid_geometry(&base);
        assert_eq!(origin, Vec3::new(1.0, 2.0, 3.0));
        assert_eq!(cell, Vec3::new(0.5, 0.25, 2.0));
        // grid_dimensions is [4, 1, 1] for a 4-probe base.
        assert_eq!(extent, Vec3::new(2.0, 0.25, 2.0));
    }

    #[test]
    fn validity_and_density_marker_colors_source_from_streamed_probes() {
        let base = base_with_probes(vec![
            OctahedralShProbe {
                validity: 1,
                density_level: 1,
                node_scale: 0,
                ..Default::default()
            },
            OctahedralShProbe {
                validity: 0,
                density_level: 0,
                node_scale: 0,
                ..Default::default()
            },
        ]);

        assert_eq!(
            marker_color(MarkerMode::Validity, &base, None, 0),
            Some(COLOR_PROBE_VALID)
        );
        assert_eq!(
            marker_color(MarkerMode::Validity, &base, None, 1),
            Some(COLOR_PROBE_INVALID)
        );
        assert_eq!(
            marker_color(MarkerMode::DensityLevel, &base, None, 0),
            Some(density_level_marker_color(1, 0))
        );
        // Out of range: tolerated like the whole-loaded path — treated as
        // invalid rather than skipped, so a partial mirror doesn't panic.
        assert_eq!(
            marker_color(MarkerMode::Validity, &base, None, 5),
            Some(COLOR_PROBE_INVALID)
        );
    }

    #[test]
    fn irradiance_mode_draws_nothing_on_a_streamed_map() {
        let base = base_with_probes(vec![OctahedralShProbe {
            validity: 1,
            ..Default::default()
        }]);
        assert_eq!(marker_color(MarkerMode::Irradiance, &base, None, 0), None);
    }

    #[test]
    fn residency_mode_draws_nothing_without_a_classified_session() {
        // `residency_classes` is only ever `Some` when computed from an
        // active `ShResidencyState` (see `emit_markers`); `None` stands in
        // for "no streaming session" and must draw nothing rather than
        // guessing a color.
        let base = base_with_probes(vec![OctahedralShProbe {
            validity: 1,
            ..Default::default()
        }]);
        assert_eq!(marker_color(MarkerMode::Residency, &base, None, 0), None);
    }

    #[test]
    fn residency_mode_colors_from_classified_state() {
        let base = base_with_probes(vec![OctahedralShProbe {
            validity: 1,
            ..Default::default()
        }]);
        let classes = [ProbeResidencyClass::Sampleable];
        assert_eq!(
            marker_color(MarkerMode::Residency, &base, Some(&classes), 0),
            Some(COLOR_RESIDENCY_SAMPLEABLE)
        );
    }
}
