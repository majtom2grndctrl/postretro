// Runtime portal traversal: single-pass polygon-vs-frustum clipping + narrowing.
// See: context/lib/build_pipeline.md §Runtime visibility

use std::collections::VecDeque;

use glam::Vec3;

use crate::prl::LevelWorld;
use crate::visibility::{Frustum, FrustumPlane};

/// Half-space boundary epsilon for Sutherland-Hodgman.
///
/// Points within `CLIP_EPSILON` of a plane are treated as on the plane (kept as
/// inside). Over-inclusion at the boundary cannot violate the strict-subset
/// invariant — the next narrowing iteration will exclude any genuinely-outside
/// slop introduced here.
const CLIP_EPSILON: f32 = 1e-4;

/// Perform single-pass polygon-clipped portal traversal to determine which
/// leaves are visible from the camera's current leaf.
///
/// For each portal reached during the BFS, the portal polygon is clipped
/// against every plane of the current frustum (Sutherland-Hodgman). An empty
/// clip output is the unified rejection signal — the portal is not visible
/// through the current sight cone. The clipped polygon then feeds frustum
/// narrowing, which builds a new cone strictly inside the current one.
///
/// By induction from the camera's initial frustum, every narrowed frustum
/// reachable through any portal chain is a strict subset of the camera
/// frustum. A per-leaf AABB cull is therefore unnecessary on this path.
///
/// When `capture` is true the walk emits one log line per portal touched
/// (accept/reject + reason) plus a per-frame summary, all under the
/// `postretro::portal_trace` target. Already-visited rejections are counted
/// in the summary but not line-logged — they are the bulk of intra-frame
/// noise. Triggered by the `Alt+Shift+1` diagnostic chord; see
/// `context/lib/input.md` §7.
pub fn portal_traverse(
    camera_position: Vec3,
    camera_leaf: usize,
    frustum: &Frustum,
    world: &LevelWorld,
    capture: bool,
) -> Vec<bool> {
    let leaf_count = world.leaves.len();
    let mut visible = vec![false; leaf_count];

    if capture {
        log::info!(
            target: "postretro::portal_trace",
            "[portal_trace] start camera_leaf={} leaf_count={}",
            camera_leaf,
            leaf_count,
        );
    }

    if camera_leaf >= leaf_count {
        if capture {
            log::info!(
                target: "postretro::portal_trace",
                "[portal_trace] abort camera_leaf out of range",
            );
        }
        return visible;
    }

    visible[camera_leaf] = true;

    let mut queue: VecDeque<(usize, Frustum)> = VecDeque::new();
    queue.push_back((camera_leaf, frustum.clone()));

    let mut considered: u32 = 0;
    let mut accepted: u32 = 0;
    let mut rejected_already_visited: u32 = 0;
    let mut rejected_solid: u32 = 0;
    let mut rejected_clipped: u32 = 0;
    let mut rejected_narrow: u32 = 0;
    let mut rejected_invalid: u32 = 0;

    while let Some((current_leaf, current_frustum)) = queue.pop_front() {
        for &portal_idx in &world.leaf_portals[current_leaf] {
            let portal = &world.portals[portal_idx];

            // Determine the neighbor leaf (the portal's other side).
            let neighbor = if portal.front_leaf == current_leaf {
                portal.back_leaf
            } else {
                portal.front_leaf
            };

            considered += 1;

            if neighbor >= leaf_count {
                rejected_invalid += 1;
                continue;
            }

            // Skip already-visible leaves (avoids cycles).
            if visible[neighbor] {
                rejected_already_visited += 1;
                continue;
            }

            // Skip solid leaves.
            if world.leaves[neighbor].is_solid {
                rejected_solid += 1;
                if capture {
                    log::info!(
                        target: "postretro::portal_trace",
                        "[portal_trace] reject src={} dst={} reason=solid_neighbor",
                        current_leaf,
                        neighbor,
                    );
                }
                continue;
            }

            // Clip the portal polygon against the current frustum. An empty
            // result unifies "portal entirely outside cone" and "portal
            // degenerate after clipping" into one rejection path.
            let clipped = clip_polygon_to_frustum(&portal.polygon, &current_frustum);
            if clipped.len() < 3 {
                rejected_clipped += 1;
                if capture {
                    log::info!(
                        target: "postretro::portal_trace",
                        "[portal_trace] reject src={} dst={} reason=clipped_to_empty clipped_verts={} portal_verts={}",
                        current_leaf,
                        neighbor,
                        clipped.len(),
                        portal.polygon.len(),
                    );
                }
                continue;
            }

            // Narrow the frustum through the clipped polygon. The clipped
            // polygon lies entirely inside the current frustum, so the edge
            // planes it produces form a cone strictly inside the current one.
            if let Some(narrowed) =
                narrow_frustum(camera_position, &clipped, &current_frustum)
            {
                visible[neighbor] = true;
                accepted += 1;
                if capture {
                    log::info!(
                        target: "postretro::portal_trace",
                        "[portal_trace] accept src={} dst={} clipped_verts={}",
                        current_leaf,
                        neighbor,
                        clipped.len(),
                    );
                }
                queue.push_back((neighbor, narrowed));
            } else {
                rejected_narrow += 1;
                if capture {
                    log::info!(
                        target: "postretro::portal_trace",
                        "[portal_trace] reject src={} dst={} reason=narrow_frustum_failed clipped_verts={}",
                        current_leaf,
                        neighbor,
                        clipped.len(),
                    );
                }
            }
        }
    }

    if capture {
        let reach_count = visible.iter().filter(|&&v| v).count();
        log::info!(
            target: "postretro::portal_trace",
            "[portal_trace] summary reach={} considered={} accepted={} \
             rejected_clipped={} rejected_narrow={} rejected_solid={} \
             rejected_already_visited={} rejected_invalid={}",
            reach_count,
            considered,
            accepted,
            rejected_clipped,
            rejected_narrow,
            rejected_solid,
            rejected_already_visited,
            rejected_invalid,
        );
    }

    visible
}

/// Clip a convex polygon against every plane of a frustum (Sutherland-Hodgman).
///
/// Returns the clipped polygon as a new `Vec<Vec3>`. A result with fewer than
/// 3 vertices means the polygon is entirely outside the frustum (or clipped
/// down to a degenerate edge/point at a boundary).
///
/// Each frustum plane is in Hessian normal form pointing inward: a vertex `v`
/// is inside when `plane.normal · v + plane.dist >= -CLIP_EPSILON`. The
/// epsilon tilts boundary cases toward "inside". This cannot violate the
/// strict-subset invariant — any slop kept at one hop becomes outside the
/// next narrowing's edge planes and is discarded there.
///
/// Because every clipped vertex is either an original polygon vertex or an
/// intersection of a polygon edge with a frustum plane (both on the polygon
/// plane), the clipped polygon remains planar. This is required for
/// `narrow_frustum` to produce meaningful edge planes.
pub(crate) fn clip_polygon_to_frustum(polygon: &[Vec3], frustum: &Frustum) -> Vec<Vec3> {
    if polygon.len() < 3 {
        return Vec::new();
    }

    let mut input: Vec<Vec3> = polygon.to_vec();
    let mut output: Vec<Vec3> = Vec::with_capacity(polygon.len() + frustum.planes.len());

    for plane in &frustum.planes {
        if input.is_empty() {
            break;
        }
        output.clear();
        clip_polygon_to_plane(&input, plane, &mut output);
        std::mem::swap(&mut input, &mut output);
    }

    input
}

/// Clip a convex polygon against a single half-space (one Sutherland-Hodgman step).
///
/// Writes the clipped vertices into `output` (which is cleared on entry by the
/// caller). The input polygon must be closed in winding order; vertex order is
/// preserved in the output.
fn clip_polygon_to_plane(input: &[Vec3], plane: &FrustumPlane, output: &mut Vec<Vec3>) {
    let signed_distance = |v: Vec3| plane.normal.dot(v) + plane.dist;

    let n = input.len();
    for i in 0..n {
        let current = input[i];
        let previous = input[(i + n - 1) % n];

        let d_current = signed_distance(current);
        let d_previous = signed_distance(previous);

        let current_inside = d_current >= -CLIP_EPSILON;
        let previous_inside = d_previous >= -CLIP_EPSILON;

        if current_inside {
            if !previous_inside {
                // Entering: emit the intersection, then the current vertex.
                if let Some(intersection) = intersect_edge_plane(previous, current, d_previous, d_current) {
                    output.push(intersection);
                }
            }
            output.push(current);
        } else if previous_inside {
            // Leaving: emit the intersection only.
            if let Some(intersection) = intersect_edge_plane(previous, current, d_previous, d_current) {
                output.push(intersection);
            }
        }
        // Both outside: emit nothing.
    }
}

/// Intersect the line segment `[a, b]` with a plane, given their signed
/// distances to the plane.
///
/// Returns `None` if the segment is parallel to the plane (distances equal to
/// within numerical precision) — in that case the caller's inside/outside
/// classification handles the vertices directly.
fn intersect_edge_plane(a: Vec3, b: Vec3, d_a: f32, d_b: f32) -> Option<Vec3> {
    let denom = d_a - d_b;
    if denom.abs() < f32::EPSILON {
        return None;
    }
    let t = d_a / denom;
    Some(a + (b - a) * t)
}

/// Narrow a frustum by constructing clip planes through the camera and the
/// portal polygon edges.
///
/// For a portal polygon with N vertices, constructs N edge planes (each through
/// the camera position and one edge of the portal) plus the portal's own plane
/// as a near clip. The far plane is retained from the original frustum.
///
/// Returns None if the portal is behind the camera or degenerate.
pub fn narrow_frustum(
    camera_position: Vec3,
    portal_polygon: &[Vec3],
    original_frustum: &Frustum,
) -> Option<Frustum> {
    if portal_polygon.len() < 3 {
        return None;
    }

    // Compute the portal plane from the polygon.
    let v0 = portal_polygon[0];
    let v1 = portal_polygon[1];
    let v2 = portal_polygon[2];
    let portal_normal = (v1 - v0).cross(v2 - v0);
    if portal_normal.length_squared() < 1e-12 {
        return None;
    }
    let portal_normal = portal_normal.normalize();

    // Orient the portal normal to face away from the camera.
    // The near plane should clip away the side of the portal the camera is on.
    let camera_side = portal_normal.dot(camera_position - v0);
    let oriented_normal = if camera_side > 0.0 {
        -portal_normal
    } else {
        portal_normal
    };
    let portal_dist = -oriented_normal.dot(v0);

    let mut planes = Vec::with_capacity(portal_polygon.len() + 2);

    // Portal plane as near clip.
    planes.push(crate::visibility::FrustumPlane {
        normal: oriented_normal,
        dist: portal_dist,
    });

    // Edge planes: for each portal edge, the clip plane passes through the
    // camera and the edge, oriented to face the portal centroid. This is the
    // exact visibility cone from a point camera through the portal.
    let n = portal_polygon.len();
    let centroid = portal_polygon.iter().copied().sum::<Vec3>() / n as f32;
    for i in 0..n {
        let edge_a = portal_polygon[i];
        let edge_b = portal_polygon[(i + 1) % n];
        let edge_dir = edge_b - edge_a;
        let to_camera = camera_position - edge_a;

        let mut edge_normal = edge_dir.cross(to_camera);
        if edge_normal.length_squared() < 1e-12 {
            continue;
        }
        edge_normal = edge_normal.normalize();
        if edge_normal.dot(centroid - edge_a) < 0.0 {
            edge_normal = -edge_normal;
        }
        let dist = -edge_normal.dot(edge_a);

        planes.push(crate::visibility::FrustumPlane {
            normal: edge_normal,
            dist,
        });
    }

    // Keep the far plane from the original frustum (always the last plane).
    if let Some(&far_plane) = original_frustum.planes.last() {
        planes.push(far_plane);
    }

    Some(Frustum { planes })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::prl::{BspChild, LeafData, LevelWorld, NodeData, PortalData};
    use crate::visibility::{FrustumPlane, is_aabb_outside_frustum};
    use glam::Mat4;

    /// Extract frustum from a view-projection matrix (reuse from visibility module).
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
        let view = Mat4::look_at_rh(position, target, Vec3::Y);
        let aspect = 16.0 / 9.0;
        let hfov = 100.0_f32.to_radians();
        let vfov = 2.0 * ((hfov / 2.0).tan() / aspect).atan();
        let proj = Mat4::perspective_rh(vfov, aspect, 0.1, 4096.0);
        extract_test_frustum(proj * view)
    }

    /// Build a three-leaf chain: A (leaf 0) -- portal 0 -- B (leaf 1) -- portal 1 -- C (leaf 2)
    /// arranged along the X axis.
    fn three_leaf_chain() -> LevelWorld {
        let portal_0 = PortalData {
            polygon: vec![
                Vec3::new(32.0, 0.0, 0.0),
                Vec3::new(32.0, 64.0, 0.0),
                Vec3::new(32.0, 64.0, 64.0),
                Vec3::new(32.0, 0.0, 64.0),
            ],
            front_leaf: 0,
            back_leaf: 1,
        };
        let portal_1 = PortalData {
            polygon: vec![
                Vec3::new(64.0, 0.0, 0.0),
                Vec3::new(64.0, 64.0, 0.0),
                Vec3::new(64.0, 64.0, 64.0),
                Vec3::new(64.0, 0.0, 64.0),
            ],
            front_leaf: 1,
            back_leaf: 2,
        };

        LevelWorld {
            vertices: vec![],
            indices: vec![],
            face_meta: vec![],
            nodes: vec![
                NodeData {
                    plane_normal: Vec3::X,
                    plane_distance: 32.0,
                    front: BspChild::Node(1),
                    back: BspChild::Leaf(0),
                },
                NodeData {
                    plane_normal: Vec3::X,
                    plane_distance: 64.0,
                    front: BspChild::Leaf(2),
                    back: BspChild::Leaf(1),
                },
            ],
            leaves: vec![
                LeafData {
                    bounds_min: Vec3::new(0.0, 0.0, 0.0),
                    bounds_max: Vec3::new(32.0, 64.0, 64.0),
                    face_start: 0,
                    face_count: 0,
                    pvs: vec![],
                    is_solid: false,
                    texture_sub_ranges: vec![],
                },
                LeafData {
                    bounds_min: Vec3::new(32.0, 0.0, 0.0),
                    bounds_max: Vec3::new(64.0, 64.0, 64.0),
                    face_start: 0,
                    face_count: 0,
                    pvs: vec![],
                    is_solid: false,
                    texture_sub_ranges: vec![],
                },
                LeafData {
                    bounds_min: Vec3::new(64.0, 0.0, 0.0),
                    bounds_max: Vec3::new(96.0, 64.0, 64.0),
                    face_start: 0,
                    face_count: 0,
                    pvs: vec![],
                    is_solid: false,
                    texture_sub_ranges: vec![],
                },
            ],
            root: BspChild::Node(0),
            has_pvs: false,
            portals: vec![portal_0, portal_1],
            leaf_portals: vec![
                vec![0],    // leaf 0 touches portal 0
                vec![0, 1], // leaf 1 touches portals 0 and 1
                vec![1],    // leaf 2 touches portal 1
            ],
            has_portals: true,
            texture_names: vec![],
        }
    }

    #[test]
    fn portal_traverse_camera_leaf_always_visible() {
        let world = three_leaf_chain();
        // Camera in leaf 0, looking away from all portals.
        let frustum = make_camera_frustum(Vec3::new(16.0, 32.0, 32.0), Vec3::NEG_X);
        let visible = portal_traverse(Vec3::new(16.0, 32.0, 32.0), 0, &frustum, &world, false);
        assert!(visible[0], "camera leaf should always be visible");
    }

    #[test]
    fn portal_traverse_straight_corridor_sees_all_three() {
        let world = three_leaf_chain();
        // Camera in leaf 0, looking through portals toward +X.
        let camera_pos = Vec3::new(16.0, 32.0, 32.0);
        let frustum = make_camera_frustum(camera_pos, Vec3::X);
        let visible = portal_traverse(camera_pos, 0, &frustum, &world, false);
        assert!(visible[0], "camera leaf A should be visible");
        assert!(visible[1], "leaf B should be visible through portal 0");
        assert!(visible[2], "leaf C should be visible through portals 0+1");
    }

    #[test]
    fn portal_traverse_looking_away_hides_distant_leaves() {
        let world = three_leaf_chain();
        // Camera in leaf 0, looking away from the portals (toward -X).
        let camera_pos = Vec3::new(16.0, 32.0, 32.0);
        let frustum = make_camera_frustum(camera_pos, Vec3::NEG_X);
        let visible = portal_traverse(camera_pos, 0, &frustum, &world, false);
        assert!(visible[0], "camera leaf should be visible");
        // Portals are at X=32 and X=64, camera looks toward -X, so they're behind.
        assert!(
            !visible[1],
            "leaf B should not be visible when looking away"
        );
        assert!(
            !visible[2],
            "leaf C should not be visible when looking away"
        );
    }

    #[test]
    fn portal_traverse_skips_solid_neighbors() {
        let mut world = three_leaf_chain();
        world.leaves[1].is_solid = true;

        let camera_pos = Vec3::new(16.0, 32.0, 32.0);
        let frustum = make_camera_frustum(camera_pos, Vec3::X);
        let visible = portal_traverse(camera_pos, 0, &frustum, &world, false);
        assert!(visible[0], "camera leaf should be visible");
        assert!(!visible[1], "solid leaf should not be visible");
        // Leaf 2 is behind solid leaf 1, so it can't be reached.
        assert!(!visible[2], "leaf behind solid should not be visible");
    }

    #[test]
    fn portal_traverse_empty_world() {
        let world = LevelWorld {
            vertices: vec![],
            indices: vec![],
            face_meta: vec![],
            nodes: vec![],
            leaves: vec![],
            root: BspChild::Leaf(0),
            has_pvs: false,
            portals: vec![],
            leaf_portals: vec![],
            has_portals: false,
            texture_names: vec![],
        };

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
            front_leaf: 0,
            back_leaf: 1,
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
            front_leaf: 1,
            back_leaf: 2,
        };

        let world = LevelWorld {
            vertices: vec![],
            indices: vec![],
            face_meta: vec![],
            nodes: vec![],
            leaves: vec![
                LeafData {
                    bounds_min: Vec3::new(0.0, 0.0, 0.0),
                    bounds_max: Vec3::new(32.0, 64.0, 64.0),
                    face_start: 0,
                    face_count: 0,
                    pvs: vec![],
                    is_solid: false,
                    texture_sub_ranges: vec![],
                },
                LeafData {
                    bounds_min: Vec3::new(32.0, 0.0, 0.0),
                    bounds_max: Vec3::new(64.0, 64.0, 200.0),
                    face_start: 0,
                    face_count: 0,
                    pvs: vec![],
                    is_solid: false,
                    texture_sub_ranges: vec![],
                },
                LeafData {
                    bounds_min: Vec3::new(32.0, 0.0, 200.0),
                    bounds_max: Vec3::new(64.0, 64.0, 264.0),
                    face_start: 0,
                    face_count: 0,
                    pvs: vec![],
                    is_solid: false,
                    texture_sub_ranges: vec![],
                },
            ],
            root: BspChild::Leaf(0),
            has_pvs: false,
            portals: vec![portal_0, portal_1],
            leaf_portals: vec![vec![0], vec![0, 1], vec![1]],
            has_portals: true,
            texture_names: vec![],
        };

        // Camera in leaf A, looking straight along +X toward portal 0.
        let camera_pos = Vec3::new(16.0, 32.0, 32.0);
        let frustum = make_camera_frustum(camera_pos, Vec3::X);
        let visible = portal_traverse(camera_pos, 0, &frustum, &world, false);
        assert!(visible[0], "camera leaf A should be visible");
        assert!(visible[1], "leaf B should be visible through portal 0");
        assert!(
            !visible[2],
            "leaf C should not be visible — portal 1 is around the corner at Z=200"
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

    #[test]
    fn portal_traversal_sees_room_through_both_sides_of_pillar() {
        // Room layout with NARROW portals (2 units wide) matching the pillar
        // gap dimensions that cause issues in portal generation:
        //
        // Leaf A (camera room, X=0..120) --[portal 0 at X=120, Z=62..64]--> Leaf B (left gap)
        //                                --[portal 1 at X=120, Z=66..68]--> Leaf C (right gap)
        // Leaf B --[portal 2 at X=136, Z=62..64]--> Leaf D (far room, X=136..256)
        // Leaf C --[portal 3 at X=136, Z=66..68]--> Leaf D
        //
        // The portals are only 2 units wide (matching a narrow doorway gap).
        let portal_a_b = PortalData {
            polygon: vec![
                Vec3::new(120.0, 16.0, 62.0),
                Vec3::new(120.0, 112.0, 62.0),
                Vec3::new(120.0, 112.0, 64.0),
                Vec3::new(120.0, 16.0, 64.0),
            ],
            front_leaf: 0,
            back_leaf: 1,
        };
        let portal_a_c = PortalData {
            polygon: vec![
                Vec3::new(120.0, 16.0, 66.0),
                Vec3::new(120.0, 112.0, 66.0),
                Vec3::new(120.0, 112.0, 68.0),
                Vec3::new(120.0, 16.0, 68.0),
            ],
            front_leaf: 0,
            back_leaf: 2,
        };
        let portal_b_d = PortalData {
            polygon: vec![
                Vec3::new(136.0, 16.0, 62.0),
                Vec3::new(136.0, 112.0, 62.0),
                Vec3::new(136.0, 112.0, 64.0),
                Vec3::new(136.0, 16.0, 64.0),
            ],
            front_leaf: 1,
            back_leaf: 3,
        };
        let portal_c_d = PortalData {
            polygon: vec![
                Vec3::new(136.0, 16.0, 66.0),
                Vec3::new(136.0, 112.0, 66.0),
                Vec3::new(136.0, 112.0, 68.0),
                Vec3::new(136.0, 16.0, 68.0),
            ],
            front_leaf: 2,
            back_leaf: 3,
        };

        let world = LevelWorld {
            vertices: vec![],
            indices: vec![],
            face_meta: vec![],
            nodes: vec![
                // Root splits at X=120
                NodeData {
                    plane_normal: Vec3::X,
                    plane_distance: 120.0,
                    front: BspChild::Node(1),
                    back: BspChild::Leaf(0),
                },
                // Split at X=136
                NodeData {
                    plane_normal: Vec3::X,
                    plane_distance: 136.0,
                    front: BspChild::Leaf(3),
                    back: BspChild::Node(2),
                },
                // Split at Z=65 (between the two gaps) to separate B and C
                NodeData {
                    plane_normal: Vec3::Z,
                    plane_distance: 65.0,
                    front: BspChild::Leaf(2),
                    back: BspChild::Leaf(1),
                },
            ],
            leaves: vec![
                // Leaf 0: camera room (A), X=0..120
                LeafData {
                    bounds_min: Vec3::new(0.0, 0.0, 0.0),
                    bounds_max: Vec3::new(120.0, 128.0, 128.0),
                    face_start: 0,
                    face_count: 0,
                    pvs: vec![],
                    is_solid: false,
                    texture_sub_ranges: vec![],
                },
                // Leaf 1: left gap passage (B), Z=62..64
                LeafData {
                    bounds_min: Vec3::new(120.0, 16.0, 62.0),
                    bounds_max: Vec3::new(136.0, 112.0, 64.0),
                    face_start: 0,
                    face_count: 0,
                    pvs: vec![],
                    is_solid: false,
                    texture_sub_ranges: vec![],
                },
                // Leaf 2: right gap passage (C), Z=66..68
                LeafData {
                    bounds_min: Vec3::new(120.0, 16.0, 66.0),
                    bounds_max: Vec3::new(136.0, 112.0, 68.0),
                    face_start: 0,
                    face_count: 0,
                    pvs: vec![],
                    is_solid: false,
                    texture_sub_ranges: vec![],
                },
                // Leaf 3: far room (D), X=136..256
                LeafData {
                    bounds_min: Vec3::new(136.0, 0.0, 0.0),
                    bounds_max: Vec3::new(256.0, 128.0, 128.0),
                    face_start: 0,
                    face_count: 0,
                    pvs: vec![],
                    is_solid: false,
                    texture_sub_ranges: vec![],
                },
            ],
            root: BspChild::Node(0),
            has_pvs: false,
            portals: vec![portal_a_b, portal_a_c, portal_b_d, portal_c_d],
            leaf_portals: vec![
                vec![0, 1],    // leaf A touches portal 0 (A-B) and portal 1 (A-C)
                vec![0, 2],    // leaf B touches portal 0 (A-B) and portal 2 (B-D)
                vec![1, 3],    // leaf C touches portal 1 (A-C) and portal 3 (C-D)
                vec![2, 3],    // leaf D touches portal 2 (B-D) and portal 3 (C-D)
            ],
            has_portals: true,
            texture_names: vec![],
        };

        // Camera looking through the LEFT passage (Z=63, center of Z=62..64 gap).
        // Camera is in leaf A, looking toward +X.
        {
            let camera_pos = Vec3::new(16.0, 64.0, 63.0);
            let frustum = make_camera_frustum(camera_pos, Vec3::X);
            let visible = portal_traverse(camera_pos, 0, &frustum, &world, false);
            assert!(visible[0], "camera leaf A should be visible");
            assert!(
                visible[1],
                "leaf B (left gap) should be visible when looking through left doorway"
            );
            assert!(
                visible[3],
                "leaf D (far room) should be visible through left passage (A->B->D). \
                 If not, the narrow frustum through the 2-unit-wide portal A-B may be \
                 rejecting the 2-unit-wide portal B-D."
            );
        }

        // Camera looking through the RIGHT passage (Z=67, center of Z=66..68 gap).
        {
            let camera_pos = Vec3::new(16.0, 64.0, 67.0);
            let frustum = make_camera_frustum(camera_pos, Vec3::X);
            let visible = portal_traverse(camera_pos, 0, &frustum, &world, false);
            assert!(visible[0], "camera leaf A should be visible");
            assert!(
                visible[2],
                "leaf C (right gap) should be visible when looking through right doorway"
            );
            assert!(
                visible[3],
                "leaf D (far room) should be visible through right passage (A->C->D). \
                 If not, the narrow frustum through the 2-unit-wide portal A-C may be \
                 rejecting the 2-unit-wide portal C-D."
            );
        }

    }

    // --- Polygon-vs-frustum clipping tests ---

    /// Classify a polygon vertex as strictly inside every plane of a frustum
    /// (within the clip epsilon).
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

        let clipped = clip_polygon_to_frustum(&polygon, &frustum);
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

        let clipped = clip_polygon_to_frustum(&polygon, &frustum);
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

        let clipped = clip_polygon_to_frustum(&polygon, &frustum);
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
        assert!(clip_polygon_to_frustum(&[], &frustum).is_empty());
        assert!(clip_polygon_to_frustum(&[Vec3::X, Vec3::Y], &frustum).is_empty());
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

        let clipped = clip_polygon_to_frustum(&portal, &parent);
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
        // hop, every leaf visible in the narrowed frustum must also be inside
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

        // Hop 1.
        let clipped_a = clip_polygon_to_frustum(&portal_a, &parent);
        assert!(clipped_a.len() >= 3);
        let narrowed_1 = narrow_frustum(camera_pos, &clipped_a, &parent).expect("hop 1");

        // Hop 2: clip next portal against hop-1 frustum.
        let clipped_b = clip_polygon_to_frustum(&portal_b, &narrowed_1);
        assert!(clipped_b.len() >= 3);
        let narrowed_2 = narrow_frustum(camera_pos, &clipped_b, &narrowed_1).expect("hop 2");

        // Hop 3.
        let clipped_c = clip_polygon_to_frustum(&portal_c, &narrowed_2);
        assert!(clipped_c.len() >= 3);
        let narrowed_3 = narrow_frustum(camera_pos, &clipped_c, &narrowed_2).expect("hop 3");

        // Strict-subset check: every vertex of every clipped polygon lies
        // inside the original parent frustum.
        for v in clipped_a.iter().chain(clipped_b.iter()).chain(clipped_c.iter()) {
            assert!(
                point_inside_frustum(*v, &parent),
                "clipped vertex {v:?} must lie inside the original camera frustum"
            );
        }

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
        // Straight-through layout: camera in leaf 0 looking +X.
        // Portal 0 (A -> B) straddles the camera's side plane — it extends
        // far beyond the frustum to the +Y direction. Without polygon
        // clipping, frustum narrowing through the un-clipped portal could
        // produce a cone that extends into -Y regions the camera cannot see
        // and incorrectly admit off-axis neighbors.
        //
        // This test asserts that with clipping in place, leaf B is still
        // visible (the portal is in view) and a far off-axis leaf C reached
        // through an orthogonal portal at leaf B is correctly hidden.
        let portal_a_b = PortalData {
            polygon: vec![
                // 1000-unit-tall portal at X=10, centered on Z=0.
                Vec3::new(10.0, -500.0, -1.0),
                Vec3::new(10.0, 500.0, -1.0),
                Vec3::new(10.0, 500.0, 1.0),
                Vec3::new(10.0, -500.0, 1.0),
            ],
            front_leaf: 0,
            back_leaf: 1,
        };
        // Portal 1 (B -> C) is far out in +Y, well outside the camera's
        // actual view cone even though leaf B is reachable.
        let portal_b_c = PortalData {
            polygon: vec![
                Vec3::new(15.0, 400.0, -1.0),
                Vec3::new(20.0, 400.0, -1.0),
                Vec3::new(20.0, 400.0, 1.0),
                Vec3::new(15.0, 400.0, 1.0),
            ],
            front_leaf: 1,
            back_leaf: 2,
        };

        let world = LevelWorld {
            vertices: vec![],
            indices: vec![],
            face_meta: vec![],
            nodes: vec![],
            leaves: vec![
                LeafData {
                    bounds_min: Vec3::new(0.0, -500.0, -500.0),
                    bounds_max: Vec3::new(10.0, 500.0, 500.0),
                    face_start: 0,
                    face_count: 0,
                    pvs: vec![],
                    is_solid: false,
                    texture_sub_ranges: vec![],
                },
                LeafData {
                    bounds_min: Vec3::new(10.0, -500.0, -500.0),
                    bounds_max: Vec3::new(25.0, 500.0, 500.0),
                    face_start: 0,
                    face_count: 0,
                    pvs: vec![],
                    is_solid: false,
                    texture_sub_ranges: vec![],
                },
                LeafData {
                    bounds_min: Vec3::new(15.0, 400.0, -500.0),
                    bounds_max: Vec3::new(25.0, 600.0, 500.0),
                    face_start: 0,
                    face_count: 0,
                    pvs: vec![],
                    is_solid: false,
                    texture_sub_ranges: vec![],
                },
            ],
            root: BspChild::Leaf(0),
            has_pvs: false,
            portals: vec![portal_a_b, portal_b_c],
            leaf_portals: vec![vec![0], vec![0, 1], vec![1]],
            has_portals: true,
            texture_names: vec![],
        };

        let camera_pos = Vec3::new(1.0, 0.0, 0.0);
        let frustum = make_camera_frustum(camera_pos, Vec3::X);
        let visible = portal_traverse(camera_pos, 0, &frustum, &world, false);

        assert!(visible[0], "camera leaf should always be visible");
        assert!(
            visible[1],
            "leaf B should be visible through the straddling portal"
        );
        assert!(
            !visible[2],
            "leaf C should be hidden: portal 1 is far off-axis and \
             unreachable through the clipped sight cone"
        );
    }
}
