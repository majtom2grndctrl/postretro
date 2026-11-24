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
/// the body occupies. 5 cm clears f32 rounding of plane distances by orders of
/// magnitude and stays far below any body height. The lightmap walk
/// measurement probes a floor's standing cell at the same height
/// (`FLOOR_PROBE`); the two are separate constants.
pub const MESH_CULL_LIFT: f32 = 0.05;

/// The lifted point a model is first located at for its cell cull: its origin
/// raised [`MESH_CULL_LIFT`] along its own up axis, taken from `model`, the
/// model's world transform. Following the model's up rather than world up
/// keeps a prop mounted on a wall or hung from a ceiling, turned so its feet
/// face the surface, lifted off that surface into the room. A degenerate axis
/// (zero or non-finite scale) lifts along world up.
pub fn mesh_cull_point(model: &glam::Mat4) -> glam::Vec3 {
    let up = model
        .transform_vector3(glam::Vec3::Y)
        .normalize_or(glam::Vec3::Y);
    model.w_axis.truncate() + up * MESH_CULL_LIFT
}

/// The cell a model culls by: the cell holding its [`mesh_cull_point`], or,
/// when that cell is solid, the cell holding its bare origin.
///
/// A body can never be seen from inside solid, so a solid lifted cell means the
/// lift itself left the open space: a prop pivoted at its top and hung upright
/// from a ceiling lifts into the ceiling. Its origin then decides, as it did
/// before the lift, so the lift never culls a model its origin would draw.
pub fn locate_mesh_cell(world: &LevelWorld, model: &glam::Mat4) -> u32 {
    let lifted = world.locate_cell(mesh_cull_point(model));
    let cell = if world.cells.get(lifted).is_some_and(|cell| cell.is_solid) {
        world.locate_cell(model.w_axis.truncate())
    } else {
        lifted
    };
    cell as u32
}

/// Pure cull decision for one skinned-mesh instance at world transform
/// `model` — GPU-free, unit-testable.
///
/// The instance draws iff the visible set is `DrawAll`, or its
/// [`locate_mesh_cell`] is a member of the visible cell set.
///
/// The mesh ghost collector (`scripting/frame_systems/ghost_render.rs`) calls
/// this, holding the `LevelWorld` and the frame's `VisibleCells`, before
/// pushing an instance into the draw list, so the renderer's GPU pass never
/// needs a world reference. The skinned-mesh collector
/// (`scripting/frame_systems/mesh_render.rs`) tests the main view and the
/// planar mirror's through [`mesh_visible_in_cell`] instead (`CullViews::reach`),
/// locating the model once for both. The mesh cull tests the entity's
/// CURRENT-TICK transform (stable per-tick visibility), not the sub-tick
/// interpolated one.
pub fn mesh_model_visible(world: &LevelWorld, visible: &VisibleCells, model: &glam::Mat4) -> bool {
    // `DrawAll` short-circuits before the cell lookup: every instance draws, so
    // the locator descent is pure waste on that path.
    let VisibleCells::Culled(_) = visible else {
        return true;
    };
    mesh_visible_in_cell(visible, locate_mesh_cell(world, model))
}

/// Pure cull decision for a point-located instance (a billboard ghost): it
/// draws iff the visible set is `DrawAll`, or the cell `pos` lands in is a
/// member of the visible cell set. Skinned meshes cull by
/// [`mesh_model_visible`] instead. The locator lookup and the membership
/// decision are split so the decision is unit-testable without constructing a
/// full `LevelWorld`.
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
        let collapsed = Mat4::from_scale_rotation_translation(Vec3::ZERO, Quat::IDENTITY, origin);
        assert!(
            mesh_cull_point(&collapsed).abs_diff_eq(origin + Vec3::Y * MESH_CULL_LIFT, 1e-6),
            "a zero-scale model, with no up axis, lifts along world up",
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
