// Surface radiance cache tests: patch tiling, seeds, lookup and layer determinism.
// See: context/lib/build_pipeline.md §Compiler pipeline

use std::collections::HashSet;

use glam::{DVec3, Vec3};
use postretro_level_format::geometry::{FaceMeta, GeometrySection, Vertex};
use postretro_level_format::texture_names::TextureNamesSection;
use rayon::ThreadPoolBuilder;

use super::layer::PATCHES_PER_CHUNK;
use super::*;
use crate::bake_control::BakeControl;
use crate::bvh_build::build_bvh;
use crate::geometry::{FaceIndexRange, GeometryResult, extract_geometry};
use crate::map_data::{Face, FalloffModel, LightType, MapLight, ShadowType, TextureProjection};
use crate::partition::{Aabb, BspLeaf, BspTree};
use crate::sh_bake::{RaytracingCtx, gather_hit};

/// Geometry with one fan-triangulated face per convex polygon, in order.
pub(super) fn polygon_geometry(polygons: &[Vec<Vec3>]) -> GeometryResult {
    let (mut vertices, mut indices, mut faces, mut ranges) = (vec![], vec![], vec![], vec![]);
    for polygon in polygons {
        let base = vertices.len() as u32;
        vertices.extend(polygon.iter().map(|p| {
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
        for k in 1..polygon.len() as u32 - 1 {
            indices.extend([base, base + k, base + k + 1]);
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
    GeometryResult {
        geometry: GeometrySection {
            vertices,
            indices,
            faces,
        },
        texture_names: TextureNamesSection { names: Vec::new() },
        face_index_ranges: ranges,
    }
}

/// An axis-aligned rectangle at height `y`, wound to face `+Y`.
fn floor(min: [f32; 2], max: [f32; 2], y: f32) -> Vec<Vec3> {
    vec![
        Vec3::new(min[0], y, min[1]),
        Vec3::new(min[0], y, max[1]),
        Vec3::new(max[0], y, max[1]),
        Vec3::new(max[0], y, min[1]),
    ]
}

/// A regular pentagon of radius `r` about `centre` in the plane of `a`, `b`.
fn pentagon(centre: Vec3, a: Vec3, b: Vec3, r: f32) -> Vec<Vec3> {
    (0..5)
        .map(|k| {
            let t = k as f32 * std::f32::consts::TAU / 5.0;
            centre + (a * t.cos() + b * t.sin()) * r
        })
        .collect()
}

/// A polygon's area and unit normal, from its summed fan cross products.
fn area_and_normal(polygon: &[Vec3]) -> (f64, DVec3) {
    let p0 = polygon[0].as_dvec3();
    let twice: DVec3 = polygon
        .windows(2)
        .skip(1)
        .map(|pair| (pair[0].as_dvec3() - p0).cross(pair[1].as_dvec3() - p0))
        .sum();
    (twice.length() / 2.0, twice.normalize())
}

fn varied_faces() -> Vec<Vec<Vec3>> {
    let tilt_a = Vec3::new(1.0, 0.4, 0.2).normalize();
    let tilt_b = tilt_a.cross(Vec3::new(0.1, 0.3, 1.0)).normalize();
    vec![
        floor([0.1, -0.3], [1.4, 1.8], 0.2),
        pentagon(Vec3::new(3.0, 1.1, -2.0), tilt_a, tilt_b, 1.1),
        // Smaller than one cell, straddling a cell corner.
        vec![
            Vec3::new(-0.1, 2.0, -0.05),
            Vec3::new(0.1, 2.0, 0.08),
            Vec3::new(0.12, 2.0, -0.07),
        ],
    ]
}

fn ray_ctx<'a>(
    geometry: &'a GeometryResult,
    bvh: &'a bvh::bvh::Bvh<f32, 3>,
    primitives: &'a [crate::bvh_build::BvhPrimitive],
) -> RaytracingCtx<'a> {
    RaytracingCtx {
        bounce_materials: None,
        bvh,
        primitives,
        geometry,
    }
}

fn soft_point(origin: [f64; 3], light_size: f32) -> MapLight {
    MapLight {
        origin: DVec3::from(origin),
        carrier: String::new(),
        light_type: LightType::Point,
        intensity: 1.0,
        color: [1.0, 1.0, 1.0],
        falloff_model: FalloffModel::Linear,
        falloff_range: 20.0,
        light_size,
        angular_diameter: 0.0,
        cone_angle_inner: None,
        cone_angle_outer: None,
        cone_direction: None,
        animation: None,
        bake_only: false,
        is_dynamic: false,
        casts_entity_shadows: false,
        is_animated: false,
        tags: vec![],
        shadow_type: ShadowType::StaticLightMap,
    }
}

#[test]
fn patch_areas_on_each_face_sum_to_the_face_area() {
    let polygons = varied_faces();
    let geometry = polygon_geometry(&polygons);
    for size in [PATCH_SIZE_M, 0.25] {
        let patches = PatchSet::build_with_size(&geometry, size);
        for (face, polygon) in polygons.iter().enumerate() {
            let (area, normal) = area_and_normal(polygon);
            let face_patches = patches.face_patches(face as u32);
            assert!(!face_patches.is_empty(), "face {face} has a patch");
            let sum: f64 = face_patches.iter().map(|p| f64::from(p.area)).sum();
            assert!(
                (sum - area).abs() <= 1e-5 * area,
                "face {face} at {size} m: patches {sum} m², face {area} m²"
            );
            for patch in face_patches {
                assert_eq!(patch.face, face as u32);
                assert!(patch.normal.as_dvec3().dot(normal) > 0.999_99);
                let off_plane = (patch.centroid - polygon[0]).as_dvec3().dot(normal);
                assert!(
                    off_plane.abs() < 1e-4,
                    "centroid {off_plane} m off its face"
                );
                assert!(patch.area as f64 <= size * size * (1.0 + 1e-6));
            }
            assert!(
                face_patches.windows(2).all(|w| w[0].cell < w[1].cell),
                "a face's patches ascend by cell, one per cell"
            );
        }
    }
}

#[test]
fn patch_seed_ids_follow_the_face_and_cell_not_the_patch_rank() {
    let polygons = varied_faces();
    let before = PatchSet::build(&polygon_geometry(&polygons[..2]));
    let after = PatchSet::build(&polygon_geometry(&polygons));
    assert!(after.len() > before.len());
    for (a, b) in before.patches().iter().zip(after.patches()) {
        assert_eq!(a, b, "a face added after leaves earlier patches");
        assert_eq!(a.seed_id(), b.seed_id());
        assert_eq!(a.seed_id(), patch_seed_id(a.face, a.cell));
    }
    let seeds: HashSet<u64> = after.patches().iter().map(Patch::seed_id).collect();
    assert_eq!(seeds.len(), after.len(), "every patch seeds apart");
}

#[test]
fn patches_cover_only_the_faces_exterior_culling_keeps() {
    let face = |polygon: Vec<Vec3>| {
        let vertices: Vec<DVec3> = polygon.iter().map(|p| p.as_dvec3()).collect();
        let (_, normal) = area_and_normal(&polygon);
        Face {
            distance: normal.dot(vertices[0]),
            vertices,
            normal,
            texture: "t".to_owned(),
            tex_projection: TextureProjection::Standard {
                u_offset: 0.0,
                v_offset: 0.0,
                angle: 0.0,
                scale_u: 1.0,
                scale_v: 1.0,
            },
            brush_index: 0,
        }
    };
    let faces = vec![
        face(floor([0.0, 0.0], [2.0, 2.0], 0.0)),
        face(floor([5.0, 0.0], [7.0, 2.0], 0.0)),
    ];
    let leaf = |face_indices: Vec<usize>| BspLeaf {
        face_indices,
        bounds: Aabb {
            min: DVec3::splat(-10.0),
            max: DVec3::splat(10.0),
        },
        is_solid: false,
        defining_planes: Vec::new(),
    };
    let tree = BspTree {
        nodes: Vec::new(),
        leaves: vec![leaf(vec![0]), leaf(vec![1])],
    };
    let exterior: HashSet<usize> = [1].into();
    let geometry = extract_geometry(&faces, &tree, &exterior);
    let patches = PatchSet::build(&geometry);
    assert_eq!(geometry.geometry.faces.len(), 1);
    assert!(!patches.patches().is_empty());
    assert!(
        patches.patches().iter().all(|p| p.centroid.x < 2.0),
        "the exterior leaf's face holds no patch"
    );
    let area: f32 = patches.patches().iter().map(|p| p.area).sum();
    assert!((area - 4.0).abs() < 1e-4);
}

#[test]
fn lookup_returns_the_patch_holding_the_hit_and_the_side_the_ray_comes_from() {
    let geometry = polygon_geometry(&[
        floor([-1.0, -1.0], [2.0, 2.0], 0.0),
        floor([-1.0, -1.0], [2.0, 2.0], 3.0),
    ]);
    let (bvh, primitives, _) = build_bvh(&geometry).unwrap();
    let ctx = ray_ctx(&geometry, &bvh, &primitives);
    let patches = PatchSet::build(&geometry);
    let size = PATCH_SIZE_M as f32;
    for (origin, dir, side) in [
        (Vec3::new(0.3, 1.0, 0.7), Vec3::NEG_Y, Side::Front),
        (Vec3::new(0.3, -1.0, 0.7), Vec3::Y, Side::Back),
        (Vec3::new(1.7, 2.0, -0.6), Vec3::Y, Side::Back),
        (Vec3::new(1.7, 2.0, -0.6), Vec3::NEG_Y, Side::Front),
    ] {
        let hit = gather_hit(&ctx, origin, dir).expect("the ray hits a floor");
        let face = patches.face_of_triangle(hit.triangle).unwrap();
        let (patch, got) = patches.lookup(hit.triangle, hit.point, dir).unwrap();
        let patch = patches.patches()[patch as usize];
        assert_eq!(got, side, "ray from {origin} along {dir}");
        assert_eq!(patch.face, face);
        // A full cell's centroid is its centre, half a cell from every edge.
        let offset = (hit.point - patch.centroid).abs();
        assert!(offset.x <= size / 2.0 + 1e-4 && offset.z <= size / 2.0 + 1e-4);
    }

    // A point whose cell holds no patch (past the face's edge) reads the
    // face's nearest patch.
    let hit = gather_hit(&ctx, Vec3::new(0.0, 1.0, 0.0), Vec3::NEG_Y).unwrap();
    let outside = Vec3::new(5.0, 0.0, 5.0);
    let (patch, _) = patches.lookup(hit.triangle, outside, Vec3::NEG_Y).unwrap();
    let face = patches.face_of_triangle(hit.triangle).unwrap();
    let nearest = patches
        .face_patches(face)
        .iter()
        .map(|p| p.centroid.distance(outside))
        .fold(f32::INFINITY, f32::min);
    assert_eq!(
        patches.patches()[patch as usize].centroid.distance(outside),
        nearest
    );
}

/// A 6 m floor under a soft light, partly shadowed by a hovering plate.
fn shadowed_floor() -> GeometryResult {
    let mut plate = floor([-0.5, -2.0], [0.5, 2.0], 1.0);
    plate.reverse(); // faces the floor
    polygon_geometry(&[floor([-3.0, -3.0], [3.0, 3.0], 0.0), plate])
}

#[test]
fn a_layer_is_byte_identical_at_one_worker_and_many() {
    let geometry = shadowed_floor();
    let (bvh, primitives, _) = build_bvh(&geometry).unwrap();
    let ctx = ray_ctx(&geometry, &bvh, &primitives);
    let patches = PatchSet::build_with_size(&geometry, 0.25);
    let light = soft_point([0.2, 2.0, 0.1], 0.4);
    let in_reach = PatchReachIndex::new(patches.patches()).in_reach(&light);
    let control = BakeControl::unrestricted();
    let bits = |layer: &SparseLayer| -> Vec<(u32, Side, [u32; 3])> {
        layer
            .entries
            .iter()
            .map(|e| (e.patch, e.side, e.radiance.map(f32::to_bits)))
            .collect()
    };
    for embedded in [false, true] {
        let at = |threads| {
            let pool = ThreadPoolBuilder::new()
                .num_threads(threads)
                .build()
                .unwrap();
            pool.install(|| {
                shade_light_layer(&ctx, &patches, &in_reach, &light, 7, embedded, &control)
            })
        };
        let one = at(1);
        assert!(in_reach.len() > PATCHES_PER_CHUNK, "the layer spans chunks");
        assert!(
            one.entries
                .windows(2)
                .all(|w| (w[0].patch, w[0].side) < (w[1].patch, w[1].side)),
            "entries ascend by patch and side"
        );
        let lit: Vec<f32> = one.entries.iter().map(|e| e.radiance[0]).collect();
        let brightest = lit.iter().copied().fold(0.0, f32::max);
        assert!(
            lit.iter().any(|&r| r > 0.0 && r < 0.5 * brightest),
            "the plate shadows part of the floor"
        );
        assert_eq!(bits(&one), bits(&at(8)), "embedded {embedded}");
    }
}

#[test]
fn a_layer_bakes_the_lights_colour_and_intensity_in() {
    let geometry = shadowed_floor();
    let (bvh, primitives, _) = build_bvh(&geometry).unwrap();
    let ctx = ray_ctx(&geometry, &bvh, &primitives);
    let patches = PatchSet::build(&geometry);
    let control = BakeControl::unrestricted();
    let white = soft_point([0.2, 2.0, 0.1], 0.0);
    let mut red = white.clone();
    red.color = [1.0, 0.0, 0.0];
    red.intensity = 2.0;
    let in_reach = PatchReachIndex::new(patches.patches()).in_reach(&white);
    let white = shade_light_layer(&ctx, &patches, &in_reach, &white, 0, false, &control);
    let red = shade_light_layer(&ctx, &patches, &in_reach, &red, 0, false, &control);
    assert!(!white.entries.is_empty());
    assert_eq!(white.entries.len(), red.entries.len());
    for (w, r) in white.entries.iter().zip(&red.entries) {
        assert_eq!((w.patch, w.side), (r.patch, r.side));
        assert_eq!(r.radiance[0], 2.0 * w.radiance[0]);
        assert_eq!([r.radiance[1], r.radiance[2]], [0.0, 0.0]);
    }
    let mut sum = PatchRadiance::zeroed(patches.len());
    sum.add(&white);
    sum.add(&red);
    let entry = white.entries[0];
    assert_eq!(
        sum.get(entry.patch, entry.side),
        Vec3::from(entry.radiance) + Vec3::from(red.entries[0].radiance)
    );
}

// The bucketed reach count equals the bounce's own range test at every
// patch, for lights near, far, straddling bucket edges, off the patch set,
// and of a range wider than the level.
#[test]
fn reach_count_equals_the_range_test_at_every_patch() {
    let geometry = polygon_geometry(&[
        floor([-20.0, -20.0], [20.0, 20.0], 0.0),
        floor([-3.0, -3.0], [3.0, 3.0], 2.5),
    ]);
    let patches = PatchSet::build(&geometry);
    let index = PatchReachIndex::new(patches.patches());
    let mut lights = Vec::new();
    for (origin, range) in [
        ([0.0, 1.0, 0.0], 3.0),
        ([4.0, 0.5, -4.0], 4.0),
        ([-19.9, 0.1, 19.9], 7.5),
        ([30.0, 1.0, 0.0], 5.0),
        ([0.0, 2.0, 0.0], 500.0),
    ] {
        let mut light = soft_point(origin, 0.0);
        light.falloff_range = range;
        lights.push(light);
    }
    let refs: Vec<&MapLight> = lights.iter().collect();
    let counts = index.counts_in_reach(&refs);
    for (light, count) in lights.iter().zip(counts) {
        let brute = patches
            .patches()
            .iter()
            .filter(|patch| crate::sh_bake::light_reaches_point(light, patch.centroid))
            .count() as u64;
        assert_eq!(count, brute, "{:?}", light.origin);
    }
    let mut sun = soft_point([0.0, 5.0, 0.0], 0.0);
    sun.light_type = LightType::Directional;
    assert_eq!(index.count_in_reach(&sun), patches.len() as u64);
}

// AC 31, the gate's half: a light's layer holds at most one entry per side
// of each patch the gate counted in its reach, and only those patches, so
// the gate's layer term bounds what shading allocates. Progress advances
// one unit per chunk of the reach.
#[test]
fn a_layer_holds_entries_only_on_patches_the_gate_counts_in_reach() {
    let geometry = shadowed_floor();
    let (bvh, primitives, _) = build_bvh(&geometry).unwrap();
    let ctx = ray_ctx(&geometry, &bvh, &primitives);
    let patches = PatchSet::build_with_size(&geometry, 0.25);
    let index = PatchReachIndex::new(patches.patches());
    for (origin, range) in [([0.2, 2.0, 0.1], 20.0), ([2.5, 0.4, -2.5], 1.5)] {
        let mut light = soft_point(origin, 0.4);
        light.falloff_range = range;
        let progress = crate::reporter::StageProgress::indeterminate();
        let control = BakeControl::new(
            std::sync::Arc::new(crate::governor::Governor::new(usize::MAX, false)),
            &progress,
        );
        let in_reach = index.in_reach(&light);
        let layer = shade_light_layer(&ctx, &patches, &in_reach, &light, 0, false, &control);
        let reach = index.count_in_reach(&light);
        assert_eq!(in_reach.len() as u64, reach);
        assert!(in_reach.windows(2).all(|w| w[0] < w[1]), "ascending");
        assert!(!layer.entries.is_empty());
        assert!(layer.entries.len() as u64 <= 2 * reach, "{origin:?}");
        assert!(layer.entries.iter().all(|entry| {
            crate::sh_bake::light_reaches_point(
                &light,
                patches.patches()[entry.patch as usize].centroid,
            )
        }));
        assert_eq!(
            progress.completed(),
            in_reach.len().div_ceil(PATCHES_PER_CHUNK)
        );
    }
}
