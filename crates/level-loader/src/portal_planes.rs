// Per-portal supporting planes, oriented toward each portal's front cell.
// See: context/lib/rendering_pipeline.md §2

use glam::Vec3;

use crate::prl::{CellData, LevelWorld, PortalData};

/// Off-plane distance below which a point gives no side evidence.
const SIDE_EVIDENCE_EPSILON: f32 = 1e-3;

/// A portal's supporting plane: `normal · p + dist` is the signed distance of
/// `p`. When `oriented`, the normal points into the front cell's side, so the
/// distance is positive on that side. An unoriented plane keeps its Newell
/// normal; callers that need a side must treat it as unknown.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PortalPlane {
    pub normal: Vec3,
    pub dist: f32,
    pub oriented: bool,
}

impl PortalPlane {
    pub fn signed_distance(&self, point: Vec3) -> f32 {
        self.normal.dot(point) + self.dist
    }
}

impl LevelWorld {
    /// One plane per `portals[i]`. Built on first use from the portal
    /// polygons, the cells' portal refs and cell bounds — data every world
    /// carries, whatever its locator — and kept for the world's lifetime.
    pub fn portal_planes(&self) -> &[PortalPlane] {
        self.portal_planes
            .get_or_init(|| build_portal_planes(&self.cells, &self.cell_portal_refs, &self.portals))
    }
}

/// Orientation evidence is geometric. Cells are convex, so every portal of a
/// cell lies on that cell's side of each of its other portals' planes, and
/// the mean of a cell's portal centroids lies strictly on its side unless all
/// of them are coplanar (a portal's own centroid sits on its plane and adds
/// nothing). Both cells' means are read first; only when neither gives a side
/// are the cell bounds centres tried, since fixtures and degenerate cells may
/// share bounds. Disagreement within a tier, or no evidence, leaves the portal
/// unoriented.
fn build_portal_planes(
    cells: &[CellData],
    cell_portal_refs: &[u32],
    portals: &[PortalData],
) -> Vec<PortalPlane> {
    let centroids: Vec<Vec3> = portals
        .iter()
        .map(|p| polygon_centroid(&p.polygon))
        .collect();
    let cell_portal_means: Vec<Option<Vec3>> = cells
        .iter()
        .map(|cell| {
            let start = cell.portal_ref_start as usize;
            let end = start + cell.portal_ref_count as usize;
            let refs = cell_portal_refs.get(start..end)?;
            let mut sum = Vec3::ZERO;
            let mut count = 0u32;
            for &portal in refs {
                if let Some(&c) = centroids.get(portal as usize) {
                    sum += c;
                    count += 1;
                }
            }
            (count > 0).then(|| sum / count as f32)
        })
        .collect();

    portals
        .iter()
        .zip(&centroids)
        .map(|(portal, &centroid)| {
            let normal = newell_normal(&portal.polygon);
            if normal == Vec3::ZERO {
                return PortalPlane {
                    normal,
                    dist: 0.0,
                    oriented: false,
                };
            }
            let dist = -normal.dot(centroid);
            // +1 / -1: the cell lies on the normal's positive / negative side.
            let side = |point: Option<Vec3>| -> f32 {
                let Some(point) = point else { return 0.0 };
                let s = normal.dot(point) + dist;
                if s.abs() > SIDE_EVIDENCE_EPSILON {
                    s.signum()
                } else {
                    0.0
                }
            };
            let mean = |cell: usize| cell_portal_means.get(cell).copied().flatten();
            let bounds_centre = |cell: usize| {
                cells
                    .get(cell)
                    .map(|data| (data.bounds_min + data.bounds_max) * 0.5)
            };
            let tiers = [
                (side(mean(portal.front_cell)), side(mean(portal.back_cell))),
                (
                    side(bounds_centre(portal.front_cell)),
                    side(bounds_centre(portal.back_cell)),
                ),
            ];
            let mut sign = 0.0;
            for (front, back) in tiers {
                if front == 0.0 && back == 0.0 {
                    continue;
                }
                if front != 0.0 && front == back {
                    break;
                }
                sign = if front != 0.0 { front } else { -back };
                break;
            }
            if sign == 0.0 {
                PortalPlane {
                    normal,
                    dist,
                    oriented: false,
                }
            } else {
                PortalPlane {
                    normal: normal * sign,
                    dist: dist * sign,
                    oriented: true,
                }
            }
        })
        .collect()
}

fn polygon_centroid(polygon: &[Vec3]) -> Vec3 {
    if polygon.is_empty() {
        return Vec3::ZERO;
    }
    polygon.iter().copied().sum::<Vec3>() / polygon.len() as f32
}

/// Newell's method: robust against near-colinear leading vertices. Zero for a
/// degenerate polygon.
fn newell_normal(polygon: &[Vec3]) -> Vec3 {
    let n = polygon.len();
    if n < 3 {
        return Vec3::ZERO;
    }
    let mut normal = Vec3::ZERO;
    for i in 0..n {
        let cur = polygon[i];
        let nxt = polygon[(i + 1) % n];
        normal.x += (cur.y - nxt.y) * (cur.z + nxt.z);
        normal.y += (cur.z - nxt.z) * (cur.x + nxt.x);
        normal.z += (cur.x - nxt.x) * (cur.y + nxt.y);
    }
    if normal.length_squared() < 1e-12 {
        return Vec3::ZERO;
    }
    normal.normalize()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cell(bounds_min: Vec3, bounds_max: Vec3, ref_start: u32, ref_count: u32) -> CellData {
        CellData {
            bounds_min,
            bounds_max,
            face_start: 0,
            face_count: 0,
            portal_ref_start: ref_start,
            portal_ref_count: ref_count,
            is_solid: false,
            is_exterior: false,
            is_drawable: false,
        }
    }

    fn quad_at_x(x: f32, front_cell: usize, back_cell: usize) -> PortalData {
        PortalData {
            polygon: vec![
                Vec3::new(x, -1.0, -1.0),
                Vec3::new(x, 1.0, -1.0),
                Vec3::new(x, 1.0, 1.0),
                Vec3::new(x, -1.0, 1.0),
            ],
            front_cell,
            back_cell,
        }
    }

    #[test]
    fn portal_planes_orient_toward_front_cell_from_neighbouring_portals_under_shared_bounds() {
        // Three cells along X with identical, enclosing bounds: only the
        // other portals carry side evidence. Cell 0 has one portal, so its
        // side comes from cell 1's portal at x = 20.
        let shared = || cell(Vec3::splat(-100.0), Vec3::splat(100.0), 0, 0);
        let mut cells = vec![shared(), shared(), shared()];
        cells[0].portal_ref_count = 1;
        cells[1].portal_ref_start = 1;
        cells[1].portal_ref_count = 2;
        cells[2].portal_ref_start = 3;
        cells[2].portal_ref_count = 1;
        let refs = vec![0, 0, 1, 1];
        // Cell 1 sits between x = 10 and x = 20 and is both portals' front.
        let portals = vec![quad_at_x(10.0, 1, 0), quad_at_x(20.0, 1, 2)];
        let planes = build_portal_planes(&cells, &refs, &portals);

        assert!(planes[0].oriented && planes[1].oriented);
        assert!(planes[0].signed_distance(Vec3::new(15.0, 0.0, 0.0)) > 0.0);
        assert!(planes[1].signed_distance(Vec3::new(15.0, 0.0, 0.0)) > 0.0);
    }

    #[test]
    fn portal_planes_leave_portal_unoriented_without_side_evidence() {
        // Two cells, one portal, both bounds centred on the portal plane.
        let cells = vec![
            cell(Vec3::splat(-1.0), Vec3::splat(1.0), 0, 1),
            cell(Vec3::splat(-1.0), Vec3::splat(1.0), 0, 1),
        ];
        let planes = build_portal_planes(&cells, &[0], &[quad_at_x(0.0, 0, 1)]);
        assert!(!planes[0].oriented);
    }
}
