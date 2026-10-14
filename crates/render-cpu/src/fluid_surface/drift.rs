// Fluid drift: each surface face's world-to-UV map, and the per-vertex scroll
// rates a resolved world drift becomes.
// See: context/lib/rendering_pipeline.md §7.3a

use glam::{Vec2, Vec3};
use postretro_level_format::environment_volumes::{
    EnvironmentSurfaceFace, EnvironmentVolumeRecord,
};

use super::face_normal;

/// A face vertex lies on a map's plane within this distance, meters: the
/// compiler's 1 mm flush tolerance.
const PLANE_TOLERANCE: f32 = 1.0e-3;
/// A face vertex fits a map's UVs within this many texels.
const UV_TOLERANCE_TEXELS: f32 = 0.01;

/// How one drawable surface face turns world motion into UV motion.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FluidFaceDriftMap {
    /// First vertex of the face in [`super::FluidSurfaceMesh::vertices`].
    pub vertex_start: u32,
    pub vertex_count: u32,
    /// Gradients of the normalized U and V over world space, per meter. Both
    /// lie in the face's plane, so a world drift's normal part never scrolls.
    pub uv_gradient: [Vec3; 2],
}

impl FluidFaceDriftMap {
    /// The scroll rate, in texture periods per second, that moves texture
    /// features with world velocity `drift`. The sample point runs against
    /// the motion: a feature at `u0` shows where `uv + scroll = u0`.
    pub fn scroll_rate(&self, drift: Vec3) -> Vec2 {
        -Vec2::new(
            self.uv_gradient[0].dot(drift),
            self.uv_gradient[1].dot(drift),
        )
    }
}

/// A world-to-UV map fitted to one face, shared by the faces that fit it.
struct FaceMap {
    texture_index: u32,
    normal: Vec3,
    distance: f32,
    origin: Vec3,
    origin_uv: Vec2,
    uv_gradient: [Vec3; 2],
}

impl FaceMap {
    /// Fitted to the face's largest-area vertex triangle; `None` when no
    /// triangle spans an area.
    fn fit(
        face: &EnvironmentSurfaceFace,
        normal: Vec3,
        uv: impl Fn(usize) -> Vec2,
    ) -> Option<Self> {
        let position = |i: usize| Vec3::from(face.vertices[i].position);
        let count = face.vertices.len();
        let mut best = (0.0f32, 0, 0, 0);
        for a in 0..count {
            for b in a + 1..count {
                for c in b + 1..count {
                    let area = (position(b) - position(a))
                        .cross(position(c) - position(a))
                        .length_squared();
                    if area > best.0 {
                        best = (area, a, b, c);
                    }
                }
            }
        }
        let (area, a, b, c) = best;
        if !(area > 0.0 && area.is_finite()) {
            return None;
        }
        let (e1, e2) = (position(b) - position(a), position(c) - position(a));
        let n = e1.cross(e2);
        let inverse = 1.0 / n.length_squared();
        let gradient = |d1: f32, d2: f32| (e2.cross(n) * d1 + n.cross(e1) * d2) * inverse;
        let (du1, du2) = (uv(b) - uv(a), uv(c) - uv(a));
        let origin = position(a);
        Some(Self {
            texture_index: face.texture_index,
            normal,
            distance: normal.dot(origin),
            origin,
            origin_uv: uv(a),
            uv_gradient: [gradient(du1.x, du2.x), gradient(du1.y, du2.y)],
        })
    }

    fn predict(&self, position: Vec3) -> Vec2 {
        let delta = position - self.origin;
        self.origin_uv
            + Vec2::new(
                self.uv_gradient[0].dot(delta),
                self.uv_gradient[1].dot(delta),
            )
    }

    /// Whether `face` lies on this map's plane, faces its way and carries its
    /// UVs: a part of the same split brush side.
    fn fits(
        &self,
        face: &EnvironmentSurfaceFace,
        normal: Vec3,
        uv: impl Fn(usize) -> Vec2,
        uv_tolerance: Vec2,
    ) -> bool {
        face.texture_index == self.texture_index
            && normal.dot(self.normal) > 0.0
            && face.vertices.iter().enumerate().all(|(i, vertex)| {
                let position = Vec3::from(vertex.position);
                let off_plane = (self.normal.dot(position) - self.distance).abs();
                let uv_error = (self.predict(position) - uv(i)).abs();
                off_plane <= PLANE_TOLERANCE
                    && uv_error.x <= uv_tolerance.x
                    && uv_error.y <= uv_tolerance.y
            })
    }
}

/// Build every drawable face's drift map for one volume. `vertex_start[i]`
/// is face `i`'s first mesh vertex, or `None` for a face that draws nothing;
/// `uv_of(face, vertex)` is the normalized UV the mesh stores and
/// `uv_scale(face)` its texels per normalized unit.
///
/// A split brush side reaches the runtime as several faces, and a thin part's
/// own vertices fit its UVs poorly. Each face therefore takes the map of the
/// largest face in its volume it fits, so every part of one side scrolls with
/// one map, however thin.
pub(super) fn volume_drift_maps(
    volume: &EnvironmentVolumeRecord,
    vertex_start: &[Option<u32>],
    uv_of: impl Fn(&EnvironmentSurfaceFace, usize) -> Vec2,
    uv_scale: impl Fn(&EnvironmentSurfaceFace) -> Vec2,
) -> Vec<Option<FluidFaceDriftMap>> {
    let faces = &volume.surface_faces;
    let normals: Vec<Option<Vec3>> = faces.iter().map(face_normal).collect();
    let mut order: Vec<usize> = (0..faces.len())
        .filter(|&i| normals[i].is_some() && vertex_start[i].is_some())
        .collect();
    order.sort_by(|&a, &b| face_area(&faces[b]).total_cmp(&face_area(&faces[a])));

    let mut maps: Vec<FaceMap> = Vec::new();
    let mut result = vec![None; faces.len()];
    for index in order {
        let face = &faces[index];
        let normal = normals[index].expect("ordered faces have normals");
        let uv = |vertex: usize| uv_of(face, vertex);
        let tolerance = Vec2::splat(UV_TOLERANCE_TEXELS) / uv_scale(face);
        let fitted = match maps
            .iter()
            .position(|map| map.fits(face, normal, uv, tolerance))
        {
            Some(position) => Some(position),
            None => FaceMap::fit(face, normal, uv).map(|map| {
                maps.push(map);
                maps.len() - 1
            }),
        };
        result[index] = fitted.map(|map| FluidFaceDriftMap {
            vertex_start: vertex_start[index].expect("ordered faces have vertices"),
            vertex_count: face.vertices.len() as u32,
            uv_gradient: maps[map].uv_gradient,
        });
    }
    result
}

fn face_area(face: &EnvironmentSurfaceFace) -> f32 {
    let vertices = &face.vertices;
    let mut twice = Vec3::ZERO;
    for (i, vertex) in vertices.iter().enumerate() {
        let next = &vertices[(i + 1) % vertices.len()];
        twice += Vec3::from(vertex.position).cross(Vec3::from(next.position));
    }
    0.5 * twice.length()
}

/// Per-vertex scroll rates for `faces` (one map per surface face of every
/// volume, in record order, `None` for faces that draw nothing) under
/// `drifts`, one world drift per surface face in the same order. Writes into
/// `rates`, one entry per mesh vertex; vertices of faces without a drift stay
/// still. A `drifts` slice of the wrong length leaves every vertex still.
pub fn fill_drift_rates(
    faces: &[Option<FluidFaceDriftMap>],
    drifts: &[Vec3],
    rates: &mut [[f32; 2]],
) {
    rates.fill([0.0; 2]);
    if drifts.len() != faces.len() {
        return;
    }
    for (map, drift) in faces.iter().zip(drifts) {
        let Some(map) = map else {
            continue;
        };
        let rate = map.scroll_rate(*drift);
        if !rate.is_finite() {
            continue;
        }
        let start = map.vertex_start as usize;
        let end = start + map.vertex_count as usize;
        if let Some(vertices) = rates.get_mut(start..end) {
            vertices.fill(rate.to_array());
        }
    }
}

#[cfg(test)]
#[path = "drift_tests.rs"]
mod tests;
