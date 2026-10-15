// Reflection-probe placement: up-facing fluid faces, their groups, fitted boxes and capture points.
// See: context/lib/resource_management.md §5.1

use glam::{DVec3, Vec3};
use postretro_level_format::environment_volumes::EnvironmentVolumesSection;
use postretro_level_format::reflection_probes::{MAX_REFLECTION_PROBES, ReflectionProbeBinding};

use super::box_fit::{FitRay, WorldBounds, fit_box, fit_rays};
use super::hit::SceneTracer;

/// A face is up-facing when its outward normal is within 1° of up. Water is
/// authored flat; a brush top a degree off still mirrors the room with at most
/// a 2° dip below the horizon, which the shader's horizon clamp absorbs, while
/// a visibly sloped face would reflect the box from the wrong plane.
pub(crate) const UP_FACING_MIN_COS: f64 = 0.999_847_695_156_391_3;
/// The capture point and fit origins sit this far above the water plane, so
/// lookups stay in the upper hemisphere and the point stays out of the fluid.
pub(crate) const CAPTURE_LIFT_M: f64 = 0.002;
/// Faces of one volume whose planes differ by less than this share a plane.
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
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct UpFacingFace {
    pub volume: u32,
    pub face: u32,
    pub geometry: FaceGeometry,
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
                });
            }
        }
    }
    out
}

/// Groups one volume's up-facing faces into probes, as indices into `faces`,
/// each group ascending and groups ordered by their first face. Faces share a
/// group when their planes lie within [`PLANE_TOLERANCE_M`] of the group's
/// first face.
///
/// The minimal grouping: placement completeness replaces it with connected
/// components of the box-match relation, which splits one plane across rooms.
pub(crate) fn group_volume_faces(faces: &[UpFacingFace]) -> Vec<Vec<usize>> {
    let mut groups: Vec<Vec<usize>> = Vec::new();
    for (i, face) in faces.iter().enumerate() {
        let height = face.geometry.plane_height();
        match groups.iter_mut().find(|group| {
            (faces[group[0]].geometry.plane_height() - height).abs() < PLANE_TOLERANCE_M
        }) {
            Some(group) => group.push(i),
            None => groups.push(vec![i]),
        }
    }
    groups
}

/// One probe before capture.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct PlacedProbe {
    /// The fluid entity of a derived probe.
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

/// Derived probes for every up-facing fluid face, in fluid entity order. Each
/// face fits its own box; a group stores the union of its members' boxes and
/// captures from its horizontal area centroid, [`CAPTURE_LIFT_M`] above its
/// lowest plane.
pub(crate) fn derive_probes(
    volumes: &EnvironmentVolumesSection,
    tracer: &SceneTracer<'_>,
    bounds: &WorldBounds,
) -> Vec<PlacedProbe> {
    let faces = up_facing_faces(volumes);
    let rays = fit_rays();
    let mut probes = Vec::new();
    let mut start = 0;
    while start < faces.len() {
        let volume = faces[start].volume;
        let end = start + faces[start..].partition_point(|f| f.volume == volume);
        let volume_faces = &faces[start..end];
        for group in group_volume_faces(volume_faces) {
            let members: Vec<&UpFacingFace> = group.iter().map(|&i| &volume_faces[i]).collect();
            probes.push(group_probe(
                volumes.volumes[volume as usize].source_entity,
                &members,
                tracer,
                bounds,
                &rays,
            ));
        }
        start = end;
    }
    probes
}

fn group_probe(
    source_entity: u32,
    members: &[&UpFacingFace],
    tracer: &SceneTracer<'_>,
    bounds: &WorldBounds,
    rays: &[FitRay],
) -> PlacedProbe {
    let mut box_lo = DVec3::splat(f64::INFINITY);
    let mut box_hi = DVec3::splat(f64::NEG_INFINITY);
    let mut area = 0.0;
    let mut weighted = DVec3::ZERO;
    let mut lowest_plane = f64::INFINITY;
    for member in members {
        let g = &member.geometry;
        let origin = g.centroid + DVec3::Y * CAPTURE_LIFT_M;
        let (lo, hi) = fit_box(tracer, origin, bounds, rays);
        box_lo = box_lo.min(lo);
        box_hi = box_hi.max(hi);
        area += g.area;
        weighted += g.centroid * g.area;
        lowest_plane = lowest_plane.min(g.plane_height());
    }
    let centroid = weighted / area;
    let capture = DVec3::new(centroid.x, lowest_plane + CAPTURE_LIFT_M, centroid.z);
    let box_min = box_lo.as_vec3();
    let box_max = box_hi.as_vec3();
    // The point is a convex combination of points inside the union box; the
    // clamp only absorbs f32 rounding at a box face.
    let capture_point = capture.as_vec3().clamp(box_min, box_max);
    PlacedProbe {
        source_entity,
        box_min: box_min.to_array(),
        box_max: box_max.to_array(),
        capture_point: capture_point.to_array(),
        bound_area: area,
        faces: members.iter().map(|m| (m.volume, m.face)).collect(),
    }
}

/// Keep at most [`MAX_REFLECTION_PROBES`] derived probes, the largest bound
/// surface area first (ties to the earlier probe), in their original order.
/// Dropped probes' faces bind nothing; one warning names the count.
pub(crate) fn apply_probe_cap(probes: Vec<PlacedProbe>) -> Vec<PlacedProbe> {
    if probes.len() <= MAX_REFLECTION_PROBES {
        return probes;
    }
    let mut ranked: Vec<usize> = (0..probes.len()).collect();
    ranked.sort_by(|&a, &b| {
        probes[b]
            .bound_area
            .total_cmp(&probes[a].bound_area)
            .then(a.cmp(&b))
    });
    let mut keep = vec![false; probes.len()];
    for &i in &ranked[..MAX_REFLECTION_PROBES] {
        keep[i] = true;
    }
    log::warn!(
        "[ReflectionProbes] {} derived probe(s) past the {MAX_REFLECTION_PROBES}-probe cap \
         dropped, smallest bound water first; their faces reflect nothing",
        probes.len() - MAX_REFLECTION_PROBES
    );
    probes
        .into_iter()
        .zip(keep)
        .filter_map(|(probe, kept)| kept.then_some(probe))
        .collect()
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
    use crate::bvh_build::build_bvh;
    use crate::reflection_probes::reflection_probes_test_fixtures::{
        box_room, fluid_volume, up_rect, volumes,
    };

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
    fn side_bottom_and_sloped_fluid_faces_bind_no_probe() {
        let room = box_room([-4.0, 0.0, -4.0], [4.0, 3.0, 4.0]);
        let (bvh, primitives, _) = build_bvh(&room).unwrap();
        let tracer = SceneTracer::new(&bvh, &primitives, &room);
        let bounds =
            WorldBounds::from_positions(room.geometry.vertices.iter().map(|v| &v.position))
                .unwrap();
        let pool = vec![
            // Top (face 0): up-facing.
            up_rect(-1.0, -1.0, 1.0, 1.0, 1.0),
            // Bottom (face 1): wound to face down.
            vec![
                [-1.0, 0.5, -1.0],
                [1.0, 0.5, -1.0],
                [1.0, 0.5, 1.0],
                [-1.0, 0.5, 1.0],
            ],
            // Side (face 2).
            vec![
                [1.0, 0.5, -1.0],
                [1.0, 1.0, -1.0],
                [1.0, 1.0, 1.0],
                [1.0, 0.5, 1.0],
            ],
            // Sloped (face 3): 10° off up.
            vec![
                [2.0, 1.0, -1.0],
                [2.0, 1.0, 1.0],
                [3.0, 1.176, 1.0],
                [3.0, 1.176, -1.0],
            ],
        ];
        let section = volumes(vec![fluid_volume(7, &pool)]);
        let probes = derive_probes(&section, &tracer, &bounds);
        assert_eq!(probes.len(), 1);
        assert_eq!(probes[0].source_entity, 7);
        assert_eq!(
            bindings(&probes),
            vec![ReflectionProbeBinding {
                volume: 0,
                face: 0,
                probe: 0,
            }]
        );
    }

    #[test]
    fn derived_probe_fits_the_room_and_captures_above_its_water() {
        let room = box_room([-4.0, -1.0, -3.0], [4.0, 3.0, 3.0]);
        let (bvh, primitives, _) = build_bvh(&room).unwrap();
        let tracer = SceneTracer::new(&bvh, &primitives, &room);
        let bounds =
            WorldBounds::from_positions(room.geometry.vertices.iter().map(|v| &v.position))
                .unwrap();
        // One pool built from two coplanar faces of one entity, offset from
        // the room centre so the capture point is not trivially the origin.
        let section = volumes(vec![fluid_volume(
            3,
            &[
                up_rect(0.0, -1.0, 1.0, 1.0, 0.5),
                up_rect(1.0, -1.0, 3.0, 1.0, 0.5),
            ],
        )]);
        let probes = derive_probes(&section, &tracer, &bounds);
        assert_eq!(
            probes.len(),
            1,
            "coplanar faces of one entity share a probe"
        );
        let probe = &probes[0];
        assert_eq!(probe.faces, vec![(0, 0), (0, 1)]);

        let close = |a: f32, b: f32| (a - b).abs() < 1e-4;
        // The box reaches the room's walls and ceiling, and its floor is the
        // capture plane: water reads only its upper hemisphere.
        assert!(
            close(probe.box_min[0], -4.0) && close(probe.box_max[0], 4.0),
            "{probe:?}"
        );
        assert!(
            close(probe.box_min[2], -3.0) && close(probe.box_max[2], 3.0),
            "{probe:?}"
        );
        assert!(close(probe.box_max[1], 3.0), "{probe:?}");
        assert!(close(probe.box_min[1], 0.502), "{probe:?}");
        // Horizontal area centroid of 2 m² at x 0.5 and 4 m² at x 2: x = 1.5.
        assert!(close(probe.capture_point[0], 1.5), "{probe:?}");
        assert!(close(probe.capture_point[2], 0.0), "{probe:?}");
        assert!(close(probe.capture_point[1], 0.502), "{probe:?}");
        assert!((probe.bound_area - 6.0).abs() < 1e-9);
    }

    #[test]
    fn faces_of_one_entity_at_two_heights_get_separate_probes() {
        let room = box_room([-4.0, -1.0, -3.0], [4.0, 3.0, 3.0]);
        let (bvh, primitives, _) = build_bvh(&room).unwrap();
        let tracer = SceneTracer::new(&bvh, &primitives, &room);
        let bounds =
            WorldBounds::from_positions(room.geometry.vertices.iter().map(|v| &v.position))
                .unwrap();
        let section = volumes(vec![
            fluid_volume(
                3,
                &[
                    up_rect(-2.0, -1.0, -1.0, 1.0, 0.0),
                    up_rect(1.0, -1.0, 2.0, 1.0, 0.5),
                ],
            ),
            fluid_volume(4, &[up_rect(-3.0, -2.0, -2.5, -1.5, 0.0)]),
        ]);
        let probes = derive_probes(&section, &tracer, &bounds);
        let entities: Vec<u32> = probes.iter().map(|p| p.source_entity).collect();
        assert_eq!(
            entities,
            vec![3, 3, 4],
            "fluid entity order, one probe per plane"
        );
        assert_eq!(
            bindings(&probes)
                .iter()
                .map(|b| (b.volume, b.face, b.probe))
                .collect::<Vec<_>>(),
            vec![(0, 0, 0), (0, 1, 1), (1, 0, 2)]
        );
    }

    #[test]
    fn probe_cap_keeps_the_largest_bound_water_in_order() {
        let probe = |area: f64| PlacedProbe {
            source_entity: 0,
            box_min: [0.0; 3],
            box_max: [1.0; 3],
            capture_point: [0.5; 3],
            bound_area: area,
            faces: Vec::new(),
        };
        let mut probes: Vec<PlacedProbe> = (0..MAX_REFLECTION_PROBES).map(|_| probe(2.0)).collect();
        probes.insert(10, probe(1.0));
        probes.push(probe(1.0));
        probes.push(probe(5.0));
        let kept = apply_probe_cap(probes);
        assert_eq!(kept.len(), MAX_REFLECTION_PROBES);
        assert!(kept.iter().all(|p| p.bound_area >= 2.0));
        assert_eq!(
            kept.last().unwrap().bound_area,
            5.0,
            "original order survives"
        );
    }
}
