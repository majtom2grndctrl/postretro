// Capture exclusion on a glossy floor through the stage: a freestanding monument leaves no withheld floor, a kept one does, and the floor stride keys floor captures alone.
// See: context/lib/resource_management.md §5.2 (Exclusion), §5.4 (Exclusion) · rendering_pipeline.md §7.3a

use glam::{DVec2, Vec3};
use log::Level;
use postretro_level_format::reflection_probes::{MAX_REFLECTION_PROBES, ReflectionProbesSection};
use postretro_test_log_capture::LogCapture;

use super::box_fit::WorldBounds;
use super::capture_exclusion::{ProbeExclusion, RayOutcome};
use super::capture_exclusion_test_fixtures::{
    CaptureFixture, GLOSSY_FLOOR_TEXTURE, PILLAR_TEXTURE, ROOM_MAX, ROOM_MIN, textured_room,
};
use super::capture_point::PointClassifier;
use super::capture_visibility::DistanceTile;
use super::exclusion::{ExclusionConstants, ExclusionScene, classify_probes_within_boxes};
use super::facing::Facing;
use super::glossy_placement::glossy_world_faces;
use super::hit::SceneTracer;
use super::placement::{PlacedProbe, PlacementScene, ReflectorFace, place_probes};
use super::placement_test_fixtures::{engine, engine_box, textured_box_brush};
use super::self_transparency::transparent_faces;
use super::{ReflectionProbeInputs, STAGE_ID, bake_or_load_under};
use crate::bake_control::BakeControl;
use crate::cache::StageCache;
use crate::map_data::ReflectionCapture;

// ---------------------------------------------------------------------------
// Fixtures
// ---------------------------------------------------------------------------

/// The monument's brushes in map units: a plinth, a column and a cap
/// overhanging the column, about 0.8 m square and 1.3 m tall, at the room's
/// centre, where the floor's area centroid falls.
const MONUMENT: [([f64; 3], [f64; 3]); 3] = [
    ([-16.0, -16.0, 0.0], [16.0, 16.0, 8.0]),
    ([-8.0, -8.0, 8.0], [8.0, 8.0, 48.0]),
    ([-12.0, -12.0, 48.0], [12.0, 12.0, 52.0]),
];

/// The capture-exclusion room with a glossy floor, the monument on it, and
/// `--glossy-reflections` on.
fn monument_room() -> CaptureFixture {
    let mut world = textured_room();
    // The room's floor slab is its first brush: make it glossy.
    let ([x0, y0, z0], [x1, y1, _]) = (ROOM_MIN, ROOM_MAX);
    world[0] = textured_box_brush(
        [x0 - 16.0, y0 - 16.0, z0 - 16.0],
        [x1 + 16.0, y1 + 16.0, z0],
        GLOSSY_FLOOR_TEXTURE,
    );
    world.extend(
        MONUMENT
            .iter()
            .map(|&(min, max)| textured_box_brush(min, max, PILLAR_TEXTURE)),
    );
    let mut room = CaptureFixture::compile(&world, &[]);
    room.glossy_reflections = true;
    room
}

/// The monument's brushes, as indices into the room's brushes.
fn monument_brushes(room: &CaptureFixture) -> Vec<u32> {
    MONUMENT
        .iter()
        .map(|&(min, max)| {
            let centre = [0, 1, 2].map(|axis| 0.5 * (min[axis] + max[axis]));
            room.brush_at(centre)
        })
        .collect()
}

/// The stage's id 54 for `room` under `constants`, through `cache` when
/// given.
fn bake(
    room: &CaptureFixture,
    constants: &ExclusionConstants,
    cache: Option<&StageCache>,
) -> ReflectionProbesSection {
    room.with_inputs(|inputs| {
        bake_or_load_under(inputs, cache, &BakeControl::unrestricted(), constants)
    })
    .unwrap()
    .expect("the room keeps a probe")
}

/// Placement as the stage runs it, glossy world faces included, with those
/// faces.
fn place(inputs: &ReflectionProbeInputs<'_>) -> (Vec<PlacedProbe>, Vec<ReflectorFace>) {
    let control = BakeControl::unrestricted();
    let world_faces = glossy_world_faces(inputs, control.governor());
    let tracer = SceneTracer::new(inputs.bvh, inputs.primitives, inputs.geometry);
    let bounds = WorldBounds::from_positions(
        inputs
            .geometry
            .geometry
            .vertices
            .iter()
            .map(|v| &v.position),
    )
    .unwrap();
    let classifier = PointClassifier::new(
        inputs.tree,
        inputs.exterior_leaves,
        inputs.environment_volumes,
    );
    let probes = place_probes(
        inputs.environment_volumes,
        inputs.reflection_volumes,
        &world_faces,
        &PlacementScene {
            tracer: &tracer,
            bounds: &bounds,
            classifier: &classifier,
            governor: control.governor(),
        },
        MAX_REFLECTION_PROBES,
    )
    .unwrap();
    (probes, world_faces)
}

/// Whether the convex `outline` holds `p`, whichever way it winds.
fn holds(outline: &[DVec2], p: DVec2) -> bool {
    let n = outline.len();
    let sides: Vec<f64> = (0..n)
        .map(|i| (outline[(i + 1) % n] - outline[i]).perp_dot(p - outline[i]))
        .collect();
    sides.iter().all(|&s| s >= 0.0) || sides.iter().all(|&s| s <= 0.0)
}

/// Every 4 u grid point of the glossy floor, as `(probe, point)`, that lies
/// behind the monument from the capture point of the probe binding it: the
/// level segment from that point to it crosses the plinth's footprint, and
/// the point lies at least 8 u past the plinth.
fn floor_behind_the_monument(
    section: &ReflectionProbesSection,
    world_faces: &[ReflectorFace],
) -> Vec<(usize, Vec3)> {
    let (plinth_lo, plinth_hi) = engine_box(MONUMENT[0].0, MONUMENT[0].1);
    let (lo, hi) = (
        DVec2::new(plinth_lo.x, plinth_lo.z),
        DVec2::new(plinth_hi.x, plinth_hi.z),
    );
    let margin = 8.0 * super::placement_test_fixtures::UNIT;
    let crosses = |a: DVec2, b: DVec2| {
        (0..=64).any(|k| {
            let p = a.lerp(b, f64::from(k) / 64.0);
            p.cmpge(lo).all() && p.cmple(hi).all()
        })
    };
    let mut out = Vec::new();
    for x in (-124..=124).step_by(4) {
        for y in (-124..=124).step_by(4) {
            let point = engine([f64::from(x), f64::from(y), 0.0]);
            let p = DVec2::new(point.x, point.z);
            if p.cmpge(lo - margin).all() && p.cmple(hi + margin).all() {
                continue;
            }
            let Some(face) = world_faces.iter().find(|face| holds(&face.outline, p)) else {
                continue;
            };
            let Some(binding) = section
                .world_bindings
                .iter()
                .find(|binding| Some(binding.face) == face.world_face())
            else {
                continue;
            };
            let probe = binding.probe as usize;
            let capture = Vec3::from(section.probes[probe].capture_point).as_dvec3();
            if crosses(DVec2::new(capture.x, capture.z), p) {
                out.push((probe, point.as_vec3()));
            }
        }
    }
    out
}

/// How many of `points` their probe's capture point sees, by the overlay's
/// capture-shadow cutoff read from the stored distance tile.
fn seen(section: &ReflectionProbesSection, points: &[(usize, Vec3)]) -> usize {
    points
        .iter()
        .filter(|&&(probe, point)| {
            let record = &section.probes[probe];
            DistanceTile::decode(section, record.layer as usize).sees_glossy(
                Vec3::from(record.capture_point),
                point,
                &Facing::Up,
            )
        })
        .count()
}

// ---------------------------------------------------------------------------
// The monument
// ---------------------------------------------------------------------------

// A freestanding monument on a glossy floor, each of
// its brushes under the exclusion threshold, leaves the floor's probe, so no
// floor fragment behind it, seen from the capture point, loses its probe
// term: the probe holds the room behind it, and the build warns of no
// hidden floor. The capture point sits a centimetre off the plinth (the
// floor's centroid falls on it), so a fill rebuilding the floor under the
// excluded plinth would withhold most of that floor.
//
// Set `always`, the probe keeps the monument, and
// the floor behind it, seen from the capture point, is withheld.
#[test]
fn a_freestanding_monument_on_a_glossy_floor_withholds_no_floor_behind_it() {
    let mut room = monument_room();
    let monument = monument_brushes(&room);
    let logs = LogCapture::start();
    let world_faces = room.with_inputs(|inputs| {
        let (probes, world_faces) = place(inputs);
        assert!(!probes.is_empty() && probes.iter().all(|p| p.facing.is_up()));
        let excluded = classify_probes_within_boxes(
            &ExclusionScene {
                brush_volumes: inputs.brush_volumes,
                capture_override: inputs.capture_override,
                face_brush: inputs.face_brushes,
                geometry: inputs.geometry,
                environment_volumes: inputs.environment_volumes,
                world_faces: &world_faces,
            },
            &probes,
            &ExclusionConstants::PRODUCTION,
        );
        for set in &excluded {
            for brush in &monument {
                assert!(
                    set.contains(brush),
                    "{brush} leaves every probe: {excluded:?}"
                );
            }
        }
        world_faces
    });
    let excluded = bake(&room, &ExclusionConstants::PRODUCTION, None);
    let behind = floor_behind_the_monument(&excluded, &world_faces);
    assert!(behind.len() >= 100, "{} floor points behind", behind.len());
    assert_eq!(
        seen(&excluded, &behind),
        behind.len(),
        "every floor point behind the excluded monument keeps its probe term"
    );
    logs.assert_not_logged(Level::Warn, "of the glossy floor it binds");

    for &brush in &monument {
        room.world.capture_override[brush as usize] = ReflectionCapture::Always;
    }
    logs.clear();
    let kept = bake(&room, &ExclusionConstants::PRODUCTION, None);
    let behind = floor_behind_the_monument(&kept, &world_faces);
    let unseen = behind.len() - seen(&kept, &behind);
    assert!(
        unseen * 10 >= behind.len() * 9,
        "{unseen} of {} floor points behind the kept monument are withheld",
        behind.len()
    );
}

// The mechanism: a ray just under the horizon from the capture point into
// the excluded plinth's base crosses the floor's plane inside the plinth.
// It reads behind the floor, as a ray beside the plinth does through the
// transparent floor, neither rebuilding the floor under the plinth (a fill)
// nor reading the plinth (the leak guard). Without the reflector plane the
// same exclusion fills there, a few centimetres off.
#[test]
fn a_ray_into_an_excluded_plinths_base_reads_behind_the_floor_not_a_fill() {
    let room = monument_room();
    let plinth = monument_brushes(&room)[0];
    room.with_inputs(|inputs| {
        let (probes, world_faces) = place(inputs);
        let tracer = SceneTracer::new(inputs.bvh, inputs.primitives, inputs.geometry);
        let transparent = transparent_faces(inputs.geometry, &probes, &world_faces);
        let index = probes
            .iter()
            .position(|probe| {
                let capture = probe.capture_point_vec3().as_dvec3();
                let (lo, hi) = engine_box(MONUMENT[0].0, MONUMENT[0].1);
                let (x, z) = (capture.x.clamp(lo.x, hi.x), capture.z.clamp(lo.z, hi.z));
                (capture.x - x).hypot(capture.z - z) < 0.05
            })
            .expect("a probe captures beside the plinth");
        let probe = &probes[index];
        let origin = probe.capture_point_vec3();
        let target = engine([0.0, 0.0, 0.0]).as_vec3();
        let direction = (target - origin).normalize();
        assert!(direction.y < 0.0, "the ray heads under the horizon");
        let exclusion = || {
            ProbeExclusion::new(&tracer, room.brushes(), &[plinth], probe)
                .with_transparent_faces(&tracer, transparent[index].clone())
        };
        let max = 100.0;
        match exclusion().trace(&tracer, origin, direction, max) {
            RayOutcome::Miss => {}
            RayOutcome::Hit(hit) => assert!(
                room.face_brush(hit.face) != Some(plinth),
                "the ray reads behind the floor: {hit:?}"
            ),
            other => panic!("the ray reads behind the floor, not {other:?}"),
        }
        let filled = exclusion()
            .without_behind_reads()
            .trace(&tracer, origin, direction, max);
        assert!(
            matches!(filled, RayOutcome::Fill(fill) if fill.distance < 0.5),
            "without the reflector plane the plinth's base fills: {filled:?}"
        );
    });
}

// ---------------------------------------------------------------------------
// The floor stride and the cache key
// ---------------------------------------------------------------------------

// Retuning the floor layer's coarsest stride re-keys a
// level whose probe binds a glossy floor and leaves a water-only level's key
// and capture byte-identical.
#[test]
fn retuning_the_floor_stride_rekeys_floor_captures_and_leaves_water_captures_identical() {
    let retuned = ExclusionConstants {
        floor_stride_divisor: 120,
        ..ExclusionConstants::PRODUCTION
    };
    let dir = tempfile::tempdir().unwrap();
    let cache = StageCache::new(dir.path()).unwrap();
    let water = CaptureFixture::pool_room(Some(ROOM_MAX[2]));
    let before = bake(&water, &ExclusionConstants::PRODUCTION, Some(&cache));
    let after = bake(&water, &retuned, Some(&cache));
    let access = cache.test_access(STAGE_ID);
    assert_eq!((access.writes, access.read_hits), (1, 1), "water hits");
    assert_eq!(before.to_bytes(), after.to_bytes());
    assert_eq!(
        after.to_bytes(),
        bake(&water, &retuned, None).to_bytes(),
        "a cold water bake under the retuned stride is byte-identical"
    );

    let dir = tempfile::tempdir().unwrap();
    let cache = StageCache::new(dir.path()).unwrap();
    let floor = monument_room();
    bake(&floor, &ExclusionConstants::PRODUCTION, Some(&cache));
    bake(&floor, &retuned, Some(&cache));
    bake(&floor, &retuned, Some(&cache));
    let access = cache.test_access(STAGE_ID);
    assert_eq!(
        (access.writes, access.read_hits),
        (2, 1),
        "the floor misses on the retune, then hits"
    );
}
