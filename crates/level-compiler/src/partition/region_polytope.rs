#![allow(dead_code)]

// Convex BSP leaf-region substrate. Regions are represented as facet windings
// plus their inward half-space planes, so splitting can clip caps without
// re-deriving planes from polygon vertices.
// See: context/plans/in-progress/bsp-exact-leaf-solidity/index.md Task 2

use glam::DVec3;

use super::types::Aabb;
use crate::geometry_utils::{clip_winding_to_half_spaces, make_base_winding, split_polygon};

const CLIP_EPSILON: f64 = 1e-6;

#[derive(Debug, Clone)]
struct Facet {
    normal: DVec3,
    distance: f64,
    winding: Vec<DVec3>,
}

/// Convex region represented as an intersection of inward-facing half-spaces.
#[derive(Debug, Clone)]
pub(crate) struct RegionPolytope {
    facets: Vec<Facet>,
}

impl RegionPolytope {
    pub(crate) fn from_aabb(bounds: &Aabb) -> Self {
        let min = bounds.min;
        let max = bounds.max;
        let facets = vec![
            Facet {
                normal: DVec3::X,
                distance: min.x,
                winding: vec![
                    DVec3::new(min.x, min.y, min.z),
                    DVec3::new(min.x, max.y, min.z),
                    DVec3::new(min.x, max.y, max.z),
                    DVec3::new(min.x, min.y, max.z),
                ],
            },
            Facet {
                normal: DVec3::NEG_X,
                distance: -max.x,
                winding: vec![
                    DVec3::new(max.x, min.y, min.z),
                    DVec3::new(max.x, min.y, max.z),
                    DVec3::new(max.x, max.y, max.z),
                    DVec3::new(max.x, max.y, min.z),
                ],
            },
            Facet {
                normal: DVec3::Y,
                distance: min.y,
                winding: vec![
                    DVec3::new(min.x, min.y, min.z),
                    DVec3::new(min.x, min.y, max.z),
                    DVec3::new(max.x, min.y, max.z),
                    DVec3::new(max.x, min.y, min.z),
                ],
            },
            Facet {
                normal: DVec3::NEG_Y,
                distance: -max.y,
                winding: vec![
                    DVec3::new(min.x, max.y, min.z),
                    DVec3::new(max.x, max.y, min.z),
                    DVec3::new(max.x, max.y, max.z),
                    DVec3::new(min.x, max.y, max.z),
                ],
            },
            Facet {
                normal: DVec3::Z,
                distance: min.z,
                winding: vec![
                    DVec3::new(min.x, min.y, min.z),
                    DVec3::new(max.x, min.y, min.z),
                    DVec3::new(max.x, max.y, min.z),
                    DVec3::new(min.x, max.y, min.z),
                ],
            },
            Facet {
                normal: DVec3::NEG_Z,
                distance: -max.z,
                winding: vec![
                    DVec3::new(min.x, min.y, max.z),
                    DVec3::new(min.x, max.y, max.z),
                    DVec3::new(max.x, max.y, max.z),
                    DVec3::new(max.x, min.y, max.z),
                ],
            },
        ];

        Self { facets }
    }

    pub(crate) fn clip(&self, normal: DVec3, distance: f64) -> (Self, Self) {
        let mut has_front = false;
        let mut has_back = false;
        for vertex in self.vertices() {
            let signed_distance = vertex.dot(normal) - distance;
            has_front |= signed_distance > CLIP_EPSILON;
            has_back |= signed_distance < -CLIP_EPSILON;
        }

        if !has_front && !has_back {
            return (self.clone(), Self::empty());
        }
        if !has_front {
            return (Self::empty(), self.clone());
        }
        if !has_back {
            return (self.clone(), Self::empty());
        }

        let mut front_facets = Vec::new();
        let mut back_facets = Vec::new();
        for facet in &self.facets {
            let (front, back) = split_polygon(&facet.winding, normal, distance, CLIP_EPSILON);
            if let Some(winding) = front {
                push_facet(&mut front_facets, facet.normal, facet.distance, winding);
            }
            if let Some(winding) = back {
                push_facet(&mut back_facets, facet.normal, facet.distance, winding);
            }
        }

        if let Some(cap) = self.cap_winding(normal, distance) {
            push_facet(&mut front_facets, normal, distance, cap);
        }
        if let Some(cap) = self.cap_winding(-normal, -distance) {
            push_facet(&mut back_facets, -normal, -distance, cap);
        }

        (
            Self {
                facets: front_facets,
            },
            Self {
                facets: back_facets,
            },
        )
    }

    pub(crate) fn all_vertices_behind(&self, normal: DVec3, distance: f64, tol: f64) -> bool {
        let mut saw_vertex = false;
        for vertex in self.vertices() {
            saw_vertex = true;
            if vertex.dot(normal) - distance > tol {
                return false;
            }
        }
        saw_vertex
    }

    pub(crate) fn plane_spans(&self, normal: DVec3, distance: f64, tol: f64) -> bool {
        let mut has_front = false;
        let mut has_back = false;
        for vertex in self.vertices() {
            let signed_distance = vertex.dot(normal) - distance;
            has_front |= signed_distance > tol;
            has_back |= signed_distance < -tol;
            if has_front && has_back {
                return true;
            }
        }
        false
    }

    pub(crate) fn vertex_aabb(&self) -> Aabb {
        let mut bounds = Aabb::empty();
        for vertex in self.vertices() {
            bounds.expand_point(vertex);
        }
        bounds
    }

    fn empty() -> Self {
        Self { facets: Vec::new() }
    }

    fn half_spaces(&self) -> Vec<(DVec3, f64)> {
        self.facets
            .iter()
            .map(|facet| (facet.normal, facet.distance))
            .collect()
    }

    fn cap_winding(&self, normal: DVec3, distance: f64) -> Option<Vec<DVec3>> {
        let winding = make_base_winding(normal, distance);
        let winding = clip_winding_to_half_spaces(winding, &self.half_spaces(), CLIP_EPSILON)?;
        if winding.len() >= 3 {
            Some(winding)
        } else {
            None
        }
    }

    fn vertices(&self) -> impl Iterator<Item = DVec3> + '_ {
        self.facets
            .iter()
            .flat_map(|facet| facet.winding.iter().copied())
    }
}

fn push_facet(facets: &mut Vec<Facet>, normal: DVec3, distance: f64, winding: Vec<DVec3>) {
    if winding.len() >= 3 {
        facets.push(Facet {
            normal,
            distance,
            winding,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const EPS: f64 = 1e-6;

    fn box_region() -> RegionPolytope {
        RegionPolytope::from_aabb(&Aabb {
            min: DVec3::ZERO,
            max: DVec3::splat(10.0),
        })
    }

    fn assert_vec3_near(actual: DVec3, expected: DVec3) {
        assert!(
            (actual - expected).abs().max_element() <= EPS,
            "actual {actual:?}, expected {expected:?}"
        );
    }

    fn unique_vertices(polytope: &RegionPolytope) -> Vec<DVec3> {
        let mut vertices = Vec::new();
        for vertex in polytope.vertices() {
            if !vertices
                .iter()
                .any(|existing: &DVec3| (*existing - vertex).length() <= EPS)
            {
                vertices.push(vertex);
            }
        }
        vertices
    }

    fn assert_has_vertex(vertices: &[DVec3], expected: DVec3) {
        assert!(
            vertices
                .iter()
                .any(|vertex| (*vertex - expected).length() <= EPS),
            "missing vertex {expected:?} in {vertices:?}"
        );
    }

    #[test]
    fn aabb_seed_round_trips_extents() {
        let bounds = Aabb {
            min: DVec3::new(-1.0, 2.0, -3.0),
            max: DVec3::new(4.0, 6.0, 8.0),
        };

        let polytope = RegionPolytope::from_aabb(&bounds);
        let vertex_bounds = polytope.vertex_aabb();

        assert!(vertex_bounds.is_valid());
        assert_vec3_near(vertex_bounds.min, bounds.min);
        assert_vec3_near(vertex_bounds.max, bounds.max);
        assert_eq!(unique_vertices(&polytope).len(), 8);
    }

    #[test]
    fn axial_clip_produces_sensible_child_aabbs() {
        let (front, back) = box_region().clip(DVec3::X, 4.0);

        let front_bounds = front.vertex_aabb();
        let back_bounds = back.vertex_aabb();

        assert_vec3_near(front_bounds.min, DVec3::new(4.0, 0.0, 0.0));
        assert_vec3_near(front_bounds.max, DVec3::new(10.0, 10.0, 10.0));
        assert_vec3_near(back_bounds.min, DVec3::new(0.0, 0.0, 0.0));
        assert_vec3_near(back_bounds.max, DVec3::new(4.0, 10.0, 10.0));
        assert!(!front.plane_spans(DVec3::X, 4.0, EPS));
        assert!(front.plane_spans(DVec3::X, 6.0, EPS));
        assert!(back.all_vertices_behind(DVec3::X, 4.0, EPS));
    }

    #[test]
    fn diagonal_clip_produces_vertices_on_both_children() {
        let normal = DVec3::new(1.0, 0.0, -1.0).normalize();
        let (front, back) = box_region().clip(normal, 0.0);

        let front_vertices = unique_vertices(&front);
        let back_vertices = unique_vertices(&back);

        assert_eq!(front_vertices.len(), 6);
        assert_eq!(back_vertices.len(), 6);
        assert!(front_vertices.iter().all(|v| v.x + EPS >= v.z));
        assert!(back_vertices.iter().all(|v| v.x <= v.z + EPS));
        assert_has_vertex(&front_vertices, DVec3::new(10.0, 0.0, 0.0));
        assert_has_vertex(&front_vertices, DVec3::new(10.0, 10.0, 10.0));
        assert_has_vertex(&back_vertices, DVec3::new(0.0, 0.0, 10.0));
        assert_has_vertex(&back_vertices, DVec3::new(10.0, 10.0, 10.0));
    }

    #[test]
    fn wedge_assembly_from_box_has_expected_vertices_and_queries() {
        let normal = DVec3::new(1.0, 0.0, -1.0).normalize();
        let (x_ge_z, x_le_z) = box_region().clip(normal, 0.0);

        let x_ge_z_vertices = unique_vertices(&x_ge_z);
        let x_le_z_vertices = unique_vertices(&x_le_z);
        for expected in [
            DVec3::new(0.0, 0.0, 0.0),
            DVec3::new(10.0, 0.0, 0.0),
            DVec3::new(10.0, 0.0, 10.0),
            DVec3::new(0.0, 10.0, 0.0),
            DVec3::new(10.0, 10.0, 0.0),
            DVec3::new(10.0, 10.0, 10.0),
        ] {
            assert_has_vertex(&x_ge_z_vertices, expected);
        }
        for expected in [
            DVec3::new(0.0, 0.0, 0.0),
            DVec3::new(0.0, 0.0, 10.0),
            DVec3::new(10.0, 0.0, 10.0),
            DVec3::new(0.0, 10.0, 0.0),
            DVec3::new(0.0, 10.0, 10.0),
            DVec3::new(10.0, 10.0, 10.0),
        ] {
            assert_has_vertex(&x_le_z_vertices, expected);
        }

        assert!(x_ge_z.all_vertices_behind(-normal, 0.0, EPS));
        assert!(x_le_z.all_vertices_behind(normal, 0.0, EPS));
        assert!(!x_ge_z.plane_spans(normal, 0.0, EPS));
        assert!(!x_le_z.plane_spans(normal, 0.0, EPS));
        assert!(x_ge_z.plane_spans(DVec3::X, 5.0, EPS));
        assert!(x_le_z.plane_spans(DVec3::Z, 5.0, EPS));
    }

    #[test]
    fn tolerance_contracts_count_on_plane_as_behind_but_not_spanning() {
        let polytope = box_region();

        assert!(polytope.all_vertices_behind(DVec3::X, 10.0, 0.0));
        assert!(!polytope.all_vertices_behind(DVec3::X, 10.0 - 1e-5, 1e-6));
        assert!(!polytope.plane_spans(DVec3::X, 10.0, 0.0));
        assert!(!polytope.plane_spans(DVec3::X, 1e-7, 1e-6));
        assert!(polytope.plane_spans(DVec3::X, 1e-5, 1e-6));
    }

    #[test]
    fn clip_outside_box_returns_empty_side() {
        let (front, back) = box_region().clip(DVec3::X, 20.0);

        assert!(!front.vertex_aabb().is_valid());
        assert!(back.vertex_aabb().is_valid());
        assert_eq!(unique_vertices(&front).len(), 0);
        assert_eq!(unique_vertices(&back).len(), 8);
    }

    #[test]
    fn empty_polytope_queries_are_explicit() {
        let empty = RegionPolytope::empty();

        assert!(!empty.vertex_aabb().is_valid());
        assert!(!empty.all_vertices_behind(DVec3::X, 0.0, 1e-6));
        assert!(!empty.plane_spans(DVec3::X, 0.0, 1e-6));
        let (front, back) = empty.clip(DVec3::X, 0.0);
        assert!(!front.vertex_aabb().is_valid());
        assert!(!back.vertex_aabb().is_valid());
    }

    #[test]
    fn clipping_by_boundary_plane_keeps_reached_side_only() {
        let (front, back) = box_region().clip(DVec3::X, 0.0);

        assert!(front.vertex_aabb().is_valid());
        assert!(!back.vertex_aabb().is_valid());
        assert_eq!(unique_vertices(&front).len(), 8);
        assert_eq!(unique_vertices(&back).len(), 0);
    }
}
