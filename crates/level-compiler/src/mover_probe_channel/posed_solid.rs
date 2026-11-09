// One mover posed at one terminus as solid geometry: its brushes'
// half-spaces, and its sides cut where its other brushes cover them, met
// through the static tracer's own hit test with τ. Every posed trace of the
// channel meets a pose here: the bounce half's gather rays, the direct
// half's shadow segments and the bounce half's patch-direct segments.
// See: context/lib/build_pipeline.md §Compiler pipeline step 12 (Mover probe channel)

use std::sync::Arc;

use glam::{DVec3, Vec3};
use postretro_level_format::geometry::{FaceMeta, GeometrySection, Vertex};
use postretro_level_format::texture_names::TextureNamesSection;

use crate::geometry::{FaceIndexRange, GeometryResult};
use crate::lightmap_bake::RAY_EPSILON;
use crate::map_data::MapKinematicMover;
use crate::mover_light_gate::{BOX_SLACK_M, MoverPoses, Pose, Terminus, point_touches_brush};
use crate::mover_pose::PosedBrush;
use crate::partition::Aabb;
use crate::ray_traversal::{ShadowSegment, TraceRay, triangle_points};

/// Where a ray first meets the pose.
#[derive(Debug, Clone, Copy)]
pub(crate) struct PosedHit {
    /// Distance from the ray's lifted origin, as the static tracer measures
    /// its own hits.
    pub(crate) distance: f32,
    pub(crate) point: Vec3,
    /// The posed triangle hit, as its index-buffer offset in the solid's
    /// geometry; `None` when the lifted origin lies inside a posed brush,
    /// which reads black at distance 0.
    pub(crate) triangle: Option<u32>,
}

/// One posed side's triangles, their bounds for the ray prefilter, and the
/// brush they came from.
struct PosedFace {
    bounds: Aabb,
    /// Index-buffer offsets of its triangles' first indices.
    first: u32,
    end: u32,
    /// Index into the pose's brushes.
    brush: usize,
}

/// The posed mover as the static tracer would see it placed: its brushes as
/// half-spaces, for origins inside them, and its sides as a small geometry
/// of their own. The static scene's BVH is never rebuilt; a posed trace is
/// the static one plus this solid's answer, through the same hit test.
pub(crate) struct PosedSolid {
    pose: Pose,
    faces: Vec<PosedFace>,
    geometry: GeometryResult,
}

impl PosedSolid {
    /// `mover` at `pose`, one of its analysed termini. Texture indices
    /// resolve against `texture_names` read-only: the geometry stage already
    /// appended every mover-only name, so nothing here can add one.
    pub(crate) fn new(
        mover: &MapKinematicMover,
        pose: &Pose,
        texture_names: &TextureNamesSection,
    ) -> Self {
        let pose = pose.clone();
        // `Pose` keeps the valid-bounds brushes in authored order.
        let authored: Vec<_> = mover
            .brush_volumes
            .iter()
            .filter(|brush| brush.aabb.is_valid())
            .collect();
        debug_assert_eq!(authored.len(), pose.brushes.len());
        let mut section = GeometrySection {
            vertices: Vec::new(),
            indices: Vec::new(),
            faces: Vec::new(),
        };
        let mut face_index_ranges = Vec::new();
        let mut face_brushes = Vec::new();
        for (index, (brush, posed)) in authored.iter().zip(&pose.brushes).enumerate() {
            let offset = posed.bounds.min - brush.aabb.min;
            for side in &brush.sides {
                if side.vertices.len() < 3 {
                    continue;
                }
                // The parts of the side no other brush of the mover covers,
                // as a static brush's faces are cut where brushes meet.
                let mut pieces = vec![side.vertices.iter().map(|v| *v + offset).collect()];
                for (other, covering) in pose.brushes.iter().enumerate() {
                    if other != index {
                        pieces = pieces
                            .into_iter()
                            .flat_map(|piece| subtract_brush(piece, side.normal, covering))
                            .collect();
                    }
                }
                push_side(
                    &mut section,
                    &mut face_index_ranges,
                    &pieces,
                    side.normal,
                    texture_index(texture_names, &side.texture),
                );
                face_brushes.push(index);
            }
        }
        let geometry = GeometryResult {
            geometry: section,
            texture_names: TextureNamesSection { names: Vec::new() },
            face_index_ranges,
        };
        let faces = face_bounds(&geometry, &face_brushes);
        Self {
            pose,
            faces,
            geometry,
        }
    }

    pub(crate) fn pose(&self) -> &Pose {
        &self.pose
    }

    pub(crate) fn bounds(&self) -> &Aabb {
        &self.pose.bounds
    }

    /// The posed sides as geometry: what the bounce half's patches cover.
    pub(crate) fn geometry(&self) -> &GeometryResult {
        &self.geometry
    }

    /// Distance from `point` to the pose's bounds, zero inside them.
    pub(crate) fn distance_to_bounds(&self, point: Vec3) -> f64 {
        let point = point.as_dvec3();
        point.distance(point.clamp(self.pose.bounds.min, self.pose.bounds.max))
    }

    /// The posed brushes `point` lies in or on, within the contact band, by
    /// index.
    pub(crate) fn containing(&self, point: Vec3) -> Vec<usize> {
        let point = point.as_dvec3();
        let slack = DVec3::splat(BOX_SLACK_M);
        if (self.pose.bounds.min - slack).cmpgt(point).any()
            || point.cmpgt(self.pose.bounds.max + slack).any()
        {
            return Vec::new();
        }
        self.pose
            .brushes
            .iter()
            .enumerate()
            .filter(|(_, brush)| point_touches_brush(point, brush))
            .map(|(index, _)| index)
            .collect()
    }

    /// Whether `point` lies inside a posed brush or on its surface, by the
    /// brushes' half-spaces alone: a ray lifted from there starts in solid.
    pub(crate) fn contains(&self, point: Vec3) -> bool {
        self.contains_except(point.as_dvec3(), |_| false)
    }

    fn contains_except(&self, point: DVec3, skip: impl Fn(usize) -> bool) -> bool {
        self.pose
            .brushes
            .iter()
            .enumerate()
            .any(|(index, brush)| !skip(index) && in_half_spaces(point, brush))
    }

    /// Whether the unbounded ray from `origin` along `dir` can come near
    /// the pose's bounds: a gather ray that cannot never meets the pose.
    pub(crate) fn may_meet_ray(&self, origin: Vec3, dir: Vec3) -> bool {
        let lifted = origin + dir * RAY_EPSILON;
        ray_may_touch_box(
            lifted.as_dvec3(),
            dir.as_dvec3(),
            f64::INFINITY,
            &self.pose.bounds,
        )
    }

    /// The pose's nearest hit along `dir` from `origin`, nearer than
    /// `nearest` (the static tracer's hit there, or infinity), as the static
    /// tracer finds its own: the origin lifted by [`RAY_EPSILON`], then the
    /// watertight hit test with τ over each posed side's triangles past
    /// `RAY_EPSILON`. A lifted origin inside a posed brush hits at distance 0
    /// and reads black: the inside-brush rule. A posed face flush with a
    /// static one ties and leaves the static hit.
    pub(crate) fn nearest_hit(&self, origin: Vec3, dir: Vec3, nearest: f32) -> Option<PosedHit> {
        let lifted = origin + dir * RAY_EPSILON;
        let (o, d) = (lifted.as_dvec3(), dir.as_dvec3());
        let reach = f64::from(nearest);
        if !ray_may_touch_box(o, d, reach, &self.pose.bounds) {
            return None;
        }
        if self.contains(lifted) {
            return Some(PosedHit {
                distance: 0.0,
                point: lifted,
                triangle: None,
            });
        }
        let ray = TraceRay::new(lifted, dir);
        let mut best: Option<(f32, u32)> = None;
        for face in &self.faces {
            let bound = best.map_or(nearest, |(distance, _)| distance);
            if !ray_may_touch_box(o, d, f64::from(bound), &face.bounds) {
                continue;
            }
            for triangle in (face.first..face.end).step_by(3) {
                let bound = best.map_or(nearest, |(distance, _)| distance);
                let points = triangle_points(&self.geometry, triangle as usize);
                if let Some(hit) = ray.hit(points, RAY_EPSILON, bound) {
                    let distance = hit.t as f32;
                    if distance < bound {
                        best = Some((distance, triangle));
                    }
                }
            }
        }
        best.map(|(distance, triangle)| PosedHit {
            distance,
            point: lifted + dir * distance,
            triangle: Some(triangle),
        })
    }

    /// Whether the shadow segment `from → to` meets the pose, as the static
    /// tracer's [`crate::ray_traversal::segment_clear`] meets the same
    /// brushes placed statically: [`ShadowSegment`] over the posed sides'
    /// triangles, so the same lift, far bound, hit test, τ and grazing-ray
    /// rule. A segment whose lifted start lies inside a posed brush starts
    /// in solid and is blocked: the inside-brush rule, as
    /// [`Self::nearest_hit`] reads such a ray at distance 0. `skip(brush)`
    /// leaves that brush out, its half-spaces and its sides.
    pub(crate) fn segment_blocked(
        &self,
        from: Vec3,
        to: Vec3,
        skip: impl Fn(usize) -> bool,
    ) -> bool {
        let Some(segment) = ShadowSegment::new(from, to) else {
            return false;
        };
        let ray = segment.ray();
        let (o, d) = (ray.origin().as_dvec3(), ray.direction().as_dvec3());
        let reach = f64::from(segment.max_distance());
        if !ray_may_touch_box(o, d, reach, &self.pose.bounds) {
            return false;
        }
        if self.contains_except(o, &skip) {
            return true;
        }
        self.faces.iter().any(|face| {
            !skip(face.brush)
                && ray_may_touch_box(o, d, reach, &face.bounds)
                && (face.first..face.end).step_by(3).any(|triangle| {
                    segment.meets(triangle_points(&self.geometry, triangle as usize))
                })
        })
    }

    /// Fold what the pose occludes with: its bounds, each brush's bounds and
    /// planes (the inside test), and its sides' triangles (the hit test).
    /// Texture indices are not folded: they shade, never occlude.
    pub(crate) fn fold_key(&self, hasher: &mut blake3::Hasher) {
        fold_aabb(hasher, &self.pose.bounds);
        hasher.update(&(self.pose.brushes.len() as u32).to_le_bytes());
        for brush in &self.pose.brushes {
            fold_aabb(hasher, &brush.bounds);
            hasher.update(&(brush.planes.len() as u32).to_le_bytes());
            for &(normal, distance) in &brush.planes {
                for component in normal.to_array() {
                    hasher.update(&component.to_bits().to_le_bytes());
                }
                hasher.update(&distance.to_bits().to_le_bytes());
            }
        }
        let geometry = &self.geometry.geometry;
        hasher.update(&(geometry.vertices.len() as u64).to_le_bytes());
        for vertex in &geometry.vertices {
            for component in vertex.position {
                hasher.update(&component.to_bits().to_le_bytes());
            }
        }
        hasher.update(&(geometry.indices.len() as u64).to_le_bytes());
        for index in &geometry.indices {
            hasher.update(&index.to_le_bytes());
        }
        hasher.update(&(self.geometry.face_index_ranges.len() as u64).to_le_bytes());
        for range in &self.geometry.face_index_ranges {
            hasher.update(&range.index_offset.to_le_bytes());
            hasher.update(&range.index_count.to_le_bytes());
        }
    }

    /// Heap bytes the solid's geometry and face bounds hold.
    pub(crate) fn heap_bytes(&self) -> u64 {
        let geometry = &self.geometry.geometry;
        (geometry.vertices.capacity() * size_of::<Vertex>()
            + geometry.indices.capacity() * size_of::<u32>()
            + geometry.faces.capacity() * size_of::<FaceMeta>()
            + self.geometry.face_index_ranges.capacity() * size_of::<FaceIndexRange>()
            + self.faces.capacity() * size_of::<PosedFace>()) as u64
    }
}

/// One analysed mover's solids, by [`Terminus::index`].
pub(crate) struct PosedMover {
    pub(crate) mover_id: u32,
    pub(crate) solids: [Arc<PosedSolid>; 2],
}

/// Every analysed mover posed at both termini as solids, ascending by id:
/// each pose built once and shared by every trace that meets it.
pub(crate) struct PosedMovers(Vec<PosedMover>);

impl PosedMovers {
    /// Each of `poses`' analysed movers, from its map mover's sides. An
    /// analysed mover with no map mover is an error.
    pub(crate) fn resolve(
        poses: &MoverPoses,
        movers: &[MapKinematicMover],
        texture_names: &TextureNamesSection,
    ) -> anyhow::Result<Self> {
        poses
            .analysed
            .iter()
            .map(|analysed| {
                let mover = movers
                    .iter()
                    .find(|mover| mover.mover_id == analysed.mover_id)
                    .ok_or_else(|| {
                        anyhow::anyhow!("analysed mover #{} is not a map mover", analysed.mover_id)
                    })?;
                Ok(PosedMover {
                    mover_id: analysed.mover_id,
                    solids: Terminus::BOTH.map(|terminus| {
                        Arc::new(PosedSolid::new(
                            mover,
                            analysed.pose(terminus),
                            texture_names,
                        ))
                    }),
                })
            })
            .collect::<anyhow::Result<_>>()
            .map(Self)
    }

    pub(crate) fn movers(&self) -> &[PosedMover] {
        &self.0
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

/// Whether `point` lies inside `brush` or on its surface, with no band.
fn in_half_spaces(point: DVec3, brush: &PosedBrush) -> bool {
    brush
        .planes
        .iter()
        .all(|&(normal, distance)| normal.dot(point) - distance <= 0.0)
}

fn fold_aabb(hasher: &mut blake3::Hasher, bounds: &Aabb) {
    for component in bounds
        .min
        .to_array()
        .into_iter()
        .chain(bounds.max.to_array())
    {
        hasher.update(&component.to_bits().to_le_bytes());
    }
}

/// Each posed side's triangle range, bounds and brush, in face order.
fn face_bounds(geometry: &GeometryResult, brushes: &[usize]) -> Vec<PosedFace> {
    geometry
        .face_index_ranges
        .iter()
        .zip(brushes)
        .map(|(range, &brush)| {
            let (first, end) = (range.index_offset, range.index_offset + range.index_count);
            let mut bounds = Aabb {
                min: DVec3::splat(f64::INFINITY),
                max: DVec3::splat(f64::NEG_INFINITY),
            };
            for &index in &geometry.geometry.indices[first as usize..end as usize] {
                let point = Vec3::from(geometry.geometry.vertices[index as usize].position);
                bounds.min = bounds.min.min(point.as_dvec3());
                bounds.max = bounds.max.max(point.as_dvec3());
            }
            PosedFace {
                bounds,
                first,
                end,
                brush,
            }
        })
        .filter(|face| face.first < face.end)
        .collect()
}

/// Slab test of the ray `o + t·d`, `0 ≤ t ≤ max`, against `bounds` grown by
/// the gate's box slack, which exceeds τ, so a triangle hit the tolerance
/// accepts is never cut.
fn ray_may_touch_box(o: DVec3, d: DVec3, max: f64, bounds: &Aabb) -> bool {
    let slack = DVec3::splat(BOX_SLACK_M);
    let (min_b, max_b) = (bounds.min - slack, bounds.max + slack);
    let (mut enter, mut exit) = (0.0_f64, max);
    for axis in 0..3 {
        if d[axis] == 0.0 {
            if o[axis] < min_b[axis] || o[axis] > max_b[axis] {
                return false;
            }
            continue;
        }
        let near = (min_b[axis] - o[axis]) / d[axis];
        let far = (max_b[axis] - o[axis]) / d[axis];
        enter = enter.max(near.min(far));
        exit = exit.min(near.max(far));
        if enter > exit {
            return false;
        }
    }
    true
}

fn texture_index(names: &TextureNamesSection, texture: &str) -> u32 {
    names
        .names
        .iter()
        .position(|name| name == texture)
        .map_or(u32::MAX, |index| index as u32)
}

/// How far a polygon vertex may sit off a covering brush's plane and still
/// lie on it.
const CUT_EPSILON_M: f64 = 1.0e-4;

/// The parts of convex `polygon` (front normal `normal`) outside convex
/// `brush`, as convex pieces: each plane of the brush splits off the part in
/// front of it, and the remainder inside every plane is dropped. A polygon
/// lying on a brush face that faces it is covered; one on a face turned its
/// own way is not.
fn subtract_brush(polygon: Vec<DVec3>, normal: DVec3, brush: &PosedBrush) -> Vec<Vec<DVec3>> {
    let mut inside = polygon;
    let mut pieces = Vec::new();
    for &(plane, distance) in &brush.planes {
        if inside
            .iter()
            .all(|v| (plane.dot(*v) - distance).abs() <= CUT_EPSILON_M)
        {
            if plane.dot(normal) > 0.0 {
                pieces.push(inside);
                return pieces;
            }
            continue;
        }
        let (front, back) =
            crate::geometry_utils::split_polygon(&inside, plane, distance, CUT_EPSILON_M);
        pieces.extend(front);
        match back {
            Some(back) => inside = back,
            None => return pieces,
        }
    }
    pieces
}

/// Append one posed side's surviving pieces as one face of fans, each wound
/// so its Newell normal is the side's outward normal: a ray from outside
/// reads the patch's front. A side every other brush covers has no
/// triangles, so no patch.
fn push_side(
    section: &mut GeometrySection,
    ranges: &mut Vec<FaceIndexRange>,
    pieces: &[Vec<DVec3>],
    outward: DVec3,
    texture_index: u32,
) {
    let index_offset = section.indices.len() as u32;
    let normal = outward.as_vec3().to_array();
    for piece in pieces {
        let mut points = piece.clone();
        let newell: DVec3 = (1..points.len() - 1)
            .map(|i| (points[i] - points[0]).cross(points[i + 1] - points[0]))
            .sum();
        if newell.dot(outward) < 0.0 {
            points.reverse();
        }
        let base = section.vertices.len() as u32;
        for point in &points {
            section.vertices.push(Vertex::new(
                point.as_vec3().to_array(),
                [0.0, 0.0],
                normal,
                [1.0, 0.0, 0.0],
                true,
                [0.0, 0.0],
                0,
            ));
        }
        for i in 1..points.len() as u32 - 1 {
            section.indices.extend([base, base + i, base + i + 1]);
        }
    }
    ranges.push(FaceIndexRange {
        index_offset,
        index_count: section.indices.len() as u32 - index_offset,
    });
    section.faces.push(FaceMeta {
        leaf_index: 0,
        texture_index,
    });
}

/// A convex brush bounded by `planes` (outward normal, distance), with the
/// sides a parsed brush carries: each plane's polygon clipped by the rest.
#[cfg(test)]
pub(crate) fn brush_with_sides(planes: &[(DVec3, f64)]) -> crate::map_data::BrushVolume {
    use crate::geometry_utils::{clip_winding_to_half_spaces, make_base_winding};
    use crate::map_data::{BrushPlane, BrushSide, BrushVolume, TextureProjection};

    let mut aabb = Aabb::empty();
    let sides: Vec<BrushSide> = planes
        .iter()
        .enumerate()
        .filter_map(|(index, &(normal, distance))| {
            let others: Vec<(DVec3, f64)> = planes
                .iter()
                .enumerate()
                .filter(|&(other, _)| other != index)
                .map(|(_, &(n, d))| (-n, -d))
                .collect();
            let vertices =
                clip_winding_to_half_spaces(make_base_winding(normal, distance), &others, 1.0e-9)?;
            Some(BrushSide {
                vertices,
                normal,
                distance,
                texture: String::new(),
                tex_projection: TextureProjection::default(),
            })
        })
        .collect();
    for vertex in sides.iter().flat_map(|side| &side.vertices) {
        aabb.expand_point(*vertex);
    }
    BrushVolume {
        planes: planes
            .iter()
            .map(|&(normal, distance)| BrushPlane { normal, distance })
            .collect(),
        sides,
        aabb,
    }
}

/// An axis-aligned box brush, sides included.
#[cfg(test)]
pub(crate) fn box_brush(min: DVec3, max: DVec3) -> crate::map_data::BrushVolume {
    brush_with_sides(&[
        (DVec3::X, max.x),
        (DVec3::NEG_X, -min.x),
        (DVec3::Y, max.y),
        (DVec3::NEG_Y, -min.y),
        (DVec3::Z, max.z),
        (DVec3::NEG_Z, -min.z),
    ])
}
