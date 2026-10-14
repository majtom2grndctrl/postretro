// Fluid surface drift: each surface face's world drift, resolved from the push
// over it at the speed that push carries a swimmer.
// See: context/lib/rendering_pipeline.md §7.3a · context/lib/movement.md §6

use glam::Vec3;
use postretro_collision::{EnvironmentView, EnvironmentVolumes, resolve_particle_acceleration};
use postretro_combat_model::FluidMovementTable;
use postretro_level_format::environment_volumes::EnvironmentVolumeRecord;
use postretro_render_cpu::fluid_surface::face_normal;

/// How far inside the fluid a face samples its push, meters: past the
/// compiler's 1 mm flush tolerance, so a current flush with the waterline
/// drifts the face it covers and a wind resting on the waterline drifts none.
pub(crate) const FLUID_DRIFT_SAMPLE_DEPTH: f32 = 0.002;

/// One world drift per surface face of every record, in record order (volume,
/// then face): the steady-state drift a swimmer settles at under the nominal
/// push winning just inside the face, in that face's own fluid. The renderer
/// keeps only its in-plane part. A dry record, a fluid the table lacks, and a
/// degenerate face drift zero.
///
/// `fluids` is the table prediction uses, read by the record's fluid name so a
/// renamed table never reads a stale index. `terminal_velocity` caps it as the
/// swimmer's drift layer is capped; `dt` is the fixed tick movement integrates.
pub(crate) fn resolve_fluid_surface_drift(
    records: &[EnvironmentVolumeRecord],
    volumes: &EnvironmentVolumes,
    fluids: &FluidMovementTable,
    terminal_velocity: f32,
    dt: f32,
) -> Vec<Vec3> {
    // Push is a point rule over the volumes; gravity and fluid are unread.
    let view = EnvironmentView::new(0.0, volumes, &[]);
    let mut drifts = Vec::with_capacity(records.iter().map(|r| r.surface_faces.len()).sum());
    for record in records {
        let viscosity = fluids
            .names()
            .iter()
            .position(|name| *name == record.fluid)
            .and_then(|index| fluids.viscosities().get(index).copied());
        for face in &record.surface_faces {
            let drift = match (viscosity, face_normal(face)) {
                (Some(viscosity), Some(normal)) if !face.vertices.is_empty() => {
                    let centroid = face
                        .vertices
                        .iter()
                        .map(|vertex| Vec3::from(vertex.position))
                        .sum::<Vec3>()
                        / face.vertices.len() as f32;
                    let inside = centroid - normal * FLUID_DRIFT_SAMPLE_DEPTH;
                    let (_, push) = resolve_particle_acceleration(view, inside);
                    postretro_physics::movement::steady_drift_velocity(
                        push,
                        viscosity,
                        terminal_velocity,
                        dt,
                    )
                }
                _ => Vec3::ZERO,
            };
            drifts.push(drift);
        }
    }
    drifts
}

#[cfg(test)]
#[path = "drift_tests.rs"]
mod tests;
