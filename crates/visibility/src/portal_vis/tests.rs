// Portal clipping and frustum-narrowing tests.

use super::clip::*;
use crate::visibility::{Frustum, FrustumPlane, is_aabb_outside_frustum};
use glam::{Mat4, Vec3};

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
