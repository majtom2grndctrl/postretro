// Reflection-probe placement: up-facing fluid faces, authored and derived probes, their drops and binding.
// See: context/lib/resource_management.md §5.1

use glam::{DVec2, DVec3, Vec3};
use postretro_level_format::environment_volumes::EnvironmentVolumesSection;
use postretro_level_format::reflection_probes::ReflectionProbeBinding;

use crate::governor::Governor;
use crate::map_data::MapReflectionVolume;

use super::authored::place_authored;
use super::box_fit::WorldBounds;
use super::capture_point::PointClassifier;
use super::derived::derive_probes;
use super::hit::SceneTracer;

/// A face is up-facing when its outward normal is within 1° of up. Water is
/// authored flat; a brush top a degree off still mirrors the room with at most
/// a 2° dip below the horizon, which the shader's horizon clamp absorbs, while
/// a visibly sloped face would reflect the box from the wrong plane.
pub(crate) const UP_FACING_MIN_COS: f64 = 0.999_847_695_156_391_3;
/// The capture point and fit origins sit this far above the water plane, so
/// lookups stay in the upper hemisphere and the point stays out of the fluid.
pub(crate) const CAPTURE_LIFT_M: f64 = 0.002;
/// Faces whose planes differ by less than this share a plane.
pub(crate) const PLANE_TOLERANCE_M: f64 = 0.001;

/// Area-weighted geometry of one planar surface polygon.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct FaceGeometry {
    /// Unit Newell normal, outward from the fluid (its winding's normal).
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

    pub(crate) fn plane_height(&self) -> f64 {
        self.centroid.y
    }
}

/// An up-facing fluid face in EnvironmentVolumes (id 52) numbering.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct UpFacingFace {
    pub volume: u32,
    pub face: u32,
    pub geometry: FaceGeometry,
    /// The polygon seen from above, `(x, z)` per vertex.
    pub outline: Vec<DVec2>,
}

/// Every up-facing fluid face, in volume then face order. Side, bottom and
/// sloped faces bind nothing.
pub(crate) fn up_facing_faces(volumes: &EnvironmentVolumesSection) -> Vec<UpFacingFace> {
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
                out.push(UpFacingFace {
                    volume: volume as u32,
                    face: face as u32,
                    geometry,
                    outline: points.iter().map(|p| DVec2::new(p.x, p.z)).collect(),
                });
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
    /// The authored box's entity, or the fluid entity of a derived probe.
    pub source_entity: u32,
    pub box_min: [f32; 3],
    pub box_max: [f32; 3],
    pub capture_point: [f32; 3],
    /// Summed area of the faces it binds; the cap keeps the largest.
    pub bound_area: f64,
    /// Bound faces as `(volume, face)`, ascending.
    pub faces: Vec<(u32, u32)>,
}

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

/// Placement's output: authored probes in map entity order, then derived
/// probes in fluid entity order. Splitting and the cap act on it next.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Placement {
    pub probes: Vec<PlacedProbe>,
    /// How many of `probes`, from the front, are authored.
    pub authored: usize,
}

/// Every probe placement keeps. In order: authored capture points and their
/// drops, binding among the kept boxes, dropping boxes that bind nothing,
/// then derived probes for the unclaimed faces and their drops. Authored
/// probes past `cap` refuse the build; derived probes past it drop only
/// after splitting ([`super::cap::apply_probe_cap`]).
pub(crate) fn place_probes(
    volumes: &EnvironmentVolumesSection,
    authored: &[MapReflectionVolume],
    scene: &PlacementScene<'_>,
    cap: usize,
) -> anyhow::Result<Placement> {
    let faces = up_facing_faces(volumes);
    let authored = place_authored(&faces, authored, scene.classifier)?;
    if authored.probes.len() > cap {
        anyhow::bail!(
            "{} reflection volumes keep a probe, past the {cap}-probe cap; remove or merge \
             reflection volumes until at most {cap} remain",
            authored.probes.len()
        );
    }
    let unclaimed: Vec<&UpFacingFace> = faces
        .iter()
        .zip(&authored.claimed)
        .filter_map(|(face, &claimed)| (!claimed).then_some(face))
        .collect();
    let derived = derive_probes(&unclaimed, volumes, scene);
    let count = authored.probes.len();
    let mut probes = authored.probes;
    probes.extend(derived);
    Ok(Placement {
        probes,
        authored: count,
    })
}

/// Placement then the cap, with no splitting between: what the placement
/// contracts test.
#[cfg(test)]
pub(crate) fn place_and_cap(
    volumes: &EnvironmentVolumesSection,
    authored: &[MapReflectionVolume],
    scene: &PlacementScene<'_>,
    cap: usize,
) -> anyhow::Result<Vec<PlacedProbe>> {
    use super::cap::{apply_probe_cap, warn_capped};
    use super::split::StageProbe;
    let placement = place_probes(volumes, authored, scene, cap)?;
    let mut probes: Vec<StageProbe> = placement
        .probes
        .into_iter()
        .map(|placed| StageProbe {
            placed,
            parent: None,
        })
        .collect();
    let (kept, dropped) = apply_probe_cap(&mut probes, placement.authored, cap);
    warn_capped(dropped, cap);
    Ok(kept.into_iter().map(|i| probes[i].placed.clone()).collect())
}

/// Every kept probe's faces as bindings, ascending by `(volume, face)`.
pub(crate) fn bindings(probes: &[PlacedProbe]) -> Vec<ReflectionProbeBinding> {
    let mut out: Vec<ReflectionProbeBinding> = probes
        .iter()
        .enumerate()
        .flat_map(|(probe, placed)| {
            placed
                .faces
                .iter()
                .map(move |&(volume, face)| ReflectionProbeBinding {
                    volume,
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
