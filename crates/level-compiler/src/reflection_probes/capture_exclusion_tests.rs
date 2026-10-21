// Capture exclusion: masked primary rays, hull exits, the leak guard and the face → brush table through the cut.
// See: context/lib/resource_management.md §5.2 (Exclusion)

use glam::{DVec3, Vec3};
use postretro_level_format::alpha_lights::AlphaShadowType;
use postretro_level_format::octahedral::irradiance_tile_source_texel;
use postretro_level_format::reflection_probes::{
    REFLECTION_PROBE_TILE_BORDER, REFLECTION_PROBE_TILE_DIMENSION,
};

use super::capture::{
    RowCapture, SUPERSAMPLE_OFFSETS, TILE_INTERIOR, capture_row, encode_tile, octahedral_direction,
    sample_angle,
};
use super::capture_exclusion::RayOutcome;
use super::capture_exclusion_test_fixtures::{
    BACK_WALL_X, CaptureFixture, PILLAR_MAX, PILLAR_MIN, ROOM_MAX, TRIM_TEXTURE, exclusion,
    pool_entity, textured_room, toward,
};
use super::hit::SceneTracer;
use super::placement::PlacedProbe;
use super::placement_test_fixtures::{engine, textured_box_brush};
use super::shading::{CaptureScene, ShadeScratch};
use super::tests::{alpha_light, tile_texel};
use crate::bake_control::BakeControl;
use crate::bvh_build::build_bvh;
use crate::geometry::remap_face_brushes;
use crate::light_namespaces::StaticBakedLights;
use crate::map_data::{FalloffModel, LightType, MapLight, ShadowType};

/// A point inside the pillar, the wall behind it, and the ceiling slab.
pub(super) const IN_PILLAR: [f64; 3] = [48.0, 0.0, 64.0];
pub(super) const IN_BACK_WALL: [f64; 3] = [136.0, 0.0, 64.0];
pub(super) const IN_CEILING: [f64; 3] = [0.0, 0.0, 136.0];

/// Points on the wall behind the pillar that the capture point sees only
/// through the pillar's body, whose sides they leave into empty space. Test
/// rays keep off the axis planes through the capture point: the BVH's slab
/// test rejects a ray lying in a box face plane (`ray_traversal.rs`).
fn wall_behind_the_pillar() -> Vec<[f64; 3]> {
    let mut points = Vec::new();
    for y in [-4.5, 0.5, 4.5] {
        for z in [32.0, 64.0, 96.0] {
            points.push([BACK_WALL_X, y, z]);
        }
    }
    points
}

/// Radiance and distance one ray reads.
pub(super) fn read(
    scene: &CaptureScene<'_>,
    origin: Vec3,
    direction: Vec3,
    excluded: &super::capture_exclusion::ProbeExclusion<'_>,
) -> (Vec3, Option<f32>) {
    scene
        .radiance(
            origin,
            direction,
            sample_angle(),
            excluded,
            &mut ShadeScratch::default(),
        )
        .unwrap()
}

pub(super) fn only_probe(probes: &[PlacedProbe]) -> &PlacedProbe {
    assert_eq!(probes.len(), 1, "the pool keeps one probe: {probes:?}");
    &probes[0]
}

/// The mip a fixture texture (one texel per map unit) reads at `distance`.
fn wall_lod(distance: f32) -> f32 {
    (distance * sample_angle() / 0.0254).max(1.0).log2().round()
}

// Acceptance: directions through the pillar's body read the wall behind it,
// in colour and distance, with distance and texture mip from the capture point.
#[test]
fn rays_through_an_excluded_pillar_read_the_wall_behind_from_the_capture_point() {
    let room = CaptureFixture::pool_room(Some(ROOM_MAX[2]));
    let pillar = room.brush_at(IN_PILLAR);
    let back_wall = room.brush_at(IN_BACK_WALL);
    let capture = room.with_scene(|_, probes| only_probe(probes).capture_point_vec3());
    let targets = wall_behind_the_pillar();

    let filtered: Vec<(Vec3, Option<f32>)> = room.with_scene(|scene, probes| {
        let probe = only_probe(probes);
        let none = exclusion(scene, probe, &[]);
        let filter = exclusion(scene, probe, &[pillar]);
        targets
            .iter()
            .map(|&q| {
                let (direction, distance) = toward(capture, q);
                let unfiltered = scene.trace(capture, direction, &none);
                let face = unfiltered.surface().expect("the room is closed").face;
                assert_eq!(
                    room.face_brush(face),
                    Some(pillar),
                    "{q:?} is behind the pillar"
                );
                match scene.trace(capture, direction, &filter) {
                    RayOutcome::Hit(hit) => {
                        assert_eq!(room.face_brush(hit.face), Some(back_wall), "{q:?}");
                        assert!((hit.distance - distance).abs() < 1e-3, "{q:?}: {hit:?}");
                    }
                    other => panic!("{q:?} reads the wall behind, not {other:?}"),
                }
                read(scene, capture, direction, &filter)
            })
            .collect()
    });

    // The same rays in the room with the pillar deleted.
    let deleted = CaptureFixture::pool_room(None);
    let reference: Vec<(Vec3, Option<f32>)> = deleted.with_scene(|scene, probes| {
        let none = exclusion(scene, only_probe(probes), &[]);
        targets
            .iter()
            .map(|&q| read(scene, capture, toward(capture, q).0, &none))
            .collect()
    });
    for ((colour, distance), ((expected, expected_distance), q)) in
        filtered.iter().zip(reference.iter().zip(&targets))
    {
        assert!(
            (*colour - *expected).abs().max_element() < 1e-5,
            "{q:?}: {colour} vs {expected}"
        );
        assert!(
            (distance.unwrap() - expected_distance.unwrap()).abs() < 1e-4,
            "{q:?}: {distance:?} vs {expected_distance:?}"
        );
        // The wall's mips differ: from the capture point its footprint reads
        // level 1 (blue); measured from the pillar's exit it would read
        // level 0 (green).
        let exit = engine([PILLAR_MAX[0], 0.0, 0.0]).z as f32 - capture.z;
        let past_exit = distance.unwrap() * (1.0 - exit / (engine(*q).z as f32 - capture.z));
        assert_eq!(
            (wall_lod(distance.unwrap()), wall_lod(past_exit)),
            (1.0, 0.0)
        );
        assert!(colour.z > 0.1 && colour.y < 1e-4, "{q:?}: {colour}");
    }
}

// Acceptance: the wall behind keeps the pillar's sdf shadow, because the
// capture's shadow rays still see the pillar. The fixture's faces carry no
// lightmap, so no lightmap shadow exists for the wall to read instead.
#[test]
fn the_wall_behind_an_excluded_pillar_keeps_its_sdf_shadow() {
    let light = alpha_light([0.0, 1.6, -0.6], [2.0; 3], 12.0, AlphaShadowType::Sdf);
    let mut room = CaptureFixture::pool_room(Some(ROOM_MAX[2]));
    let pillar = room.brush_at(IN_PILLAR);
    let capture = room.with_scene(|_, probes| only_probe(probes).capture_point_vec3());
    // Behind the pillar from both the capture point and the light; and a
    // point the light reaches past the pillar's side.
    let shadowed = toward(capture, [BACK_WALL_X, 0.5, 63.0]).0;
    let lit = toward(capture, [BACK_WALL_X, -100.0, 63.0]).0;

    let reads = |room: &CaptureFixture, excluded: &[u32]| {
        room.with_scene(|scene, probes| {
            let filter = exclusion(scene, only_probe(probes), excluded);
            [shadowed, lit].map(|direction| read(scene, capture, direction, &filter).0)
        })
    };
    let dark = reads(&room, &[pillar]);
    room.alpha_lights.lights = vec![light.clone()];
    let [in_shadow, in_light] = reads(&room, &[pillar]);
    let mut deleted = CaptureFixture::pool_room(None);
    deleted.alpha_lights.lights = vec![light];
    let [open_behind, open_light] = reads(&deleted, &[]);

    assert!(
        (in_shadow - dark[0]).abs().max_element() < 1e-5,
        "{in_shadow} vs unlit {}",
        dark[0]
    );
    assert!(
        open_behind.z > in_shadow.z + 0.05,
        "{open_behind} vs {in_shadow}"
    );
    assert!(
        (in_light - open_light).abs().max_element() < 1e-5,
        "{in_light} vs {open_light}"
    );
    assert!(
        in_light.z > dark[1].z + 0.05,
        "{in_light} vs unlit {}",
        dark[1]
    );

    // And in the encoded colour tile.
    let tile = |room: &CaptureFixture, excluded: Vec<u32>| {
        room.with_scene(|scene, probes| {
            super::capture_section(
                scene,
                probes,
                &[excluded],
                &[],
                &BakeControl::unrestricted(),
            )
            .unwrap()
            .0
        })
    };
    let filtered = tile(&room, vec![pillar]);
    let open = tile(&deleted, Vec::new());
    let behind = tile_texel(&filtered, 0, shadowed);
    assert!(
        tile_texel(&open, 0, shadowed).z > behind.z + 0.05,
        "the tile keeps the shadow: {behind}"
    );
}

/// Each interior row of the probe's tile, captured under `excluded`.
fn rows(scene: &CaptureScene<'_>, probe: &PlacedProbe, excluded: &[u32]) -> Vec<RowCapture> {
    let filter = exclusion(scene, probe, excluded);
    (0..TILE_INTERIOR)
        .map(|row| capture_row(scene, probe.capture_point_vec3(), &filter, row, &[]).unwrap())
        .collect()
}

// Acceptance (first slice): texels no unfiltered supersample sent into the
// pillar are bit-identical before encoding, colour and distance, and every
// BC6H block holding none of the pillar's texels is byte-identical.
#[test]
fn texels_no_ray_sent_into_the_pillar_capture_bit_identically_when_it_is_excluded() {
    let room = CaptureFixture::pool_room(Some(ROOM_MAX[2]));
    let pillar = room.brush_at(IN_PILLAR);
    room.with_scene(|scene, probes| {
        let probe = only_probe(probes);
        let capture = probe.capture_point_vec3();
        let none = exclusion(scene, probe, &[]);
        let interior = TILE_INTERIOR as usize;
        // Texels any unfiltered supersample reads the pillar in.
        let mut touched = vec![false; interior * interior];
        for y in 0..interior {
            for x in 0..interior {
                touched[y * interior + x] = SUPERSAMPLE_OFFSETS.iter().any(|[dx, dy]| {
                    let u = (x as f32 + 0.5 + dx) / interior as f32;
                    let v = (y as f32 + 0.5 + dy) / interior as f32;
                    let outcome = scene.trace(capture, octahedral_direction(u, v), &none);
                    outcome
                        .surface()
                        .is_some_and(|hit| room.face_brush(hit.face) == Some(pillar))
                });
            }
        }
        let unfiltered = rows(scene, probe, &[]);
        let filtered = rows(scene, probe, &[pillar]);
        let mut changed = 0;
        for y in 0..interior {
            for x in 0..interior {
                let (a, b) = (&unfiltered[y], &filtered[y]);
                if touched[y * interior + x] {
                    changed += usize::from(a.texels[x] != b.texels[x]);
                    continue;
                }
                assert_eq!(
                    a.texels[x].to_array().map(f32::to_bits),
                    b.texels[x].to_array().map(f32::to_bits),
                    "texel ({x}, {y})"
                );
                assert_eq!(
                    a.distances[x].to_bits(),
                    b.distances[x].to_bits(),
                    "({x}, {y})"
                );
            }
        }
        assert!(
            changed > 0,
            "excluding the pillar changes the texels that saw it"
        );

        let encode = |rows: &[RowCapture]| {
            let texels: Vec<&[Vec3]> = rows.iter().map(|r| r.texels.as_slice()).collect();
            encode_tile(&texels)
        };
        let (a, b) = (encode(&unfiltered), encode(&filtered));
        let dimension = REFLECTION_PROBE_TILE_DIMENSION;
        let blocks = dimension / 4;
        let mut untouched_blocks = 0;
        for by in 0..blocks {
            for bx in 0..blocks {
                let holds_pillar = (0..16).any(|i| {
                    let [sx, sy] = irradiance_tile_source_texel(
                        bx * 4 + i % 4,
                        by * 4 + i / 4,
                        dimension,
                        REFLECTION_PROBE_TILE_BORDER,
                    );
                    touched[sy as usize * interior + sx as usize]
                });
                if holds_pillar {
                    continue;
                }
                untouched_blocks += 1;
                let at = ((by * blocks + bx) * 16) as usize;
                assert_eq!(a[at..at + 16], b[at..at + 16], "block ({bx}, {by})");
            }
        }
        assert!(untouched_blocks > 0);
    });
}

// O10, rule 4: a pillar whose top stops short of the ceiling leaves the
// ceiling whole. A ray leaving its top is in empty space and re-traces from
// the exit onto the intact ceiling, never past it into the leak guard. Brush
// CSG still treats a gap up to about 1 mm as contact and cuts the hole, so
// the gap here is 2 mm.
#[test]
fn rays_through_a_pillar_short_of_the_ceiling_re_trace_onto_the_intact_ceiling() {
    let short = ROOM_MAX[2] - 0.002 / 0.0254;
    let room = CaptureFixture::pool_room(Some(short));
    let pillar = room.brush_at(IN_PILLAR);
    let ceiling = room.brush_at(IN_CEILING);
    room.with_scene(|scene, probes| {
        let probe = only_probe(probes);
        let capture = probe.capture_point_vec3();
        let filter = exclusion(scene, probe, &[pillar]);
        for x in [44.0, 48.0, 52.0] {
            let (direction, distance) = toward(capture, [x, 0.5, ROOM_MAX[2]]);
            match scene.trace(capture, direction, &filter) {
                RayOutcome::Hit(hit) => {
                    assert_eq!(room.face_brush(hit.face), Some(ceiling), "{x}");
                    assert!((hit.distance - distance).abs() < 1e-3, "{x}: {hit:?}");
                }
                other => panic!("{x}: reads the ceiling, not {other:?}"),
            }
        }
    });
}

// Acceptance: a small brush embedded in a kept brush, with no coplanar kept
// face where the ray crosses into it, is excluded and reads its own surface
// where it is the first hit. Here a cap encloses a kept beam's end, so the
// beam's end side survives nowhere.
#[test]
fn an_excluded_cap_embedded_in_a_kept_beam_reads_its_own_surface() {
    let mut world = textured_room();
    world.push(textured_box_brush(
        [80.0, -8.0, 48.0],
        [128.0, 8.0, 64.0],
        "test/ceiling",
    ));
    world.push(textured_box_brush(
        [72.0, -16.0, 40.0],
        [88.0, 16.0, 72.0],
        TRIM_TEXTURE,
    ));
    let room = CaptureFixture::compile(&world, &[pool_entity()]);
    let cap = room.brush_at([76.0, 0.0, 44.0]);
    room.with_scene(|scene, probes| {
        let probe = only_probe(probes);
        let capture = probe.capture_point_vec3();
        let none = exclusion(scene, probe, &[]);
        let filter = exclusion(scene, probe, &[cap]);
        // At the beam's axis on the cap's back plane: the ray enters the
        // cap's front, crosses into the beam, and leaves the cap inside it.
        for (y, z) in [(0.5, 56.0), (-4.5, 52.0), (4.5, 60.0)] {
            let (direction, _) = toward(capture, [88.0, y, z]);
            let front = engine([72.0, 0.0, 0.0]).z as f32;
            let entry_distance = (front - capture.z) / direction.z;
            let own = scene.trace(capture, direction, &none);
            let hit = *own.surface().expect("the cap faces the ray");
            assert_eq!(room.face_brush(hit.face), Some(cap));
            assert!((hit.distance - entry_distance).abs() < 1e-3, "{hit:?}");
            assert_eq!(
                scene.trace(capture, direction, &filter),
                RayOutcome::LeakGuard(hit)
            );
            let (colour, distance) = read(scene, capture, direction, &filter);
            let (expected, expected_distance) = read(scene, capture, direction, &none);
            assert_eq!((colour, distance), (expected, expected_distance));
            assert!(
                colour.x > 0.1 && colour.z > 0.1 && colour.y < 1e-4,
                "{colour}"
            );
        }
    });
}

// O2: the oversize cut renumbers faces; the face → brush table follows it,
// each sub-face taking its parent's brush and an emptied face none, and the
// mask built from it skips exactly the pillar's post-cut faces even where a
// cut kept face precedes them.
#[test]
fn oversize_cut_remaps_each_sub_face_to_its_parent_brush_and_the_mask_follows() {
    // A face the cut emptied leaves no post-cut entry.
    let remapped = remap_face_brushes(&[Some(4), Some(7), Some(9)], &[0..2, 2..2, 2..3]);
    assert_eq!(remapped, [Some(4), Some(4), Some(9)]);

    let room = CaptureFixture::pool_room(Some(ROOM_MAX[2]));
    let pillar = room.brush_at(IN_PILLAR);
    let mut geometry = room.world.geometry.clone();
    let lights = [MapLight {
        origin: DVec3::new(0.0, 2.0, 0.0),
        carrier: String::new(),
        light_type: LightType::Point,
        intensity: 1.0,
        color: [1.0; 3],
        falloff_model: FalloffModel::Linear,
        falloff_range: 20.0,
        light_size: 0.0,
        angular_diameter: 0.0,
        cone_angle_inner: None,
        cone_angle_outer: None,
        cone_direction: None,
        animation: None,
        bake_only: false,
        is_dynamic: false,
        casts_entity_shadows: false,
        is_animated: false,
        tags: Vec::new(),
        shadow_type: ShadowType::StaticLightMap,
    }];
    // A small pool edge forces the cut: 3.25 m at 0.1 m per texel passes 32.
    let cut = crate::lightmap_bake::plan_cut_charts(
        &mut geometry,
        &StaticBakedLights::from_lights(&lights),
        0.1,
        &[],
        32,
        crate::working_set::DEFAULT_MAX_BYTES,
        false,
    )
    .expect("fixture charts plan");
    let face_remap = cut.face_remap.expect("the walls are cut");
    let before = &room.world.face_brushes;
    let after = remap_face_brushes(before, &face_remap);
    assert_eq!(after.len(), geometry.geometry.faces.len());
    for (old, range) in face_remap.iter().enumerate() {
        for new in range.clone() {
            assert_eq!(after[new], before[old], "sub-face {new} of face {old}");
        }
    }

    // The pillar's post-cut faces, found from the geometry alone: every
    // vertex on the pillar's hull.
    let (lo, hi) = (
        engine(PILLAR_MIN).min(engine(PILLAR_MAX)),
        engine(PILLAR_MIN).max(engine(PILLAR_MAX)),
    );
    let on_pillar: Vec<u32> = geometry
        .face_index_ranges
        .iter()
        .enumerate()
        .filter(|(_, range)| {
            let start = range.index_offset as usize;
            geometry.geometry.indices[start..start + range.index_count as usize]
                .iter()
                .all(|&i| {
                    let p = DVec3::from(
                        geometry.geometry.vertices[i as usize]
                            .position
                            .map(f64::from),
                    );
                    p.cmpge(lo - 1e-3).all() && p.cmple(hi + 1e-3).all()
                })
        })
        .map(|(face, _)| face as u32)
        .collect();
    let first_pillar_face = *on_pillar.first().expect("the pillar keeps faces");
    assert!(
        face_remap
            .iter()
            .enumerate()
            .any(|(old, range)| range.len() > 1
                && before[old] != Some(pillar)
                && range.start < first_pillar_face as usize),
        "a cut kept face precedes the pillar's faces"
    );
    let by_table: Vec<u32> = (0..after.len() as u32)
        .filter(|&face| after[face as usize] == Some(pillar))
        .collect();
    assert_eq!(by_table, on_pillar);
    let unremapped: Vec<u32> = (0..before.len() as u32)
        .filter(|&face| before[face as usize] == Some(pillar))
        .collect();
    assert_ne!(unremapped, on_pillar, "the cut shifts the pillar's faces");

    let (bvh, primitives, _) = build_bvh(&geometry).unwrap();
    let tracer = SceneTracer::new(&bvh, &primitives, &geometry);
    assert_eq!(
        tracer.face_mask(by_table),
        tracer.face_mask(on_pillar.iter().copied())
    );
}
