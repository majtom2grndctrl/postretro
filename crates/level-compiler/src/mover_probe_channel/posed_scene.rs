// One mover posed at one terminus as solid geometry for the SH base's rays:
// its sides traced through the static tracer's own hit test, the brushes'
// half-spaces for origins inside them, and patches on its sides.
// See: context/lib/build_pipeline.md §Compiler pipeline step 12 (Mover probe channel)

use glam::{DVec3, Vec3};
use postretro_level_format::geometry::{FaceMeta, GeometrySection, Vertex};
use postretro_level_format::texture_names::TextureNamesSection;

use super::InsideBrushRule;
use crate::geometry::{FaceIndexRange, GeometryResult};
use crate::lightmap_bake::{RAY_EPSILON, emitter_radius, hard_ray_target};
use crate::map_data::{LightType, MapKinematicMover, MapLight};
use crate::mover_light_gate::{
    AnalysedMover, Pose, Terminus, point_touches_brush, segment_may_touch_box,
};
use crate::mover_pose::PosedBrush;
use crate::partition::Aabb;
use crate::ray_traversal::{TraceRay, triangle_points};
use crate::surface_radiance::{PatchCountExceeded, PatchSet};

/// Whether posed sides become patches that reflect their albedo, or read
/// black: the owner's lean (patches) and its A/B alternative.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PosedSurfaces {
    Patches,
    Black,
}

/// Where a ray first meets the pose.
#[derive(Debug, Clone, Copy)]
pub(crate) struct PosedHit {
    /// Distance from the ray's lifted origin, as the static tracer measures
    /// its own hits.
    pub(crate) distance: f32,
    pub(crate) point: Vec3,
    /// The posed triangle hit, as its index-buffer offset in
    /// [`PosedScene`]'s geometry; `None` when the lifted origin lies inside
    /// a posed brush, which reads black at distance 0.
    pub(crate) triangle: Option<u32>,
}

/// One posed side's triangles and their bounds, for the ray prefilter.
struct PosedFace {
    bounds: Aabb,
    /// Index-buffer offsets of its triangles' first indices.
    first: u32,
    end: u32,
}

/// The posed mover: its brushes as half-space solids, and its sides as a
/// small geometry of their own with a patch set over it. The static scene's
/// BVH is never rebuilt; a ray's nearest hit is the nearer of the static
/// tree's and this scene's own, which runs the static tracer's hit test,
/// tolerance τ included, over the posed sides' triangles.
pub(crate) struct PosedScene {
    pub(crate) mover_id: u32,
    pub(crate) terminus: Terminus,
    pub(crate) label: String,
    pub(crate) rule: InsideBrushRule,
    pub(crate) surfaces: PosedSurfaces,
    pose: Pose,
    faces: Vec<PosedFace>,
    pub(crate) geometry: GeometryResult,
    pub(crate) patches: PatchSet,
}

impl PosedScene {
    /// `mover` posed at `terminus` of its analysis. Texture indices resolve
    /// against `texture_names` read-only: the geometry stage already
    /// appended every mover-only name, so nothing here can add one.
    pub(crate) fn new(
        mover: &MapKinematicMover,
        analysed: &AnalysedMover,
        terminus: Terminus,
        texture_names: &TextureNamesSection,
        rule: InsideBrushRule,
        surfaces: PosedSurfaces,
    ) -> Result<Self, PatchCountExceeded> {
        let pose = analysed.pose(terminus).clone();
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
            }
        }
        let geometry = GeometryResult {
            geometry: section,
            texture_names: TextureNamesSection { names: Vec::new() },
            face_index_ranges,
        };
        let patches = PatchSet::try_build(&geometry)?;
        let faces = face_bounds(&geometry);
        Ok(Self {
            mover_id: analysed.mover_id,
            terminus,
            label: analysed.label.clone(),
            rule,
            surfaces,
            pose,
            faces,
            geometry,
            patches,
        })
    }

    /// Distance from `point` to the pose's bounds, zero inside them.
    pub(crate) fn distance_to_bounds(&self, point: Vec3) -> f64 {
        let point = point.as_dvec3();
        point.distance(point.clamp(self.pose.bounds.min, self.pose.bounds.max))
    }

    /// Whether the closed segment touches a posed brush: the posed half of
    /// a patch's shadow ray, contact within the gate's band. The direct
    /// half's q grows the band to the static tracer's τ
    /// (`visibility.rs`, `SHADOW_SEGMENT_BAND_M`).
    pub(crate) fn segment_blocked(&self, from: Vec3, to: Vec3) -> bool {
        self.pose.segment_touches(from.as_dvec3(), to.as_dvec3())
    }

    /// The posed brushes `point` lies in or on, by index.
    pub(crate) fn containing(&self, point: Vec3) -> Vec<usize> {
        let point = point.as_dvec3();
        let slack = DVec3::splat(crate::mover_light_gate::BOX_SLACK_M);
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

    /// Whether any soft-visibility sample segment from `point` (lifted
    /// along its normal by at most [`RAY_EPSILON`]) toward `light` can meet
    /// the pose. Every target lies within a ball about one centre: the
    /// emitter sphere of a point or spot, the cone's far cap of a sun; so
    /// every segment lies within that radius of the centre segment.
    pub(crate) fn may_shadow(&self, point: Vec3, light: &MapLight) -> bool {
        let radius = f64::from(emitter_radius(light).max(0.0));
        let (centre, reach) = match light.light_type {
            LightType::Point | LightType::Spot => (light.origin, radius),
            LightType::Directional => {
                let centre = hard_ray_target(point, light).as_dvec3();
                let length = centre.distance(point.as_dvec3());
                // A chord of the cone's cap is at most its arc.
                (centre, length * (radius.to_radians() * 0.5) * 1.01 + 1.0e-3)
            }
        };
        let grow = DVec3::splat(reach + f64::from(RAY_EPSILON));
        let grown = Aabb {
            min: self.pose.bounds.min - grow,
            max: self.pose.bounds.max + grow,
        };
        segment_may_touch_box(point.as_dvec3(), centre, &grown)
    }

    /// Whether `light` can shade anything whose shadow segments meet the
    /// pose: a point or spot whose range ball (or emitter sphere) misses
    /// the pose's bounds cannot, since every segment it shades along lies
    /// inside that ball.
    pub(crate) fn light_may_reach(&self, light: &MapLight) -> bool {
        match light.light_type {
            LightType::Directional => true,
            LightType::Point | LightType::Spot => {
                let reach = f64::from(light.falloff_range.max(emitter_radius(light)).max(0.0))
                    + f64::from(RAY_EPSILON)
                    + crate::mover_light_gate::BOX_SLACK_M;
                let origin = light.origin;
                !origin.is_finite()
                    || origin.distance(origin.clamp(self.pose.bounds.min, self.pose.bounds.max))
                        <= reach
            }
        }
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

    /// Whether `point` lies inside a posed brush or on its surface, by the
    /// brushes' half-spaces alone: a ray lifted from there starts in solid.
    pub(crate) fn contains(&self, point: Vec3) -> bool {
        let point = point.as_dvec3();
        self.pose.brushes.iter().any(|brush| {
            brush
                .planes
                .iter()
                .all(|&(normal, distance)| normal.dot(point) - distance <= 0.0)
        })
    }

    /// The pose's nearest hit along `dir` from `origin`, nearer than
    /// `nearest` (the static tracer's hit there, or infinity), as the static
    /// tracer finds its own: the origin lifted by [`RAY_EPSILON`], then the
    /// watertight hit test with τ over each posed side's triangles past
    /// `RAY_EPSILON`. A lifted origin inside a posed brush hits at distance 0
    /// and reads black: the inside-brush rule. A posed face flush with a
    /// static one ties and leaves the static hit.
    pub(crate) fn trace(&self, origin: Vec3, dir: Vec3, nearest: f32) -> Option<PosedHit> {
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

    /// Fold what the pose bakes from: its bounds, each brush's bounds and
    /// planes (the inside test and the shadow segments), and its sides'
    /// geometry (the posed trace and the patches).
    pub(crate) fn fold_key(&self, hasher: &mut blake3::Hasher) {
        let fold_box = |hasher: &mut blake3::Hasher, bounds: &Aabb| {
            for component in bounds
                .min
                .to_array()
                .into_iter()
                .chain(bounds.max.to_array())
            {
                hasher.update(&component.to_bits().to_le_bytes());
            }
        };
        fold_box(hasher, &self.pose.bounds);
        hasher.update(&(self.pose.brushes.len() as u32).to_le_bytes());
        for brush in &self.pose.brushes {
            fold_box(hasher, &brush.bounds);
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
        for (range, face) in self.geometry.face_index_ranges.iter().zip(&geometry.faces) {
            hasher.update(&range.index_offset.to_le_bytes());
            hasher.update(&range.index_count.to_le_bytes());
            hasher.update(&face.texture_index.to_le_bytes());
        }
    }

    /// Heap bytes the scene holds: its geometry, patch set and face bounds.
    pub(crate) fn heap_bytes(&self) -> u64 {
        let geometry = &self.geometry.geometry;
        (geometry.vertices.capacity() * size_of::<Vertex>()
            + geometry.indices.capacity() * size_of::<u32>()
            + geometry.faces.capacity() * size_of::<FaceMeta>()
            + self.geometry.face_index_ranges.capacity() * size_of::<FaceIndexRange>()
            + self.faces.capacity() * size_of::<PosedFace>()) as u64
            + self.patches.heap_bytes()
    }
}

/// Each posed side's triangle range and bounds, in face order.
fn face_bounds(geometry: &GeometryResult) -> Vec<PosedFace> {
    geometry
        .face_index_ranges
        .iter()
        .map(|range| {
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
            PosedFace { bounds, first, end }
        })
        .filter(|face| face.first < face.end)
        .collect()
}

/// Slab test of the ray `o + t·d`, `0 ≤ t ≤ max`, against `bounds` grown by
/// the gate's box slack, which exceeds τ, so a triangle hit the tolerance
/// accepts is never cut.
fn ray_may_touch_box(o: DVec3, d: DVec3, max: f64, bounds: &Aabb) -> bool {
    let slack = DVec3::splat(crate::mover_light_gate::BOX_SLACK_M);
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
