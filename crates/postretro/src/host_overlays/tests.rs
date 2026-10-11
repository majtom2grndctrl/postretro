use super::*;
use crate::impact_policy::ImpactPolicyRuntime;
use glam::Vec3;
use postretro_entities::components::health::{
    DamageContext, DamageProducer, HealthComponent, apply_damage_with_context,
};
use postretro_entities::provenance::{
    DescriptorComponentKind, DescriptorProvenance, DescriptorSpawnPath,
};
use postretro_entities::{ScriptCtx, Transform};
use postretro_foundation::DamagePayload;
use serde_json::json;

fn runtime(ctx: &ScriptCtx) -> ImpactPolicyRuntime {
    let mut runtime = ImpactPolicyRuntime::new(ctx.clone());
    runtime.replace_presentation_templates(vec![
        serde_json::from_value(json!({
            "id": "test.status",
            "root": { "kind": "text", "content": "0", "fontSize": 16.0,
                "color": [1.0, 1.0, 1.0, 1.0] },
            "worldAnchor": { "socket": "status", "offsetY": 0.25 },
            "lifetimeMs": 750,
            "motion": { "rise": 0.0, "easing": "easeOut" },
            "fade": { "startMs": 500 }, "spawnScatter": { "radius": 0.0 }
        }))
        .unwrap(),
    ]);
    runtime.replace_presentation_overlays(vec![
        serde_json::from_value(json!({
            "over": { "kind": "damagedEnemies", "lingerMs": 1000, "hideAtFull": true },
            "template": "test.status", "maxVisible": 2
        }))
        .unwrap(),
    ]);
    runtime
}

fn fresh_target(registry: &mut EntityRegistry) -> EntityId {
    let entity = registry.spawn(Transform::default());
    registry
        .set_component(
            entity,
            HealthComponent::from_descriptor(
                &postretro_scripting_core::data_descriptors::HealthDescriptor {
                    max: 100.0,
                    hitbox: None,
                    zone_multipliers: Default::default(),
                },
            ),
        )
        .unwrap();
    registry
        .set_component(
            entity,
            DescriptorProvenance {
                canonical_name: "test.enemy".into(),
                owned_components: [DescriptorComponentKind::Health].into_iter().collect(),
                map_overrides: Default::default(),
                spawn_path: DescriptorSpawnPath::RuntimeSpawn,
            },
        )
        .unwrap();
    entity
}

fn hit(registry: &mut EntityRegistry, target: EntityId) {
    apply_damage_with_context(
        registry,
        target,
        &DamagePayload {
            amount: 10.0,
            impulse: Vec3::ZERO,
        },
        DamageContext::new("app-host-overlay", DamageProducer::InTick),
    );
}

#[test]
fn normal_host_damage_registers_same_tick_spawn_without_local_notice_backlog() {
    let ctx = ScriptCtx::new();
    let mut runtime = runtime(&ctx);
    let config = runtime.client_overlay_config().unwrap();
    let mut allocator = NetworkIdAllocator::for_test();
    let mut replicable = ReplicableSet::default();
    let mut tracked = HashSet::new();
    let mut overrides = PresentationOverrides::default();
    let mut tracker = HostOverlayFactTracker::default();
    let identities = HostOverlayIdentities::new(&mut allocator, &mut replicable, &mut tracked);
    let mut registry = ctx.registry.borrow_mut();
    let target = fresh_target(&mut registry);
    assert!(
        identities
            .allocator()
            .network_id_for_entity(target)
            .is_none()
    );
    ctx.presentation_script_time.set(7.5);
    hit(&mut registry, target);
    runtime.evaluate_pending_in_registry_with_overlay_sink(&mut registry, |registry, damage| {
        assert_eq!(damage.entity, target);
        assert!((damage.damaged_at - 7.5).abs() < 1.0e-9);
        identities.record_damage(registry, damage, &mut overrides, &mut tracker, &config);
    });
    let network_id = identities
        .allocator()
        .network_id_for_entity(target)
        .expect("same-tick damage stamps canonical identity");
    assert_eq!(tracker.tracked_len(), 1);
    assert!(runtime.take_damaged_enemy_overlay_damage().is_empty());
    drop(identities);
    assert!(tracked.contains(&target));
    assert_eq!(allocator.network_id_for_entity(target), Some(network_id));
}

#[test]
fn remote_hit_registration_covers_fresh_splash_targets_during_immutable_intake() {
    let ctx = ScriptCtx::new();
    let mut runtime = runtime(&ctx);
    let config = runtime.client_overlay_config().unwrap();
    let mut allocator = NetworkIdAllocator::for_test();
    let mut replicable = ReplicableSet::default();
    let mut tracked = HashSet::new();
    let mut overrides = PresentationOverrides::default();
    let mut tracker = HostOverlayFactTracker::default();
    let identities = HostOverlayIdentities::new(&mut allocator, &mut replicable, &mut tracked);
    let mut registry = ctx.registry.borrow_mut();
    let declared = fresh_target(&mut registry);
    let secondary = fresh_target(&mut registry);
    identities.register_before_remote_hits(&registry, &mut overrides);
    // This guard has the exact lifetime of ready/pending HIT application. A
    // host-resolved splash can damage a newly spawned secondary target while
    // netcode holds the allocator immutably for its declared target lookup.
    let intake_allocator = identities.allocator();
    assert!(intake_allocator.network_id_for_entity(declared).is_some());
    assert!(intake_allocator.network_id_for_entity(secondary).is_some());
    ctx.presentation_script_time.set(4.0);
    for target in [declared, secondary] {
        hit(&mut registry, target);
    }
    runtime.evaluate_pending_in_registry_with_overlay_sink(&mut registry, |registry, damage| {
        identities.record_damage(registry, damage, &mut overrides, &mut tracker, &config);
    });
    assert_eq!(tracker.tracked_len(), 2);
    assert!(runtime.take_damaged_enemy_overlay_damage().is_empty());
    drop(intake_allocator);
    drop(identities);
    assert_eq!(tracked.len(), 2);
}
