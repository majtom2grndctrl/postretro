// Descriptor-world combat and canonical ownership through the replication harness.
// See: context/lib/networking.md

use super::*;
use crate::scripting_systems::hit_zones::HitZoneStore;
use postretro_entities::components::health::HealthComponent;
use postretro_entities::components::inventory::Inventory;
use postretro_entities::components::weapon::WeaponComponent;
use postretro_entities::data_descriptors::{HealthDescriptor, HitboxDescriptor};
use postretro_entities::slot_table::{
    ReplicationScope, SlotOwnership, SlotRecord, SlotSchema, SlotType, SlotValue,
};
use postretro_foundation::{ImpactEventDescriptor, Seat, ShotId, WeaponDescriptor};
use serde_json::json;

pub(super) fn wieldable_descriptor() -> WeaponDescriptor {
    WeaponDescriptor {
        sounds: None,
        knockback: None,
        damage: 48.0,
        pellet_count: 1,
        spread_degrees: 0.0,
        bloom_per_shot_degrees: 0.0,
        bloom_max_degrees: 0.0,
        bloom_decay_degrees_per_second: 0.0,
        bloom_decay_delay_ms: 0.0,
        movement_spread_degrees: 0.0,
        spread_vertical_bias: 0.0,
        range: 20.0,
        primary: postretro_foundation::WeaponActivationDescriptor::single(
            postretro_foundation::ActivationTrigger::Press,
            100.0,
        ),
        secondary: None,
        resolution: postretro_foundation::ResolutionMode::Hitscan,
        projectile: None,
        splash: None,
        credit_source: None,
        third_person_model: None,
        viewmodel: None,
        placement: None,
        muzzle_offset: None,
        resource: None,
        lower_ms: 0,
        raise_ms: 0,
        block_during_reload: None,
    }
}

fn health() -> HealthDescriptor {
    HealthDescriptor {
        max: 48.0,
        hitbox: Some(HitboxDescriptor {
            half_extents: [0.5, 1.0, 0.5],
            offset: None,
        }),
        zone_multipliers: Default::default(),
    }
}

#[test]
fn held_default_weapon_retains_health_and_networks_only_after_release() {
    let mut h = EnemyReplicationHarness::new(perfect_link());
    let mut descriptor = world_item_descriptor("held_loadout");
    descriptor.weapon = Some(wieldable_descriptor());
    descriptor.health = Some(health());
    h.descriptors.push(descriptor.clone());
    crate::scripting::builtins::data_archetype::apply_data_archetype_dispatch(
        &[placement("held_loadout")],
        &[descriptor],
        &HashSet::new(),
        &mut h.host_registry,
        None,
    );
    let item = h
        .host_registry
        .iter_with_kind(ComponentKind::DescriptorProvenance)
        .next()
        .unwrap()
        .0;
    let mut provenance = h
        .host_registry
        .get_component::<DescriptorProvenance>(item)
        .unwrap()
        .clone();
    provenance.spawn_path = DescriptorSpawnPath::DefaultWeapon;
    h.host_registry.set_component(item, provenance).unwrap();
    let mut retained = h
        .host_registry
        .get_component::<HealthComponent>(item)
        .unwrap()
        .clone();
    retained.current = 17.0;
    h.host_registry
        .set_component(item, retained.clone())
        .unwrap();
    let holder = h.host_registry.spawn(Transform::default());
    let mut inventory = Inventory::default();
    inventory.wieldables[4] = Some(item);
    h.host_registry.set_component(holder, inventory).unwrap();
    h.host_register_descriptors();
    assert!(!h.replicable.contains(item));
    assert!(!h.allocator.maps_entity(item));
    assert!(
        h.host_registry
            .get_component::<WeaponComponent>(item)
            .is_ok()
    );
    assert_eq!(
        h.host_registry
            .get_component::<HealthComponent>(item)
            .unwrap(),
        &retained
    );

    h.host_registry
        .set_component(holder, Inventory::default())
        .unwrap();
    let mut dropped_pose = *h.host_registry.get_component::<Transform>(item).unwrap();
    dropped_pose.position = Vec3::new(7.0, 0.0, 0.0);
    h.host_registry.set_component(item, dropped_pose).unwrap();
    h.host_register_descriptors();
    let network_id = h.allocator.stamp(item);
    assert!(h.replicable.contains(item));
    h.step_until_client_maps(network_id, 16);
    let remote = h
        .client_mapped_entity(network_id)
        .expect("released loadout reaches client");
    assert!(
        h.client_registry
            .get_component::<MeshComponent>(remote)
            .is_ok()
    );
    assert_eq!(
        h.host_registry
            .get_component::<HealthComponent>(item)
            .unwrap(),
        &retained
    );
    assert_eq!(
        h.host_registry
            .get_component::<DescriptorProvenance>(item)
            .unwrap()
            .spawn_path,
        DescriptorSpawnPath::DefaultWeapon
    );
}

#[test]
fn descriptor_dummy_client_hit_credits_owner_and_despawns() {
    descriptor_hit_credits_owner_and_despawns(false);
}

#[test]
fn meshless_hit_volume_client_declaration_applies_on_host() {
    descriptor_hit_credits_owner_and_despawns(true);
}

fn descriptor_hit_credits_owner_and_despawns(meshless: bool) {
    let mut h = EnemyReplicationHarness::new(perfect_link());
    let mut descriptor = prop_descriptor("target_dummy");
    descriptor.health = Some(health());
    if meshless {
        descriptor.mesh = None;
    }
    h.descriptors.push(descriptor.clone());
    let mut dummy_placement = placement("target_dummy");
    dummy_placement.origin = Vec3::new(0.0, 0.0, -5.0);
    dummy_placement.tags = vec!["dummy".into()];
    crate::scripting::builtins::data_archetype::apply_data_archetype_dispatch(
        &[dummy_placement.clone()],
        &[descriptor],
        &HashSet::new(),
        &mut h.host_registry,
        None,
    );
    let target = h
        .host_registry
        .iter_with_kind(ComponentKind::DescriptorProvenance)
        .next()
        .unwrap()
        .0;
    assert!(
        filter_out_client_host_replicated_placements(&[dummy_placement], &h.descriptors).is_empty()
    );
    h.host_register_descriptors();
    let network_id = h.allocator.stamp(target);
    h.step_until_client_maps(network_id, 16);
    let remote = h
        .client_mapped_entity(network_id)
        .expect("dummy display copy");
    assert_eq!(
        h.client_registry
            .get_component::<MeshComponent>(remote)
            .is_ok(),
        !meshless
    );
    assert!(
        h.client_registry
            .get_component::<postretro_entities::components::hit_volume::HitVolumeComponent>(remote)
            .is_ok()
    );
    assert!(
        h.client_registry
            .get_component::<HealthComponent>(remote)
            .is_err()
    );
    let resolution = crate::weapon::resolve_test_client_shot(
        None,
        &mut WeaponComponent::from_descriptor(&wieldable_descriptor()),
        "weapon.test.net",
        0,
        crate::weapon::FireButtonState {
            pressed: true,
            active: true,
        },
        Vec3::ZERO,
        -Vec3::Z,
        &Default::default(),
        None,
        1,
        &[],
        &[],
        &crate::collision::CollisionWorld::new(),
        &h.client_registry,
        &HitZoneStore::new(),
        0.0,
        1.0 / 60.0,
    )
    .expect("client fires");
    assert_eq!(resolution.hits.len(), 1);
    assert_eq!(resolution.hits[0].target, remote);
    assert!(
        matches!(resolution.impact_contacts()[0].hit, postretro_entities::ContactHit::Entity(id) if id == remote)
    );
    let records =
        crate::local_hits_to_wire_records(&resolution.hits, &resolution.world_contacts, |id| {
            h.client_replication.network_id_for_entity(id)
        });
    assert_eq!(
        records.len(),
        1,
        "client declares a named hit rather than dropping it"
    );
    assert_eq!(records[0].target, network_id.0);

    let pawn = h.host_registry.spawn(Transform::default());
    let movement = PlayerMovementComponent::from_descriptor(&PlayerMovementDescriptor {
        sounds: None,
        knockback: Default::default(),
        capsule: CapsuleParams {
            radius: 0.35,
            half_height: 0.9,
            eye_height: 1.1,
        },
        ground: GroundParams {
            speed: SpeedParams {
                walk: 7.0,
                run: 11.0,
                crouch: 3.0,
            },
            accel: 12.0,
            step_height: 0.35,
            max_slope: 45.0,
        },
        air: AirParams {
            forward_steer: 0.0,
            accel: 0.7,
            max_control_speed: 0.5,
            bunny_hop: false,
            jumps: 1,
            jump_velocity: 5.0,
            jump_ceiling: 2.0,
        },
        fall: FallParams {
            terminal_velocity: 50.0,
        },
        stuck_stop_enabled: true,
        stuck_stop_threshold: 0.001,
        dash: None,
        forgiveness: None,
        crouch: None,
        slide: None,
        swim: None,
        view_feel: None,
    });
    h.host_registry.set_component(pawn, movement).unwrap();
    h.host_registry.bind_pawn_seat(pawn, Seat(7));
    let weapon = h.host_registry.spawn(Transform::default());
    let shot = postretro_combat_model::AuthorizedShot {
        sounds: None,
        action: None,
        source_weapon: None,
        knockback: None,
        shot_id: ShotId::from_parts(
            pawn.to_raw(),
            1,
            postretro_foundation::ActivationLane::Primary,
            0,
        ),
        pawn,
        weapon,
        fire_tick: 1,
        damage: 48.0,
        range: 20.0,
        pellet_count: 1,
        credit_source: "weapon.test.net".into(),
        splash: None,
        projectile_radius: None,
        projectile_direction: None,
        projectile_speed: None,
        projectile_lifetime_seconds: None,
        projectile_tick_seconds: None,
        projectile_static_contact_distance: None,
        is_projectile: false,
        fire_origin: Vec3::ZERO,
        timeout_budget_ticks: 180,
    };
    let ctx = ScriptCtx::new();
    ctx.slot_table
        .borrow_mut()
        .insert(
            "progression.xp".into(),
            SlotRecord::new(SlotSchema {
                slot_type: SlotType::Number,
                default: Some(SlotValue::Number(0.0)),
                range: None,
                persist: false,
                readonly: false,
                ownership: SlotOwnership::Mod,
                network: ReplicationScope::OwnerPrivatePlayer,
                per_owner: true,
                accumulate: None,
            }),
        )
        .unwrap();
    let mut policy = postretro_sim::impact_policy::ImpactPolicyRuntime::new(ctx.clone());
    policy.replace_global_events(vec![ImpactEventDescriptor {
        id: "dummy-death-reward".into(), is_override: false, levels: vec![], filter_tag: Some("dummy".into()),
        policy: vec![json!({
            "when": { "op": "le", "a": { "op": "input", "name": "@impact.healthAfter" }, "b": { "op": "const", "value": 0.0 } },
            "do": [
                { "primitive": "slot.set", "target": "@impact.source", "args": { "slot": "progression.xp", "value": { "op": "add", "a": { "op": "input", "name": "progression.xp", "owner": "@impact.source" }, "b": { "op": "const", "value": 10.0 } } } },
                { "primitive": "despawn", "target": "@impact.target", "args": {} }
            ]
        })],
    }]);
    assert!(crate::apply_valid_hit_record(
        &mut h.host_registry,
        &crate::collision::CollisionWorld::new(),
        &h.allocator,
        &shot,
        &records[0]
    ));
    assert_eq!(h.host_registry.seat_for_pawn(pawn), Some(Seat(7)));
    policy.evaluate_pending_in_registry(&mut h.host_registry);
    assert_eq!(
        h.host_registry.is_marked_for_end_of_frame_removal(target),
        Ok(true),
        "installed impact policy commits lethal dummy removal"
    );
    for removed in h.host_registry.take_end_of_frame_removals() {
        h.host_registry.despawn(removed).unwrap();
    }
    assert!(!h.host_registry.exists(target));
    let table = ctx.slot_table.borrow();
    let xp = table.get("progression.xp").unwrap();
    assert!(
        matches!(xp.per_seat_value(Seat(7)), Some(SlotValue::Number(value)) if (*value - 10.0).abs() < f32::EPSILON)
    );
    assert!(
        matches!(xp.per_seat_value(Seat(0)), Some(SlotValue::Number(value)) if value.abs() < f32::EPSILON),
        "host seat receives no client kill credit"
    );
    drop(table);
    h.host_register_descriptors();
    h.step();
    assert!(
        h.client_mapped_entity(network_id).is_none(),
        "next snapshot tombstones dummy"
    );
    assert!(!h.client_registry.exists(remote));
}
