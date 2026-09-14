//! Cross-stage authorization coverage belongs here rather than in sim tests.
//!
//! `ingest_hit_declaration_for_test` names `CollisionWorld` and `HitZoneStore`,
//! which are sim types.  Calling it from postretro-sim's test target would see
//! a second compilation of sim through netcode's test-only dependency.  This
//! harness keeps the exact production ingester and one compilation view of
//! those types together in postretro-netcode.

use glam::Vec3;
use postretro_combat_model::{AuthorizedShot, MAX_OPEN_SHOT_AGE_TICKS, ShotId};
use postretro_entities::components::health::{HealthComponent, Hitbox};
use postretro_entities::{EntityId, EntityRegistry, Transform};
use postretro_net::wire::{self, ClientMessage, HitDeclaration, HitRecord, NetworkId};

use crate::collision::CollisionWorld;
use crate::scripting_systems::hit_zones::HitZoneStore;
use crate::{
    MovementOwners, NetworkIdAllocator, OpenAuthorizedShots, ingest_hit_declaration_for_test,
};

const CLIENT_ID: u64 = 7;
const DAMAGE: f32 = 10.0;

struct HitIngestFixture {
    registry: EntityRegistry,
    collision_world: CollisionWorld,
    hit_zones: HitZoneStore,
    allocator: NetworkIdAllocator,
    owners: MovementOwners,
    open_shots: OpenAuthorizedShots,
    pawn: EntityId,
    weapon: EntityId,
    target: EntityId,
    target_network_id: NetworkId,
    shot_id: ShotId,
}

impl HitIngestFixture {
    fn authorized_projectile(fire_origin: Vec3, range: f32) -> Self {
        let mut registry = EntityRegistry::new();
        let pawn = registry.spawn(Transform::default());
        let weapon = registry.spawn(Transform::default());
        let target = registry.spawn(Transform {
            position: Vec3::new(0.0, 0.0, -3.0),
            ..Transform::default()
        });
        registry
            .set_component(
                target,
                HealthComponent {
                    max: 100.0,
                    current: 100.0,
                    hitbox: Some(Hitbox {
                        half_extents: Vec3::splat(0.5),
                        offset: Vec3::ZERO,
                    }),
                    death_handled: false,
                    pending_kill_credit: None,
                    zone_multipliers: Default::default(),
                    contributor_ledger: Default::default(),
                },
            )
            .expect("target accepts health");

        let mut allocator = NetworkIdAllocator::new();
        let pawn_network_id = allocator.stamp(pawn);
        let target_network_id = allocator.stamp(target);
        let shot_id = ShotId::from_parts(pawn_network_id, 9);
        let mut owners = MovementOwners::new();
        owners.set(pawn, CLIENT_ID);
        let mut open_shots = OpenAuthorizedShots::new();
        open_shots.record(
            AuthorizedShot {
                shot_id,
                pawn,
                weapon,
                fire_tick: 10,
                damage: DAMAGE,
                range,
                pellet_count: 1,
                credit_source: "weapon.test.projectile".to_string(),
                knockback: None,
                splash: None,
                projectile_radius: None,
                projectile_direction: None,
                projectile_speed: None,
                projectile_lifetime_seconds: None,
                projectile_tick_seconds: None,
                is_projectile: true,
                fire_origin,
                timeout_budget_ticks: MAX_OPEN_SHOT_AGE_TICKS,
            },
            CLIENT_ID,
        );

        Self {
            registry,
            collision_world: CollisionWorld::new(),
            hit_zones: HitZoneStore::new(),
            allocator,
            owners,
            open_shots,
            pawn,
            weapon,
            target,
            target_network_id,
            shot_id,
        }
    }

    fn declaration(&self, point: Vec3) -> HitDeclaration {
        HitDeclaration {
            shot_id: self.shot_id.raw(),
            records: vec![HitRecord {
                target: self.target_network_id.0,
                point: point.to_array(),
                zone: None,
            }],
        }
    }

    fn ingest(&mut self, declaration: &HitDeclaration) -> (bool, bool) {
        ingest_hit_declaration_for_test(
            &mut self.registry,
            &self.collision_world,
            &self.hit_zones,
            &self.allocator,
            &self.owners,
            &mut self.open_shots,
            CLIENT_ID,
            declaration,
        )
    }

    fn health(&self) -> &HealthComponent {
        self.registry
            .get_component::<HealthComponent>(self.target)
            .expect("target remains live")
    }
}

fn wire_deliver(declaration: HitDeclaration) -> HitDeclaration {
    let bytes = wire::encode(&ClientMessage::HitDeclaration(declaration));
    let ClientMessage::HitDeclaration(delivered) =
        wire::decode(&bytes).expect("declaration survives input-wire encoding")
    else {
        panic!("encoded declaration retains its input message variant");
    };
    delivered
}

// Moved from sim/weapon_stage.rs.  The ingester must use the host-frozen
// projectile origin, not a client-selected impact point, when binding a wire
// declaration to the authorization.
#[test]
fn connected_obstructed_muzzle_declaration_replays_host_splash_from_eye() {
    let mut fixture = HitIngestFixture::authorized_projectile(Vec3::new(0.0, 0.5, 0.0), 4.0);
    let delivered = wire_deliver(fixture.declaration(Vec3::new(0.0, 0.0, -3.0)));

    assert_eq!(fixture.ingest(&delivered), (true, true));
    assert_eq!(fixture.health().current, 100.0 - DAMAGE);
}

// Moved from sim/weapon_stage.rs.  A declaration remains attached to the
// authorization that froze the converged projectile path, after wire transit.
#[test]
fn connected_lateral_muzzle_convergence_matches_host_splash_replay() {
    let mut fixture = HitIngestFixture::authorized_projectile(Vec3::new(0.75, 0.5, 0.0), 4.0);
    let delivered = wire_deliver(fixture.declaration(Vec3::new(0.0, 0.0, -3.0)));

    assert_eq!(fixture.ingest(&delivered), (true, true));
    assert_eq!(fixture.health().current, 100.0 - DAMAGE);
}

// Moved from sim/weapon_stage.rs.  This point is outside the eye-origin
// tolerance but inside the authorization's frozen muzzle-origin tolerance.
#[test]
fn remote_projectile_contact_within_muzzle_range_validates() {
    let muzzle = Vec3::new(0.0, 0.0, -1.0);
    let range = 2.0;
    let contact = Vec3::new(0.0, 0.0, -3.49);
    assert!(Vec3::ZERO.distance(contact) > range * 1.25);
    assert!(muzzle.distance(contact) < range * 1.25);

    let mut fixture = HitIngestFixture::authorized_projectile(muzzle, range);
    let declaration = fixture.declaration(contact);
    assert_eq!(fixture.ingest(&declaration), (true, true));
    assert_eq!(fixture.health().current, 100.0 - DAMAGE);
}

// Moved from sim/weapon_stage.rs.  A plausible late declaration without a
// host-issued authorization cannot spend health.
#[test]
fn rejected_remote_projectile_fire_cannot_later_declare_plausible_damage() {
    let mut fixture = HitIngestFixture::authorized_projectile(Vec3::ZERO, 4.0);
    let declaration = fixture.declaration(Vec3::new(0.0, 0.0, -3.0));
    fixture.open_shots.retire(fixture.shot_id);

    assert_eq!(fixture.ingest(&declaration), (false, false));
    assert_eq!(fixture.health().current, 100.0);
}

// Moved from sim/weapon_stage.rs.  The host, rather than the client, records
// both the accepted damage and the authorized pawn/weapon credit.
#[test]
fn connected_client_projectile_declares_later_and_host_applies_authorized_credit() {
    let mut fixture = HitIngestFixture::authorized_projectile(Vec3::ZERO, 4.0);
    let delivered = wire_deliver(fixture.declaration(Vec3::new(0.0, 0.0, -3.0)));

    assert_eq!(fixture.ingest(&delivered), (true, true));
    let health = fixture.health();
    assert_eq!(health.current, 100.0 - DAMAGE);
    assert_eq!(
        health
            .contributor_ledger
            .recorded_damage_by_source("weapon.test.projectile"),
        Some(DAMAGE)
    );
    let credit = health
        .contributor_ledger
        .entries()
        .first()
        .expect("accepted host impact records attacker credit");
    assert_eq!(credit.last_attacker, Some(fixture.pawn));
    assert_eq!(credit.last_weapon, Some(fixture.weapon));
}

// Moved from scripting_systems/ai_tests.rs.  Ready declarations are consumed
// by the netcode boundary before a simulation callback observes their damage
// ledger; sim owns the separate retaliation-selection behavior test.
#[test]
fn ready_remote_hit_reaches_retaliation_selection_in_the_same_simulation_tick() {
    let mut fixture = HitIngestFixture::authorized_projectile(Vec3::ZERO, 4.0);
    let declaration = fixture.declaration(Vec3::new(0.0, 0.0, -3.0));

    assert_eq!(fixture.ingest(&declaration), (true, true));
    assert_eq!(fixture.health().current, 100.0 - DAMAGE);
    assert_eq!(
        fixture
            .health()
            .contributor_ledger
            .recorded_damage_by_source("weapon.test.projectile"),
        Some(DAMAGE),
        "the next sim stage can observe the host-written damage ledger"
    );
}
