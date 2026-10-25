// Portal traversal tests: chain walk, clipping and narrowing.

use super::clip::*;
use super::*;
use crate::visibility::{FrustumPlane, extract_frustum_planes, is_aabb_outside_frustum};
use glam::Mat4;
use postretro_level_loader::{CellData, LevelWorld, PortalData};

fn extract_test_frustum(view_proj: Mat4) -> Frustum {
    use glam::Vec4;

    let row = |n: usize| -> Vec4 {
        Vec4::new(
            view_proj.col(0)[n],
            view_proj.col(1)[n],
            view_proj.col(2)[n],
            view_proj.col(3)[n],
        )
    };

    let r0 = row(0);
    let r1 = row(1);
    let r2 = row(2);
    let r3 = row(3);

    let raw_planes = [
        r3 + r0, // Left
        r3 - r0, // Right
        r3 + r1, // Bottom
        r3 - r1, // Top
        r3 + r2, // Near
        r3 - r2, // Far
    ];

    let mut planes = Vec::with_capacity(6);

    for raw in &raw_planes {
        let normal = Vec3::new(raw.x, raw.y, raw.z);
        let length = normal.length();
        if length > 0.0 {
            let inv_len = 1.0 / length;
            planes.push(FrustumPlane {
                normal: normal * inv_len,
                dist: raw.w * inv_len,
            });
        } else {
            planes.push(FrustumPlane {
                normal: Vec3::ZERO,
                dist: 0.0,
            });
        }
    }

    Frustum { planes }
}

fn make_camera_frustum(position: Vec3, look_dir: Vec3) -> Frustum {
    let target = position + look_dir;
    let view = glam::camera::rh::view::look_at_mat4(position, target, Vec3::Y);
    let aspect = 16.0 / 9.0;
    let hfov = 100.0_f32.to_radians();
    let vfov = 2.0 * ((hfov / 2.0).tan() / aspect).atan();
    let proj = glam::camera::rh::proj::directx::perspective(vfov, aspect, 0.1, 4096.0);
    extract_test_frustum(proj * view)
}

fn test_cell(bounds_min: Vec3, bounds_max: Vec3, is_solid: bool) -> CellData {
    CellData {
        bounds_min,
        bounds_max,
        face_start: 0,
        face_count: 0,
        portal_ref_start: 0,
        portal_ref_count: 0,
        is_solid,
        is_exterior: false,
        is_drawable: false,
    }
}

fn set_portal_refs(cells: &mut [CellData], refs_by_cell: &[&[u32]]) -> Vec<u32> {
    let mut refs = Vec::new();
    for (cell, cell_refs) in cells.iter_mut().zip(refs_by_cell.iter()) {
        if cell_refs.is_empty() {
            cell.portal_ref_start = 0;
            cell.portal_ref_count = 0;
        } else {
            cell.portal_ref_start = refs.len() as u32;
            cell.portal_ref_count = cell_refs.len() as u32;
            refs.extend_from_slice(cell_refs);
        }
    }
    refs
}

fn test_world(
    mut cells: Vec<CellData>,
    refs_by_cell: &[&[u32]],
    portals: Vec<PortalData>,
) -> LevelWorld {
    let cell_portal_refs = set_portal_refs(&mut cells, refs_by_cell);
    LevelWorld::new_visibility_only(
        cells,
        cell_portal_refs,
        postretro_level_loader::CellLocatorChild::Cell(0),
        vec![],
        portals,
        true,
    )
    .expect("valid portal visibility test world")
}

fn empty_test_world() -> LevelWorld {
    LevelWorld::new_visibility_only(
        Vec::new(),
        Vec::new(),
        postretro_level_loader::CellLocatorChild::Cell(0),
        vec![],
        Vec::new(),
        false,
    )
    .expect("valid empty visibility-only test world")
}

/// Build a three-cell chain: A (cell 0) -- portal 0 -- B (cell 1) -- portal 1 -- C (cell 2)
/// arranged along the X axis.
fn three_cell_chain() -> LevelWorld {
    let portal_0 = PortalData {
        polygon: vec![
            Vec3::new(32.0, 0.0, 0.0),
            Vec3::new(32.0, 64.0, 0.0),
            Vec3::new(32.0, 64.0, 64.0),
            Vec3::new(32.0, 0.0, 64.0),
        ],
        front_cell: 0,
        back_cell: 1,
    };
    let portal_1 = PortalData {
        polygon: vec![
            Vec3::new(64.0, 0.0, 0.0),
            Vec3::new(64.0, 64.0, 0.0),
            Vec3::new(64.0, 64.0, 64.0),
            Vec3::new(64.0, 0.0, 64.0),
        ],
        front_cell: 1,
        back_cell: 2,
    };

    test_world(
        vec![
            test_cell(Vec3::new(0.0, 0.0, 0.0), Vec3::new(32.0, 64.0, 64.0), false),
            test_cell(
                Vec3::new(32.0, 0.0, 0.0),
                Vec3::new(64.0, 64.0, 64.0),
                false,
            ),
            test_cell(
                Vec3::new(64.0, 0.0, 0.0),
                Vec3::new(96.0, 64.0, 64.0),
                false,
            ),
        ],
        &[&[0], &[0, 1], &[1]],
        vec![portal_0, portal_1],
    )
}

#[test]
fn portal_traverse_camera_cell_always_visible() {
    let world = three_cell_chain();
    // Camera in cell 0, looking away from all portals.
    let frustum = make_camera_frustum(Vec3::new(16.0, 32.0, 32.0), Vec3::NEG_X);
    let visible = portal_traverse(Vec3::new(16.0, 32.0, 32.0), 0, &frustum, &world, false);
    assert!(visible[0], "camera cell should always be visible");
}

#[test]
fn portal_traverse_straight_corridor_sees_all_three() {
    let world = three_cell_chain();
    // Camera in cell 0, looking through portals toward +X.
    let camera_pos = Vec3::new(16.0, 32.0, 32.0);
    let frustum = make_camera_frustum(camera_pos, Vec3::X);
    let visible = portal_traverse(camera_pos, 0, &frustum, &world, false);
    assert!(visible[0], "camera cell A should be visible");
    assert!(visible[1], "cell B should be visible through portal 0");
    assert!(visible[2], "cell C should be visible through portals 0+1");
}

#[test]
fn blocked_portal_rejects_in_both_directions_and_reports_trace_stat() {
    let world = three_cell_chain();
    let camera_from_a = Vec3::new(16.0, 32.0, 32.0);
    let frustum_from_a = make_camera_frustum(camera_from_a, Vec3::X);
    let (from_a, trace) = portal_traverse_inner(
        camera_from_a,
        0,
        &frustum_from_a,
        &world,
        &[true],
        true,
        MAX_PORTAL_WALK_STEPS,
        TimingGate::OFF,
    );

    assert!(from_a.visible[0], "camera cell stays visible");
    assert!(!from_a.visible[1], "blocked portal must reject A→B");
    assert_eq!(from_a.stats.rejected_blocked_portal, 1);
    let trace = trace.expect("captured portal walk must include a trace");
    assert!(
        trace.contains("blocked"),
        "blocked reject missing from trace: {trace}"
    );
    assert!(
        trace.contains("blocked=1"),
        "blocked reject missing from trace summary: {trace}"
    );

    let camera_from_c = Vec3::new(80.0, 32.0, 32.0);
    let frustum_from_c = make_camera_frustum(camera_from_c, Vec3::NEG_X);
    let from_c = portal_traverse_detailed(
        camera_from_c,
        2,
        &frustum_from_c,
        &world,
        &[false, true],
        false,
    );

    assert!(from_c.visible[2], "camera cell stays visible");
    assert!(!from_c.visible[1], "blocked portal must reject C→B");
    assert_eq!(from_c.stats.rejected_blocked_portal, 1);
}

#[test]
fn portal_traverse_reports_step_limit_before_unbounded_walk() {
    let world = three_cell_chain();
    let camera_pos = Vec3::new(16.0, 32.0, 32.0);
    let frustum = make_camera_frustum(camera_pos, Vec3::X);

    let result = portal_traverse_with_step_limit(
        camera_pos,
        0,
        &frustum,
        &world,
        &[],
        false,
        0,
        TimingGate::OFF,
    );

    assert!(
        result.stats.step_limit_hit,
        "zero step budget should trip the traversal fuse"
    );
    assert_eq!(
        result.stats.considered, 0,
        "no portal should be considered after the budget trips"
    );
    assert!(result.visible[0], "camera cell remains visible");
    assert!(
        !result.visible[1],
        "walk stops before entering neighbor cells"
    );
}

#[test]
fn portal_traverse_looking_away_hides_distant_leaves() {
    let world = three_cell_chain();
    // Camera in cell 0, looking away from the portals (toward -X).
    let camera_pos = Vec3::new(16.0, 32.0, 32.0);
    let frustum = make_camera_frustum(camera_pos, Vec3::NEG_X);
    let visible = portal_traverse(camera_pos, 0, &frustum, &world, false);
    assert!(visible[0], "camera cell should be visible");
    // Portals are at X=32 and X=64, camera looks toward -X, so they're behind.
    assert!(
        !visible[1],
        "cell B should not be visible when looking away"
    );
    assert!(
        !visible[2],
        "cell C should not be visible when looking away"
    );
}

#[test]
fn portal_traverse_skips_solid_neighbors() {
    let mut world = three_cell_chain();
    world.cells[1].is_solid = true;

    let camera_pos = Vec3::new(16.0, 32.0, 32.0);
    let frustum = make_camera_frustum(camera_pos, Vec3::X);
    let visible = portal_traverse(camera_pos, 0, &frustum, &world, false);
    assert!(visible[0], "camera cell should be visible");
    assert!(!visible[1], "solid cell should not be visible");
    // Cell 2 is behind solid cell 1, so it can't be reached.
    assert!(!visible[2], "cell behind solid should not be visible");
}

#[test]
fn portal_traverse_empty_world() {
    let world = empty_test_world();

    let frustum = make_camera_frustum(Vec3::ZERO, Vec3::NEG_Z);
    let visible = portal_traverse(Vec3::ZERO, 0, &frustum, &world, false);
    assert!(visible.is_empty());
}

#[test]
fn portal_traverse_l_shaped_corridor_hides_c() {
    // L-shaped corridor: A -- portal 0 (at X=32 in YZ plane) -- B -- portal 1 (at Z=64 in XY plane) -- C
    // Camera in A looking along +X sees B through portal 0,
    // but portal 1 is perpendicular (in the Z direction), so C is not visible
    // through the narrow frustum left after passing through portal 0.
    let portal_0 = PortalData {
        polygon: vec![
            Vec3::new(32.0, 0.0, 0.0),
            Vec3::new(32.0, 64.0, 0.0),
            Vec3::new(32.0, 64.0, 64.0),
            Vec3::new(32.0, 0.0, 64.0),
        ],
        front_cell: 0,
        back_cell: 1,
    };
    // Portal 1 is on the Z=64 plane — perpendicular to the camera's line of sight.
    // Positioned far to the +Z side of the corridor.
    let portal_1 = PortalData {
        polygon: vec![
            Vec3::new(32.0, 0.0, 200.0),
            Vec3::new(64.0, 0.0, 200.0),
            Vec3::new(64.0, 64.0, 200.0),
            Vec3::new(32.0, 64.0, 200.0),
        ],
        front_cell: 1,
        back_cell: 2,
    };

    let world = test_world(
        vec![
            test_cell(Vec3::new(0.0, 0.0, 0.0), Vec3::new(32.0, 64.0, 64.0), false),
            test_cell(
                Vec3::new(32.0, 0.0, 0.0),
                Vec3::new(64.0, 64.0, 200.0),
                false,
            ),
            test_cell(
                Vec3::new(32.0, 0.0, 200.0),
                Vec3::new(64.0, 64.0, 264.0),
                false,
            ),
        ],
        &[&[0], &[0, 1], &[1]],
        vec![portal_0, portal_1],
    );

    // Camera in cell A, looking straight along +X toward portal 0.
    let camera_pos = Vec3::new(16.0, 32.0, 32.0);
    let frustum = make_camera_frustum(camera_pos, Vec3::X);
    let visible = portal_traverse(camera_pos, 0, &frustum, &world, false);
    assert!(visible[0], "camera cell A should be visible");
    assert!(visible[1], "cell B should be visible through portal 0");
    assert!(
        !visible[2],
        "cell C should not be visible — portal 1 is around the corner at Z=200"
    );
}

#[test]
fn narrow_frustum_produces_tighter_frustum() {
    // Camera at origin looking along +X.
    let camera_pos = Vec3::ZERO;
    let frustum = make_camera_frustum(camera_pos, Vec3::X);

    // A small portal at X=10 centered at Y=5,Z=5, 2x2 units.
    let portal = vec![
        Vec3::new(10.0, 4.0, 4.0),
        Vec3::new(10.0, 6.0, 4.0),
        Vec3::new(10.0, 6.0, 6.0),
        Vec3::new(10.0, 4.0, 6.0),
    ];

    let narrowed = narrow_frustum(camera_pos, &portal, &frustum);
    assert!(
        narrowed.is_some(),
        "narrow_frustum should succeed for a visible portal"
    );

    let narrowed = narrowed.unwrap();

    // The narrowed frustum should be tighter: a point far from the portal
    // line of sight should be outside the narrowed frustum but might be
    // inside the original.
    let far_off_point_mins = Vec3::new(20.0, 50.0, 50.0);
    let far_off_point_maxs = Vec3::new(21.0, 51.0, 51.0);

    let narrowed_rejects =
        is_aabb_outside_frustum(far_off_point_mins, far_off_point_maxs, &narrowed);
    assert!(
        narrowed_rejects,
        "narrowed frustum should reject a point far off the portal's line of sight"
    );
}

#[test]
fn narrow_frustum_rejects_degenerate_portal() {
    let camera_pos = Vec3::ZERO;
    let frustum = make_camera_frustum(camera_pos, Vec3::X);

    // Degenerate: less than 3 vertices.
    assert!(narrow_frustum(camera_pos, &[Vec3::X, Vec3::Y], &frustum).is_none());
    assert!(narrow_frustum(camera_pos, &[], &frustum).is_none());
}

/// Regression gate for the `test-2.prl` S-maze missing-panels bug.
///
/// The old two-state Sutherland-Hodgman clipper produced clipped
/// polygons whose first two vertices were near-duplicates whenever a
/// polygon vertex lay within `CLIP_EPSILON` of a clip plane: for a
/// quad `[A, B, C, D]` with `A` inside-by-epsilon and `D` outside, the
/// clipper emitted `intersect(D,A), A, B, C, intersect(C,D)` — and
/// `intersect(D,A)` sat within epsilon of `A` itself. The first two
/// output vertices were then effectively coincident, and
/// `narrow_frustum`'s leading-triple cross-product normal collapsed
/// below its `1e-12` early-out, silently dropping the portal as
/// `rej A->B v=5/4 narrow`.
///
/// In the broken `test-2.prl` trace, this was the failure along chain
/// `41 → 43 → 38 → 37 → 31 → 30`: `rej 43->38 v=5/4 narrow` broke the
/// only chain that reached cell 30, the cell holding the missing wall
/// and ceiling panels.
///
/// The fix is the three-state `FRONT`/`BACK`/`ON` classifier from
/// Doom 3's `idWinding::Split` (same lineage as Quake's 1999
/// `qbsp/winding.c` and ericw-tools' `polylib::winding_base_t::clip`):
/// `ON` vertices are emitted verbatim, and the "skip split point if
/// next is ON or same-side" predicate prevents emitting a lerped
/// intersection adjacent to an already-emitted on-plane vertex.
///
/// This probe runs the production clipper on exactly the quad shape
/// that used to trigger the bug (one vertex 5e-8 units inside a clip
/// plane — cumulative Sutherland-Hodgman imprecision after a handful
/// of portal-chain hops routinely lands vertices this close to clip
/// boundaries). The clipped polygon now has no degenerate leading
/// vertices and `narrow_frustum` accepts it.
#[test]
fn narrow_frustum_accepts_sutherland_hodgman_near_duplicate_leading_vertices() {
    // Quad that yields the pathological clip: A inside-by-epsilon, D
    // outside. Epsilon of 5e-8 puts cross^2 firmly below the 1e-12
    // narrow-rejection threshold after clipping (verified by sweep).
    let polygon = vec![
        Vec3::new(10.0, 5e-8, 0.0),
        Vec3::new(10.0, 5.0, 0.0),
        Vec3::new(10.0, 5.0, 5.0),
        Vec3::new(10.0, -5.0, 5.0),
    ];
    let clip_plane = FrustumPlane {
        normal: Vec3::new(0.0, 1.0, 0.0),
        dist: 0.0,
    };
    let front_frustum = Frustum {
        planes: vec![clip_plane],
    };

    let mut scratch_a: Vec<Vec3> = Vec::new();
    let mut scratch_b: Vec<Vec3> = Vec::new();
    let clipped =
        clip_polygon_to_frustum(&polygon, &front_frustum, &mut scratch_a, &mut scratch_b).to_vec();

    // Sanity: the clipper survived as a polygon the DFS will hand to
    // narrow_frustum (>= 3 verts, so `clipped_len < 3` doesn't early-
    // reject the portal). Localizes the failure site if the clipper
    // itself has regressed (as opposed to narrow_frustum).
    assert!(
        clipped.len() >= 3,
        "Sutherland-Hodgman must emit >= 3 vertices for this input \
         so the DFS hands the polygon to narrow_frustum; got {}",
        clipped.len()
    );

    let camera_pos = Vec3::ZERO;
    let camera_frustum = make_camera_frustum(camera_pos, Vec3::X);
    let narrowed = narrow_frustum(camera_pos, &clipped, &camera_frustum);

    assert!(
        narrowed.is_some(),
        "narrow_frustum rejected a geometrically valid portal polygon — \
         the three-state clipper should have prevented near-duplicate \
         leading vertices from reaching narrow_frustum in the first \
         place. If this fails, the SIDE_ON dedupe predicate in \
         clip_polygon_to_plane has regressed."
    );
}

/// Defense-in-depth gate for the `narrow_frustum` leading-triple
/// fragility flagged by edge-case review on commit 1535c92.
///
/// The S-maze fix hardened the clipper so it no longer emits the
/// specific "near-duplicate leading pair from inside-by-epsilon
/// vertex" shape, but the old `narrow_frustum` still trusted
/// `(v1-v0) × (v2-v0)` and would collapse on any polygon whose
/// first three vertices happened to be colinear — a shape that can
/// still arise from BSP fragmentation or from polygons that were
/// colinear at the source before any clipping occurred.
///
/// Newell's method (Graphics Gems III, 1992) sums edge cross
/// products across the entire polygon, so a single degenerate
/// triple contributes zero while the rest of the polygon still
/// supplies a correct normal. This probe feeds a pentagon whose
/// leading three vertices are exactly colinear and asserts
/// `narrow_frustum` still returns a valid frustum.
#[test]
fn narrow_frustum_accepts_colinear_leading_triple() {
    let camera_pos = Vec3::ZERO;
    let frustum = make_camera_frustum(camera_pos, Vec3::X);

    // Pentagon on the plane X=10: the 2×2 quad used in
    // `narrow_frustum_produces_tighter_frustum`, plus a midpoint
    // inserted on the bottom edge so v0, v1, v2 are colinear along
    // Z=4. (v1-v0)×(v2-v0) is exactly zero; Newell's sum of all
    // five edges still resolves to (±8, 0, 0).
    let portal = vec![
        Vec3::new(10.0, 4.0, 4.0),
        Vec3::new(10.0, 5.0, 4.0),
        Vec3::new(10.0, 6.0, 4.0),
        Vec3::new(10.0, 6.0, 6.0),
        Vec3::new(10.0, 4.0, 6.0),
    ];

    let narrowed = narrow_frustum(camera_pos, &portal, &frustum);
    assert!(
        narrowed.is_some(),
        "narrow_frustum rejected a pentagon with colinear leading \
         vertices — Newell's method should have derived a valid \
         normal from the remaining non-degenerate edges. If this \
         fails, the leading-triple cross product has regressed \
         into the polygon-normal computation."
    );
}

#[test]
fn portal_traversal_sees_room_through_both_sides_of_pillar() {
    // Room layout with NARROW portals (2 units wide) matching the pillar
    // gap dimensions that cause issues in portal generation:
    //
    // Cell A (camera room, X=0..120) --[portal 0 at X=120, Z=62..64]--> Cell B (left gap)
    //                                --[portal 1 at X=120, Z=66..68]--> Cell C (right gap)
    // Cell B --[portal 2 at X=136, Z=62..64]--> Cell D (far room, X=136..256)
    // Cell C --[portal 3 at X=136, Z=66..68]--> Cell D
    //
    // The portals are only 2 units wide (matching a narrow doorway gap).
    let portal_a_b = PortalData {
        polygon: vec![
            Vec3::new(120.0, 16.0, 62.0),
            Vec3::new(120.0, 112.0, 62.0),
            Vec3::new(120.0, 112.0, 64.0),
            Vec3::new(120.0, 16.0, 64.0),
        ],
        front_cell: 0,
        back_cell: 1,
    };
    let portal_a_c = PortalData {
        polygon: vec![
            Vec3::new(120.0, 16.0, 66.0),
            Vec3::new(120.0, 112.0, 66.0),
            Vec3::new(120.0, 112.0, 68.0),
            Vec3::new(120.0, 16.0, 68.0),
        ],
        front_cell: 0,
        back_cell: 2,
    };
    let portal_b_d = PortalData {
        polygon: vec![
            Vec3::new(136.0, 16.0, 62.0),
            Vec3::new(136.0, 112.0, 62.0),
            Vec3::new(136.0, 112.0, 64.0),
            Vec3::new(136.0, 16.0, 64.0),
        ],
        front_cell: 1,
        back_cell: 3,
    };
    let portal_c_d = PortalData {
        polygon: vec![
            Vec3::new(136.0, 16.0, 66.0),
            Vec3::new(136.0, 112.0, 66.0),
            Vec3::new(136.0, 112.0, 68.0),
            Vec3::new(136.0, 16.0, 68.0),
        ],
        front_cell: 2,
        back_cell: 3,
    };

    let world = test_world(
        vec![
            test_cell(
                Vec3::new(0.0, 0.0, 0.0),
                Vec3::new(120.0, 128.0, 128.0),
                false,
            ),
            test_cell(
                Vec3::new(120.0, 16.0, 62.0),
                Vec3::new(136.0, 112.0, 64.0),
                false,
            ),
            test_cell(
                Vec3::new(120.0, 16.0, 66.0),
                Vec3::new(136.0, 112.0, 68.0),
                false,
            ),
            test_cell(
                Vec3::new(136.0, 0.0, 0.0),
                Vec3::new(256.0, 128.0, 128.0),
                false,
            ),
        ],
        &[&[0, 1], &[0, 2], &[1, 3], &[2, 3]],
        vec![portal_a_b, portal_a_c, portal_b_d, portal_c_d],
    );

    // Camera looking through the LEFT passage (Z=63, center of Z=62..64 gap).
    // Camera is in cell A, looking toward +X.
    {
        let camera_pos = Vec3::new(16.0, 64.0, 63.0);
        let frustum = make_camera_frustum(camera_pos, Vec3::X);
        let visible = portal_traverse(camera_pos, 0, &frustum, &world, false);
        assert!(visible[0], "camera cell A should be visible");
        assert!(
            visible[1],
            "cell B (left gap) should be visible when looking through left doorway"
        );
        assert!(
            visible[3],
            "cell D (far room) should be visible through left passage (A->B->D). \
             If not, the narrow frustum through the 2-unit-wide portal A-B may be \
             rejecting the 2-unit-wide portal B-D."
        );
    }

    // Camera looking through the RIGHT passage (Z=67, center of Z=66..68 gap).
    {
        let camera_pos = Vec3::new(16.0, 64.0, 67.0);
        let frustum = make_camera_frustum(camera_pos, Vec3::X);
        let visible = portal_traverse(camera_pos, 0, &frustum, &world, false);
        assert!(visible[0], "camera cell A should be visible");
        assert!(
            visible[2],
            "cell C (right gap) should be visible when looking through right doorway"
        );
        assert!(
            visible[3],
            "cell D (far room) should be visible through right passage (A->C->D). \
             If not, the narrow frustum through the 2-unit-wide portal A-C may be \
             rejecting the 2-unit-wide portal C-D."
        );
    }
}

// --- Polygon-vs-frustum clipping tests ---

fn point_inside_frustum(point: Vec3, frustum: &Frustum) -> bool {
    frustum
        .planes
        .iter()
        .all(|p| p.normal.dot(point) + p.dist >= -CLIP_EPSILON)
}

#[test]
fn clip_polygon_fully_inside_is_unchanged() {
    let camera_pos = Vec3::ZERO;
    let frustum = make_camera_frustum(camera_pos, Vec3::X);

    // Small polygon centered on the line of sight, well inside the cone.
    let polygon = vec![
        Vec3::new(10.0, -0.5, -0.5),
        Vec3::new(10.0, 0.5, -0.5),
        Vec3::new(10.0, 0.5, 0.5),
        Vec3::new(10.0, -0.5, 0.5),
    ];

    let mut scratch_a: Vec<Vec3> = Vec::new();
    let mut scratch_b: Vec<Vec3> = Vec::new();
    let clipped = clip_polygon_to_frustum(&polygon, &frustum, &mut scratch_a, &mut scratch_b);
    assert_eq!(
        clipped.len(),
        4,
        "polygon fully inside frustum should retain all 4 vertices"
    );
    for (i, v) in clipped.iter().enumerate() {
        assert!(
            point_inside_frustum(*v, &frustum),
            "clipped vertex {i} should be inside the frustum"
        );
    }
}

#[test]
fn clip_polygon_fully_outside_yields_empty() {
    let camera_pos = Vec3::ZERO;
    let frustum = make_camera_frustum(camera_pos, Vec3::X);

    // Polygon entirely behind the camera (on -X side, past the near plane).
    let polygon = vec![
        Vec3::new(-10.0, -1.0, -1.0),
        Vec3::new(-10.0, 1.0, -1.0),
        Vec3::new(-10.0, 1.0, 1.0),
        Vec3::new(-10.0, -1.0, 1.0),
    ];

    let mut scratch_a: Vec<Vec3> = Vec::new();
    let mut scratch_b: Vec<Vec3> = Vec::new();
    let clipped = clip_polygon_to_frustum(&polygon, &frustum, &mut scratch_a, &mut scratch_b);
    assert!(
        clipped.len() < 3,
        "polygon fully outside frustum should clip to empty (got {} verts)",
        clipped.len()
    );
}

#[test]
fn clip_polygon_partial_stays_inside_frustum() {
    let camera_pos = Vec3::ZERO;
    let frustum = make_camera_frustum(camera_pos, Vec3::X);

    // Large polygon straddling the camera cone — extends from deep inside
    // the cone well past the left/right frustum planes.
    let polygon = vec![
        Vec3::new(10.0, -500.0, -1.0),
        Vec3::new(10.0, 500.0, -1.0),
        Vec3::new(10.0, 500.0, 1.0),
        Vec3::new(10.0, -500.0, 1.0),
    ];

    let mut scratch_a: Vec<Vec3> = Vec::new();
    let mut scratch_b: Vec<Vec3> = Vec::new();
    let clipped = clip_polygon_to_frustum(&polygon, &frustum, &mut scratch_a, &mut scratch_b);
    assert!(
        clipped.len() >= 3,
        "a polygon that straddles the frustum should clip to a non-empty polygon"
    );
    for (i, v) in clipped.iter().enumerate() {
        assert!(
            point_inside_frustum(*v, &frustum),
            "clipped vertex {i} at {v:?} should be inside the frustum"
        );
    }
}

#[test]
fn clip_polygon_degenerate_input_yields_empty() {
    let frustum = make_camera_frustum(Vec3::ZERO, Vec3::X);
    let mut scratch_a: Vec<Vec3> = Vec::new();
    let mut scratch_b: Vec<Vec3> = Vec::new();
    assert!(clip_polygon_to_frustum(&[], &frustum, &mut scratch_a, &mut scratch_b).is_empty());
    assert!(
        clip_polygon_to_frustum(
            &[Vec3::X, Vec3::Y],
            &frustum,
            &mut scratch_a,
            &mut scratch_b
        )
        .is_empty()
    );
}

/// Test that a clipped polygon feeds a narrowed frustum whose vertices all
/// lie inside the parent frustum. This is the strict-subset invariant at
/// one hop.
#[test]
fn narrowed_frustum_from_clipped_polygon_is_subset_of_parent() {
    let camera_pos = Vec3::ZERO;
    let parent = make_camera_frustum(camera_pos, Vec3::X);

    // Portal that straddles the frustum boundary (large in Y).
    let portal = vec![
        Vec3::new(10.0, -500.0, -1.0),
        Vec3::new(10.0, 500.0, -1.0),
        Vec3::new(10.0, 500.0, 1.0),
        Vec3::new(10.0, -500.0, 1.0),
    ];

    let mut scratch_a: Vec<Vec3> = Vec::new();
    let mut scratch_b: Vec<Vec3> = Vec::new();
    let clipped: Vec<Vec3> =
        clip_polygon_to_frustum(&portal, &parent, &mut scratch_a, &mut scratch_b).to_vec();
    assert!(clipped.len() >= 3, "clipped polygon should be non-empty");

    // All clipped vertices lie inside the parent frustum by construction.
    for v in &clipped {
        assert!(
            point_inside_frustum(*v, &parent),
            "clipped polygon vertex {v:?} must lie inside parent frustum"
        );
    }

    // The narrowed frustum produced from the clipped polygon should accept
    // points that are clearly inside the narrowed cone and also inside the
    // parent — and should not accept points outside the parent frustum.
    let narrowed = narrow_frustum(camera_pos, &clipped, &parent)
        .expect("narrow_frustum should succeed for a clipped, visible polygon");

    // A sample point far outside the parent's side plane must also be
    // rejected by the narrowed frustum (strict subset means: outside
    // parent implies outside narrowed).
    let outside_parent = Vec3::new(20.0, 500.0, 0.0);
    assert!(
        !point_inside_frustum(outside_parent, &parent),
        "sanity: test point should be outside parent"
    );
    assert!(
        !point_inside_frustum(outside_parent, &narrowed),
        "point outside parent must be outside the narrowed (subset) frustum"
    );
}

#[test]
fn multi_hop_narrowed_frustums_preserve_strict_subset_invariant() {
    // Three collinear portals along +X. After clipping+narrowing at each
    // hop, every cell visible in the narrowed frustum must also be inside
    // the original camera frustum.
    let camera_pos = Vec3::new(0.0, 0.0, 0.0);
    let parent = make_camera_frustum(camera_pos, Vec3::X);

    let portal_a = vec![
        Vec3::new(10.0, -2.0, -2.0),
        Vec3::new(10.0, 2.0, -2.0),
        Vec3::new(10.0, 2.0, 2.0),
        Vec3::new(10.0, -2.0, 2.0),
    ];
    let portal_b = vec![
        Vec3::new(20.0, -2.0, -2.0),
        Vec3::new(20.0, 2.0, -2.0),
        Vec3::new(20.0, 2.0, 2.0),
        Vec3::new(20.0, -2.0, 2.0),
    ];
    let portal_c = vec![
        Vec3::new(30.0, -2.0, -2.0),
        Vec3::new(30.0, 2.0, -2.0),
        Vec3::new(30.0, 2.0, 2.0),
        Vec3::new(30.0, -2.0, 2.0),
    ];

    let mut scratch_a: Vec<Vec3> = Vec::new();
    let mut scratch_b: Vec<Vec3> = Vec::new();

    let clipped_a: Vec<Vec3> =
        clip_polygon_to_frustum(&portal_a, &parent, &mut scratch_a, &mut scratch_b).to_vec();
    assert!(clipped_a.len() >= 3);
    let narrowed_1 = narrow_frustum(camera_pos, &clipped_a, &parent).expect("hop 1");

    // Hop 2: clip next portal against hop-1 frustum.
    let clipped_b: Vec<Vec3> =
        clip_polygon_to_frustum(&portal_b, &narrowed_1, &mut scratch_a, &mut scratch_b).to_vec();
    assert!(clipped_b.len() >= 3);
    let narrowed_2 = narrow_frustum(camera_pos, &clipped_b, &narrowed_1).expect("hop 2");

    // Hop 3.
    let clipped_c: Vec<Vec3> =
        clip_polygon_to_frustum(&portal_c, &narrowed_2, &mut scratch_a, &mut scratch_b).to_vec();
    assert!(clipped_c.len() >= 3);
    let narrowed_3 = narrow_frustum(camera_pos, &clipped_c, &narrowed_2).expect("hop 3");

    // Hop-wise strict-subset check: each clipped polygon must lie fully
    // inside the frustum it was clipped against. This is the inductive
    // step that guarantees every narrowed frustum is a subset of its
    // immediate predecessor — from which "subset of the original camera
    // frustum" follows by induction.
    for v in &clipped_a {
        assert!(
            point_inside_frustum(*v, &parent),
            "hop 1 clipped vertex {v:?} must lie inside the parent frustum"
        );
    }
    for v in &clipped_b {
        assert!(
            point_inside_frustum(*v, &narrowed_1),
            "hop 2 clipped vertex {v:?} must lie inside the hop-1 narrowed frustum"
        );
    }
    for v in &clipped_c {
        assert!(
            point_inside_frustum(*v, &narrowed_2),
            "hop 3 clipped vertex {v:?} must lie inside the hop-2 narrowed frustum"
        );
    }

    // Transitively, every clipped vertex at any hop lies inside the
    // original parent frustum — the induction target.
    for v in clipped_a
        .iter()
        .chain(clipped_b.iter())
        .chain(clipped_c.iter())
    {
        assert!(
            point_inside_frustum(*v, &parent),
            "clipped vertex {v:?} must lie inside the original camera frustum"
        );
    }

    // Subset-at-each-hop sampled check: a sample of points that lie inside
    // the narrowed frustum at hop N must also lie inside the frustum at
    // hop N-1. Sample the clipped polygon vertices themselves plus the
    // polygon centroid at each hop — both are guaranteed inside the
    // narrowed frustum (they lie on the near plane and are bounded by the
    // edge planes).
    let centroid = |poly: &[Vec3]| poly.iter().copied().sum::<Vec3>() / poly.len() as f32;

    // Hop 1 → parent.
    for v in &clipped_a {
        assert!(point_inside_frustum(*v, &parent));
    }
    assert!(point_inside_frustum(centroid(&clipped_a), &parent));
    // Hop 2 → hop 1.
    for v in &clipped_b {
        assert!(point_inside_frustum(*v, &narrowed_1));
    }
    assert!(point_inside_frustum(centroid(&clipped_b), &narrowed_1));
    // Hop 3 → hop 2.
    for v in &clipped_c {
        assert!(point_inside_frustum(*v, &narrowed_2));
    }
    assert!(point_inside_frustum(centroid(&clipped_c), &narrowed_2));

    // And points clearly outside the parent must be outside every
    // narrowed frustum, at every hop.
    let way_off = Vec3::new(15.0, 500.0, 0.0);
    assert!(!point_inside_frustum(way_off, &parent));
    assert!(
        !point_inside_frustum(way_off, &narrowed_1),
        "hop 1 must reject points outside the parent"
    );
    assert!(
        !point_inside_frustum(way_off, &narrowed_2),
        "hop 2 must reject points outside the parent"
    );
    assert!(
        !point_inside_frustum(way_off, &narrowed_3),
        "hop 3 must reject points outside the parent"
    );
}

#[test]
fn portal_traverse_straddling_portal_hides_unreachable_side_branch() {
    // Straight-through layout: camera in cell 0 looking +X.
    // Portal 0 (A -> B) straddles the camera's side plane — it extends
    // far beyond the frustum to the +Y direction. Without polygon
    // clipping, frustum narrowing through the un-clipped portal could
    // produce a cone that extends into -Y regions the camera cannot see
    // and incorrectly admit off-axis neighbors.
    //
    // This test asserts that with clipping in place, cell B is still
    // visible (the portal is in view) and a far off-axis cell C reached
    // through an orthogonal portal at cell B is correctly hidden.
    let portal_a_b = PortalData {
        polygon: vec![
            // 1000-unit-tall portal at X=10, centered on Z=0.
            Vec3::new(10.0, -500.0, -1.0),
            Vec3::new(10.0, 500.0, -1.0),
            Vec3::new(10.0, 500.0, 1.0),
            Vec3::new(10.0, -500.0, 1.0),
        ],
        front_cell: 0,
        back_cell: 1,
    };
    // Portal 1 (B -> C) is far out in +Y, well outside the camera's
    // actual view cone even though cell B is reachable.
    let portal_b_c = PortalData {
        polygon: vec![
            Vec3::new(15.0, 400.0, -1.0),
            Vec3::new(20.0, 400.0, -1.0),
            Vec3::new(20.0, 400.0, 1.0),
            Vec3::new(15.0, 400.0, 1.0),
        ],
        front_cell: 1,
        back_cell: 2,
    };

    let world = test_world(
        vec![
            test_cell(
                Vec3::new(0.0, -500.0, -500.0),
                Vec3::new(10.0, 500.0, 500.0),
                false,
            ),
            test_cell(
                Vec3::new(10.0, -500.0, -500.0),
                Vec3::new(25.0, 500.0, 500.0),
                false,
            ),
            test_cell(
                Vec3::new(15.0, 400.0, -500.0),
                Vec3::new(25.0, 600.0, 500.0),
                false,
            ),
        ],
        &[&[0], &[0, 1], &[1]],
        vec![portal_a_b, portal_b_c],
    );

    let camera_pos = Vec3::new(1.0, 0.0, 0.0);
    let frustum = make_camera_frustum(camera_pos, Vec3::X);
    let visible = portal_traverse(camera_pos, 0, &frustum, &world, false);

    assert!(visible[0], "camera cell should always be visible");
    assert!(
        visible[1],
        "cell B should be visible through the straddling portal"
    );
    assert!(
        !visible[2],
        "cell C should be hidden: portal 1 is far off-axis and \
         unreachable through the clipped sight cone"
    );
}

/// Regression test for the "two paths to the same cell, narrower path
/// wins, downstream reach is lost" topology fixed by per-chain DFS.
///
/// Topology (abstract; bounding boxes are not used by portal_traverse):
///
///   A (camera) -- portal 0 (NARROW 0.1x0.1 at X=10) --> B
///   A          -- portal 1 (WIDE   4.0x4.0 at X=10) --> C
///   B          -- portal 2 (2.0x2.0 at X=20) --------> X
///   C          -- portal 3 (2.0x2.0 at X=20) --------> X
///   X          -- portal 4 (1.0x1.0 at X=30, offset  -> Y
///                           to Y=1..2, Z=-0.5..0.5)
///
/// All portals lie in YZ planes perpendicular to +X. Portal 0 and portal 1
/// share the same spatial position (both at X=10 centered on the origin)
/// — portal_traverse cares about topology and polygon shape, not physical
/// room layout, so this is a legal test fixture. Likewise portals 2 and 3
/// overlap at X=20.
///
/// The load-bearing geometry:
///
/// - Camera frustum (100° hfov) easily contains both A-outbound portals.
/// - The B-path's frustum narrows severely through portal 0's 0.1x0.1 slit.
///   By X=30 that cone has a radius of roughly 0.15 units from the X axis.
/// - The C-path's frustum narrows gently through portal 1's 4.0x4.0 aperture.
///   By X=30 that cone has a radius of several units.
/// - Portal 4 is offset to Y=1..2 so it lies **outside** the narrow B-path
///   cone at X=30 (clips to empty) and **inside** the wide C-path cone
///   (passes through cleanly).
///
/// Under BFS-keyed-on-cells (the former implementation):
///   A's outbound iteration order is [0, 1] → A→B runs first → X marked
///   visible with the narrow frustum planted by the B-path → A→C→X is
///   then rejected by the already-visited early-skip → X's outbound
///   portal 4 is evaluated against the narrow B-frustum → Y missed.
///
/// Under DFS-with-per-path-tracking (the current implementation):
///   Both A→B→X and A→C→X chains run independently. The C-path produces
///   a wide frustum at X that does not clip X→Y to empty → Y visible.
///
/// The test asserts visibility of all five cells. The BFS topology fails
/// on `visible[4]` (Y) and DFS passes.
#[test]
fn portal_traverse_two_paths_to_same_leaf_uses_widest_frustum() {
    // Portal 0: A→B, NARROW slit at X=10.
    let portal_a_b = PortalData {
        polygon: vec![
            Vec3::new(10.0, -0.05, -0.05),
            Vec3::new(10.0, 0.05, -0.05),
            Vec3::new(10.0, 0.05, 0.05),
            Vec3::new(10.0, -0.05, 0.05),
        ],
        front_cell: 0, // A
        back_cell: 1,  // B
    };

    // Portal 1: A→C, WIDE aperture at X=10 (same spatial position as
    // portal 0; they serve different topology roles in this abstract
    // fixture).
    let portal_a_c = PortalData {
        polygon: vec![
            Vec3::new(10.0, -2.0, -2.0),
            Vec3::new(10.0, 2.0, -2.0),
            Vec3::new(10.0, 2.0, 2.0),
            Vec3::new(10.0, -2.0, 2.0),
        ],
        front_cell: 0, // A
        back_cell: 2,  // C
    };

    // Portal 2: B→X at X=20.
    let portal_b_x = PortalData {
        polygon: vec![
            Vec3::new(20.0, -1.0, -1.0),
            Vec3::new(20.0, 1.0, -1.0),
            Vec3::new(20.0, 1.0, 1.0),
            Vec3::new(20.0, -1.0, 1.0),
        ],
        front_cell: 1, // B
        back_cell: 3,  // X
    };

    // Portal 3: C→X at X=20 (same spatial position as portal 2).
    let portal_c_x = PortalData {
        polygon: vec![
            Vec3::new(20.0, -1.0, -1.0),
            Vec3::new(20.0, 1.0, -1.0),
            Vec3::new(20.0, 1.0, 1.0),
            Vec3::new(20.0, -1.0, 1.0),
        ],
        front_cell: 2, // C
        back_cell: 3,  // X
    };

    // Portal 4: X→Y at X=30, offset to Y=1..2 so it sits outside the
    // narrow B-path cone and inside the wide C-path cone.
    let portal_x_y = PortalData {
        polygon: vec![
            Vec3::new(30.0, 1.0, -0.5),
            Vec3::new(30.0, 2.0, -0.5),
            Vec3::new(30.0, 2.0, 0.5),
            Vec3::new(30.0, 1.0, 0.5),
        ],
        front_cell: 3, // X
        back_cell: 4,  // Y
    };

    let cell_template = || {
        test_cell(
            Vec3::new(-1000.0, -1000.0, -1000.0),
            Vec3::new(1000.0, 1000.0, 1000.0),
            false,
        )
    };

    let world = test_world(
        vec![
            cell_template(), // 0: A
            cell_template(), // 1: B
            cell_template(), // 2: C
            cell_template(), // 3: X
            cell_template(), // 4: Y
        ],
        // Iteration order matters: A lists portal 0 (narrow) BEFORE
        // portal 1 (wide) so BFS would deterministically plant the
        // narrow frustum at X first.
        &[&[0, 1], &[0, 2], &[1, 3], &[2, 3, 4], &[4]],
        vec![portal_a_b, portal_a_c, portal_b_x, portal_c_x, portal_x_y],
    );

    // Camera at origin looking +X. The camera frustum is wide enough that
    // both A-outbound portals are accepted on the first hop.
    let camera_pos = Vec3::new(0.0, 0.0, 0.0);
    let frustum = make_camera_frustum(camera_pos, Vec3::X);
    let visible = portal_traverse(camera_pos, 0, &frustum, &world, false);

    assert!(visible[0], "cell A (camera) must be visible");
    assert!(visible[1], "cell B must be visible (A→B direct)");
    assert!(visible[2], "cell C must be visible (A→C direct)");
    assert!(
        visible[3],
        "cell X must be visible (reachable via either path)"
    );
    assert!(
        visible[4],
        "cell Y must be visible via the A→C→X→Y chain. Under the \
         previous BFS-keyed-on-cells implementation, the A→B→X chain \
         would plant a narrow frustum at X that clips X→Y to empty, \
         and the wider A→C→X chain would be dropped by the \
         visible[X] early-skip before it ever reached X→Y."
    );
}

/// Regression probe: camera sits 0.03 units from a vertical portal
/// wall, reproducing the blank-frame scenario captured from
/// `test-3.prl` at 2026-04-11T22:52:11Z. Camera at `(4.91, 0.92,
/// -14.67)` inside cell 99 whose -X wall is on `x = 4.88`.
///
/// **Root cause (confirmed by diagnostic trace on 2026-04-11):** the
/// render-pipeline near clip (`camera::NEAR = 0.1`) is baked into the
/// view-projection matrix used to build the visibility frustum. When
/// the camera sits within 0.1 units of a portal plane, the entire
/// portal polygon lies between the camera apex and the near plane, so
/// every vertex fails the near-plane test and Sutherland-Hodgman
/// clips the polygon to empty. Side-plane clipping is not the
/// culprit: pushing only the near plane up to the camera apex (near
/// ≈ 0) makes the portal reach its neighbor on this exact fixture.
///
/// The test geometry is copied from the live trace verbatim — same
/// camera position, same cell bounds, same 6.5×4.88 portal rectangle.
/// Cell A is the camera cell (+X side), cell B is the -X neighbor.
///
/// Routes through `visibility::extract_frustum_planes` directly
/// rather than the module-private `extract_test_frustum` copy to
/// exercise the real production path end-to-end.
#[test]
fn portal_traverse_reaches_neighbor_when_camera_is_close_to_portal_wall() {
    const HFOV: f32 = 100.0_f32.to_radians();
    const NEAR: f32 = 0.1;
    const FAR: f32 = 4096.0;

    let portal = PortalData {
        polygon: vec![
            Vec3::new(4.88, 0.00, -17.88),
            Vec3::new(4.88, 0.00, -11.38),
            Vec3::new(4.88, 4.88, -11.38),
            Vec3::new(4.88, 4.88, -17.88),
        ],
        front_cell: 0,
        back_cell: 1,
    };

    let world = test_world(
        vec![
            test_cell(
                Vec3::new(4.88, 0.00, -17.88),
                Vec3::new(11.38, 4.88, -11.38),
                false,
            ),
            test_cell(
                Vec3::new(-50.00, 0.00, -17.88),
                Vec3::new(4.88, 4.88, -11.38),
                false,
            ),
        ],
        &[&[0], &[0]],
        vec![portal],
    );

    // Camera pose from the captured blank-frame trace. The live trace
    // did not record view direction; -X stares straight at the portal
    // wall, which is the natural "camera pressed against a wall"
    // orientation and reproduces the same clip-to-empty symptom.
    let camera_pos = Vec3::new(4.91, 0.92, -14.67);
    let look_dir = Vec3::NEG_X;

    let aspect = 16.0 / 9.0;
    let vfov = 2.0 * ((HFOV / 2.0).tan() / aspect).atan();
    let view = glam::camera::rh::view::look_at_mat4(camera_pos, camera_pos + look_dir, Vec3::Y);
    let proj = glam::camera::rh::proj::directx::perspective(vfov, aspect, NEAR, FAR);
    let frustum = extract_frustum_planes(proj * view);

    // Precondition: at least one portal vertex must lie outside at
    // least one frustum plane, i.e. the clip step has real work to do.
    // A test that passes because every vertex is trivially inside
    // every plane proves nothing about the clip behaviour.
    let any_vertex_outside = world.portals[0].polygon.iter().any(|&v| {
        frustum
            .planes
            .iter()
            .any(|p| p.normal.dot(v) + p.dist < 0.0)
    });
    assert!(
        any_vertex_outside,
        "precondition failed: every portal vertex is inside every \
         frustum plane, so the clip step is a no-op and this test \
         does not exercise the blank-frame scenario."
    );

    let visible = portal_traverse(camera_pos, 0, &frustum, &world, false);

    assert!(visible[0], "camera cell must always be visible");
    assert!(
        visible[1],
        "cell B must be reachable through the portal even when the \
         camera sits 0.03 units from the portal plane. Failure here \
         means the visibility frustum's near plane (inherited from \
         the render pipeline's 0.1-unit near clip) is clipping the \
         entire portal polygon to empty — the blank-frame bug in \
         tight corridors."
    );
}

/// Camera sitting **exactly on** a portal plane, looking through it.
///
/// This reproduces the gray-patch flicker captured on
/// `occlusion-test.prl` at 2026-04-17T04:50:11Z. The live trace at
/// frame 360 showed `cam=(-34.54, 6.50, -13.00) leaf=31` with all 7
/// outbound portals rejecting `v=0/4 clip` and `reach=1` — the
/// renderer fell back to drawing just the camera cell while the
/// player could see into several neighbors. The camera Z matched the
/// cell's max-Z face to the float, and adjacent frames oscillated
/// between two view-proj hashes (sub-texel camera jitter), one of
/// which clipped every portal to empty.
///
/// Setup:
/// - Cell A: a slab `x ∈ [−36.37, −34.34], y ∈ [0, 13], z ∈ [−13.41, −13.00]`
///   matching cell 31's bounds from the trace.
/// - Portal on cell A's `+Z` face (`z = −13.00`), shared with cell B
///   on the `+Z` side.
/// - Camera at `(−34.54, 6.50, −13.00)` — the same position as the
///   captured trace, sitting exactly on the portal plane.
/// - Look direction `+Z`, staring straight through the portal.
///
/// If the near-plane slide leaves portal vertices sitting exactly on
/// the slid plane and any side/near plane's `CLIP_EPSILON`
/// classification rejects them as BACK, Sutherland-Hodgman clips the
/// polygon to empty and `cell B` is unreachable — the blank-frame
/// bug, but for the "camera on the portal plane" case rather than
/// "0.03 units in front of it".
#[test]
fn portal_traverse_reaches_neighbor_when_camera_is_on_portal_plane() {
    const HFOV: f32 = 100.0_f32.to_radians();
    const NEAR: f32 = 0.1;
    const FAR: f32 = 4096.0;

    // Portal on the +Z face of cell A, shared with cell B. Vertices
    // are all at z = -13.00 (the plane the camera will sit on).
    let portal = PortalData {
        polygon: vec![
            Vec3::new(-36.37, 0.00, -13.00),
            Vec3::new(-34.34, 0.00, -13.00),
            Vec3::new(-34.34, 13.00, -13.00),
            Vec3::new(-36.37, 13.00, -13.00),
        ],
        front_cell: 0,
        back_cell: 1,
    };

    let world = test_world(
        vec![
            // Cell A — camera cell, matches bounds of legacy leaf 31 from
            // the live trace.
            test_cell(
                Vec3::new(-36.37, 0.00, -13.41),
                Vec3::new(-34.34, 13.00, -13.00),
                false,
            ),
            // Cell B — neighbor on the +Z side of the portal.
            test_cell(
                Vec3::new(-36.37, 0.00, -13.00),
                Vec3::new(-34.34, 13.00, -5.00),
                false,
            ),
        ],
        &[&[0], &[0]],
        vec![portal],
    );

    // Camera pose from the captured blank-frame trace. z = -13.00
    // puts the camera exactly on the portal plane.
    let camera_pos = Vec3::new(-34.54, 6.50, -13.00);
    let look_dir = Vec3::Z;

    let aspect = 16.0 / 9.0;
    let vfov = 2.0 * ((HFOV / 2.0).tan() / aspect).atan();
    let view = glam::camera::rh::view::look_at_mat4(camera_pos, camera_pos + look_dir, Vec3::Y);
    let proj = glam::camera::rh::proj::directx::perspective(vfov, aspect, NEAR, FAR);
    let frustum = extract_frustum_planes(proj * view);

    let visible = portal_traverse(camera_pos, 0, &frustum, &world, false);

    assert!(visible[0], "camera cell must always be visible");
    assert!(
        visible[1],
        "cell B must be reachable through the portal when the camera \
         sits exactly on the portal plane and looks through it. \
         Failure here means Sutherland-Hodgman is being used even \
         though the view-frustum cross-section at apex depth is a \
         single point — the `camera_on_polygon_plane` bypass in \
         `flood` is not firing. Reproduces the occlusion-test.prl \
         gray-patch flicker captured 2026-04-17T04:50:11Z at \
         cam=(-34.54,6.50,-13.00)."
    );
}

/// Shape-check the compact trace format: header fields, at least one
/// event line, and the new-format summary. Uses the module-private
/// `portal_traverse_inner` to read the formatted buffer directly —
/// asserting on `log::info!` output would need a custom test logger and
/// would buy nothing over reading the source-of-truth string.
#[test]
fn portal_traverse_capture_emits_compact_header_fields() {
    let world = three_cell_chain();
    let camera_pos = Vec3::new(16.0, 32.0, 32.0);
    let frustum = make_camera_frustum(camera_pos, Vec3::X);
    let (_visible, trace) = portal_traverse_inner(
        camera_pos,
        0,
        &frustum,
        &world,
        &[],
        true,
        MAX_PORTAL_WALK_STEPS,
        TimingGate::OFF,
    );
    let buf = trace.expect("capture: true should produce a trace buffer");

    // Header fields — these are the per-frame camera-cell diagnostics
    // added for the flicker bug hunt.
    assert!(buf.contains("cam=("), "header missing cam=(: {buf}");
    assert!(buf.contains("cell="), "header missing cell=: {buf}");
    assert!(buf.contains("faces="), "header missing faces=: {buf}");
    assert!(buf.contains("bnds=("), "header missing bnds=(: {buf}");
    assert!(buf.contains("cells="), "header missing cells=: {buf}");

    // At least one accepted/rejected event line under the header. The
    // straight corridor walks into cell B and cell C, so there's at
    // least one `  acc ` line.
    let has_event = buf
        .lines()
        .any(|line| line.starts_with("  acc ") || line.starts_with("  rej "));
    assert!(has_event, "expected at least one event line: {buf}");

    // Summary: starts with `  = reach=` and contains `rej[`.
    let summary = buf
        .lines()
        .find(|line| line.starts_with("  = reach="))
        .expect("expected a summary line starting with `  = reach=`");
    assert!(
        summary.contains("rej["),
        "summary missing rej[ bracket: {summary}"
    );
}
