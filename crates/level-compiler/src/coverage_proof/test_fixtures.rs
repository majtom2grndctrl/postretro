// Coverage proof fixtures: polygon geometry, closed boxes, and a sealed room compiled through extraction and the oversize-face cut.
// See: context/lib/testing_guide.md §4

use glam::{DQuat, DVec3, Vec3};
use postretro_level_format::geometry::{FaceMeta, GeometrySection, Vertex};
use postretro_level_format::texture_names::TextureNamesSection;

use super::weld::{Cluster, WeldRule, clusters};
use super::{CoverageReport, UnprovenSpan, WELD_RADIUS_METERS, prove_coverage, prove_welded};
use crate::geometry::{FaceIndexRange, GeometryResult, extract_geometry_with_brushes};
use crate::light_namespaces::StaticBakedLights;
use crate::ray_traversal::TRACER_TOLERANCE_METERS;
use crate::reflection_probes::placement_test_fixtures::{box_brush, entity, parse_inline};
use crate::{lightmap_bake, partition, portals, visibility};

pub(super) fn tau() -> f64 {
    f64::from(TRACER_TOLERANCE_METERS)
}

pub(super) fn prove(polygons: &[Polygon]) -> CoverageReport {
    let (geometry, brushes) = polygon_geometry(polygons);
    prove_coverage(&geometry, &brushes)
}

/// δ, the production weld radius.
pub(super) fn delta() -> f64 {
    WELD_RADIUS_METERS
}

/// The proof with corners welded by `rule` instead of production's.
pub(super) fn prove_with(polygons: &[Polygon], rule: WeldRule) -> CoverageReport {
    let (geometry, brushes) = polygon_geometry(polygons);
    prove_welded(&geometry, &brushes, rule)
}

/// The proof with the weld off (δ = 0): plain strict balance.
pub(super) fn prove_unwelded(polygons: &[Polygon]) -> CoverageReport {
    prove_with(
        polygons,
        WeldRule {
            radius: 0.0,
            keep_triangles_whole: true,
        },
    )
}

/// Distance from `p` to the segment `a`–`b`.
pub(super) fn to_segment(p: DVec3, a: DVec3, b: DVec3) -> f64 {
    let ab = b - a;
    let s = ((p - a).dot(ab) / ab.length_squared()).clamp(0.0, 1.0);
    (p - (a + ab * s)).length()
}

/// Both ends of `span` lie within `tolerance` of the segment `a`–`b`.
pub(super) fn lies_along(span: &UnprovenSpan, a: DVec3, b: DVec3, tolerance: f64) -> bool {
    span.endpoints
        .iter()
        .all(|&p| to_segment(p, a, b) <= tolerance)
}

/// A face for [`polygon_geometry`]: its loop, wound counter-clockwise about
/// its outward normal, and its brush.
#[derive(Clone)]
pub(super) struct Polygon {
    pub(super) corners: Vec<Vec3>,
    pub(super) brush: Option<u32>,
}

/// One fan-triangulated face per polygon, in order, as geometry extraction
/// and the oversize-face cut lay faces out. Corners given bit-identical stay
/// bit-identical.
pub(super) fn polygon_geometry(polygons: &[Polygon]) -> (GeometryResult, Vec<Option<u32>>) {
    let mut vertices = Vec::new();
    let mut indices = Vec::new();
    let mut faces = Vec::new();
    let mut ranges = Vec::new();
    for polygon in polygons {
        let base = vertices.len() as u32;
        vertices.extend(polygon.corners.iter().map(|p| {
            Vertex::new(
                p.to_array(),
                [0.0, 0.0],
                [0.0, 1.0, 0.0],
                [1.0, 0.0, 0.0],
                true,
                [0.0, 0.0],
                0,
            )
        }));
        let index_offset = indices.len() as u32;
        for i in 1..polygon.corners.len() as u32 - 1 {
            indices.extend([base, base + i, base + i + 1]);
        }
        faces.push(FaceMeta {
            leaf_index: 0,
            texture_index: 0,
        });
        ranges.push(FaceIndexRange {
            index_offset,
            index_count: indices.len() as u32 - index_offset,
        });
    }
    let geometry = GeometryResult {
        geometry: GeometrySection {
            vertices,
            indices,
            faces,
        },
        texture_names: TextureNamesSection { names: Vec::new() },
        face_index_ranges: ranges,
    };
    (geometry, polygons.iter().map(|p| p.brush).collect())
}

/// Each fan-triangulated face of `geometry` back to its loop, with its brush:
/// the inverse of [`polygon_geometry`].
pub(super) fn face_polygons(geometry: &GeometryResult, brushes: &[Option<u32>]) -> Vec<Polygon> {
    let geom = &geometry.geometry;
    geometry
        .face_index_ranges
        .iter()
        .zip(brushes)
        .map(|(range, &brush)| {
            let start = range.index_offset as usize;
            let indices = &geom.indices[start..start + range.index_count as usize];
            let mut corners = vec![indices[0], indices[1], indices[2]];
            corners.extend(indices.chunks(3).skip(1).map(|tri| tri[2]));
            Polygon {
                corners: corners
                    .into_iter()
                    .map(|i| Vec3::from(geom.vertices[i as usize].position))
                    .collect(),
                brush,
            }
        })
        .collect()
}

/// A box's frame: corners map through `rotation` then `offset`.
#[derive(Clone, Copy)]
pub(super) struct Frame {
    pub(super) rotation: DQuat,
    pub(super) offset: DVec3,
}

impl Frame {
    pub(super) const IDENTITY: Self = Self {
        rotation: DQuat::IDENTITY,
        offset: DVec3::ZERO,
    };

    /// A frame turned off every axis, so its creases are not axial.
    pub(super) fn oblique(offset: DVec3) -> Self {
        Self {
            rotation: DQuat::from_euler(glam::EulerRot::YXZ, 0.41, 0.27, -0.33),
            offset,
        }
    }

    pub(super) fn point(&self, local: DVec3) -> Vec3 {
        (self.rotation * local + self.offset).as_vec3()
    }
}

/// The six faces of the box `min`..`max` in `frame`, each wound
/// counter-clockwise about its outward normal, in the order −X, +X, −Y, +Y
/// (top), −Z, +Z. Shared corners are bit-identical. `lift` raises the top
/// face's corners by that many metres along the frame's up axis, opening a
/// crack of that width along its four creases.
pub(super) fn closed_box(
    frame: Frame,
    min: DVec3,
    max: DVec3,
    lift: f64,
    brush: u32,
) -> Vec<Polygon> {
    let corner = |x: bool, y: bool, z: bool| {
        DVec3::new(
            if x { max.x } else { min.x },
            if y { max.y } else { min.y },
            if z { max.z } else { min.z },
        )
    };
    let top = |x: bool, z: bool| corner(x, true, z) + DVec3::Y * lift;
    let quads: [([DVec3; 4], DVec3); 6] = [
        (
            [
                corner(false, false, false),
                corner(false, false, true),
                corner(false, true, true),
                corner(false, true, false),
            ],
            DVec3::NEG_X,
        ),
        (
            [
                corner(true, false, false),
                corner(true, true, false),
                corner(true, true, true),
                corner(true, false, true),
            ],
            DVec3::X,
        ),
        (
            [
                corner(false, false, false),
                corner(true, false, false),
                corner(true, false, true),
                corner(false, false, true),
            ],
            DVec3::NEG_Y,
        ),
        (
            [
                top(false, false),
                top(false, true),
                top(true, true),
                top(true, false),
            ],
            DVec3::Y,
        ),
        (
            [
                corner(false, false, false),
                corner(false, true, false),
                corner(true, true, false),
                corner(true, false, false),
            ],
            DVec3::NEG_Z,
        ),
        (
            [
                corner(false, false, true),
                corner(true, false, true),
                corner(true, true, true),
                corner(false, true, true),
            ],
            DVec3::Z,
        ),
    ];
    quads
        .into_iter()
        .map(|(local, outward)| {
            let normal = (local[1] - local[0]).cross(local[2] - local[1]);
            assert!(normal.dot(outward) > 0.0, "box quad wound outward");
            Polygon {
                corners: local.iter().map(|&p| frame.point(p)).collect(),
                brush: Some(brush),
            }
        })
        .collect()
}

/// Two closed boxes meeting along the line x = y = 1 (local), 0 ≤ z ≤ 1:
/// box A's +X and +Y faces and box B's −X and −Y faces share it. Returns
/// the faces, the four hinge faces' slots (A +X, A +Y, B −X, B −Y) and the
/// hinge's ends.
pub(super) fn hinge(frame: Frame) -> (Vec<Polygon>, [usize; 4], (DVec3, DVec3)) {
    let mut polygons = closed_box(frame, DVec3::ZERO, DVec3::ONE, 0.0, 0);
    polygons.extend(closed_box(
        frame,
        DVec3::new(1.0, 1.0, 0.0),
        DVec3::new(2.0, 2.0, 1.0),
        0.0,
        1,
    ));
    let line = (
        frame.point(DVec3::new(1.0, 1.0, 0.0)).as_dvec3(),
        frame.point(DVec3::new(1.0, 1.0, 1.0)).as_dvec3(),
    );
    (polygons, [1, 3, 6, 8], line)
}

/// The four faces of the tetrahedron `corners` in `frame`, each wound
/// counter-clockwise about its outward normal.
pub(super) fn tetrahedron(frame: Frame, corners: [DVec3; 4], brush: u32) -> Vec<Polygon> {
    [[0, 1, 2, 3], [0, 1, 3, 2], [0, 2, 3, 1], [1, 2, 3, 0]]
        .into_iter()
        .map(|[a, b, c, opposite]| {
            let normal = (corners[b] - corners[a]).cross(corners[c] - corners[a]);
            let outward = normal.dot(corners[a] - corners[opposite]) > 0.0;
            let loop_ = if outward { [a, b, c] } else { [a, c, b] };
            Polygon {
                corners: loop_.iter().map(|&k| frame.point(corners[k])).collect(),
                brush: Some(brush),
            }
        })
        .collect()
}

/// Map units to engine metres.
const UNIT: f64 = 0.0254;
/// Interior of the room, map units.
const ROOM: [f64; 3] = [512.0, 384.0, 256.0];
/// Slab thickness, map units.
const WALL: f64 = 16.0;
/// Lightmap density and pool edge that cut every face longer than 6.4 m.
const CUT_DENSITY: f32 = 0.1;
const CUT_POOL_EDGE: u32 = 64;

/// A sealed room of six slabs with a light, two overlapping platforms whose
/// coplanar tops overlap, and a block buried half in a wall and the floor,
/// compiled through parse, BSP, exterior culling and geometry extraction,
/// then cut at a small pool edge so its long faces split into sub-faces
/// whose new corners sit mid-edge on their neighbours (T-junctions). Returns
/// the post-cut geometry, its face → brush table, and how many faces the cut
/// added.
pub(super) fn extracted_room() -> (GeometryResult, Vec<Option<u32>>, usize) {
    let [x, y, z] = ROOM;
    let w = WALL;
    let world = vec![
        box_brush([-w, -w, -w], [x + w, y + w, 0.0]),
        box_brush([-w, -w, z], [x + w, y + w, z + w]),
        box_brush([-w, -w, 0.0], [0.0, y + w, z]),
        box_brush([x, -w, 0.0], [x + w, y + w, z]),
        box_brush([0.0, -w, 0.0], [x, 0.0, z]),
        box_brush([0.0, y, 0.0], [x, y + w, z]),
        box_brush([64.0, 64.0, 0.0], [192.0, 192.0, 32.0]),
        box_brush([128.0, 128.0, 0.0], [256.0, 256.0, 32.0]),
        box_brush([-8.0, 256.0, -8.0], [40.0, 320.0, 96.0]),
    ];
    let light = entity(
        "light",
        "\"origin\" \"256 192 128\"\n\"light\" \"300\"\n\"_falloff_range\" \"800\"",
        &[],
    );
    let map = parse_inline(&format!("{}\n{light}\n", entity("worldspawn", "", &world)));
    let result = partition::partition(&map.brush_volumes).expect("partition");
    let generated = portals::generate_portals(&result.tree);
    let exterior = visibility::find_exterior_leaves(&result.tree, &generated);
    let (mut geometry, brushes) =
        extract_geometry_with_brushes(&result.faces, &result.tree, &exterior);
    let before = geometry.face_index_ranges.len();
    let static_lights = StaticBakedLights::from_lights(&map.lights);
    let cut = lightmap_bake::plan_cut_charts(
        &mut geometry,
        &static_lights,
        CUT_DENSITY,
        &[],
        CUT_POOL_EDGE,
        crate::working_set::DEFAULT_MAX_BYTES,
        false,
    )
    .expect("charts plan");
    let brushes = match &cut.face_remap {
        Some(remap) => crate::geometry::remap_face_brushes(&brushes, remap),
        None => brushes,
    };
    let added = geometry.face_index_ranges.len() - before;
    (geometry, brushes, added)
}

/// The room's interior box in engine metres.
pub(super) fn room_interior() -> (DVec3, DVec3) {
    let engine = |q: [f64; 3]| DVec3::new(-q[1], q[2], -q[0]) * UNIT;
    let (a, b) = (engine([0.0; 3]), engine(ROOM));
    (a.min(b), a.max(b))
}

/// `polygon` clipped to the half-space `(p − origin) · normal ≥ 0`.
pub(super) fn clip_polygon(polygon: &[Vec3], origin: DVec3, normal: DVec3) -> Vec<Vec3> {
    let side = |p: Vec3| (p.as_dvec3() - origin).dot(normal);
    let mut out = Vec::new();
    for i in 0..polygon.len() {
        let (a, b) = (polygon[i], polygon[(i + 1) % polygon.len()]);
        let (sa, sb) = (side(a), side(b));
        if sa >= 0.0 {
            out.push(a);
        }
        if (sa >= 0.0) != (sb >= 0.0) {
            let t = sa / (sa - sb);
            out.push((a.as_dvec3() + (b - a).as_dvec3() * t).as_vec3());
        }
    }
    out
}

/// Total length of the unproven spans lying along the segment `a`–`b`.
pub(super) fn unproven_along(report: &CoverageReport, (a, b): (DVec3, DVec3)) -> f64 {
    report
        .unproven
        .iter()
        .filter(|span| lies_along(span, a, b, 1e-6))
        .map(|span| span.length)
        .sum()
}

/// Two brushes meeting on the hinge x = z = 0, 0 ≤ y ≤ `length` (local):
/// brush 0 = [−1, 0] × [0, length] × [−1, 0] and brush 1 = [0, 1] ×
/// [0, length] × [0, 1]. Brush 0's top (z = 0) is replaced by `top` and
/// brush 1's bottom by `bottom`, each a list of local loops. Returns the
/// faces, brush 1's −X slot (a hinge face) and the hinge's ends.
pub(super) fn chained_hinge(
    frame: Frame,
    length: f64,
    top: Vec<Vec<DVec3>>,
    bottom: Vec<Vec<DVec3>>,
) -> (Vec<Polygon>, usize, (DVec3, DVec3)) {
    let face = |brush: u32| {
        move |corners: Vec<DVec3>| Polygon {
            corners: corners.iter().map(|&p| frame.point(p)).collect(),
            brush: Some(brush),
        }
    };
    let mut polygons = closed_box(
        frame,
        DVec3::new(-1.0, 0.0, -1.0),
        DVec3::new(0.0, length, 0.0),
        0.0,
        0,
    );
    // Order −X, +X, −Y, +Y, −Z, +Z: brush 0's top is slot 5.
    polygons.remove(5);
    polygons.extend(top.into_iter().map(face(0)));
    let mut second = closed_box(frame, DVec3::ZERO, DVec3::new(1.0, length, 1.0), 0.0, 1);
    second.remove(4);
    let dropped = polygons.len();
    polygons.extend(second);
    polygons.extend(bottom.into_iter().map(face(1)));
    let line = (
        frame.point(DVec3::ZERO).as_dvec3(),
        frame.point(DVec3::Y * length).as_dvec3(),
    );
    (polygons, dropped, line)
}

/// Hinge stations: 0, then every `step` from `first`, then `length`.
pub(super) fn stations(length: f64, first: f64, step: f64) -> Vec<f64> {
    let mut ys = vec![0.0];
    let mut k = 0.0;
    while first + step * k < length - 1e-9 {
        ys.push(first + step * k);
        k += 1.0;
    }
    ys.push(length);
    ys.dedup();
    ys
}

/// Brush 0's top as strips between consecutive `ys`.
pub(super) fn top_strips(ys: &[f64]) -> Vec<Vec<DVec3>> {
    ys.windows(2)
        .map(|w| {
            vec![
                DVec3::new(-1.0, w[0], 0.0),
                DVec3::new(0.0, w[0], 0.0),
                DVec3::new(0.0, w[1], 0.0),
                DVec3::new(-1.0, w[1], 0.0),
            ]
        })
        .collect()
}

/// Brush 1's bottom as strips between consecutive `ys`.
pub(super) fn bottom_strips(ys: &[f64]) -> Vec<Vec<DVec3>> {
    ys.windows(2)
        .map(|w| {
            vec![
                DVec3::new(0.0, w[0], 0.0),
                DVec3::new(0.0, w[1], 0.0),
                DVec3::new(1.0, w[1], 0.0),
                DVec3::new(1.0, w[0], 0.0),
            ]
        })
        .collect()
}

/// Assert `polygons` prove closed, and that without the face at `dropped`
/// the hinge `line` reads unproven along all but `slack` of its length.
pub(super) fn assert_hinge_hole_found(
    polygons: Vec<Polygon>,
    dropped: usize,
    line: (DVec3, DVec3),
    slack: f64,
) {
    let length = (line.1 - line.0).length();
    let intact = prove(&polygons);
    assert!(
        intact.unproven.is_empty(),
        "closed brushes: {:#?}",
        intact.unproven
    );
    let mut open = polygons;
    open.remove(dropped);
    let report = prove(&open);
    let along = unproven_along(&report, line);
    assert!(
        along >= length - slack,
        "{along} m of the {length} m hinge unproven ({} spans)",
        report.unproven.len()
    );
}

/// `corners` (local) in `frame` as one face of `brush`, wound
/// counter-clockwise about `outward`.
pub(super) fn facing(frame: Frame, mut corners: Vec<DVec3>, outward: DVec3, brush: u32) -> Polygon {
    let normal = (corners[1] - corners[0]).cross(corners[2] - corners[1]);
    if normal.dot(outward) < 0.0 {
        corners.reverse();
    }
    Polygon {
        corners: corners.iter().map(|&p| frame.point(p)).collect(),
        brush: Some(brush),
    }
}

pub(super) fn permutations(n: usize) -> Vec<Vec<usize>> {
    if n == 0 {
        return vec![Vec::new()];
    }
    let mut out = Vec::new();
    for rest in permutations(n - 1) {
        for slot in 0..=rest.len() {
            let mut order = rest.clone();
            order.insert(slot, n - 1);
            out.push(order);
        }
    }
    out
}

/// `polygons` with the faces at `slots` reordered by `order`, and each
/// output face's original index.
pub(super) fn reorder(
    polygons: &[Polygon],
    slots: &[usize],
    order: &[usize],
) -> (Vec<Polygon>, Vec<usize>) {
    let mut out = polygons.to_vec();
    let mut origin: Vec<usize> = (0..polygons.len()).collect();
    for (k, &slot) in slots.iter().enumerate() {
        out[slot] = polygons[slots[order[k]]].clone();
        origin[slot] = slots[order[k]];
    }
    (out, origin)
}

/// The report's spans with each face named by its original index.
pub(super) fn spans_by_origin(report: &CoverageReport, origin: &[usize]) -> Vec<UnprovenSpan> {
    report
        .unproven
        .iter()
        .map(|span| UnprovenSpan {
            face: origin[span.face as usize] as u32,
            ..span.clone()
        })
        .collect()
}

/// Total length of the unproven spans lying within `tolerance` of the
/// segment `a`–`b`.
pub(super) fn unproven_near(
    report: &CoverageReport,
    (a, b): (DVec3, DVec3),
    tolerance: f64,
) -> f64 {
    report
        .unproven
        .iter()
        .filter(|span| lies_along(span, a, b, tolerance))
        .map(|span| span.length)
        .sum()
}

/// The weld's clusters over `polygons` under production's rule.
pub(super) fn weld_clusters(polygons: &[Polygon]) -> Vec<Cluster> {
    let (geometry, _) = polygon_geometry(polygons);
    clusters(
        &geometry,
        WeldRule {
            radius: delta(),
            keep_triangles_whole: true,
        },
    )
}

/// A wall block [−1, 0] × [−1, 1] × [−1, 1] with a beam against its +X face
/// above the line x = y = 0 (local): the beam's flat bottom (y = 0) and
/// sloped face meet the wall at the corner P = origin. The wall face splits
/// along that line, as a cell boundary splits it. Each of the three faces
/// at P carries its own copy, at local z = `copies` (wall, sloped,
/// bottom): the wall's upper face from its copy up, the bottom from its
/// copy down. `tee` adds a collinear corner at that z on the wall's upper
/// face, a T-junction of its own.
pub(super) struct BeamCorner {
    pub(super) polygons: Vec<Polygon>,
    /// Slots of the wall's upper face, the bottom and the sloped face.
    pub(super) slots: [usize; 3],
    /// P in world space.
    pub(super) corner: DVec3,
    /// The sloped face's rims in world space.
    pub(super) sloped_rims: [(DVec3, DVec3); 3],
}

pub(super) fn beam_corner(frame: Frame, copies: [f64; 3], tee: Option<f64>) -> BeamCorner {
    let (a, c, e) = (
        DVec3::new(0.0, 0.0, -1.0),
        DVec3::new(0.0, 1.0, -1.0),
        DVec3::new(1.0, 0.0, -1.0),
    );
    let [wall_p, p, bottom_p] = copies.map(|z| DVec3::Z * z);
    let mut polygons = closed_box(frame, DVec3::splat(-1.0), DVec3::new(0.0, 1.0, 1.0), 0.0, 0);
    // Order −X, +X, …: the wall's +X face is slot 1.
    polygons.remove(1);
    polygons.push(facing(
        frame,
        vec![
            DVec3::new(0.0, -1.0, -1.0),
            DVec3::new(0.0, 0.0, -1.0),
            DVec3::new(0.0, 0.0, 1.0),
            DVec3::new(0.0, -1.0, 1.0),
        ],
        DVec3::X,
        0,
    ));
    let wall = polygons.len();
    let mut upper = vec![wall_p];
    upper.extend(tee.map(|z| DVec3::Z * z));
    upper.extend([DVec3::Z, DVec3::new(0.0, 1.0, 1.0), c]);
    polygons.push(facing(frame, upper, DVec3::X, 0));
    let bottom = polygons.len();
    polygons.push(facing(frame, vec![bottom_p, a, e], DVec3::NEG_Y, 1));
    polygons.push(facing(frame, vec![a, c, e], DVec3::NEG_Z, 1));
    let sloped = polygons.len();
    polygons.push(facing(frame, vec![p, c, e], DVec3::ONE, 1));
    let world = |q: DVec3| frame.point(q).as_dvec3();
    BeamCorner {
        polygons,
        slots: [wall, bottom, sloped],
        corner: world(p),
        sloped_rims: [
            (world(p), world(c)),
            (world(c), world(e)),
            (world(e), world(p)),
        ],
    }
}

/// One face of the box `min`..`max` in `frame`: the side on `axis` at
/// `max` when `high`, wound outward. With `cut`, it is strips across that
/// axis between consecutive stations, each spanning the third axis whole.
pub(super) fn box_face(
    frame: Frame,
    (min, max): (DVec3, DVec3),
    (axis, high): (usize, bool),
    cut: Option<(usize, &[f64])>,
    brush: u32,
) -> Vec<Polygon> {
    let value = if high { max[axis] } else { min[axis] };
    let others: Vec<usize> = (0..3).filter(|&k| k != axis).collect();
    let (across, windows): (usize, Vec<[f64; 2]>) = match cut {
        Some((across, stations)) => (across, stations.windows(2).map(|w| [w[0], w[1]]).collect()),
        None => (others[0], vec![[min[others[0]], max[others[0]]]]),
    };
    let long = others
        .into_iter()
        .find(|&k| k != across)
        .expect("a third axis");
    let mut outward = DVec3::ZERO;
    outward[axis] = if high { 1.0 } else { -1.0 };
    windows
        .into_iter()
        .map(|[s0, s1]| {
            let point = |s: f64, l: f64| {
                let mut p = DVec3::ZERO;
                p[axis] = value;
                p[across] = s;
                p[long] = l;
                p
            };
            let (l0, l1) = (min[long], max[long]);
            facing(
                frame,
                vec![point(s0, l0), point(s1, l0), point(s1, l1), point(s0, l1)],
                outward,
                brush,
            )
        })
        .collect()
}
