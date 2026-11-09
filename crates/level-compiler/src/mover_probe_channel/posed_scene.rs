// One mover posed at one terminus as solid geometry for the SH base's rays:
// analytic entry into each convex brush, and patches on its sides.
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
use crate::surface_radiance::{PatchCountExceeded, PatchSet};

/// How far a plane and a brush side may disagree and still be one face.
const SIDE_MATCH_EPSILON: f64 = 1.0e-6;

/// Whether posed sides become patches that reflect their albedo, or read
/// black: the owner's lean (patches) and its A/B alternative.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PosedSurfaces {
    Patches,
    Black,
}

/// Where a ray first meets a posed brush.
#[derive(Debug, Clone, Copy)]
pub(crate) struct PosedHit {
    /// Distance from the ray's origin.
    pub(crate) distance: f32,
    pub(crate) point: Vec3,
    /// The side the ray enters through, as a face of [`PosedScene`]'s
    /// geometry; `None` when the origin lies inside the brush or the
    /// entered side is degenerate. Either reads black.
    pub(crate) face: Option<u32>,
}

/// The posed mover: its brushes as half-space solids, and its sides as a
/// small geometry of their own with a patch set over it. The static scene's
/// BVH is never rebuilt; a ray's nearest hit is the nearer of the static
/// tree's and this scene's analytic entry.
pub(crate) struct PosedScene {
    pub(crate) mover_id: u32,
    pub(crate) terminus: Terminus,
    pub(crate) label: String,
    pub(crate) rule: InsideBrushRule,
    pub(crate) surfaces: PosedSurfaces,
    pose: Pose,
    /// Per posed brush, per plane: the face its side became.
    plane_faces: Vec<Vec<Option<u32>>>,
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
        let mut plane_faces = Vec::with_capacity(authored.len());
        for (index, (brush, posed)) in authored.iter().zip(&pose.brushes).enumerate() {
            let offset = posed.bounds.min - brush.aabb.min;
            let mut faces = vec![None; brush.planes.len()];
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
                let face = section.faces.len() as u32;
                push_side(
                    &mut section,
                    &mut face_index_ranges,
                    &pieces,
                    side.normal,
                    texture_index(texture_names, &side.texture),
                );
                if pieces.is_empty() {
                    continue;
                }
                if let Some(plane) = brush.planes.iter().position(|plane| {
                    plane.normal.dot(side.normal) > 1.0 - SIDE_MATCH_EPSILON
                        && (plane.distance - side.distance).abs() < SIDE_MATCH_EPSILON.sqrt()
                }) {
                    faces[plane] = Some(face);
                }
            }
            plane_faces.push(faces);
        }
        let geometry = GeometryResult {
            geometry: section,
            texture_names: TextureNamesSection { names: Vec::new() },
            face_index_ranges,
        };
        let patches = PatchSet::try_build(&geometry)?;
        Ok(Self {
            mover_id: analysed.mover_id,
            terminus,
            label: analysed.label.clone(),
            rule,
            surfaces,
            pose,
            plane_faces,
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
    /// a shadow ray, contact included, as the direct bake tests it.
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

    /// The nearest entry of the ray `origin + t·dir`, `0 ≤ t ≤ max`, into a
    /// posed brush other than those in `skip`. An origin inside a brush
    /// enters it at 0. `None` when no brush is met by `max`.
    pub(crate) fn ray_entry(
        &self,
        origin: Vec3,
        dir: Vec3,
        max: f32,
        skip: &[usize],
        grow: f64,
    ) -> Option<PosedHit> {
        let (o, d) = (origin.as_dvec3(), dir.as_dvec3());
        // The box tests run in f64 with a hair of slack; the distance test
        // compares the `f32` distance a hit reports, so a caller passing a
        // reported distance back as `max` finds the same hit.
        let reach = f64::from(max) * (1.0 + 1.0e-6) + 1.0e-6;
        if !ray_may_touch_box(o, d, reach, &self.pose.bounds) {
            return None;
        }
        let mut best: Option<(f64, usize, Option<usize>)> = None;
        for (index, brush) in self.pose.brushes.iter().enumerate() {
            if skip.contains(&index) || !ray_may_touch_box(o, d, reach, &brush.bounds) {
                continue;
            }
            let Some((t, plane)) = brush_entry(o, d, brush, grow) else {
                continue;
            };
            if (t as f32) <= max && best.is_none_or(|(nearest, _, _)| t < nearest) {
                best = Some((t, index, plane));
            }
        }
        best.map(|(t, brush, plane)| PosedHit {
            distance: t as f32,
            point: (o + d * t).as_vec3(),
            face: plane.and_then(|plane| self.plane_faces[brush][plane]),
        })
    }

    /// A triangle of face `face`, as the patch set's lookups name a face.
    pub(crate) fn face_triangle(&self, face: u32) -> u32 {
        self.geometry.face_index_ranges[face as usize].index_offset
    }
}

/// The closed-form entry of a ray into one convex brush: the parameter
/// where it crosses into the last half-space it starts outside, and that
/// plane; `(0, None)` from inside. `None` when it never enters.
// TEMPORARY `grow` (task 2, B7 τ experiment): every plane pushed out by
// `grow` metres; 0 outside `POSTRETRO_DEV_POSED_TAU`.
fn brush_entry(o: DVec3, d: DVec3, brush: &PosedBrush, grow: f64) -> Option<(f64, Option<usize>)> {
    let (mut enter, mut exit) = (0.0_f64, f64::INFINITY);
    let mut entered: Option<usize> = None;
    for (index, &(normal, distance)) in brush.planes.iter().enumerate() {
        let start = normal.dot(o) - distance - grow;
        let along = normal.dot(d);
        if along == 0.0 {
            if start > 0.0 {
                return None;
            }
            continue;
        }
        let t = -start / along;
        if along < 0.0 {
            if start > 0.0 && t > enter {
                enter = t;
                entered = Some(index);
            }
        } else {
            exit = exit.min(t);
        }
        if enter > exit {
            return None;
        }
    }
    Some((enter, entered))
}

/// Slab test of the ray `o + t·d`, `0 ≤ t ≤ max`, against `bounds` grown by
/// the gate's box slack.
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
