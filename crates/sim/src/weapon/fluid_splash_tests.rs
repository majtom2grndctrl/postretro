// Hitscan fluid splash facts through the real fire path: counts, the buried-floor and shared-face rules, and host/client parity.
// See: context/lib/entity_model.md §7 (Environment Volumes: Hitscan splash facts)

use super::tests::weapon_component;
use super::*;
use postretro_collision::{ConvexLeaf, EnvironmentVolume, EnvironmentVolumes, LeafPlane};

const EPSILON: f32 = 1.0e-4;
const WATER: u16 = 3;
const SLUDGE: u16 = 5;

fn axis_box(min: Vec3, max: Vec3) -> ConvexLeaf {
    let plane = |normal: Vec3, distance: f32| LeafPlane { normal, distance };
    ConvexLeaf {
        planes: vec![
            plane(Vec3::X, max.x),
            plane(Vec3::NEG_X, -min.x),
            plane(Vec3::Y, max.y),
            plane(Vec3::NEG_Y, -min.y),
            plane(Vec3::Z, max.z),
            plane(Vec3::NEG_Z, -min.z),
        ],
        aabb_min: min,
        aabb_max: max,
    }
}

fn fluid_volume(source_entity: u32, fluid: u16, brushes: Vec<ConvexLeaf>) -> EnvironmentVolume {
    EnvironmentVolume {
        source_entity,
        priority: 0,
        gravity: None,
        push: None,
        fluid: Some(fluid),
        brushes,
    }
}

fn volumes(list: Vec<EnvironmentVolume>) -> EnvironmentVolumes {
    EnvironmentVolumes::build(list)
}

/// A water pool spanning `y` from `bottom` to 2 over x and z in -5..5.
fn pool(bottom: f32) -> EnvironmentVolumes {
    volumes(vec![fluid_volume(
        1,
        WATER,
        vec![axis_box(
            Vec3::new(-5.0, bottom, -5.0),
            Vec3::new(5.0, 2.0, 5.0),
        )],
    )])
}

/// A horizontal static quad at `y`, facing up, over x and z in -20..20.
fn floor_at(y: f32) -> CollisionWorld {
    CollisionWorld::from_triangles_for_test(
        vec![
            Vec3::new(-20.0, y, -20.0),
            Vec3::new(20.0, y, -20.0),
            Vec3::new(20.0, y, 20.0),
            Vec3::new(-20.0, y, 20.0),
        ],
        vec![[0, 2, 1], [0, 3, 2]],
    )
}

fn fire_command(origin: Vec3, direction: Vec3) -> WeaponFireCommand {
    WeaponFireCommand {
        button: FireButtonState {
            pressed: true,
            active: true,
        },
        aim_origin: origin,
        aim_direction: direction,
        can_fire: true,
    }
}

fn hitscan(range: f32, pellets: u32, spread_degrees: f32) -> WeaponComponent {
    let mut weapon = weapon_component(FireMode::Semi, 0.0);
    weapon.range = range;
    weapon.pellet_count = pellets;
    weapon.spread_degrees = spread_degrees;
    weapon
}

/// One host-authoritative shot through `tick_resolved_component`, the weapon
/// stage's resolve, returning its events.
fn host_shot(
    world: &CollisionWorld,
    volumes: &EnvironmentVolumes,
    weapon: &mut WeaponComponent,
    origin: Vec3,
    direction: Vec3,
) -> WeaponFireEvents {
    tick_resolved_component(
        &EntityRegistry::new(),
        None,
        weapon,
        "weapon.test.splash",
        0,
        &fire_command(origin, direction),
        &WeaponPlacementDescriptor::default(),
        world,
        EnvironmentView::new(-9.81, volumes, &[]),
        &HitZoneStore::new(),
        0.0,
        WeaponFireAuthorization::Accepted,
    )
}

fn splashes(
    world: &CollisionWorld,
    volumes: &EnvironmentVolumes,
    range: f32,
    origin: Vec3,
    direction: Vec3,
) -> Vec<FluidSplashFact> {
    host_shot(
        world,
        volumes,
        &mut hitscan(range, 1, 0.0),
        origin,
        direction,
    )
    .fluid_splashes
}

fn assert_splash(fact: &FluidSplashFact, point: Vec3, normal: Vec3, fluid: u16) {
    assert!(
        (fact.point - point).length() < EPSILON,
        "point {:?} expected {point:?}",
        fact.point
    );
    assert!(
        (fact.normal - normal).length() < EPSILON,
        "normal {:?} expected {normal:?}",
        fact.normal
    );
    assert_eq!(fact.fluid, fluid);
}

// Air into a deep pool, the pellet's range ending underwater.
#[test]
fn hitscan_from_air_into_fluid_emits_one_splash_at_the_surface() {
    let found = splashes(
        &CollisionWorld::new(),
        &pool(-20.0),
        10.0,
        Vec3::new(1.0, 5.0, 0.0),
        Vec3::NEG_Y,
    );
    assert_eq!(found.len(), 1, "{found:?}");
    assert_splash(&found[0], Vec3::new(1.0, 2.0, 0.0), Vec3::Y, WATER);
}

// A shot from underwater out through the surface.
#[test]
fn hitscan_from_inside_fluid_out_through_a_face_emits_one_splash() {
    let found = splashes(
        &CollisionWorld::new(),
        &pool(-20.0),
        10.0,
        Vec3::new(1.0, -1.0, 0.0),
        Vec3::Y,
    );
    assert_eq!(found.len(), 1, "{found:?}");
    assert_splash(&found[0], Vec3::new(1.0, 2.0, 0.0), Vec3::Y, WATER);
}

// A floating block gives an entry and an exit, nearest first.
#[test]
fn hitscan_through_a_floating_fluid_block_emits_two_splashes_in_order() {
    let block = volumes(vec![fluid_volume(
        1,
        WATER,
        vec![axis_box(
            Vec3::new(-2.0, 2.0, -2.0),
            Vec3::new(2.0, 4.0, 2.0),
        )],
    )]);
    let found = splashes(
        &CollisionWorld::new(),
        &block,
        10.0,
        Vec3::new(0.5, 6.0, 0.0),
        Vec3::NEG_Y,
    );
    assert_eq!(found.len(), 2, "{found:?}");
    assert_splash(&found[0], Vec3::new(0.5, 4.0, 0.0), Vec3::Y, WATER);
    assert_splash(&found[1], Vec3::new(0.5, 2.0, 0.0), Vec3::NEG_Y, WATER);
}

// A wall in front of the pool stops the pellet before any fluid face.
#[test]
fn hitscan_blocked_before_a_fluid_face_emits_no_splash() {
    let found = splashes(
        &floor_at(3.0),
        &pool(-20.0),
        10.0,
        Vec3::new(1.0, 5.0, 0.0),
        Vec3::NEG_Y,
    );
    assert!(found.is_empty(), "{found:?}");
}

// A pellet that passes beside the pool, and one in a level with no
// volumes, cross nothing.
#[test]
fn hitscan_that_never_crosses_a_fluid_face_emits_no_splash() {
    let beside = splashes(
        &CollisionWorld::new(),
        &pool(-20.0),
        10.0,
        Vec3::new(8.0, 5.0, 0.0),
        Vec3::NEG_Y,
    );
    assert!(beside.is_empty(), "{beside:?}");
    let dry = splashes(
        &CollisionWorld::new(),
        &EnvironmentVolumes::empty(),
        10.0,
        Vec3::new(1.0, 5.0, 0.0),
        Vec3::NEG_Y,
    );
    assert!(dry.is_empty(), "{dry:?}");
}

// The pool's bottom face is buried in the floor the pellet stops
// on. Its exit crossing coincides with the blocking hit and never splashes;
// the same holds when the brush is sunk below the floor.
#[test]
fn hitscan_stopping_on_a_pool_floor_emits_one_splash_at_the_top() {
    for bottom in [0.0, -0.5] {
        let found = splashes(
            &floor_at(0.0),
            &pool(bottom),
            10.0,
            Vec3::new(1.0, 5.0, 0.5),
            Vec3::NEG_Y,
        );
        assert_eq!(found.len(), 1, "bottom {bottom}: {found:?}");
        assert_splash(&found[0], Vec3::new(1.0, 2.0, 0.5), Vec3::Y, WATER);
    }

    // A slanted pellet meets the floor at the same buried face.
    let direction = Vec3::new(0.6, -0.8, 0.0);
    let found = splashes(
        &floor_at(0.0),
        &pool(0.0),
        10.0,
        Vec3::new(-2.0, 5.0, 0.0),
        direction,
    );
    assert_eq!(found.len(), 1, "{found:?}");
    assert_splash(&found[0], Vec3::new(0.25, 2.0, 0.0), Vec3::Y, WATER);
}

// One pool entity of two brushes splashes at its outer faces only,
// each fact carrying the crossing point, the crossed face's outward normal
// and the fluid.
#[test]
fn hitscan_through_a_two_brush_pool_emits_no_splash_at_the_shared_face() {
    let two_brush = volumes(vec![fluid_volume(
        1,
        WATER,
        vec![
            axis_box(Vec3::new(-5.0, -10.0, -5.0), Vec3::new(0.0, 2.0, 5.0)),
            axis_box(Vec3::new(0.0, -10.0, -5.0), Vec3::new(5.0, 2.0, 5.0)),
        ],
    )]);
    let found = splashes(
        &CollisionWorld::new(),
        &two_brush,
        20.0,
        Vec3::new(-8.0, 1.0, 0.0),
        Vec3::X,
    );
    assert_eq!(found.len(), 2, "{found:?}");
    assert_splash(&found[0], Vec3::new(-5.0, 1.0, 0.0), Vec3::NEG_X, WATER);
    assert_splash(&found[1], Vec3::new(5.0, 1.0, 0.0), Vec3::X, WATER);
}

// A pellet starting exactly on the surface, leaving or entering through
// it, never splashes there.
#[test]
fn hitscan_from_an_origin_on_a_fluid_face_never_splashes_at_that_face() {
    for direction in [Vec3::NEG_Y, Vec3::Y] {
        let found = splashes(
            &CollisionWorld::new(),
            &pool(-20.0),
            10.0,
            Vec3::new(1.0, 2.0, 0.0),
            direction,
        );
        assert!(found.is_empty(), "direction {direction:?}: {found:?}");
    }
}

// Two adjacent fluid entities give an exit from one and an entry into
// the other at their shared face.
#[test]
fn hitscan_across_two_adjacent_fluid_entities_emits_two_splashes_at_the_shared_face() {
    let adjacent = volumes(vec![
        fluid_volume(
            1,
            WATER,
            vec![axis_box(
                Vec3::new(-5.0, -10.0, -5.0),
                Vec3::new(0.0, 2.0, 5.0),
            )],
        ),
        fluid_volume(
            2,
            SLUDGE,
            vec![axis_box(
                Vec3::new(0.0, -10.0, -5.0),
                Vec3::new(5.0, 2.0, 5.0),
            )],
        ),
    ]);
    let found = splashes(
        &CollisionWorld::new(),
        &adjacent,
        4.0,
        Vec3::new(-2.0, 1.0, 0.0),
        Vec3::X,
    );
    assert_eq!(found.len(), 2, "{found:?}");
    let exit = found
        .iter()
        .find(|fact| fact.fluid == WATER)
        .expect("an exit from the water");
    let entry = found
        .iter()
        .find(|fact| fact.fluid == SLUDGE)
        .expect("an entry into the sludge");
    assert_splash(exit, Vec3::new(0.0, 1.0, 0.0), Vec3::X, WATER);
    assert_splash(entry, Vec3::new(0.0, 1.0, 0.0), Vec3::NEG_X, SLUDGE);
}

// The connected client's predicted shot resolves the same splash facts as
// the host's resolve of that shell: same pellet fan, same order, same bits.
#[test]
fn client_predicted_shot_reports_the_same_fluid_splashes_as_the_host() {
    let world = floor_at(0.0);
    let volumes = pool(0.0);
    let origin = Vec3::new(0.0, 5.0, 0.0);
    let direction = Vec3::NEG_Y;
    let salt = "weapon.test.splash";

    let mut host_weapon = hitscan(10.0, 8, 12.0);
    let host = host_shot(&world, &volumes, &mut host_weapon, origin, direction).fluid_splashes;
    assert_eq!(host.len(), 8, "every pellet enters the pool once: {host:?}");

    let mut client_weapon = hitscan(10.0, 8, 12.0);
    let advance = activation_prediction::advance_predicted_weapon_tick(
        &mut client_weapon,
        execution::ActivationCommand {
            tick: 1,
            pawn: 0,
            real_command: true,
            input: postretro_foundation::ActivationInput::default(),
            controller_starts: true,
            primary: FireButtonState {
                pressed: true,
                active: true,
            },
            secondary: FireButtonState {
                pressed: false,
                active: false,
            },
        },
        false,
        0.0,
        true,
    );
    let shot = freeze_weapon_shot(
        &client_weapon,
        advance.shot.expect("the press fires a shell"),
    );
    let client = resolve_client_shot(
        None,
        client_weapon.clone(),
        salt,
        0,
        origin,
        direction,
        &WeaponPlacementDescriptor::default(),
        &world,
        EnvironmentView::new(-9.81, &volumes, &[]),
        &EntityRegistry::new(),
        &HitZoneStore::new(),
        0.0,
        shot,
    );
    assert_eq!(client.fluid_splashes, host);
}
