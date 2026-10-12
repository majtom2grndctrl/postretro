use super::*;
use postretro_entities::components::inventory::Inventory;
use postretro_entities::components::weapon::WeaponComponent;

fn hold(registry: &mut EntityRegistry, holder: EntityId, item: EntityId) {
    let mut inventory = Inventory::default();
    // Ownership includes inactive slots, independently of the selected weapon.
    inventory.wieldables[9] = Some(item);
    registry.set_component(holder, inventory).unwrap();
}

#[test]
fn held_world_participation_suspends_mixed_actor_and_retains_private_execution() {
    let graph = standing_projectile_attack_graph("enemy.rifle");
    let mut descriptor = projectile_weapon_descriptor("enemy.rifle", 2.0, 13.0, 300.0);
    let weapon = descriptor.weapon.as_mut().unwrap();
    weapon.primary = serde_json::from_value(serde_json::json!({
        "trigger":"press", "recoveryMs":300,
        "steps":[{"kind":"shot"}, {"kind":"wait", "durationMs":80}, {"kind":"shot"}]
    }))
    .unwrap();
    weapon.resource = Some(
        serde_json::from_value(serde_json::json!({
            "kind":"ammo", "type":"bullets", "magazine":5, "reserve":0
        }))
        .unwrap(),
    );
    let mut registry = EntityRegistry::new();
    let mut runtime = AiRuntime::new();
    let holder = spawn_player(&mut registry, Vec3::X);
    let item = spawn_enemy(
        &mut registry,
        Vec3::ZERO,
        authored_brain(&graph, "strike"),
        47.0,
    );
    registry
        .set_component(item, WeaponComponent::from_descriptor(weapon))
        .unwrap();
    let descriptors = [descriptor];
    assert_eq!(
        tick_persistent_enemy_weapon(&mut registry, &mut runtime, &descriptors, 1)
            .projectile_spawns
            .len(),
        1
    );
    let brain = registry
        .get_component::<BrainComponent>(item)
        .unwrap()
        .clone();
    let agent = registry
        .get_component::<AgentComponent>(item)
        .unwrap()
        .clone();
    let transform = *registry.get_component::<Transform>(item).unwrap();
    let private = runtime.weapons.weapon(item, "enemy.rifle").unwrap().clone();
    hold(&mut registry, holder, item);
    for _ in 0..12 {
        let result = tick_persistent_enemy_weapon(&mut registry, &mut runtime, &descriptors, 1);
        assert!(result.events.is_empty());
        assert!(result.projectile_spawns.is_empty());
        agent_steering::tick(
            &mut registry,
            &CollisionWorld::new(),
            None,
            -20.0,
            1.0 / 60.0,
        );
    }
    assert_eq!(
        registry.get_component::<BrainComponent>(item).unwrap(),
        &brain
    );
    assert_eq!(
        registry.get_component::<AgentComponent>(item).unwrap(),
        &agent
    );
    assert_eq!(
        registry.get_component::<Transform>(item).unwrap(),
        &transform
    );
    assert_eq!(
        registry
            .get_component::<HealthComponent>(item)
            .unwrap()
            .current,
        47.0
    );
    let retained = runtime.weapons.weapon(item, "enemy.rifle").unwrap();
    assert_eq!(retained.magazine, private.magazine);
    assert_eq!(retained.state, private.state);
    assert_eq!(
        retained.cooldown_remaining_ms,
        private.cooldown_remaining_ms
    );
    assert!(std::sync::Arc::ptr_eq(
        retained.activation_programs.primary.as_ref().unwrap(),
        private.activation_programs.primary.as_ref().unwrap()
    ));
    registry
        .set_component(holder, Inventory::default())
        .unwrap();
    let shots: usize = (0..8)
        .map(|_| {
            tick_persistent_enemy_weapon(&mut registry, &mut runtime, &descriptors, 1)
                .projectile_spawns
                .len()
        })
        .sum();
    assert_eq!(
        shots, 1,
        "the accepted execution resumes without reinstall or a fresh start"
    );
    assert_eq!(
        runtime
            .weapons
            .weapon(item, "enemy.rifle")
            .unwrap()
            .magazine,
        3
    );
}

#[test]
fn held_world_participation_removes_retained_and_fresh_targets_until_drop() {
    let graph = standing_attack_graph();
    let mut registry = EntityRegistry::new();
    let target = spawn_enemy(
        &mut registry,
        Vec3::X,
        authored_brain(&graph, "strike"),
        47.0,
    );
    registry.entity_state_mut(target).unwrap().set(
        FACTION_STATE_FIELD,
        postretro_entities::PLAYER_FACTION_INDEX,
    );
    let actor = spawn_enemy(
        &mut registry,
        Vec3::ZERO,
        authored_brain(&graph, "strike"),
        50.0,
    );
    let holder = registry.spawn(Transform::default());
    let descriptor = projectile_weapon_descriptor("held.rifle", 2.0, 13.0, 300.0);
    registry
        .set_component(
            target,
            WeaponComponent::from_descriptor(descriptor.weapon.as_ref().unwrap()),
        )
        .unwrap();
    let mut brain = registry
        .get_component::<BrainComponent>(actor)
        .unwrap()
        .clone();
    brain.acquired_target = Some(target);
    registry.set_component(actor, brain).unwrap();
    hold(&mut registry, holder, target);
    assert!(target_candidate(&registry, target, Vec3::ZERO).is_none());
    let mut runtime = AiRuntime::new();
    assert!(run_ai_tick(&mut registry, &mut runtime, 0.016).is_empty());
    assert_eq!(
        registry
            .get_component::<HealthComponent>(target)
            .unwrap()
            .current,
        47.0
    );
    assert_eq!(
        registry
            .get_component::<BrainComponent>(actor)
            .unwrap()
            .acquired_target,
        None
    );
    registry
        .set_component(holder, Inventory::default())
        .unwrap();
    assert!(target_candidate(&registry, target, Vec3::ZERO).is_some());
    run_ai_tick(&mut registry, &mut runtime, 0.016);
    assert_eq!(
        registry
            .get_component::<HealthComponent>(target)
            .unwrap()
            .current,
        39.0
    );
}

#[test]
fn held_world_participation_rechecks_later_actor_and_target_after_contact_callback() {
    for hold_actor in [false, true] {
        let graph = standing_attack_graph();
        let mut registry = EntityRegistry::new();
        let target = spawn_player(&mut registry, Vec3::X);
        let holder = registry.spawn(Transform::default());
        spawn_enemy(
            &mut registry,
            Vec3::ZERO,
            authored_brain(&graph, "strike"),
            50.0,
        );
        let later = spawn_enemy(
            &mut registry,
            Vec3::ZERO,
            authored_brain(&graph, "strike"),
            50.0,
        );
        let descriptor = projectile_weapon_descriptor("held.rifle", 2.0, 13.0, 300.0);
        registry
            .set_component(
                if hold_actor { later } else { target },
                WeaponComponent::from_descriptor(descriptor.weapon.as_ref().unwrap()),
            )
            .unwrap();
        let mut callbacks = 0;
        let result = run_ai_tick_with_navigation_and_impact(
            &mut registry,
            &mut AiRuntime::new(),
            0.016,
            AiTickInputs {
                nav_graph: None,
                collision_world: None,
                descriptors: &[],
                descriptor_generation: 0,
                factions: &FactionRegistry::default(),
                faction_sentiment: &RefCell::new(FactionSentimentState::default()),
            },
            |registry| {
                callbacks += 1;
                hold(registry, holder, if hold_actor { later } else { target });
            },
        );
        assert_eq!(callbacks, 1);
        assert_eq!(
            event_addresses(&result.events),
            vec![Cow::Borrowed(ENEMY_ATTACK_EVENT)]
        );
        assert_eq!(
            registry
                .get_component::<BrainComponent>(later)
                .unwrap()
                .activity_attack_count(0),
            Some(0)
        );
    }
}
