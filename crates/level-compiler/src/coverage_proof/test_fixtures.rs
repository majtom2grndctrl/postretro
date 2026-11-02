// Coverage proof fixtures: polygon geometry, closed boxes, and a sealed room compiled through extraction and the oversize-face cut.
// See: context/lib/testing_guide.md §4

use glam::{DQuat, DVec3, Vec3};
use postretro_level_format::geometry::{FaceMeta, GeometrySection, Vertex};
use postretro_level_format::texture_names::TextureNamesSection;

use crate::geometry::{FaceIndexRange, GeometryResult, extract_geometry_with_brushes};
use crate::light_namespaces::StaticBakedLights;
use crate::reflection_probes::placement_test_fixtures::{box_brush, entity, parse_inline};
use crate::{lightmap_bake, partition, portals, visibility};

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
