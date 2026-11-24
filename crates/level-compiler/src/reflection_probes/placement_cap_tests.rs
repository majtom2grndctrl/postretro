// Reflection-probe cap with glossy world faces: probes binding only world faces drop first, smallest bound area first; water keeps every probe; warnings name the faces; authored water past the cap refuses.
// See: context/lib/resource_management.md §5.1 (Drops and cap), §5.4 (Cap)

use std::collections::HashSet;

use glam::DVec3;
use log::Level;
use postretro_level_format::environment_volumes::EnvironmentVolumesSection;
use postretro_test_log_capture::LogCapture;

use crate::bake_control::BakeControl;
use crate::bvh_build::build_bvh;
use crate::map_data::MapReflectionVolume;
use crate::partition::BspTree;

use super::box_fit::WorldBounds;
use super::capture_point::PointClassifier;
use super::facing::Facing;
use super::hit::SceneTracer;
use super::placement::{
    FaceGeometry, PlacedProbe, PlacementScene, ReflectorFace, ReflectorSource, place_probes,
};
use super::reflection_probes_test_fixtures::{box_room, fluid_volume, up_rect, volumes};

// ---------------------------------------------------------------------------
// Fixtures
// ---------------------------------------------------------------------------

/// Placement over a synthetic 16 × 4 × 16 m room with no BSP, so no point
/// is in solid or outside the hull: binding and the cap are all that act.
fn place(
    section: &EnvironmentVolumesSection,
    authored: &[MapReflectionVolume],
    world_faces: &[ReflectorFace],
    cap: usize,
) -> anyhow::Result<Vec<PlacedProbe>> {
    let room = box_room([-8.0, 0.0, -8.0], [8.0, 4.0, 8.0]);
    let (bvh, primitives, _) = build_bvh(&room).unwrap();
    let tracer = SceneTracer::new(&bvh, &primitives, &room);
    let bounds =
        WorldBounds::from_positions(room.geometry.vertices.iter().map(|v| &v.position)).unwrap();
    let tree = BspTree {
        nodes: Vec::new(),
        leaves: Vec::new(),
    };
    let exterior = HashSet::new();
    let classifier = PointClassifier::new(&tree, &exterior, section);
    let control = BakeControl::unrestricted();
    place_probes(
        section,
        authored,
        world_faces,
        &PlacementScene {
            tracer: &tracer,
            bounds: &bounds,
            classifier: &classifier,
            governor: control.governor(),
        },
        cap,
    )
}

/// A glossy floor world face `face`: an up-facing square of side `side`
/// centred at `(x, z)`, `y` up. Faces on different heights never share a
/// derived probe.
fn glossy_square(face: u32, x: f64, z: f64, side: f64, y: f64) -> ReflectorFace {
    let h = side * 0.5;
    let points = [
        DVec3::new(x - h, y, z - h),
        DVec3::new(x - h, y, z + h),
        DVec3::new(x + h, y, z + h),
        DVec3::new(x + h, y, z - h),
    ];
    let geometry = FaceGeometry::from_polygon(&points).unwrap();
    ReflectorFace::new(
        ReflectorSource::World { face },
        geometry,
        Facing::Up,
        &points,
    )
}

fn authored_box(entity: u32, min: [f32; 3], max: [f32; 3]) -> MapReflectionVolume {
    MapReflectionVolume {
        source_entity: entity,
        source_label: format!("entity {entity} 'reflection_volume'"),
        min,
        max,
        capture_offset: None,
    }
}

/// An authored box around `(x, z)`, a metre each way, from the floor up.
fn box_around(entity: u32, x: f32, z: f32) -> MapReflectionVolume {
    authored_box(entity, [x - 1.0, 0.0, z - 1.0], [x + 1.0, 3.0, z + 1.0])
}

/// Pools of fluid entities `10..`, 1 m² each, along x at z = −6.
fn pools(count: usize) -> EnvironmentVolumesSection {
    volumes(
        (0..count)
            .map(|i| {
                let x = -6.0 + 3.0 * i as f32;
                fluid_volume(10 + i as u32, &[up_rect(x - 0.5, -6.5, x + 0.5, -5.5, 0.2)])
            })
            .collect(),
    )
}

fn sources(probes: &[PlacedProbe]) -> Vec<(u32, bool, Vec<u32>)> {
    probes
        .iter()
        .map(|p| (p.source_entity, p.authored, p.world_faces.clone()))
        .collect()
}

// ---------------------------------------------------------------------------
// Drop order
// ---------------------------------------------------------------------------

// Row 62 (P18): one probe past the cap drops exactly one probe, the one
// binding only world faces with the smallest bound area, whatever its order,
// and the warning names its faces. At the cap, none drops and nothing warns.
#[test]
fn one_probe_past_the_cap_drops_the_smallest_world_face_only_probe_naming_its_faces() {
    let logs = LogCapture::start();
    let water = pools(1);
    // Three glossy floors on three heights, 4, 1 and 2.25 m².
    let world = [
        glossy_square(0, 0.0, 2.0, 2.0, 0.5),
        glossy_square(1, 3.0, 2.0, 1.0, 1.0),
        glossy_square(2, 6.0, 2.0, 1.5, 1.5),
    ];
    let all = place(&water, &[], &world, 4).unwrap();
    assert_eq!(all.len(), 4, "{all:?}");
    logs.assert_not_logged(Level::Warn, "past the 4-probe cap");

    let kept = place(&water, &[], &world, 3).unwrap();
    assert_eq!(kept.len(), 3);
    assert_eq!(kept[0].fluid_faces, vec![(0, 0)], "water keeps its probe");
    let faces: Vec<Vec<u32>> = kept[1..].iter().map(|p| p.world_faces.clone()).collect();
    assert!(
        !faces.contains(&vec![1]),
        "the 1 m² floor's probe drops: {faces:?}"
    );
    assert_eq!(faces.len(), 2);
    logs.assert_logged_once(
        Level::Warn,
        "1 glossy world probe(s) past the 3-probe cap dropped, smallest bound area first; \
         their faces reflect nothing: derived probe (glossy world face 1)",
    );
}

// Row 62 (P18): a water map pushed past the cap by glossy faces keeps every
// water probe, derived and authored, and drops world-face probes, authored
// and derived, even one larger than every pool.
#[test]
fn a_water_map_pushed_past_the_cap_by_glossy_faces_keeps_every_water_probe() {
    let logs = LogCapture::start();
    let water = pools(3);
    let world = [
        glossy_square(0, 0.0, 2.0, 2.0, 0.5),
        glossy_square(1, 3.0, 2.0, 1.0, 1.0),
        // 16 m², under its own authored box.
        glossy_square(2, 4.0, -2.0, 4.0, 0.3),
    ];
    // An authored box over the third pool, and one over the big floor.
    let authored = [
        box_around(40, 0.0, -6.0),
        authored_box(41, [1.5, 0.0, -4.5], [6.5, 3.0, 0.5]),
    ];
    let probes = place(&water, &authored, &world, 3).unwrap();
    assert_eq!(probes.len(), 3, "{probes:?}");
    assert!(probes.iter().all(|p| !p.fluid_faces.is_empty()));
    let mut fluid: Vec<(u32, u32)> = probes.iter().flat_map(|p| p.fluid_faces.clone()).collect();
    fluid.sort_unstable();
    assert_eq!(
        fluid,
        vec![(0, 0), (1, 0), (2, 0)],
        "every pool keeps a probe"
    );
    logs.assert_logged_once(
        Level::Warn,
        "3 glossy world probe(s) past the 3-probe cap dropped",
    );
    logs.assert_logged_once(
        Level::Warn,
        "entity 41 'reflection_volume' probe facing up (glossy world face 2)",
    );
}

// Row 63 (P25): a water-only map past the cap drops the derived probes the
// build before dropped (the smallest bound water, kept in fluid entity
// order), and the warning names each dropped probe's faces.
#[test]
fn a_water_only_map_past_the_cap_drops_as_before_and_names_each_dropped_probes_faces() {
    let logs = LogCapture::start();
    // Pools of 1, 4, 2 and 3 m², and one under an authored box.
    let mut section = volumes(
        [1.0f32, 4.0, 2.0, 3.0]
            .iter()
            .enumerate()
            .map(|(i, &area)| {
                let half = area.sqrt() * 0.5;
                let x = -6.0 + 3.0 * i as f32;
                fluid_volume(
                    10 + i as u32,
                    &[up_rect(x - half, -half, x + half, half, 0.5)],
                )
            })
            .collect(),
    );
    section
        .volumes
        .push(fluid_volume(30, &[up_rect(-1.0, 5.0, 1.0, 7.0, 0.5)]));
    let authored = [authored_box(40, [-2.0, 0.0, 4.0], [2.0, 2.0, 7.5])];
    let probes = place(&section, &authored, &[], 3).unwrap();
    assert_eq!(
        probes.iter().map(|p| p.source_entity).collect::<Vec<_>>(),
        vec![40, 11, 13],
        "the build before's kept probes"
    );
    logs.assert_logged_once(
        Level::Warn,
        "2 derived probe(s) past the 3-probe cap dropped, smallest bound water first; their \
         faces reflect nothing: fluid entity 10's probe (fluid face 0 of volume 0); fluid \
         entity 12's probe (fluid face 0 of volume 2)",
    );
}

// ---------------------------------------------------------------------------
// Authored boxes
// ---------------------------------------------------------------------------

// Row 66 (P19): authored boxes whose water-binding probes alone pass the cap
// refuse the build, the error counting them and naming each box with the
// probes it makes. Authored probes binding only world faces never refuse:
// past the cap they drop first, the smallest bound area first, and the
// warning names each box.
#[test]
fn authored_water_probes_past_the_cap_refuse_and_authored_world_only_ones_drop_first() {
    let logs = LogCapture::start();
    let water = pools(4);
    let boxes: Vec<MapReflectionVolume> = (0..4)
        .map(|i| box_around(40 + i, -6.0 + 3.0 * i as f32, -6.0))
        .collect();
    let error = place(&water, &boxes, &[], 3).expect_err("four authored water probes, cap three");
    let message = format!("{error:#}");
    assert!(
        message.contains("4 reflection volume probe(s) bind water, past the 3-probe cap"),
        "{message}"
    );
    for i in 0..4 {
        let named = format!(
            "entity {} 'reflection_volume' makes 1 probe(s), 1 binding water",
            40 + i
        );
        assert!(message.contains(&named), "{message}");
    }

    // Three boxes over water, two over glossy floors of 4 and 1 m².
    let water = pools(3);
    let world = [
        glossy_square(0, 0.0, 3.0, 2.0, 0.5),
        glossy_square(1, 4.0, 3.0, 1.0, 0.5),
    ];
    let mut authored: Vec<MapReflectionVolume> = boxes[..3].to_vec();
    authored.push(box_around(50, 0.0, 3.0));
    authored.push(box_around(51, 4.0, 3.0));
    let probes = place(&water, &authored, &world, 3).unwrap();
    assert_eq!(
        sources(&probes),
        vec![(40, true, vec![]), (41, true, vec![]), (42, true, vec![])],
        "past the cap, world-only boxes drop and water's refuse nothing"
    );
    logs.assert_logged_once(
        Level::Warn,
        "2 glossy world probe(s) past the 3-probe cap dropped, smallest bound area first; \
         their faces reflect nothing: entity 50 'reflection_volume' probe facing up (glossy \
         world face 0); entity 51 'reflection_volume' probe facing up (glossy world face 1)",
    );

    logs.clear();
    let probes = place(&water, &authored, &world, 4).unwrap();
    assert_eq!(
        sources(&probes),
        vec![
            (40, true, vec![]),
            (41, true, vec![]),
            (42, true, vec![]),
            (50, true, vec![0])
        ],
        "the 1 m² box drops first"
    );
    logs.assert_logged_once(
        Level::Warn,
        "1 glossy world probe(s) past the 4-probe cap dropped, smallest bound area first; \
         their faces reflect nothing: entity 51 'reflection_volume' probe facing up (glossy \
         world face 1)",
    );
}
