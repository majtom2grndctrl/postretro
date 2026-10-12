//! Real dev dummy geometry through descriptor materialization and client fire.
//! See: context/lib/entity_model.md §7 · coop--networked-by-default P3.

use std::collections::HashMap;

use glam::{Vec2, Vec3};
use postretro_entities::components::{
    health::HealthComponent, mesh::MeshComponent, weapon::WeaponComponent,
};
use postretro_entities::provenance::DescriptorSpawnPath;
use postretro_entities::{ContactHit, EntityId, EntityRegistry, Transform};
use postretro_foundation::{
    ActivationTrigger, AirParams, CapsuleParams, FallParams, GroundParams, PlacementOffset,
    PlayerMovementComponent, PlayerMovementDescriptor, ResolutionMode, SpeedParams,
    WeaponActivationDescriptor, WeaponDescriptor, WeaponPlacementDescriptor,
};
use postretro_scripting_core::data_descriptors::{EntityTypeDescriptor, HealthDescriptor};

use super::{
    MapEntity, data_archetype, data_archetype_test_fixtures::mesh_descriptor, net_descriptor,
};
use crate::collision::CollisionWorld;
use crate::movement::{self, MovementInput};
use crate::scripting_systems::hit_zones::{
    HitZoneStore, ModelLoadWarningOwner, model_matrix, sample_world_pose_for_probe,
};
use crate::weapon::{self, FireButtonState};

const MODEL: &str = "models/decraniated_low_poly_retro_pixel/scene.gltf";
const MAP: &str = include_str!("../../../../../content/dev/maps/splash-damage-demo.map");
const PLAYER: &str = include_str!("../../../../../content/dev/scripts/player.ts");
const PISTOL: &str = include_str!("../../../../../content/dev/scripts/reference-pistol.ts");

fn map_placement(classname: &str, tag: &str, raw_origin: &str, yaw: f32) -> MapEntity {
    // Pin the individual point entity, rather than accepting the same origin
    // somewhere else in the map. This fixture has no brush-bearing entities.
    let block = MAP
        .split('}')
        .find(|block| block.contains(&format!("\"_tags\" \"{tag}\"")))
        .expect("named splash map point entity");
    assert!(block.contains(&format!("\"classname\" \"{classname}\"")));
    assert!(block.contains(&format!("\"origin\" \"{raw_origin}\"")));
    let raw: Vec<f32> = raw_origin
        .split_whitespace()
        .map(|n| n.parse().unwrap())
        .collect();
    let scale = postretro_level_compiler::map_format::MapFormat::IdTech2.units_to_meters() as f32;
    // The actual compiler adapter: parse.rs::quake_to_engine, then unit scale.
    let origin = Vec3::new(-raw[1], raw[2], -raw[0]) * scale;
    MapEntity {
        map_ordinal: None,
        classname: classname.into(),
        origin,
        angles: Vec3::new(0.0, -yaw.to_radians(), 0.0),
        key_values: HashMap::new(),
        tags: vec![tag.into()],
    }
}

fn dummy_descriptor() -> EntityTypeDescriptor {
    let mut descriptor = mesh_descriptor("target_dummy", true);
    let mesh = descriptor.mesh.as_mut().unwrap();
    mesh.model = MODEL.into();
    mesh.animations.retain(|name, _| name == "idle");
    let idle = mesh.animations.get_mut("idle").unwrap();
    idle.clip = "mixamo.com".into();
    idle.crossfade_ms = 0.0;
    descriptor.health = Some(HealthDescriptor {
        max: 48.0,
        hitbox: None,
        zone_multipliers: HashMap::new(),
    });
    descriptor
}

fn fixture(display_copy: bool) -> (EntityRegistry, EntityId, HitZoneStore) {
    let descriptor = dummy_descriptor();
    let placement = map_placement("target_dummy", "dummy splash_direct", "352 256 0", 180.0);
    let mut registry = EntityRegistry::new();
    let id = registry.spawn(Transform {
        position: placement.origin,
        rotation: placement.rotation_quat(),
        scale: Vec3::ONE,
    });
    if display_copy {
        assert!(net_descriptor::materialize_net_mesh_presentation(
            "target_dummy",
            &[descriptor],
            &mut registry,
            id,
            None
        ));
        assert!(registry.get_component::<HealthComponent>(id).is_err());
    } else {
        data_archetype::attach_descriptor_components(
            &mut registry,
            id,
            &descriptor,
            &placement,
            false,
            DescriptorSpawnPath::MapPlacement,
            None,
        );
        assert!(
            registry
                .get_component::<HealthComponent>(id)
                .unwrap()
                .hitbox
                .is_none()
        );
    }
    let mut store = HitZoneStore::new();
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../content/dev");
    store.insert_from_load(MODEL, &root, ModelLoadWarningOwner::GameSide);
    let zones = store.get_by_name(MODEL).expect("shipped glTF loads");
    assert!(
        zones.derived_bound.is_some(),
        "real joint zones seed the broad phase"
    );
    let clip_index = zones
        .clips
        .iter()
        .position(|clip| clip.name == "mixamo.com")
        .expect("actual idle clip");
    let mut mesh = registry.get_component::<MeshComponent>(id).unwrap().clone();
    mesh.animation
        .as_mut()
        .unwrap()
        .states
        .get_mut("idle")
        .unwrap()
        .clip_index = Some(clip_index);
    registry.set_component(id, mesh).unwrap();
    (registry, id, store)
}

fn pistol() -> (WeaponComponent, WeaponPlacementDescriptor) {
    for pin in [
        "damage: 12.0",
        "range: 64.0",
        "recoveryMs: 180.0",
        "resolution: \"hitscan\"",
        "positionFromCenter: { right: 0.25, up: -0.3, forward: 0.5 }",
    ] {
        assert!(PISTOL.contains(pin), "dev pistol changed: {pin}");
    }
    let placement = WeaponPlacementDescriptor {
        offset: PlacementOffset {
            right: 0.25,
            up: -0.3,
            forward: 0.5,
        },
        rotation: Default::default(),
    };
    let weapon = WeaponComponent::from_descriptor(&WeaponDescriptor {
        sounds: None,
        knockback: None,
        damage: 12.0,
        pellet_count: 1,
        spread_degrees: 0.0,
        bloom_per_shot_degrees: 1.6,
        bloom_max_degrees: 4.5,
        bloom_decay_degrees_per_second: 20.0,
        bloom_decay_delay_ms: 90.0,
        movement_spread_degrees: 1.5,
        spread_vertical_bias: 0.0,
        range: 64.0,
        primary: WeaponActivationDescriptor::single(ActivationTrigger::Press, 180.0),
        secondary: None,
        resolution: ResolutionMode::Hitscan,
        projectile: None,
        splash: None,
        credit_source: None,
        third_person_model: None,
        viewmodel: None,
        placement: Some(placement.clone()),
        muzzle_offset: Some([0.0, 0.225, -0.574]),
        resource: None,
        lower_ms: 0,
        raise_ms: 0,
        block_during_reload: None,
    });
    assert_eq!(weapon.bloom_accumulator_degrees, 0.0);
    (weapon, placement)
}

fn assert_pistol_hit(
    registry: &EntityRegistry,
    id: EntityId,
    store: &HitZoneStore,
    origin: Vec3,
    direction: Vec3,
) {
    let (mut pistol, placement) = pistol();
    let result = weapon::resolve_test_client_shot(
        None,
        &mut pistol,
        "reference_pistol",
        0,
        FireButtonState {
            pressed: true,
            active: true,
        },
        origin,
        direction,
        &placement,
        Some(Vec3::new(0.0, 0.225, -0.574)),
        7,
        &[0.0],
        &[],
        &CollisionWorld::new(),
        registry,
        store,
        0.0,
        0.0,
    )
    .expect("fresh press produces dev pistol shot");
    assert_eq!(
        result.hits.len(),
        1,
        "real dummy ray origin={origin:?} direction={direction:?}"
    );
    assert_eq!(result.hits[0].target, id);
    assert!(
        result.hits[0].zone.is_some(),
        "real skeletal zone, no authored AABB fallback"
    );
    let contacts = result.impact_contacts();
    assert_eq!(contacts.len(), 1);
    assert_eq!(contacts[0].hit, ContactHit::Entity(id));
}

#[test]
fn real_dummy_torso_aim_hits_local_and_mesh_only_display_copy() {
    for display_copy in [false, true] {
        let (registry, id, store) = fixture(display_copy);
        let mesh = registry.get_component::<MeshComponent>(id).unwrap();
        let zones = store.get_by_name(MODEL).unwrap();
        let pose = sample_world_pose_for_probe(zones, mesh.animation.as_ref(), 0.0, id.to_raw())
            .expect("actual idle pose resolves");
        let torso = zones
            .joint_zones
            .iter()
            .position(|zone| zone.as_ref().is_some_and(|zone| zone.tag == "torso"))
            .expect("model declares torso");
        let child = zones
            .skeleton
            .joints
            .iter()
            .position(|joint| joint.parent == Some(torso))
            .unwrap_or(torso);
        let target_model = (pose[torso].w_axis.truncate() + pose[child].w_axis.truncate()) * 0.5;
        let transform = registry.get_component::<Transform>(id).unwrap();
        let target = model_matrix(transform, mesh.origin_offset)
            .unwrap()
            .transform_point3(target_model);
        let player = map_placement("player_spawn", "player", "288 256 24", 0.0);
        let origin = Vec3::new(player.origin.x, target.y, player.origin.z);
        assert_pistol_hit(&registry, id, &store, origin, (target - origin).normalize());
    }
}

fn player_movement() -> PlayerMovementDescriptor {
    assert!(PLAYER.contains("capsule: { radius: 0.2, halfHeight: 0.8, eyeHeight: 0.5 }"));
    PlayerMovementDescriptor {
        sounds: None,
        knockback: Default::default(),
        capsule: CapsuleParams {
            radius: 0.2,
            half_height: 0.8,
            eye_height: 0.5,
        },
        ground: GroundParams {
            speed: SpeedParams {
                walk: 7.0,
                run: 11.0,
                crouch: 3.0,
            },
            accel: 15.0,
            step_height: 0.5,
            max_slope: 45.0,
        },
        air: AirParams {
            forward_steer: 0.5,
            accel: 10.0,
            max_control_speed: 2.0,
            bunny_hop: true,
            jumps: 0,
            jump_velocity: 9.0,
            jump_ceiling: 0.0,
        },
        fall: FallParams {
            terminal_velocity: 40.0,
        },
        stuck_stop_enabled: true,
        stuck_stop_threshold: 0.001,
        dash: None,
        forgiveness: None,
        crouch: None,
        slide: None,
        view_feel: None,
    }
}

#[test]
fn real_dummy_splash_direct_settled_player_eye_aim_hits_local_and_display_copy() {
    // Spawn uses the authored origin unchanged for a movement-only descriptor;
    // the physics tick settles the capsule before the level reveal. Do not add
    // a guessed feet-to-eye lift to the raw map z=24 coordinate.
    let placement = map_placement("player_spawn", "player", "288 256 24", 0.0);
    let mut player_descriptor = mesh_descriptor("player", false);
    player_descriptor.mesh = None;
    player_descriptor.movement = Some(player_movement());
    let mut pawns = EntityRegistry::new();
    assert_eq!(
        data_archetype::spawn_from_player_starts(
            std::slice::from_ref(&placement),
            &[player_descriptor],
            &mut pawns,
            None
        )
        .spawned,
        1
    );
    let pawn = pawns
        .local_player_pawn()
        .expect("spawn installs local pawn");
    let mut transform = *pawns.get_component::<Transform>(pawn).unwrap();
    assert_eq!(transform.position, placement.origin);
    let mut component = pawns
        .get_component::<PlayerMovementComponent>(pawn)
        .unwrap()
        .clone();
    assert!(MAP.contains("\"initialGravity\" \"-9.81\""));
    assert!(MAP.contains("( 0 512 0 ) ( 640 0 0 ) ( 0 0 0 )"));
    let floor = CollisionWorld::from_triangles_for_test(
        vec![
            Vec3::new(-100.0, 0.0, -100.0),
            Vec3::new(-100.0, 0.0, 100.0),
            Vec3::new(100.0, 0.0, 100.0),
            Vec3::new(100.0, 0.0, -100.0),
        ],
        vec![[0, 1, 2], [0, 2, 3]],
    );
    let input = MovementInput {
        wish_dir: Vec2::ZERO,
        jump_pressed: false,
        dash_pressed: false,
        running: false,
        crouch_intent: false,
        facing_yaw: 0.0,
        use_pressed: false,
        drop_pressed: false,
    };
    for _ in 0..120 {
        transform.position = movement::tick(
            &mut component,
            &input,
            &floor,
            postretro_collision::EnvironmentView::level_only(-9.81),
            1.0 / 60.0,
            transform.position,
        )
        .0;
    }
    assert!(
        component.is_grounded(),
        "real neutral player movement settles on map floor"
    );
    // main.rs::local_pawn_eye_position with the resting presentation offset zero.
    let eye = transform.position + Vec3::Y * component.capsule.eye_height;
    let aim = transform.rotation * Vec3::NEG_Z;
    for display_copy in [false, true] {
        let (registry, id, store) = fixture(display_copy);
        assert_pistol_hit(&registry, id, &store, eye, aim);
    }
}
