// Reflection-probe placement: reflector faces, authored and derived probes, drops, binding and the cap.
// See: context/lib/resource_management.md §5.1, §5.4

use glam::{DVec2, DVec3, Vec3};
use postretro_level_format::environment_volumes::EnvironmentVolumesSection;
use postretro_level_format::reflection_probes::{
    ReflectionProbeFluidBinding, ReflectionProbeWorldBinding,
};

use crate::governor::Governor;
use crate::map_data::MapReflectionVolume;

use super::authored::place_authored;
use super::box_fit::WorldBounds;
use super::capture_point::PointClassifier;
use super::derived::derive_probes;
use super::facing::Facing;
use super::hit::SceneTracer;
use cap::{
    apply_probe_cap, binds_water, refuse_authored_water_past_cap, retain, world_probes_kept,
};

// The cap reads placement's probe records alone; a child keeps this file to
// faces, groups and binding.
#[path = "placement_cap.rs"]
mod cap;

/// A face is up-facing when its outward normal is within 1° of up. Water is
/// authored flat; a brush top a degree off still mirrors the room with at most
/// a 2° dip below the horizon, which the shader's horizon clamp absorbs, while
/// a visibly sloped face would reflect the box from the wrong plane.
pub(crate) const UP_FACING_MIN_COS: f64 = 0.999_847_695_156_391_3;
/// The capture point and fit origins sit this far off the reflector plane,
/// along its normal, so lookups stay in the face's hemisphere and the point
/// stays out of the fluid or the face's brush.
pub(crate) const CAPTURE_LIFT_M: f64 = 0.002;
/// Faces whose planes differ by less than this share a plane.
pub(crate) const PLANE_TOLERANCE_M: f64 = 0.001;
/// Box volumes and bound areas within this fraction of each other are tied,
/// and the tie rules (earlier box in map entity order; earlier fluid entity,
/// then face) decide. Both come from f32 engine coordinates, map units scaled
/// by 0.0254, so boxes or pools a mapper drew the same size almost never
/// compare exactly equal: an extent's rounding is a few f32 steps of the
/// coordinate, about 1e-6 of a 3 m extent at 100 m from the origin, and a
/// volume sums three of them.
pub(crate) const TIE_RELATIVE_TOLERANCE: f64 = 1.0e-5;

/// Whether `a` and `b` are equal within [`TIE_RELATIVE_TOLERANCE`].
pub(crate) fn tied(a: f64, b: f64) -> bool {
    (a - b).abs() <= TIE_RELATIVE_TOLERANCE * a.abs().max(b.abs())
}

/// Area-weighted geometry of one planar surface polygon.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct FaceGeometry {
    /// Unit Newell normal, its winding's: outward from a fluid, the front of
    /// a world face.
    pub normal: DVec3,
    pub area: f64,
    /// Area centroid: a collinear vertex adds a zero-area fan triangle, so
    /// the T-junction vertices the compiler inserts never move it.
    pub centroid: DVec3,
}

impl FaceGeometry {
    pub(crate) fn from_polygon(points: &[DVec3]) -> Option<Self> {
        let first = *points.first()?;
        let mut newell = DVec3::ZERO;
        for (i, &p) in points.iter().enumerate() {
            let q = points[(i + 1) % points.len()];
            newell += (p - first).cross(q - first);
        }
        let area = newell.length() * 0.5;
        if area <= f64::EPSILON {
            return None;
        }
        let normal = newell / (2.0 * area);
        let mut weighted = DVec3::ZERO;
        let mut weight = 0.0;
        for pair in points[1..].windows(2) {
            let (a, b) = (pair[0] - first, pair[1] - first);
            let tri_area = a.cross(b).dot(normal) * 0.5;
            weighted += tri_area * (a + b) / 3.0;
            weight += tri_area;
        }
        Some(Self {
            normal,
            area,
            centroid: first + weighted / weight,
        })
    }

    pub(crate) fn is_up_facing(&self) -> bool {
        self.normal.y >= UP_FACING_MIN_COS
    }
}

/// Where a reflector face comes from: the numbering its binding uses.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum ReflectorSource {
    /// A fluid surface face, in EnvironmentVolumes (id 52) numbering.
    Fluid { volume: u32, face: u32 },
    /// A static world face whose material reflects, in Geometry (id 17)
    /// numbering.
    World { face: u32 },
}

/// A face a probe can bind: an up-facing fluid face, or a glossy world face
/// of any facing.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ReflectorFace {
    pub source: ReflectorSource,
    pub geometry: FaceGeometry,
    /// Which way it looks: always up for water.
    pub facing: Facing,
    /// The polygon in `facing`'s plane coordinates: seen from above, `(x, z)`
    /// per vertex, when up.
    pub outline: Vec<DVec2>,
    /// The polygon's world bounds.
    pub min: DVec3,
    pub max: DVec3,
}

impl ReflectorFace {
    /// A face of `points` (its polygon in winding order) looking along
    /// `facing`.
    pub(crate) fn new(
        source: ReflectorSource,
        geometry: FaceGeometry,
        facing: Facing,
        points: &[DVec3],
    ) -> Self {
        let (min, max) = points
            .iter()
            .fold((DVec3::INFINITY, DVec3::NEG_INFINITY), |(lo, hi), &p| {
                (lo.min(p), hi.max(p))
            });
        Self {
            source,
            geometry,
            facing,
            outline: points.iter().map(|&p| facing.project(p)).collect(),
            min,
            max,
        }
    }

    /// The height of its plane along its facing's normal, at its area
    /// centroid: `y` when up.
    pub(crate) fn plane_height(&self) -> f64 {
        self.facing.height(self.geometry.centroid)
    }

    /// `(volume, face)` of a fluid face; `None` for a world face.
    pub(crate) fn fluid_face(&self) -> Option<(u32, u32)> {
        match self.source {
            ReflectorSource::Fluid { volume, face } => Some((volume, face)),
            ReflectorSource::World { .. } => None,
        }
    }

    /// The id-17 face of a world face; `None` for a fluid face.
    pub(crate) fn world_face(&self) -> Option<u32> {
        match self.source {
            ReflectorSource::World { face } => Some(face),
            ReflectorSource::Fluid { .. } => None,
        }
    }
}

/// Every up-facing fluid face, in volume then face order. Side, bottom and
/// sloped faces bind nothing.
pub(crate) fn up_facing_faces(volumes: &EnvironmentVolumesSection) -> Vec<ReflectorFace> {
    let mut out = Vec::new();
    for (volume, record) in volumes.volumes.iter().enumerate() {
        for (face, surface) in record.surface_faces.iter().enumerate() {
            let points: Vec<DVec3> = surface
                .vertices
                .iter()
                .map(|v| DVec3::from(v.position.map(f64::from)))
                .collect();
            if let Some(geometry) = FaceGeometry::from_polygon(&points)
                && geometry.is_up_facing()
            {
                out.push(ReflectorFace::new(
                    ReflectorSource::Fluid {
                        volume: volume as u32,
                        face: face as u32,
                    },
                    geometry,
                    Facing::Up,
                    &points,
                ));
            }
        }
    }
    out
}

/// Clusters of plane heights: sorted heights chained within
/// [`PLANE_TOLERANCE_M`], so input order never changes them. Each cluster is
/// its lowest height.
pub(crate) fn plane_heights(heights: impl IntoIterator<Item = f64>) -> Vec<f64> {
    let mut sorted: Vec<f64> = heights.into_iter().collect();
    sorted.sort_by(f64::total_cmp);
    let mut clusters: Vec<f64> = Vec::new();
    let mut last = f64::NEG_INFINITY;
    for height in sorted {
        if height - last >= PLANE_TOLERANCE_M {
            clusters.push(height);
        }
        last = height;
    }
    clusters
}

/// The cluster of [`plane_heights`] `height` belongs to.
pub(crate) fn plane_of(clusters: &[f64], height: f64) -> f64 {
    clusters
        .iter()
        .rev()
        .copied()
        .find(|&lowest| lowest <= height)
        .unwrap_or(clusters[0])
}

/// One probe before capture.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct PlacedProbe {
    /// The authored box's entity, the fluid entity of a derived water probe,
    /// or [`WORLD_SOURCE_ENTITY`] for a derived world probe.
    pub source_entity: u32,
    /// Whether a `reflection_volume` placed it, rather than derivation.
    pub authored: bool,
    pub box_min: [f32; 3],
    pub box_max: [f32; 3],
    pub capture_point: [f32; 3],
    /// Which way its faces look: its capture point lifts off them along the
    /// facing's normal, and its light shares cover that hemisphere.
    pub facing: Facing,
    /// Summed area of the faces it binds; the cap keeps the largest.
    pub bound_area: f64,
    /// Bound fluid faces as `(volume, face)`, ascending.
    pub fluid_faces: Vec<(u32, u32)>,
    /// Bound static world faces in id-17 numbering, ascending.
    pub world_faces: Vec<u32>,
}

/// A derived world probe's `source_entity`: worldspawn, as its faces belong
/// to the static world rather than one entity.
pub(crate) const WORLD_SOURCE_ENTITY: u32 = 0;

impl PlacedProbe {
    pub(crate) fn capture_point_vec3(&self) -> Vec3 {
        Vec3::from(self.capture_point)
    }
}

/// What placement reads besides the faces and the authored boxes.
pub(crate) struct PlacementScene<'a> {
    pub tracer: &'a SceneTracer<'a>,
    pub bounds: &'a WorldBounds,
    pub classifier: &'a PointClassifier<'a>,
    /// Bounds the derived box fits' parallelism, as the capture's.
    pub governor: &'a Governor,
}

/// Every kept probe: authored probes in map entity order (one per facing of
/// each box), then derived water probes in fluid entity order, then derived
/// world probes over the `world_faces` no box claimed (glossy world faces
/// in id-17 order). In order: authored capture points and their drops,
/// binding among the kept boxes, dropping boxes that bind nothing, derived
/// probes for the unclaimed faces and their drops, then the cap of `cap`
/// probes. Probes binding water come first under the cap: authored ones
/// past it alone refuse the build, and water's derived probes take the room
/// they leave, so world faces never cost water a probe. Probes binding
/// world faces alone, authored or derived, take what water leaves, the
/// smallest bound area dropping first.
pub(crate) fn place_probes(
    volumes: &EnvironmentVolumesSection,
    authored: &[MapReflectionVolume],
    world_faces: &[ReflectorFace],
    scene: &PlacementScene<'_>,
    cap: usize,
) -> anyhow::Result<Vec<PlacedProbe>> {
    let mut faces = up_facing_faces(volumes);
    let fluid_count = faces.len();
    faces.extend(world_faces.iter().cloned());
    let volumes_authored = authored;
    let authored = place_authored(&faces, authored, scene.classifier)?;
    refuse_authored_water_past_cap(&authored.probes, volumes_authored, cap)?;
    let unclaimed = |range: std::ops::Range<usize>| -> Vec<&ReflectorFace> {
        faces[range.clone()]
            .iter()
            .zip(&authored.claimed[range])
            .filter_map(|(face, &claimed)| (!claimed).then_some(face))
            .collect()
    };
    let authored_water = authored.probes.iter().filter(|p| binds_water(p)).count();
    let derived = derive_probes(&unclaimed(0..fluid_count), volumes, scene);
    let derived = apply_probe_cap(derived, cap - authored_water, cap);
    let world = unclaimed(fluid_count..faces.len());
    let derived_world = derive_probes(&world, volumes, scene);
    // Authored probes binding world faces alone, then derived world probes:
    // one pool under the room water leaves.
    let pool: Vec<&PlacedProbe> = authored
        .probes
        .iter()
        .filter(|p| !binds_water(p))
        .chain(&derived_world)
        .collect();
    let keep = world_probes_kept(
        &pool,
        volumes_authored,
        cap - authored_water - derived.len(),
        cap,
    );
    let (authored_world_keep, derived_world_keep) = keep.split_at(pool.len() - derived_world.len());
    let mut authored_world_keep = authored_world_keep.iter().copied();
    let authored_keep: Vec<bool> = authored
        .probes
        .iter()
        .map(|p| binds_water(p) || authored_world_keep.next().expect("one flag per pool probe"))
        .collect();
    let mut probes = retain(authored.probes, &authored_keep);
    probes.extend(derived);
    probes.extend(retain(derived_world, derived_world_keep));
    Ok(probes)
}

/// Every kept probe's fluid faces as bindings, ascending by `(volume, face)`.
pub(crate) fn fluid_bindings(probes: &[PlacedProbe]) -> Vec<ReflectionProbeFluidBinding> {
    let mut out: Vec<ReflectionProbeFluidBinding> = probes
        .iter()
        .enumerate()
        .flat_map(|(probe, placed)| {
            placed
                .fluid_faces
                .iter()
                .map(move |&(volume, face)| ReflectionProbeFluidBinding {
                    volume,
                    face,
                    probe: probe as u32,
                })
        })
        .collect();
    out.sort_unstable();
    out
}

/// Every kept probe's world faces as bindings, ascending by face. A world
/// face binds one authored box's probe or joins one derived group, so each
/// face binds once.
pub(crate) fn world_bindings(probes: &[PlacedProbe]) -> Vec<ReflectionProbeWorldBinding> {
    let mut out: Vec<ReflectionProbeWorldBinding> = probes
        .iter()
        .enumerate()
        .flat_map(|(probe, placed)| {
            placed
                .world_faces
                .iter()
                .map(move |&face| ReflectionProbeWorldBinding {
                    face,
                    probe: probe as u32,
                })
        })
        .collect();
    out.sort_unstable();
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(points: &[[f64; 3]]) -> Vec<DVec3> {
        points.iter().map(|&p| DVec3::from(p)).collect()
    }

    #[test]
    fn area_centroid_is_unmoved_by_an_inserted_collinear_vertex() {
        // An L-shaped face: the vertex mean and the area centroid differ.
        let l_shape = d(&[
            [0.0, 0.0, 0.0],
            [0.0, 0.0, 2.0],
            [1.0, 0.0, 2.0],
            [1.0, 0.0, 1.0],
            [3.0, 0.0, 1.0],
            [3.0, 0.0, 0.0],
        ]);
        let base = FaceGeometry::from_polygon(&l_shape).unwrap();
        // Close a T-junction on the long edge: a vertex on segment (3,0)-(0,0).
        let mut junction = l_shape.clone();
        junction.push(DVec3::new(1.5, 0.0, 0.0));
        let with_junction = FaceGeometry::from_polygon(&junction).unwrap();

        // Two 2 m² rectangles: x 0..1 × z 0..2 centred (0.5, 1) and
        // x 1..3 × z 0..1 centred (2, 0.5); the area centroid is their mean.
        assert!((base.area - 4.0).abs() < 1e-12);
        let expected = DVec3::new(1.25, 0.0, 0.75);
        assert!(
            (base.centroid - expected).length() < 1e-12,
            "{}",
            base.centroid
        );
        assert!((with_junction.centroid - base.centroid).length() < 1e-12);
        assert!((with_junction.area - base.area).abs() < 1e-12);
        // Not the vertex mean, which the inserted vertex would drag.
        let mean = l_shape.iter().sum::<DVec3>() / l_shape.len() as f64;
        assert!((mean - expected).length() > 0.05);
    }

    #[test]
    fn only_faces_within_one_degree_of_up_are_up_facing() {
        let up = FaceGeometry::from_polygon(&d(&[
            [0.0, 1.0, 0.0],
            [0.0, 1.0, 1.0],
            [1.0, 1.0, 1.0],
            [1.0, 1.0, 0.0],
        ]))
        .unwrap();
        assert!(up.is_up_facing());
        assert!((up.normal - DVec3::Y).length() < 1e-12);

        // Wound the other way the same square faces down: a pool's bottom.
        let down = FaceGeometry::from_polygon(&d(&[
            [0.0, 1.0, 0.0],
            [1.0, 1.0, 0.0],
            [1.0, 1.0, 1.0],
            [0.0, 1.0, 1.0],
        ]))
        .unwrap();
        assert!(!down.is_up_facing());

        let tilted = |degrees: f64| {
            let rise = degrees.to_radians().tan();
            FaceGeometry::from_polygon(&d(&[
                [0.0, 0.0, 0.0],
                [0.0, 0.0, 1.0],
                [1.0, rise, 1.0],
                [1.0, rise, 0.0],
            ]))
            .unwrap()
        };
        assert!(tilted(0.5).is_up_facing());
        assert!(!tilted(2.0).is_up_facing(), "a sloped face binds nothing");

        let side = FaceGeometry::from_polygon(&d(&[
            [0.0, 0.0, 0.0],
            [0.0, 1.0, 0.0],
            [0.0, 1.0, 1.0],
            [0.0, 0.0, 1.0],
        ]))
        .unwrap();
        assert!(!side.is_up_facing(), "a side face binds nothing");
    }

    #[test]
    fn plane_heights_chain_within_the_tolerance_whatever_the_order() {
        let heights = [0.5, 0.0, 0.0004, 0.5008, 0.0008, -1.0];
        let clusters = plane_heights(heights);
        assert_eq!(clusters, vec![-1.0, 0.0, 0.5]);
        let mut reversed = heights;
        reversed.reverse();
        assert_eq!(plane_heights(reversed), clusters);
        assert_eq!(plane_of(&clusters, 0.0008), 0.0);
        assert_eq!(plane_of(&clusters, 0.5008), 0.5);
    }
}
