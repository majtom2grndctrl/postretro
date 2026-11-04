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
use super::capture_bounds::{HELD_DEPTH_SLACK_M, Held, nearest_held};
use super::capture_exclusion::{CONTACT_TOLERANCE_M, RayOutcome};
use super::capture_exclusion_test_fixtures::{
    BACK_WALL_X, CaptureFixture, PILLAR_MAX, PILLAR_MIN, ROOM_MAX, ROOM_MIN, TRIM_TEXTURE,
    exclusion, pool_entity, textured_room, toward,
};
use super::hit::{FaceMask, RAY_EPSILON, SceneTracer, SurfaceHit};
use super::placement::PlacedProbe;
use super::placement_test_fixtures::{engine, engine_box, textured_box_brush};
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
/// rays keep off the axis planes through the capture point, so none runs
/// along a fixture face's edge or plane.
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

// Directions through the pillar's body read the wall behind it,
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

// The wall behind keeps the pillar's sdf shadow, because the
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

// Texels no unfiltered supersample sent into the
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

// A pillar whose top stops short of the ceiling leaves the ceiling whole. A
// ray leaving its top is in empty space and re-traces from the exit onto the
// intact ceiling, never past it into the leak guard. Brush CSG still treats a
// gap up to about 1 mm as contact and cuts the hole, so the gap here is 2 mm.
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

// A ray leaving a flush pillar's back side half a millimetre under the
// ceiling is in empty space under the intact ceiling, though within CSG's
// contact epsilon of its plane. Level, it never enters the ceiling; rising
// gently, it enters a quarter metre on. Neither takes the ceiling as kept
// solid behind the exit: they re-trace onto the wall behind and onto the
// ceiling past the pillar.
#[test]
fn rays_leaving_a_flush_pillars_side_just_under_the_ceiling_re_trace_past_the_pillar() {
    let room = CaptureFixture::pool_room(Some(ROOM_MAX[2]));
    let pillar = room.brush_at(IN_PILLAR);
    let (back_wall, ceiling) = (room.brush_at(IN_BACK_WALL), room.brush_at(IN_CEILING));
    let under = ROOM_MAX[2] - 0.0005 / 0.0254;
    // Map +x is engine −z; the exit lies on the pillar's back side.
    let exit = engine([PILLAR_MAX[0], 0.5, under]).as_vec3();
    let level = Vec3::NEG_Z;
    let rising = Vec3::new(0.0, 0.002, -1.0).normalize();
    room.with_scene(|scene, probes| {
        let filter = exclusion(scene, only_probe(probes), &[pillar]);
        for (direction, brush, distance) in [
            (
                level,
                back_wall,
                1.5 + (BACK_WALL_X - PILLAR_MAX[0]) as f32 * 0.0254,
            ),
            (rising, ceiling, 1.5 + 0.0005 / rising.y),
        ] {
            let origin = exit - direction * 1.5;
            let none = exclusion(scene, only_probe(probes), &[]);
            let first = *scene.trace(origin, direction, &none).surface().unwrap();
            assert_eq!(room.face_brush(first.face), Some(pillar), "{direction}");
            match scene.trace(origin, direction, &filter) {
                RayOutcome::Hit(hit) => {
                    assert_eq!(room.face_brush(hit.face), Some(brush), "{direction}");
                    assert!((hit.distance - distance).abs() < 1e-3, "{hit:?}");
                }
                other => panic!("{direction} re-traces, not {other:?}"),
            }
        }
    });
}

// A small brush embedded in a kept brush, with no coplanar kept face where
// the ray crosses into it, is excluded and reads its own surface where it is
// the first hit. Here a cap encloses a kept beam's end, so the beam's end
// side survives nowhere.
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

// An excluded block half a millimetre in front of a kept beam covers the
// beam's end whole. Brush CSG treats the gap as contact and cuts both facing
// faces, so the beam's end survives nowhere to fill from. A ray through the
// block toward that end leaves the block into the beam's solid and takes the
// leak guard; it never re-traces through the beam onto the wall behind.
#[test]
fn rays_through_an_excluded_block_against_a_covered_kept_end_take_the_leak_guard() {
    let gap = 0.0005 / 0.0254;
    let mut world = textured_room();
    world.push(textured_box_brush(
        [80.0, -8.0, 48.0],
        [96.0, 8.0, 64.0],
        "test/ceiling",
    ));
    world.push(textured_box_brush(
        [72.0, -16.0, 40.0],
        [80.0 - gap, 16.0, 72.0],
        TRIM_TEXTURE,
    ));
    let room = CaptureFixture::compile(&world, &[pool_entity()]);
    let beam = room.brush_at([88.0, 0.0, 56.0]);
    let block = room.brush_at([76.0, 0.0, 56.0]);
    assert_eq!(
        room.faces_on_plane(beam, 0, [80.0, 0.0, 0.0]),
        0,
        "CSG cuts the beam's end at a half-millimetre gap"
    );
    room.with_scene(|scene, probes| {
        let probe = only_probe(probes);
        let capture = probe.capture_point_vec3();
        let none = exclusion(scene, probe, &[]);
        let filter = exclusion(scene, probe, &[block]);
        for (y, z) in [(0.5, 56.0), (-4.5, 52.0), (4.5, 60.0)] {
            let (direction, _) = toward(capture, [80.0, y, z]);
            let first = *scene
                .trace(capture, direction, &none)
                .surface()
                .expect("the block faces the ray");
            assert_eq!(room.face_brush(first.face), Some(block), "{y}, {z}");
            assert_eq!(
                scene.trace(capture, direction, &filter),
                RayOutcome::LeakGuard(first),
                "{y}, {z}"
            );
        }
    });
}

// Regression: a slanted ray crossing a sub-millimetre gap read the ceiling
// behind a kept beam.
// A ray rising through the block's bottom at 10° off the beam's end plane
// leaves the block's back half a millimetre from that end, 2.9 mm along the
// ray, into the beam's solid: it takes the leak guard, never re-tracing
// through the beam's cut end.
#[test]
fn slanted_rays_through_an_excluded_block_against_a_covered_kept_end_take_the_leak_guard() {
    let gap = 0.0005 / 0.0254;
    let mut world = textured_room();
    world.push(textured_box_brush(
        [80.0, -8.0, 48.0],
        [96.0, 8.0, 64.0],
        "test/ceiling",
    ));
    world.push(textured_box_brush(
        [72.0, -16.0, 40.0],
        [80.0 - gap, 16.0, 72.0],
        TRIM_TEXTURE,
    ));
    let room = CaptureFixture::compile(&world, &[pool_entity()]);
    let block = room.brush_at([76.0, 0.0, 56.0]);
    let slant = 10f64.to_radians();
    let rise = engine([slant.sin(), 0.0, slant.cos()]).normalize();
    room.with_scene(|scene, probes| {
        let probe = only_probe(probes);
        let none = exclusion(scene, probe, &[]);
        let filter = exclusion(scene, probe, &[block]);
        for (y, z) in [(0.5, 52.0), (-4.5, 56.0), (4.5, 60.0)] {
            let exit = engine([80.0 - gap, y, z]);
            let (origin, direction) = ((exit - rise * 0.6).as_vec3(), rise.as_vec3());
            let first = *scene
                .trace(origin, direction, &none)
                .surface()
                .expect("the block faces the ray");
            assert_eq!(room.face_brush(first.face), Some(block), "{y}, {z}");
            assert_eq!(
                scene.trace(origin, direction, &filter),
                RayOutcome::LeakGuard(first),
                "{y}, {z}"
            );
        }
    });
}

// The oversize cut renumbers faces; the face → brush table follows it,
// each sub-face taking its parent's brush, and the mask built from it skips
// exactly the pillar's post-cut faces even where a cut kept face precedes
// them.
#[test]
fn oversize_cut_remaps_each_sub_face_to_its_parent_brush_and_the_mask_follows() {
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

/// The pillar's top for the tolerance-hit rows, in map units: half the
/// room's height, so its top face and back edge stand clear of the ceiling.
const LOW_PILLAR_TOP: f64 = 64.0;

/// A point inside the pillar topped at [`LOW_PILLAR_TOP`].
const IN_LOW_PILLAR: [f64; 3] = [48.0, 0.0, 32.0];

/// Map units per metre.
pub(super) const UNITS_PER_METER: f64 = 1.0 / 0.0254;

/// `probe` with its projection box the map-unit box `min`..`max`.
pub(super) fn boxed(probe: &PlacedProbe, min: [f64; 3], max: [f64; 3]) -> PlacedProbe {
    let (lo, hi) = engine_box(min, max);
    PlacedProbe {
        box_min: lo.as_vec3().to_array(),
        box_max: hi.as_vec3().to_array(),
        ..probe.clone()
    }
}

/// A ray crossing the low pillar's top plane 0.3 mm past its back edge,
/// within τ of the top face, and descending away from the pillar toward the
/// wall behind: it passes 0.15 mm over the back edge and never enters the
/// hull. Its origin, 1 m before the crossing; its direction; and the
/// distance from the origin to the wall behind, about 3 m.
fn past_the_pillar_top() -> (Vec3, Vec3, f32) {
    let crossing = engine([
        PILLAR_MAX[0] + 0.0003 * UNITS_PER_METER,
        0.5,
        LOW_PILLAR_TOP,
    ]);
    let direction = engine([1.0, 0.0, -0.5]).normalize();
    let wall = engine([BACK_WALL_X, 0.0, 0.0]).z;
    let to_wall = 1.0 + (wall - crossing.z) / direction.z;
    (
        (crossing - direction).as_vec3(),
        direction.as_vec3(),
        to_wall as f32,
    )
}

// A tolerance hit on an excluded pillar's top, from a ray that passes beside
// the hull, crosses nothing: the ray reads the wall behind as plainly hit,
// with no leak guard. The projection box stops short of that wall, so a
// ray that had skipped the pillar would take the guard there instead.
#[test]
fn a_tolerance_hit_beside_an_excluded_pillar_skips_nothing_and_reads_the_wall_behind() {
    let room = CaptureFixture::pool_room(Some(LOW_PILLAR_TOP));
    let pillar = room.brush_at(IN_LOW_PILLAR);
    let back_wall = room.brush_at(IN_BACK_WALL);
    let (origin, direction, to_wall) = past_the_pillar_top();
    room.with_scene(|scene, probes| {
        let probe = boxed(&probes[0], ROOM_MIN, [96.0, ROOM_MAX[1], ROOM_MAX[2]]);
        let none = exclusion(scene, &probe, &[]);
        let first = *scene
            .trace(origin, direction, &none)
            .surface()
            .expect("the top faces the ray");
        assert_eq!(room.face_brush(first.face), Some(pillar), "{first:?}");
        assert!((first.distance - 1.0).abs() < 1e-3, "{first:?}");
        let filter = exclusion(scene, &probe, &[pillar]);
        match scene.trace(origin, direction, &filter) {
            RayOutcome::Hit(hit) => {
                assert_eq!(room.face_brush(hit.face), Some(back_wall), "{hit:?}");
                assert!((hit.distance - to_wall).abs() < 1e-3, "{hit:?}");
            }
            other => panic!("reads the wall behind, not {other:?}"),
        }
    });
}

// The same ray ending short of the wall reaches nothing past the tolerance
// hit: it skipped no excluded brush, so it stays a miss.
#[test]
fn a_tolerance_hit_beside_an_excluded_pillar_then_nothing_stays_a_miss() {
    let room = CaptureFixture::pool_room(Some(LOW_PILLAR_TOP));
    let pillar = room.brush_at(IN_LOW_PILLAR);
    let (origin, direction, to_wall) = past_the_pillar_top();
    // Past the tolerance hit at 1 m, short of the wall.
    let reach = 1.5;
    assert!(to_wall > reach + 1.0, "{to_wall}");
    room.with_scene(|scene, probes| {
        let none = exclusion(scene, &probes[0], &[]);
        let first = none.trace(&scene.tracer, origin, direction, reach);
        assert_eq!(
            first.surface().map(|hit| room.face_brush(hit.face)),
            Some(Some(pillar)),
            "{first:?}"
        );
        let filter = exclusion(scene, &probes[0], &[pillar]);
        assert_eq!(
            filter.trace(&scene.tracer, origin, direction, reach),
            RayOutcome::Miss
        );
    });
}

// The same tolerance hit with the projection box stopping short of the
// pillar: past a point outside the box the ray can only end outside it, so
// it takes the leak guard on the pillar's top, the face the kept pillar
// shows, rather than reading the wall behind. A brush off the box captures
// the same kept or excluded.
#[test]
fn a_tolerance_hit_beside_an_excluded_pillar_off_the_box_takes_the_leak_guard_on_it() {
    let room = CaptureFixture::pool_room(Some(LOW_PILLAR_TOP));
    let pillar = room.brush_at(IN_LOW_PILLAR);
    let (origin, direction, _) = past_the_pillar_top();
    room.with_scene(|scene, probes| {
        let probe = boxed(
            &probes[0],
            ROOM_MIN,
            [PILLAR_MIN[0] - 8.0, ROOM_MAX[1], ROOM_MAX[2]],
        );
        let none = exclusion(scene, &probe, &[]);
        let kept = scene.trace(origin, direction, &none);
        let RayOutcome::Hit(first) = kept else {
            panic!("the top faces the ray, not {kept:?}");
        };
        assert_eq!(room.face_brush(first.face), Some(pillar), "{first:?}");
        let filter = exclusion(scene, &probe, &[pillar]);
        assert_eq!(
            scene.trace(origin, direction, &filter),
            RayOutcome::LeakGuard(first)
        );
    });
}

// A ray within τ of the low pillar's top back corner, outside the hull,
// takes a tolerance hit on both front faces that meet there, the top and
// the side, and never enters the hull. The first is no crossing, so the
// trace drops the pillar, the second face with it: the ray still reaches
// the floor past the pillar.
#[test]
fn two_tolerance_hits_at_an_excluded_pillars_corner_still_reach_the_kept_hit() {
    let room = CaptureFixture::pool_room(Some(LOW_PILLAR_TOP));
    let pillar = room.brush_at(IN_LOW_PILLAR);
    let floor = room.brush_at([72.0, 24.0, -8.0]);
    // 0.2 mm past the back, 0.1 mm off the side and 0.1 mm over the top.
    let near_corner = engine([
        PILLAR_MAX[0] + 0.0002 * UNITS_PER_METER,
        PILLAR_MIN[1] - 0.0001 * UNITS_PER_METER,
        LOW_PILLAR_TOP + 0.0001 * UNITS_PER_METER,
    ]);
    // Down through the top's plane, then into the side's (map +y), heading
    // away from the back (map +x): the top's crossing lies 0.23 mm off the
    // top and the side's 0.25 mm off the side.
    let direction = engine([0.5, 1.0, -2.0]).normalize();
    let to_floor = (1.0 + near_corner.y / -direction.y) as f32;
    let (origin, direction) = ((near_corner - direction).as_vec3(), direction.as_vec3());
    room.with_scene(|scene, probes| {
        let mask = scene
            .tracer
            .face_mask(room.brush_faces.of(pillar).iter().copied());
        // Steps past each tolerance hit, only to show that two of the
        // pillar's faces take one; the trace below drops the pillar at the
        // first instead.
        let mut min = RAY_EPSILON;
        let mut beside = Vec::new();
        let kept = loop {
            let masked = scene.tracer.closest_hit_masked(
                origin,
                direction,
                min,
                scene.max_distance,
                &mask,
                &FaceMask::default(),
            );
            let Some(skipped) = masked.skipped else {
                break masked.hit;
            };
            assert_eq!(room.face_brush(skipped.face), Some(pillar), "{skipped:?}");
            assert!((skipped.distance - 1.0).abs() < 1e-3, "{skipped:?}");
            beside.push(skipped.face);
            min = skipped.distance;
        };
        beside.sort_unstable();
        beside.dedup();
        assert!(
            beside.len() >= 2,
            "two faces take tolerance hits: {beside:?}"
        );
        assert_eq!(kept.map(|hit| room.face_brush(hit.face)), Some(Some(floor)));

        let probe = boxed(&probes[0], ROOM_MIN, ROOM_MAX);
        let filter = exclusion(scene, &probe, &[pillar]);
        match scene.trace(origin, direction, &filter) {
            RayOutcome::Hit(hit) => {
                assert_eq!(room.face_brush(hit.face), Some(floor), "{hit:?}");
                assert!((hit.distance - to_floor).abs() < 1e-3, "{hit:?}");
            }
            other => panic!("reaches the floor, not {other:?}"),
        }
    });
}

/// The contact-gap row's gap between an excluded block and the floor under
/// it, in map units: 0.8 mm, inside CSG's 1 mm contact epsilon, so CSG
/// removes the floor's top under the block.
pub(super) const CONTACT_GAP: f64 = 0.0008 * UNITS_PER_METER;

// Regression: a ray through the contact gap under an excluded block took a
// tolerance hit on the block's front, 0.2 mm under its bottom edge, and so
// passed beside the hull. The trace dropped the block and re-traced, and the
// ray crossed the floor's plane 6 mm inside the top CSG removed under the
// block, more than τ from any surviving face, and ran on through the floor
// to a miss. It reads the floor's fill there, as a ray entering the block
// would; kept, the block shows its front.
#[test]
fn a_ray_through_the_contact_gap_under_an_excluded_block_fills_the_floor() {
    let mut world = textured_room();
    world.push(textured_box_brush(
        [100.0, -8.0, CONTACT_GAP],
        [116.0, 8.0, 32.0],
        TRIM_TEXTURE,
    ));
    let room = CaptureFixture::compile(&world, &[pool_entity()]);
    let block = room.brush_at([108.0, 0.0, 16.0]);
    let floor = room.brush_at([72.0, 24.0, -8.0]);
    // Across the block's front (map x = 100) 0.6 mm over the floor,
    // descending at a slope of 0.1, so it meets the floor's plane 6 mm past
    // the front. Its origin lies 1 m before the crossing, past the pool.
    let crossing = engine([100.0, 0.5, 0.0006 * UNITS_PER_METER]);
    let direction = engine([1.0, 0.0, -0.1]).normalize();
    let (origin, direction) = ((crossing - direction).as_vec3(), direction.as_vec3());
    room.with_scene(|scene, probes| {
        // Straight down from the gap, the ray finds no floor top.
        let gap = engine([108.0, 0.0, CONTACT_GAP / 2.0]).as_vec3();
        let down = scene
            .tracer
            .closest_hit(gap, Vec3::NEG_Y, scene.max_distance);
        assert!(
            down.is_none_or(|hit| room.face_brush(hit.face) != Some(floor)),
            "CSG removed the floor's top under the block: {down:?}"
        );
        let probe = boxed(&probes[0], ROOM_MIN, ROOM_MAX);
        let none = exclusion(scene, &probe, &[]);
        let kept = scene.trace(origin, direction, &none);
        let RayOutcome::Hit(front) = kept else {
            panic!("the block's front faces the ray, not {kept:?}");
        };
        assert_eq!(room.face_brush(front.face), Some(block), "{front:?}");
        assert!((front.distance - 1.0).abs() < 1e-3, "{front:?}");
        let filter = exclusion(scene, &probe, &[block]);
        match scene.trace(origin, direction, &filter) {
            RayOutcome::Fill(fill) => assert_eq!(fill.brush, floor, "{fill:?}"),
            other => panic!("fills the floor under the block, not {other:?}"),
        }
    });
}

/// The room with a kept platform `height` u tall on the floor, in two
/// brushes butted at map x = 100 (the near one from 84, the far one to 116),
/// and an excluded block over the far one's whole footprint, [`CONTACT_GAP`]
/// above it, up to 40 u. CSG removes the far brush's front against the near
/// one and its whole top against the block, so where a ray through the gap
/// under the block meets the far brush, none of its faces survive.
fn platform_under_a_block(height: f64) -> CaptureFixture {
    split_platform_under_a_block(height, &[[100.0, 116.0]])
}

/// [`platform_under_a_block`] with the far brush replaced by one brush per
/// map-x span of `far`, in that order after the near brush, each `height` u
/// tall from the floor. CSG removes a brush's side where it butts another
/// and its top under the block.
pub(super) fn split_platform_under_a_block(height: f64, far: &[[f64; 2]]) -> CaptureFixture {
    let far: Vec<([f64; 2], f64)> = far.iter().map(|&span| (span, height)).collect();
    platform_under_a_block_with(Some(height), &far, height + CONTACT_GAP)
}

/// The room with the excluded block of [`split_platform_under_a_block`]
/// from `block_bottom` up to 40 u over map x 100 to 116, a near brush from
/// map x 84 to 100 topped at `near` when given, and one brush per `far`
/// entry, its map-x span and its top, in that order after the near brush.
/// Every brush stands on the floor; all heights are map units.
pub(super) fn platform_under_a_block_with(
    near: Option<f64>,
    far: &[([f64; 2], f64)],
    block_bottom: f64,
) -> CaptureFixture {
    let mut world = textured_room();
    if let Some(top) = near {
        world.push(textured_box_brush(
            [84.0, -8.0, 0.0],
            [100.0, 8.0, top],
            TRIM_TEXTURE,
        ));
    }
    for &([x0, x1], top) in far {
        world.push(textured_box_brush(
            [x0, -8.0, 0.0],
            [x1, 8.0, top],
            TRIM_TEXTURE,
        ));
    }
    world.push(textured_box_brush(
        [100.0, -8.0, block_bottom],
        [116.0, 8.0, 40.0],
        TRIM_TEXTURE,
    ));
    CaptureFixture::compile(&world, &[pool_entity()])
}

// Regression: a ray through the contact gap under an excluded block, over a
// kept platform whose faces CSG removed there, took a tolerance hit on the
// block's front and passed beside it. The fill's first crossing, the
// platform's front, pooled no surviving face, so the trace dropped the block
// and the ray ran on inside the platform: out its far side onto the wall
// behind, or, over a thin platform and steeper, out its bottom through the
// floor slab to a miss. It reads what a ray 0.4 mm higher, entering the
// block, reads: kept solid past the gap, so the leak guard on the block's
// front (or a fill).
#[test]
fn a_ray_through_the_contact_gap_over_a_platform_with_no_face_left_never_passes_through_it() {
    // An 8 u platform under a ray descending at a slope of 0.1, which leaves
    // its far side; a 2 u one under a slope of 0.25, which leaves its bottom.
    // Either way the ray meets the platform's top plane more than 1.5 mm
    // past the near brush's top, so that top is never crossed.
    for (height, slope) in [(8.0, 0.1), (2.0, 0.25)] {
        let room = platform_under_a_block(height);
        let near = room.brush_at([92.0, 0.0, height / 2.0]);
        let far = room.brush_at([108.0, 0.0, height / 2.0]);
        let block = room.brush_at([108.0, 0.0, 24.0]);
        assert_eq!(
            room.faces_on_plane(far, 0, [100.0, 0.0, 0.0]),
            0,
            "{height}: CSG removes the far brush's front against the near one"
        );
        assert_eq!(
            room.faces_on_plane(far, 2, [0.0, 0.0, height]),
            0,
            "{height}: CSG removes the far brush's top under the block"
        );
        room.with_scene(|scene, probes| {
            let probe = boxed(&probes[0], ROOM_MIN, ROOM_MAX);
            let none = exclusion(scene, &probe, &[]);
            let filter = exclusion(scene, &probe, &[block]);
            // Across the block's front (map x = 100) 0.6 mm over the
            // platform, 0.2 mm under the block's bottom edge, so a tolerance
            // hit beside the hull; and 1.0 mm over, 0.2 mm inside the block's
            // front, entering it. Each origin lies 1 m before the crossing,
            // past the pool and over the near brush.
            for over in [0.0006, 0.001] {
                let crossing = engine([100.0, 0.5, height + over * UNITS_PER_METER]);
                let direction = engine([1.0, 0.0, -slope]).normalize();
                let (origin, direction) = ((crossing - direction).as_vec3(), direction.as_vec3());
                let kept = scene.trace(origin, direction, &none);
                let RayOutcome::Hit(front) = kept else {
                    panic!("{height}, {over}: the block's front faces the ray, not {kept:?}");
                };
                assert_eq!(
                    room.face_brush(front.face),
                    Some(block),
                    "{height}, {over}: {front:?}"
                );
                assert!(
                    (front.distance - 1.0).abs() < 1e-3,
                    "{height}, {over}: {front:?}"
                );
                match scene.trace(origin, direction, &filter) {
                    RayOutcome::LeakGuard(hit) => {
                        assert_eq!(hit, front, "{height}, {over}");
                    }
                    RayOutcome::Fill(fill) => assert!(
                        [near, far].contains(&fill.brush),
                        "{height}, {over}: {fill:?}"
                    ),
                    other => {
                        panic!("{height}, {over}: reads kept solid past the gap, not {other:?}")
                    }
                }
            }
        });
    }
}

/// The split-platform rows' platform height, in map units.
pub(super) const PLATFORM_HEIGHT: f64 = 8.0;

/// A ray across the block's front (map x = 100) `over` metres above a
/// [`PLATFORM_HEIGHT`] platform, descending at `slope` (map z per map x),
/// its origin 1 m before the crossing, past the pool.
pub(super) fn over_the_platform(over: f64, slope: f64) -> (Vec3, Vec3) {
    let crossing = engine([100.0, 0.5, PLATFORM_HEIGHT + over * UNITS_PER_METER]);
    let direction = engine([1.0, 0.0, -slope]).normalize();
    ((crossing - direction).as_vec3(), direction.as_vec3())
}

/// The map x where that ray meets the platform's top plane.
pub(super) fn meets_the_platform_top(over: f64, slope: f64) -> f64 {
    100.0 + over * UNITS_PER_METER / slope
}

/// How far that ray lies under the block's bottom at map x `x`, in metres.
pub(super) fn under_the_block(over: f64, slope: f64, x: f64) -> f64 {
    CONTACT_GAP / UNITS_PER_METER - over + slope * (x - 100.0) / UNITS_PER_METER
}

/// How far along that ray map x `x` lies from its origin.
pub(super) fn reaching(slope: f64, x: f64) -> f32 {
    (1.0 + (x - 100.0) / UNITS_PER_METER * (1.0 + slope * slope).sqrt()) as f32
}

/// The far brush split at map x = 108 into a kept brush in front and an
/// excluded one behind, both under the excluded block, and the ray of
/// [`over_the_platform`] meeting the kept brush's top before x = 108 and
/// entering the excluded one there within contact of the block. Both hold
/// the block's exit; the kept one is met first, so the ray takes the leak
/// guard on the block's front, the same as with only the block excluded.
fn a_kept_brush_met_first_takes_the_leak_guard(over: f64, slope: f64) {
    assert!(
        meets_the_platform_top(over, slope) < 108.0,
        "the kept top is met first"
    );
    assert!(
        under_the_block(over, slope, 108.0) <= CONTACT_TOLERANCE_M,
        "the excluded brush is entered within contact of the block"
    );
    let h = PLATFORM_HEIGHT;
    let room = split_platform_under_a_block(h, &[[100.0, 108.0], [108.0, 116.0]]);
    let kept = room.brush_at([104.0, 0.0, h / 2.0]);
    let excluded = room.brush_at([112.0, 0.0, h / 2.0]);
    let block = room.brush_at([108.0, 0.0, 24.0]);
    assert_eq!(
        room.faces_on_plane(kept, 0, [100.0, 0.0, 0.0]),
        0,
        "CSG removes the kept brush's front against the near one"
    );
    assert_eq!(
        room.faces_on_plane(kept, 2, [0.0, 0.0, h]),
        0,
        "CSG removes the kept brush's top under the block"
    );
    let (origin, direction) = over_the_platform(over, slope);
    room.with_scene(|scene, probes| {
        let probe = boxed(&probes[0], ROOM_MIN, ROOM_MAX);
        let none = exclusion(scene, &probe, &[]);
        let unmasked = scene.trace(origin, direction, &none);
        let RayOutcome::Hit(front) = unmasked else {
            panic!("the block's front faces the ray, not {unmasked:?}");
        };
        assert_eq!(room.face_brush(front.face), Some(block), "{front:?}");
        assert!((front.distance - 1.0).abs() < 1e-3, "{front:?}");
        let filter = exclusion(scene, &probe, &[block, excluded]);
        let slot = filter.excluded().binary_search(&block).unwrap();
        let (kept_contacts, excluded_contacts) = filter.contacts_of(slot);
        assert!(kept_contacts.contains(&kept), "{kept_contacts:?}");
        assert_eq!(excluded_contacts, vec![excluded]);
        let outcome = scene.trace(origin, direction, &filter);
        assert_eq!(outcome, RayOutcome::LeakGuard(front));
        let block_only = exclusion(scene, &probe, &[block]);
        assert_eq!(
            scene.trace(origin, direction, &block_only),
            outcome,
            "excluding the brush behind changes nothing"
        );
    });
}

// Regression: a ray through the contact gap under an excluded block, over a
// kept brush whose front and top CSG removed and an excluded brush behind
// it, went on through the excluded brush, the first to hold the exit in
// brush order, though the ray met the kept top first. The re-trace from the
// excluded brush's far side read the wall behind through the kept solid.
// The ray passes 0.2 mm under the block's front edge, a tolerance hit
// beside the hull, at a slope of 0.004: it meets the kept top at map x ≈
// 105.9 and enters the excluded brush at 108, 1.01 mm under the block.
#[test]
fn a_pass_beside_a_block_meeting_a_kept_brush_before_an_excluded_one_takes_the_leak_guard() {
    let over = 0.0006;
    assert!(over * UNITS_PER_METER < CONTACT_GAP, "under the block");
    a_kept_brush_met_first_takes_the_leak_guard(over, 0.004);
}

// Regression: the real crossing's half of the same ordering. The ray enters
// the block's front 0.2 mm over its bottom edge and leaves its bottom 1.3 u
// on, at a slope of 0.006: it meets the kept top at map x ≈ 106.6 and enters
// the excluded brush at 108, 1.02 mm under the block. Brush order went on
// through the excluded brush and read the wall behind.
#[test]
fn a_ray_crossing_a_block_meeting_a_kept_brush_before_an_excluded_one_takes_the_leak_guard() {
    let over = 0.001;
    assert!(over * UNITS_PER_METER > CONTACT_GAP, "inside the block");
    a_kept_brush_met_first_takes_the_leak_guard(over, 0.006);
}

// Kept solid behind an excluded brush, both under the excluded block and
// holding its exit: the excluded brush is entered first, so the ray goes on
// through it and reads the kept brush's front past its far side, as
// `resource_management.md` §5.2 allows. Checking kept solid first, at any
// depth, would take the leak guard here. A 1 u kept sliver at the block's
// front, its front removed against the near brush, is the fill's first
// crossing, so no face fills and the exit is classified. The ray passes
// 0.2 mm under the block's front edge at a slope of 0.006: it enters the
// excluded brush (map x 101 to 105) through its top at x ≈ 103.9, and the
// kept brush from x = 106 holds the block's exit 1.11 mm under it.
#[test]
fn a_kept_brush_behind_an_excluded_one_reads_its_front_past_the_excluded_far_side() {
    let (over, slope) = (0.0006, 0.006);
    let top = meets_the_platform_top(over, slope);
    assert!(
        top > 101.0 + CONTACT_TOLERANCE_M * UNITS_PER_METER && top < 105.0,
        "the ray enters the excluded brush's top, never the sliver: {top}"
    );
    assert!(
        under_the_block(over, slope, 106.0) <= CONTACT_TOLERANCE_M,
        "the kept brush holds the block's exit"
    );
    let h = PLATFORM_HEIGHT;
    let room = split_platform_under_a_block(h, &[[100.0, 101.0], [101.0, 105.0], [106.0, 116.0]]);
    let sliver = room.brush_at([100.5, 0.0, h / 2.0]);
    let excluded = room.brush_at([103.0, 0.0, h / 2.0]);
    let kept = room.brush_at([111.0, 0.0, h / 2.0]);
    let block = room.brush_at([108.0, 0.0, 24.0]);
    assert_eq!(
        room.faces_on_plane(sliver, 0, [100.0, 0.0, 0.0]),
        0,
        "CSG removes the sliver's front against the near brush"
    );
    assert!(
        room.faces_on_plane(kept, 0, [106.0, 0.0, 0.0]) > 0,
        "the kept brush's front faces the gap"
    );
    let (origin, direction) = over_the_platform(over, slope);
    room.with_scene(|scene, probes| {
        let probe = boxed(&probes[0], ROOM_MIN, ROOM_MAX);
        let none = exclusion(scene, &probe, &[]);
        let unmasked = scene.trace(origin, direction, &none);
        let RayOutcome::Hit(front) = unmasked else {
            panic!("the block's front faces the ray, not {unmasked:?}");
        };
        assert_eq!(room.face_brush(front.face), Some(block), "{front:?}");
        assert!((front.distance - 1.0).abs() < 1e-3, "{front:?}");
        let filter = exclusion(scene, &probe, &[block, excluded]);
        let slot = |brush: u32| filter.excluded().binary_search(&brush).unwrap();
        let (kept_contacts, excluded_contacts) = filter.contacts_of(slot(block));
        assert!(
            kept_contacts.contains(&sliver) && kept_contacts.contains(&kept),
            "{kept_contacts:?}"
        );
        assert_eq!(excluded_contacts, vec![excluded]);
        // So the kept brush is read by the re-trace, not as kept solid
        // behind the excluded brush's exit.
        let (behind, _) = filter.contacts_of(slot(excluded));
        assert!(!behind.contains(&kept), "{behind:?}");
        // The ray lies under the kept brush's top at its front, so it enters
        // the kept brush there: the front is the depth the kept brush held
        // at the block's exit, which the excluded brush, nearer, passed over.
        // Reading it is the at-the-depth case of the carried depth, the read
        // kept-first would have guarded.
        assert!(
            under_the_block(over, slope, 106.0) > CONTACT_GAP / UNITS_PER_METER,
            "the kept brush's entry is its front"
        );
        match scene.trace(origin, direction, &filter) {
            RayOutcome::Hit(hit) => {
                assert_eq!(room.face_brush(hit.face), Some(kept), "{hit:?}");
                assert!(
                    (f64::from(hit.distance) - f64::from(reaching(slope, 106.0))).abs()
                        <= HELD_DEPTH_SLACK_M,
                    "{hit:?}"
                );
            }
            other => panic!("reads the kept brush's front, not {other:?}"),
        }
    });
}

// Regression: a kept brush and an excluded one overlapping where a ray
// leaves an excluded block both hold its exit at the exit itself, an exact
// tie; the excluded one won it and the ray went on through it. The kept
// brush ends inside the excluded one along the ray, so its exit lay past
// the kept solid, and the re-trace read the wall behind through it. Kept
// solid wins the tie: the leak guard. The kept brush is pushed before the
// excluded one, so `brush_at` names it inside both; its front lies inside
// the block, and its top shows only past the block, over the excluded one.
#[test]
fn kept_and_excluded_brushes_overlapping_at_a_blocks_exit_tie_to_the_leak_guard() {
    let mut world = textured_room();
    world.push(textured_box_brush(
        [100.0, -8.0, 16.0],
        [116.0, 8.0, 40.0],
        TRIM_TEXTURE,
    ));
    world.push(textured_box_brush(
        [112.0, -6.0, 20.0],
        [120.0, 6.0, 39.5],
        TRIM_TEXTURE,
    ));
    world.push(textured_box_brush(
        [110.0, -7.0, 18.0],
        [124.0, 7.0, 38.0],
        TRIM_TEXTURE,
    ));
    let room = CaptureFixture::compile(&world, &[pool_entity()]);
    let block = room.brush_at([105.0, 0.0, 28.0]);
    let kept = room.brush_at([118.0, 0.0, 28.0]);
    let excluded = room.brush_at([122.0, 0.0, 28.0]);
    assert_eq!(
        room.brush_at([118.0, 0.0, 39.0]),
        kept,
        "the kept brush, not the excluded one"
    );
    assert_eq!(
        room.faces_on_plane(kept, 0, [112.0, 0.0, 0.0]),
        0,
        "the kept brush's front lies in solid, so no face fills"
    );
    // Level but for a 1e-5 descent across the block's front; its origin lies
    // 1 m before the crossing.
    let crossing = engine([100.0, 0.5, 28.0]);
    let direction = engine([1.0, 0.0, -1.0e-5]).normalize();
    let (origin, direction) = ((crossing - direction).as_vec3(), direction.as_vec3());
    room.with_scene(|scene, probes| {
        let probe = boxed(&probes[0], ROOM_MIN, ROOM_MAX);
        let none = exclusion(scene, &probe, &[]);
        let unmasked = scene.trace(origin, direction, &none);
        let RayOutcome::Hit(front) = unmasked else {
            panic!("the block's front faces the ray, not {unmasked:?}");
        };
        assert_eq!(room.face_brush(front.face), Some(block), "{front:?}");
        assert!((front.distance - 1.0).abs() < 1e-3, "{front:?}");
        let filter = exclusion(scene, &probe, &[block, excluded]);
        let slot = filter.excluded().binary_search(&block).unwrap();
        let (kept_contacts, excluded_contacts) = filter.contacts_of(slot);
        assert!(kept_contacts.contains(&kept), "{kept_contacts:?}");
        assert_eq!(excluded_contacts, vec![excluded]);
        assert_eq!(
            scene.trace(origin, direction, &filter),
            RayOutcome::LeakGuard(front)
        );
    });
}

// An exit's held neighbours decide by depth: the nearest wins whatever its
// slot, kept solid wins an exact tie, and excluded brushes order by depth,
// not slot (brush order).
#[test]
fn the_nearest_held_neighbour_decides_and_kept_solid_wins_a_tie() {
    let excluded = |slot, leaves| Some(Held::Excluded { slot, leaves });
    assert_eq!(nearest_held([2.0], [(2.0, 0, 3.0)]), Some(Held::Kept));
    assert_eq!(nearest_held([1.5], [(2.0, 0, 3.0)]), Some(Held::Kept));
    assert_eq!(nearest_held([2.5], [(2.0, 0, 3.0)]), excluded(0, 3.0));
    assert_eq!(
        nearest_held([9.0], [(5.0, 0, 6.0), (4.0, 1, 7.0)]),
        excluded(1, 7.0)
    );
    assert_eq!(
        nearest_held([3.0, 1.0], std::iter::empty::<(f64, usize, f64)>()),
        Some(Held::Kept)
    );
    assert_eq!(
        nearest_held(
            std::iter::empty::<f64>(),
            std::iter::empty::<(f64, usize, f64)>()
        ),
        None
    );
}

// A pass beside an excluded hull that drops it spends none of the trace's
// hull passes (one per excluded brush, plus one). A ray 0.2 mm over two low
// blocks' tops, level but for a 1e-5 descent, takes a tolerance hit on each
// front and passes beside both, then crosses a tall block behind them and
// reads the wall behind: one pass, where two spent beside the low blocks
// would leave three.
#[test]
fn passes_beside_two_excluded_blocks_spend_no_hull_pass() {
    let low_top = 32.0;
    let mut world = textured_room();
    for (x, top) in [(64.0, low_top), (80.0, low_top), (96.0, 2.0 * low_top)] {
        world.push(textured_box_brush(
            [x, -8.0, 0.0],
            [x + 8.0, 8.0, top],
            TRIM_TEXTURE,
        ));
    }
    let room = CaptureFixture::compile(&world, &[pool_entity()]);
    let blocks = [68.0, 84.0, 100.0].map(|x| room.brush_at([x, 0.0, 16.0]));
    let back_wall = room.brush_at(IN_BACK_WALL);
    // Across the first block's front (map x = 64); its origin lies 1 m
    // before, over the pool's water.
    let crossing = engine([64.0, 0.5, low_top + 0.0002 * UNITS_PER_METER]);
    let direction = engine([1.0, 0.0, -1.0e-5]).normalize();
    let (origin, direction) = ((crossing - direction).as_vec3(), direction.as_vec3());
    room.with_scene(|scene, probes| {
        let probe = boxed(&probes[0], ROOM_MIN, ROOM_MAX);
        let none = exclusion(scene, &probe, &[]);
        let kept = scene.trace(origin, direction, &none);
        let RayOutcome::Hit(front) = kept else {
            panic!("the first block's front faces the ray, not {kept:?}");
        };
        assert_eq!(room.face_brush(front.face), Some(blocks[0]), "{front:?}");
        assert!((front.distance - 1.0).abs() < 1e-3, "{front:?}");
        // With the first block's faces masked, the kept hit is the second
        // block's front, 16 u on: a tolerance hit too.
        let first_masked = scene
            .tracer
            .face_mask(room.brush_faces.of(blocks[0]).iter().copied());
        let behind = scene
            .tracer
            .closest_hit_masked(
                origin,
                direction,
                RAY_EPSILON,
                scene.max_distance,
                &first_masked,
                &FaceMask::default(),
            )
            .hit
            .expect("the second block's front faces the ray");
        assert_eq!(room.face_brush(behind.face), Some(blocks[1]), "{behind:?}");
        let to_second = 1.0 + (16.0 / UNITS_PER_METER) as f32;
        assert!((behind.distance - to_second).abs() < 1e-3, "{behind:?}");
        let filter = exclusion(scene, &probe, &blocks);
        let mut hull_passes = 0;
        match filter.trace_counting(
            &scene.tracer,
            origin,
            direction,
            scene.max_distance,
            &mut hull_passes,
        ) {
            RayOutcome::Hit(hit) => {
                assert_eq!(room.face_brush(hit.face), Some(back_wall), "{hit:?}");
            }
            other => panic!("reads the wall behind, not {other:?}"),
        }
        assert_eq!(hull_passes, 1, "only the tall block's crossing spends one");
    });
}

/// The two hits a ray takes on the faces of brushes `pair` at one `f32`
/// distance, in the order the exclusion trace takes them: the first skipped
/// with both masked, then the first with that one's brush ignored.
fn tied_hits(
    room: &CaptureFixture,
    scene: &CaptureScene<'_>,
    (origin, direction): (Vec3, Vec3),
    pair: [u32; 2],
) -> [SurfaceHit; 2] {
    let faces = |brushes: &[u32]| {
        scene.tracer.face_mask(
            brushes
                .iter()
                .flat_map(|&brush| room.brush_faces.of(brush).iter().copied()),
        )
    };
    let mask = faces(&pair[..]);
    let first_skipped = |ignored: &FaceMask| {
        scene
            .tracer
            .closest_hit_masked(
                origin,
                direction,
                RAY_EPSILON,
                scene.max_distance,
                &mask,
                ignored,
            )
            .skipped
            .expect("both brushes face the ray")
    };
    let first = first_skipped(&FaceMask::default());
    let first_brush = room.face_brush(first.face).expect("a brush owns the face");
    assert!(pair.contains(&first_brush), "{first:?}");
    let second = first_skipped(&faces(&[first_brush][..]));
    assert!(
        room.face_brush(second.face)
            .is_some_and(|brush| brush != first_brush && pair.contains(&brush)),
        "{second:?}"
    );
    // The tie that hit order decides.
    assert_eq!(first.distance, second.distance, "{first:?} vs {second:?}");
    [first, second]
}

/// The seam between two crates stacked against the wall behind, in map
/// units: the lower crate's top and the upper's bottom.
const CRATE_SEAM: f64 = 32.0;

/// The room with two 32 u crates stacked from the floor against the wall
/// behind (map x 96 to 128). CSG removes the faces between them, and the
/// wall's face where they cover it.
fn stacked_crates() -> CaptureFixture {
    let mut world = textured_room();
    world.push(textured_box_brush(
        [96.0, -16.0, 0.0],
        [BACK_WALL_X, 16.0, CRATE_SEAM],
        TRIM_TEXTURE,
    ));
    world.push(textured_box_brush(
        [96.0, -16.0, CRATE_SEAM],
        [BACK_WALL_X, 16.0, 2.0 * CRATE_SEAM],
        TRIM_TEXTURE,
    ));
    CaptureFixture::compile(&world, &[pool_entity()])
}

/// A ray toward the wall behind, level but for a 1e-5 descent, crossing the
/// crates' front (map x = 96) 0.2 mm above their seam (`above`) or below
/// it: inside one crate's front and within τ of the other's. Its origin
/// lies 1 m before the crossing.
fn at_the_crate_seam(above: bool) -> (Vec3, Vec3) {
    let offset = 0.0002 * UNITS_PER_METER;
    let z = if above {
        CRATE_SEAM + offset
    } else {
        CRATE_SEAM - offset
    };
    let crossing = engine([96.0, 0.5, z]);
    let direction = engine([1.0, 0.0, -1.0e-5]).normalize();
    ((crossing - direction).as_vec3(), direction.as_vec3())
}

/// Both crates excluded, the ray of [`at_the_crate_seam`] whose first hit
/// in hit order is the tolerance hit beside one crate when
/// `tolerance_hit_first`, else the real entry into the other: it reads the
/// wall's fill or the leak guard on the crate it enters, never a plain hit
/// or a miss. The two rays share the two triangles at the seam, so hit
/// order takes the tolerance hit first on exactly one of them.
fn crate_seam_reads_the_entered_crate(tolerance_hit_first: bool) {
    let room = stacked_crates();
    let lower = room.brush_at([112.0, 0.0, CRATE_SEAM / 2.0]);
    let upper = room.brush_at([112.0, 0.0, 1.5 * CRATE_SEAM]);
    let back_wall = room.brush_at(IN_BACK_WALL);
    room.with_scene(|scene, probes| {
        let probe = boxed(&probes[0], ROOM_MIN, ROOM_MAX);
        let filter = exclusion(scene, &probe, &[lower, upper]);
        let mut rays = 0;
        for above in [true, false] {
            let (origin, direction) = at_the_crate_seam(above);
            let entered = if above { upper } else { lower };
            let [first, _] = tied_hits(&room, scene, (origin, direction), [lower, upper]);
            if (room.face_brush(first.face) != Some(entered)) != tolerance_hit_first {
                continue;
            }
            rays += 1;
            match scene.trace(origin, direction, &filter) {
                RayOutcome::Fill(fill) => assert_eq!(fill.brush, back_wall, "{fill:?}"),
                RayOutcome::LeakGuard(hit) => {
                    assert_eq!(room.face_brush(hit.face), Some(entered), "{hit:?}");
                }
                other => panic!("above {above}: enters the crate, not {other:?}"),
            }
        }
        assert_eq!(rays, 1, "exactly one ray takes this order");
    });
}

// Regression: a tolerance hit beside one excluded crate, taken first at the
// same f32 distance as the real entry into the crate stacked on it, moved
// the trace's bound past that distance. The entry was stepped over, and the
// ray ran on inside the crate, its back faces culled, through the wall's
// removed face to a miss.
#[test]
fn a_crate_seam_ray_taking_the_tolerance_hit_first_still_enters_the_other_crate() {
    crate_seam_reads_the_entered_crate(true);
}

// The other order: the real entry first, the tolerance hit beside the
// other crate behind it at the same distance.
#[test]
fn a_crate_seam_ray_taking_the_real_entry_first_enters_its_crate() {
    crate_seam_reads_the_entered_crate(false);
}

/// Which branch of the exclusion trace decides a seam ray whose two hits tie.
#[derive(Debug, Clone, Copy, PartialEq)]
enum SeamArm {
    /// The kept block wins the tie: no excluded face is skipped.
    KeptFirst,
    /// The excluded block's tolerance hit wins: a pass beside it, off the
    /// box, takes the leak guard on that face.
    OffBoxBeside,
    /// The excluded block's real entry wins: a crossing. Its fill on the
    /// kept block's front, or the kept hit tying it, lies past the box edge,
    /// so the box check after the fill takes the leak guard on that face.
    OffBoxCrossing,
}

// Regression: a tie between a kept face and an off-box excluded brush at a
// seam went to whichever the traversal reached first when both were kept,
// but to the kept face when the brush was excluded, so excluding a brush
// wholly off the box changed the capture. Two blocks side by side share a
// seam at map y = 64, their fronts coplanar. A ray within τ of the seam on
// either side ties the two fronts, and reads the same face, at the same
// distance, kept or excluded. Two layouts of one compile: the box stops 1 u
// short of the seam with the far block excluded, or 1 u short of the near
// block with that one excluded. Hit order breaks the tie by triangle index,
// and both rays tie on the same two triangles, so one block wins it on both
// rays. In the layout that excludes the winner, the ray taking its
// tolerance hit runs the off-box beside arm and the ray entering it runs
// the crossing; the other layout pins only that a kept winner skips nothing.
#[test]
fn a_tie_at_a_seam_with_an_off_box_brush_captures_the_same_kept_or_excluded() {
    let mut world = textured_room();
    world.push(textured_box_brush(
        [80.0, 32.0, 0.0],
        [96.0, 64.0, 32.0],
        TRIM_TEXTURE,
    ));
    world.push(textured_box_brush(
        [80.0, 64.0, 0.0],
        [96.0, 96.0, 32.0],
        TRIM_TEXTURE,
    ));
    let room = CaptureFixture::compile(&world, &[pool_entity()]);
    let near = room.brush_at([88.0, 48.0, 16.0]);
    let far = room.brush_at([88.0, 80.0, 16.0]);
    let direction = engine([1.0, 0.0, -1.0e-5]).normalize();
    // The last map y each layout's box holds, and the block it excludes.
    let layouts = [(63.0, far), (31.0, near)];
    // Each ray's offset across the seam, in metres, and the block taking its
    // tolerance hit, whose front it crosses 0.2 mm outside: under the seam,
    // inside the near block's front, the far block; over it, the near one.
    let rays = [(-0.0002, far), (0.0002, near)];
    let (winners, arms) = room.with_scene(|scene, probes| {
        let (mut winners, mut arms) = (Vec::new(), Vec::new());
        for &(box_edge_y, off_box) in &layouts {
            let probe = boxed(&probes[0], ROOM_MIN, [ROOM_MAX[0], box_edge_y, ROOM_MAX[2]]);
            let none = exclusion(scene, &probe, &[]);
            let filter = exclusion(scene, &probe, &[off_box]);
            assert_eq!(
                filter.excluded(),
                &[off_box][..],
                "excluded though off the box"
            );
            for &(offset, tolerance_hit) in &rays {
                let crossing = engine([80.0, 64.0 + offset * UNITS_PER_METER, 24.0]);
                let (origin, direction) = ((crossing - direction).as_vec3(), direction.as_vec3());
                let [first, _] = tied_hits(&room, scene, (origin, direction), [near, far]);
                let kept = scene.trace(origin, direction, &none);
                let RayOutcome::Hit(hit) = kept else {
                    panic!("{offset}: a front faces the ray, not {kept:?}");
                };
                assert!(
                    [Some(near), Some(far)].contains(&room.face_brush(hit.face)),
                    "{offset}: {hit:?}"
                );
                let excluded = scene.trace(origin, direction, &filter);
                assert_eq!(
                    excluded.surface(),
                    Some(&hit),
                    "box edge {box_edge_y}, {offset}: {excluded:?} vs {kept:?}"
                );
                let winner = room.face_brush(first.face);
                winners.push(winner);
                arms.push(if winner != Some(off_box) {
                    SeamArm::KeptFirst
                } else if winner == Some(tolerance_hit) {
                    SeamArm::OffBoxBeside
                } else {
                    SeamArm::OffBoxCrossing
                });
            }
        }
        (winners, arms)
    });
    assert!(
        winners.iter().all(|&winner| winner == winners[0]),
        "one block wins the tie on every ray: {winners:?}"
    );
    for arm in [SeamArm::OffBoxBeside, SeamArm::OffBoxCrossing] {
        assert!(arms.contains(&arm), "{arm:?} decides a ray: {arms:?}");
    }
}
