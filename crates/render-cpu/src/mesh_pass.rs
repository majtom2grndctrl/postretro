// CPU-side skinned-mesh visibility and clip metadata.
// See: context/lib/rendering_pipeline.md §9

use postretro_level_loader::LevelWorld;
pub use postretro_model::ClipMetadata;
use postretro_visibility::VisibleCells;

/// How far a model's cell-cull point sits above its origin, along the model's
/// own up axis, in metres.
///
/// Models are authored with the origin between the feet, so a placed model's
/// origin lies on the floor it stands on. The cell locator sends a point on a
/// splitting plane to the plane's front child, and a floor-height splitter is
/// often a wall brush's downward-facing bottom face, whose front is the solid
/// beneath the floor. The lift moves the point off that plane into the space
/// the body occupies. 5 cm clears f32 rounding of plane distances and a
/// grounded body's few-centimetre sink, and stays far below any body height.
/// It matches `FLOOR_PROBE` in the lightmap walk measurement, which asks the
/// locator for a floor's standing cell the same way.
pub const MESH_CULL_LIFT: f32 = 0.05;

/// The point a model is located at for its cell cull: its origin raised
/// [`MESH_CULL_LIFT`] along its own up axis, taken from `model`, the model's
/// world transform. Following the model's up rather than world up keeps a
/// prop mounted on a wall or hung from a ceiling, turned so its feet face the
/// surface, lifted off that surface into the room.
pub fn mesh_cull_point(model: &glam::Mat4) -> glam::Vec3 {
    let up = model
        .transform_vector3(glam::Vec3::Y)
        .normalize_or(glam::Vec3::Y);
    model.w_axis.truncate() + up * MESH_CULL_LIFT
}

/// Pure cull decision for one skinned-mesh instance — GPU-free, unit-testable.
///
/// An instance draws iff the visible set is `DrawAll`, or the cell `pos`
/// lands in is a member of the visible cell set. Mesh callers pass the
/// model's [`mesh_cull_point`], never its bare origin.
///
/// Render-frame collectors (the ghost collectors in `scripting/frame_systems/`)
/// call this, holding the `LevelWorld` and the frame's `VisibleCells`, before
/// pushing an instance into the draw list, so the renderer's GPU pass never
/// needs a world reference. The skinned-mesh collector
/// (`scripting/frame_systems/mesh_render.rs`) tests the main view and the
/// planar mirror's through [`mesh_visible_in_cell`] instead (`CullViews::reach`),
/// locating the position once for both. The mesh cull tests the entity's
/// CURRENT-TICK transform (stable per-tick visibility), not the sub-tick
/// interpolated position. The locator lookup and the membership decision are
/// split so the decision is unit-testable without constructing a full
/// `LevelWorld`.
pub fn mesh_visible(world: &LevelWorld, visible: &VisibleCells, pos: glam::Vec3) -> bool {
    // `DrawAll` short-circuits before the cell lookup: every instance draws, so
    // the locator descent is pure waste on that path.
    let VisibleCells::Culled(_) = visible else {
        return true;
    };
    let cell = world.locate_cell(pos) as u32;
    mesh_visible_in_cell(visible, cell)
}

/// Membership half of the cull decision: does `cell_id` draw given `visible`?
/// `DrawAll` always draws; otherwise the cell must be in the visible cell set,
/// which is ascending, so membership is a binary search. Pure data logic — no
/// world, no GPU. A collector testing one position against several views'
/// cells locates it once and calls this per view.
pub fn mesh_visible_in_cell(visible: &VisibleCells, cell_id: u32) -> bool {
    match visible {
        VisibleCells::DrawAll => true,
        VisibleCells::Culled(cells) => cells.binary_search(&cell_id).is_ok(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mesh_cull_excludes_instance_in_nonvisible_cell() {
        let visible = VisibleCells::Culled(vec![0]);
        assert!(!mesh_visible_in_cell(&visible, 1));
    }

    #[test]
    fn mesh_cull_point_lifts_the_origin_along_the_model_up_axis() {
        use glam::{Mat4, Quat, Vec3};
        let origin = Vec3::new(3.0, 0.0, -2.0);
        let upright = Mat4::from_scale_rotation_translation(
            Vec3::splat(4.0),
            Quat::from_rotation_y(1.0),
            origin,
        );
        assert!(
            mesh_cull_point(&upright).abs_diff_eq(origin + Vec3::Y * MESH_CULL_LIFT, 1e-6),
            "an upright model lifts straight up, by the lift alone whatever its scale or yaw",
        );
        // Turned a quarter about +Z, the model's up is world -X: a prop with
        // its feet against a wall on its +X side.
        let on_wall = Mat4::from_rotation_translation(
            Quat::from_rotation_z(std::f32::consts::FRAC_PI_2),
            origin,
        );
        assert!(
            mesh_cull_point(&on_wall).abs_diff_eq(origin - Vec3::X * MESH_CULL_LIFT, 1e-6),
            "a turned model lifts along its own up",
        );
    }

    #[test]
    fn mesh_cull_includes_instance_in_visible_cell() {
        let visible = VisibleCells::Culled(vec![0, 1]);
        assert!(mesh_visible_in_cell(&visible, 1));
    }

    #[test]
    fn mesh_cull_finds_every_member_of_a_wide_ascending_set() {
        let visible = VisibleCells::Culled((0..200).map(|cell| cell * 3).collect());
        for cell in 0..600 {
            assert_eq!(mesh_visible_in_cell(&visible, cell), cell % 3 == 0);
        }
    }

    #[test]
    fn mesh_cull_includes_instance_on_draw_all() {
        assert!(mesh_visible_in_cell(&VisibleCells::DrawAll, 1));
        assert!(mesh_visible_in_cell(&VisibleCells::DrawAll, 999));
    }
}
