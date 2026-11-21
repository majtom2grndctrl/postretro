//! A client's hitscan casts against movers at its predicted poses; host
//! validation stays static (`networking.md` §World-LOS-only validation).

use super::*;
use crate::collision::moving::CombinedCollisionWorld;
use crate::collision::test_movers::{DoorPoses, door_slab};
use glam::Quat;

const DOOR: u32 = 3;

/// The pawn's eye: the fixture pawn stands at the origin with eye height 0.5.
const EYE: Vec3 = Vec3::new(0.0, 0.5, 0.0);

fn closed_door() -> DoorPoses {
    DoorPoses::default().place(DOOR, Vec3::new(2.0, -1.0, 0.0), Quat::IDENTITY)
}

fn open_door() -> DoorPoses {
    DoorPoses::default().place(DOOR, Vec3::new(2.0, 4.0, 0.0), Quat::IDENTITY)
}

/// A fixture whose target (at x = 4) carries a hitbox the client's ray can hit.
fn fixture() -> HitIngestFixture {
    let fixture = HitIngestFixture::new(CollisionWorld::new());
    let mut health = fixture.target_health();
    health.hitbox = Some(Hitbox {
        half_extents: Vec3::splat(0.5),
        offset: Vec3::new(0.0, 0.5, 0.0),
    });
    let mut fixture = fixture;
    fixture
        .registry
        .set_component(fixture.target, health)
        .unwrap();
    fixture
}

/// Resolve the client's shot with movers at `predicted` poses and declare it.
fn client_declaration(fixture: &HitIngestFixture, predicted: &DoorPoses) -> wire::HitDeclaration {
    let movers = [door_slab(DOOR, 0.25, 3.0, 2.0)];
    let static_world = CollisionWorld::new();
    let world = CombinedCollisionWorld::new(&static_world, &movers, predicted);
    let mut weapon = fixture
        .registry
        .get_component::<WeaponComponent>(fixture.weapon)
        .unwrap()
        .clone();
    let resolution = crate::weapon::resolve_test_client_shot(
        Some(fixture.pawn),
        &mut weapon,
        "weapon.test.net",
        0,
        crate::weapon::FireButtonState {
            pressed: true,
            active: true,
        },
        EYE,
        Vec3::X,
        &Default::default(),
        None,
        11,
        &[],
        &[],
        &world,
        &fixture.registry,
        &fixture.hit_zone_store,
        0.0,
        1.0 / 60.0,
    )
    .expect("client fires");
    fixture.declaration(local_hits_to_wire_records(
        &resolution.hits,
        &resolution.world_contacts,
        |id| fixture.allocator.network_id_for_entity(id),
    ))
}

#[test]
fn client_hitscan_declares_a_world_contact_at_a_door_closed_in_its_prediction() {
    let mut fixture = fixture();
    let health_before = fixture.target_health().current;
    let declaration = client_declaration(&fixture, &closed_door());
    let [record] = declaration.records.as_slice() else {
        panic!("one pellet, one record: {:?}", declaration.records);
    };
    assert_eq!(record.target, PRESENTATION_CONTACT_TARGET);
    assert!((Vec3::from_array(record.point) - Vec3::new(1.75, 0.5, 0.0)).length() < 1.0e-4);

    let result = fixture.ingest_result(7, &declaration);

    assert!(result.fire_accepted);
    assert!(!result.hit_accepted, "a world contact deals no damage");
    assert_eq!(
        result.contacts,
        vec![emission::ImpactContact::new(
            Vec3::from_array(record.point),
            Vec3::NEG_X,
            None
        )],
        "the host keeps the door contact for presentation",
    );
    assert_eq!(fixture.target_health().current, health_before);
}

#[test]
fn client_hitscan_through_an_open_door_is_accepted_even_where_the_host_has_it_closed() {
    let mut fixture = fixture();
    let health_before = fixture.target_health().current;
    let declaration = client_declaration(&fixture, &open_door());
    let [record] = declaration.records.as_slice() else {
        panic!("one pellet, one record: {:?}", declaration.records);
    };
    assert_eq!(record.target, fixture.target_net.0);

    // Host validation reads static geometry only, so a door the host has
    // already closed (and the client still predicts open) refuses nothing.
    let result = fixture.ingest_result(7, &declaration);

    assert!(result.hit_accepted);
    assert!(fixture.target_health().current < health_before);
}
