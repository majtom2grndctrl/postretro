// Runtime portal traversal with frustum-clipped portal walk.
// See: context/plans/in-progress/portal-bsp-vis/task-08-runtime-portal-vis.md

use std::collections::VecDeque;

use glam::Vec3;

use crate::prl::LevelWorld;
use crate::visibility::{Frustum, is_aabb_outside_frustum};

/// Perform frustum-clipped portal traversal to determine which leaves are
/// visible from the camera's current leaf.
///
/// At each portal, the frustum is narrowed to only include sight lines that
/// pass through the portal polygon. This provides around-the-corner culling
/// that precomputed PVS cannot.
pub fn portal_traverse(
    camera_position: Vec3,
    camera_leaf: usize,
    frustum: &Frustum,
    world: &LevelWorld,
) -> Vec<bool> {
    let leaf_count = world.leaves.len();
    let mut visible = vec![false; leaf_count];

    if camera_leaf >= leaf_count {
        return visible;
    }

    visible[camera_leaf] = true;

    let mut queue: VecDeque<(usize, Frustum)> = VecDeque::new();
    queue.push_back((camera_leaf, frustum.clone()));

    while let Some((current_leaf, current_frustum)) = queue.pop_front() {
        for &portal_idx in &world.leaf_portals[current_leaf] {
            let portal = &world.portals[portal_idx];

            // Determine the neighbor leaf (the portal's other side).
            let neighbor = if portal.front_leaf == current_leaf {
                portal.back_leaf
            } else {
                portal.front_leaf
            };

            if neighbor >= leaf_count {
                continue;
            }

            // Skip already-visible leaves (avoids cycles).
            if visible[neighbor] {
                continue;
            }

            // Skip solid leaves.
            if world.leaves[neighbor].is_solid {
                continue;
            }

            // AABB early-out: test portal polygon's AABB against current frustum.
            let (portal_mins, portal_maxs) = portal_aabb(&portal.polygon);
            if is_aabb_outside_frustum(portal_mins, portal_maxs, &current_frustum) {
                continue;
            }

            // Test if any portal vertex is inside the current frustum.
            // If all vertices are behind any single frustum plane, the portal
            // is fully outside and we skip it.
            if is_polygon_outside_frustum(&portal.polygon, &current_frustum) {
                continue;
            }

            // Narrow the frustum through this portal.
            if let Some(narrowed) =
                narrow_frustum(camera_position, &portal.polygon, &current_frustum)
            {
                visible[neighbor] = true;
                queue.push_back((neighbor, narrowed));
            }
        }
    }

    visible
}

/// Compute the AABB of a portal polygon.
fn portal_aabb(polygon: &[Vec3]) -> (Vec3, Vec3) {
    let mut mins = Vec3::splat(f32::MAX);
    let mut maxs = Vec3::splat(f32::MIN);
    for &v in polygon {
        mins = mins.min(v);
        maxs = maxs.max(v);
    }
    (mins, maxs)
}

/// Test whether all vertices of a polygon are outside any single frustum plane.
fn is_polygon_outside_frustum(polygon: &[Vec3], frustum: &Frustum) -> bool {
    for plane in &frustum.planes {
        let all_outside = polygon
            .iter()
            .all(|&v| plane.normal.dot(v) + plane.dist < 0.0);
        if all_outside {
            return true;
        }
    }
    false
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

    // Edge planes: for each edge of the portal, construct a plane through
    // the camera position and that edge.
    let n = portal_polygon.len();
    let centroid = portal_polygon.iter().copied().sum::<Vec3>() / n as f32;
    for i in 0..n {
        let edge_a = portal_polygon[i];
        let edge_b = portal_polygon[(i + 1) % n];
        let edge_dir = edge_b - edge_a;
        let to_camera = camera_position - edge_a;

        let mut edge_normal = edge_dir.cross(to_camera);
        if edge_normal.length_squared() < 1e-12 {
            // Camera is on the edge line; this edge doesn't contribute a clip plane.
            continue;
        }
        edge_normal = edge_normal.normalize();

        // The edge plane should face inward (toward the interior of the
        // frustum pyramid). Verify by checking that the portal centroid is
        // on the positive side.
        if edge_normal.dot(centroid - edge_a) < 0.0 {
            edge_normal = -edge_normal;
        }

        let edge_dist = -edge_normal.dot(edge_a);
        planes.push(crate::visibility::FrustumPlane {
            normal: edge_normal,
            dist: edge_dist,
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
    use crate::visibility::FrustumPlane;
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
                },
                LeafData {
                    bounds_min: Vec3::new(32.0, 0.0, 0.0),
                    bounds_max: Vec3::new(64.0, 64.0, 64.0),
                    face_start: 0,
                    face_count: 0,
                    pvs: vec![],
                    is_solid: false,
                },
                LeafData {
                    bounds_min: Vec3::new(64.0, 0.0, 0.0),
                    bounds_max: Vec3::new(96.0, 64.0, 64.0),
                    face_start: 0,
                    face_count: 0,
                    pvs: vec![],
                    is_solid: false,
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
        }
    }

    #[test]
    fn portal_traverse_camera_leaf_always_visible() {
        let world = three_leaf_chain();
        // Camera in leaf 0, looking away from all portals.
        let frustum = make_camera_frustum(Vec3::new(16.0, 32.0, 32.0), Vec3::NEG_X);
        let visible = portal_traverse(Vec3::new(16.0, 32.0, 32.0), 0, &frustum, &world);
        assert!(visible[0], "camera leaf should always be visible");
    }

    #[test]
    fn portal_traverse_straight_corridor_sees_all_three() {
        let world = three_leaf_chain();
        // Camera in leaf 0, looking through portals toward +X.
        let camera_pos = Vec3::new(16.0, 32.0, 32.0);
        let frustum = make_camera_frustum(camera_pos, Vec3::X);
        let visible = portal_traverse(camera_pos, 0, &frustum, &world);
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
        let visible = portal_traverse(camera_pos, 0, &frustum, &world);
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
        let visible = portal_traverse(camera_pos, 0, &frustum, &world);
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
        };

        let frustum = make_camera_frustum(Vec3::ZERO, Vec3::NEG_Z);
        let visible = portal_traverse(Vec3::ZERO, 0, &frustum, &world);
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
                },
                LeafData {
                    bounds_min: Vec3::new(32.0, 0.0, 0.0),
                    bounds_max: Vec3::new(64.0, 64.0, 200.0),
                    face_start: 0,
                    face_count: 0,
                    pvs: vec![],
                    is_solid: false,
                },
                LeafData {
                    bounds_min: Vec3::new(32.0, 0.0, 200.0),
                    bounds_max: Vec3::new(64.0, 64.0, 264.0),
                    face_start: 0,
                    face_count: 0,
                    pvs: vec![],
                    is_solid: false,
                },
            ],
            root: BspChild::Leaf(0),
            has_pvs: false,
            portals: vec![portal_0, portal_1],
            leaf_portals: vec![vec![0], vec![0, 1], vec![1]],
            has_portals: true,
        };

        // Camera in leaf A, looking straight along +X toward portal 0.
        let camera_pos = Vec3::new(16.0, 32.0, 32.0);
        let frustum = make_camera_frustum(camera_pos, Vec3::X);
        let visible = portal_traverse(camera_pos, 0, &frustum, &world);
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
}
