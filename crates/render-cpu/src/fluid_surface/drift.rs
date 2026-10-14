// Fluid drift: each surface face's world-to-UV map, and the per-vertex scroll
// rates a resolved world drift becomes.
// See: context/lib/rendering_pipeline.md §7.3a

use glam::{DVec2, DVec3, Vec2, Vec3};
use postretro_level_format::environment_volumes::{
    EnvironmentSurfaceFace, EnvironmentSurfaceVertex, EnvironmentVolumeRecord,
};

use super::face_normal;

/// A face vertex lies on a map's plane within this distance, meters, plus its
/// rounding allowance: the compiler's 1 mm flush tolerance, far above f32
/// position rounding (about 0.03 mm at 500 m).
const PLANE_TOLERANCE: f64 = 1.0e-3;
/// A face vertex fits a map's UVs within this many texels, plus its rounding
/// allowance.
const UV_TOLERANCE_TEXELS: f64 = 0.01;
/// Multiples of the inputs' f32 rounding a fit allows beyond the fixed
/// tolerances. Generous on purpose: sharing a map between coplanar faces of
/// one texture and facing whose UVs nearly agree is harmless, while splitting
/// one brush side slides its parts' textures apart for good.
const ROUNDING_SLACK: f64 = 8.0;

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

/// The f32 rounding steps of one face's stored inputs. The compiler rounds
/// each position and texel UV to f32 separately, and texel UVs are not
/// wrapped, so both steps grow with distance from the origin.
#[derive(Clone, Copy)]
struct Rounding {
    /// Step of the largest position component, meters.
    position: f64,
    /// Step of the largest normalized U and V. Two roundings: the stored
    /// texel UV, then its normalization.
    uv: DVec2,
}

impl Rounding {
    fn of(face: &EnvironmentSurfaceFace, uv: impl Fn(usize) -> Vec2) -> Self {
        let step = f64::from(f32::EPSILON);
        let mut position = 0.0f64;
        let mut largest_uv = DVec2::ZERO;
        for (i, vertex) in face.vertices.iter().enumerate() {
            position = position.max(f64::from(Vec3::from(vertex.position).abs().max_element()));
            largest_uv = largest_uv.max(uv(i).as_dvec2().abs());
        }
        Self {
            position: position * step,
            uv: largest_uv * (2.0 * step),
        }
    }
}

/// A world-to-UV map fitted to one face, shared by the faces that fit it.
/// Fitted and evaluated in f64, so only the f32 inputs' rounding remains.
struct FaceMap {
    texture_index: u32,
    /// Unit normal of the fitted triangle.
    normal: DVec3,
    origin: DVec3,
    origin_uv: DVec2,
    uv_gradient: [DVec3; 2],
    /// Rounding of the face the map was fitted to.
    rounding: Rounding,
    /// Inverse of the fitted triangle's shortest altitude, per meter: how
    /// fast its vertices' rounding grows with distance from `origin`.
    spread: f64,
}

impl FaceMap {
    /// Fitted to the face's largest-area vertex triangle; `None` when no
    /// triangle spans an area.
    fn fit(
        face: &EnvironmentSurfaceFace,
        uv: impl Fn(usize) -> Vec2,
        rounding: Rounding,
    ) -> Option<Self> {
        let position = |i: usize| world_position(&face.vertices[i]);
        let count = face.vertices.len();
        let mut best = (0.0f64, 0, 0, 0);
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
        let twice_area = n.length();
        let inverse = 1.0 / n.length_squared();
        let gradient = |d1: f64, d2: f64| (e2.cross(n) * d1 + n.cross(e1) * d2) * inverse;
        let uv = |i: usize| uv(i).as_dvec2();
        let (du1, du2) = (uv(b) - uv(a), uv(c) - uv(a));
        let longest_edge = e1.length().max(e2.length()).max((e2 - e1).length());
        Some(Self {
            texture_index: face.texture_index,
            normal: n / twice_area,
            origin: position(a),
            origin_uv: uv(a),
            uv_gradient: [gradient(du1.x, du2.x), gradient(du1.y, du2.y)],
            rounding,
            spread: longest_edge / twice_area,
        })
    }

    fn predict(&self, position: DVec3) -> DVec2 {
        let delta = position - self.origin;
        self.origin_uv
            + DVec2::new(
                self.uv_gradient[0].dot(delta),
                self.uv_gradient[1].dot(delta),
            )
    }

    /// Whether `face` lies on this map's plane, faces its way and carries its
    /// UVs: a part of the same split brush side. `uv_tolerance` is the fixed
    /// UV tolerance in normalized units; each vertex also gets slack for both
    /// faces' rounding, the map's growing with distance from its triangle.
    fn fits(
        &self,
        face: &EnvironmentSurfaceFace,
        normal: Vec3,
        uv: impl Fn(usize) -> Vec2,
        rounding: Rounding,
        uv_tolerance: DVec2,
    ) -> bool {
        let slope = DVec2::new(self.uv_gradient[0].length(), self.uv_gradient[1].length());
        // A vertex's UV error under this map from one face's rounding: its
        // stored UV's, plus its position's carried through the gradient.
        let uv_rounding = |r: Rounding| r.uv + slope * r.position;
        let (face_uv, map_uv) = (uv_rounding(rounding), uv_rounding(self.rounding));
        face.texture_index == self.texture_index
            && normal.as_dvec3().dot(self.normal) > 0.0
            && face.vertices.iter().enumerate().all(|(i, vertex)| {
                let position = world_position(vertex);
                let reach = 1.0 + self.spread * (position - self.origin).length();
                let off_plane = self.normal.dot(position - self.origin).abs();
                let plane_tolerance = PLANE_TOLERANCE
                    + ROUNDING_SLACK * (rounding.position + self.rounding.position * reach);
                let uv_error = (self.predict(position) - uv(i).as_dvec2()).abs();
                let uv_limit = uv_tolerance + ROUNDING_SLACK * (face_uv + map_uv * reach);
                off_plane <= plane_tolerance && uv_error.cmple(uv_limit).all()
            })
    }
}

fn world_position(vertex: &EnvironmentSurfaceVertex) -> DVec3 {
    Vec3::from(vertex.position).as_dvec3()
}

/// Build every drawable face's drift map for one volume. `vertex_start[i]`
/// is face `i`'s first mesh vertex, or `None` for a face that draws nothing;
/// `uv_of(face, vertex)` is the normalized UV the mesh stores and
/// `uv_scale(face)` its texels per normalized unit.
///
/// A split brush side reaches the runtime as several faces, and a thin part's
/// own vertices fit its UVs poorly. Each face therefore takes the map of the
/// largest face in its volume it fits, so every part of one side scrolls with
/// one map, however thin. Fit tolerances grow with the inputs' rounding, so a
/// part far from the origin still fits its side's map.
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
        let rounding = Rounding::of(face, uv);
        let tolerance = DVec2::splat(UV_TOLERANCE_TEXELS) / uv_scale(face).as_dvec2();
        let fitted = match maps
            .iter()
            .position(|map| map.fits(face, normal, uv, rounding, tolerance))
        {
            Some(position) => Some(position),
            None => FaceMap::fit(face, uv, rounding).map(|map| {
                maps.push(map);
                maps.len() - 1
            }),
        };
        result[index] = fitted.map(|map| FluidFaceDriftMap {
            vertex_start: vertex_start[index].expect("ordered faces have vertices"),
            vertex_count: face.vertices.len() as u32,
            uv_gradient: maps[map].uv_gradient.map(DVec3::as_vec3),
        });
    }
    result
}

/// Polygon area, fanned from the first vertex so it holds far from the origin.
fn face_area(face: &EnvironmentSurfaceFace) -> f64 {
    let Some(first) = face.vertices.first() else {
        return 0.0;
    };
    let origin = world_position(first);
    let twice: DVec3 = face.vertices[1..]
        .windows(2)
        .map(|pair| (world_position(&pair[0]) - origin).cross(world_position(&pair[1]) - origin))
        .sum();
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
