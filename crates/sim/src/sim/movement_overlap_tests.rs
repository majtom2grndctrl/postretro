// The once-per-level overlap warning names the body, texture and position.
// See: context/lib/movement.md §1, §7 · context/lib/entity_model.md §7

use glam::Vec3;

use crate::collision::{CollisionCapsule, CollisionWorld};

#[test]
fn an_overlap_warns_once_per_level_load_naming_descriptor_texture_and_position() {
    use log::Level;
    use postretro_entities::{
        DescriptorProvenance, DescriptorSpawnPath, EntityRegistry, Transform,
    };
    use postretro_test_log_capture::LogCapture;

    use crate::movement::overlap_check::{check_own_movement, disarm_overlap_assert_for_test};

    // A floor whose two triangles carry texture 0 of the level's names.
    let level = || {
        CollisionWorld::from_textured_triangles_for_test(
            vec![
                Vec3::new(-50.0, 0.0, -50.0),
                Vec3::new(50.0, 0.0, -50.0),
                Vec3::new(50.0, 0.0, 50.0),
                Vec3::new(-50.0, 0.0, 50.0),
            ],
            vec![[0, 2, 1], [0, 3, 2]],
            vec![0, 0],
            vec![String::from("metal/floor_grate")],
        )
    };
    let mut registry = EntityRegistry::new();
    let mut spawn = |name: &str| {
        let body = registry.spawn(Transform::default());
        registry
            .set_component(
                body,
                DescriptorProvenance {
                    canonical_name: String::from(name),
                    owned_components: Default::default(),
                    map_overrides: Default::default(),
                    spawn_path: DescriptorSpawnPath::MapPlacement,
                },
            )
            .unwrap();
        body
    };
    let grunt = spawn("enemy.grunt");
    let drone = spawn("enemy.drone");
    let shape = CollisionCapsule::new(0.35, 0.55);
    let clear = Vec3::new(0.0, 0.92, 0.0);
    // Engine (x, y, z) = (-2.54, 0.85, 5.08) is TrenchBroom (-200 100 33).
    let sunk = Vec3::new(-2.54, 0.85, 5.08);
    let _disarmed = disarm_overlap_assert_for_test();
    let capture = LogCapture::start();

    let world = level();
    let report = check_own_movement(&world, clear, sunk, shape)
        .expect("an overlapping end pose handed to the check is reported");
    assert_eq!(report.position, sunk, "the check moves nothing");
    crate::sim::report_static_overlap(&world, &registry, grunt, &report);
    capture.assert_logged_once(
        Level::Warn,
        "[Movement] `enemy.grunt` ended a tick inside static geometry against texture \
         `metal/floor_grate` at TrenchBroom (-200 100 33)",
    );
    // A second overlap in the same level load is silent, whoever it is.
    crate::sim::report_static_overlap(&world, &registry, drone, &report);
    capture.assert_logged_once(Level::Warn, "inside static geometry");
    capture.assert_not_logged(Level::Warn, "enemy.drone");

    // A level reload builds a new world, which warns again.
    let reloaded = level();
    let report = check_own_movement(&reloaded, clear, sunk, shape).unwrap();
    crate::sim::report_static_overlap(&reloaded, &registry, grunt, &report);
    let warnings = capture
        .records()
        .iter()
        .filter(|record| {
            record.level == Level::Warn && record.message.contains("inside static geometry")
        })
        .count();
    assert_eq!(warnings, 2, "the reload re-arms the one warning");

    // An untextured face names the position alone.
    let untextured = CollisionWorld::from_triangles_for_test(
        vec![
            Vec3::new(-50.0, 0.0, -50.0),
            Vec3::new(50.0, 0.0, -50.0),
            Vec3::new(50.0, 0.0, 50.0),
            Vec3::new(-50.0, 0.0, 50.0),
        ],
        vec![[0, 2, 1], [0, 3, 2]],
    );
    let report = check_own_movement(&untextured, clear, sunk, shape).unwrap();
    crate::sim::report_static_overlap(&untextured, &registry, drone, &report);
    capture.assert_logged_once(
        Level::Warn,
        "[Movement] `enemy.drone` ended a tick inside static geometry at TrenchBroom \
         (-200 100 33); further overlaps this level stay silent",
    );
}
