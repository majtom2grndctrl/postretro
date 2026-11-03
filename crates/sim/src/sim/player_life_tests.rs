//! Life capability proofs through real trigger, pickup and weapon consumers.
use postretro_entities::Transform;
use std::collections::HashSet;

use super::tests::{
    remote_command, run_local_only_tick, run_remote_only_tick, sim_command, spawn_reload_pair,
    trigger_movement, weapon_component,
};
use super::*;
use postretro_entities::components::{
    health::HealthComponent, inventory::Inventory, touchable::TouchableComponent,
};
use postretro_entities::{
    DescriptorProvenance, DescriptorSpawnPath, LifeStatesDescriptor, NamedReaction,
    PrimitiveDescriptor, ReactionDescriptor, TouchMode, TriggerActivation, TriggerCommand,
    TriggerFireMode, TriggerVolumeComponent,
};
use postretro_foundation::{LifeStateProfile, enter_life_state};
use serde_json::json;

fn stamp(registry: &mut EntityRegistry, pawn: EntityId, profile: &LifeStateProfile) {
    let mut movement = registry
        .get_component::<PlayerMovementComponent>(pawn)
        .cloned()
        .unwrap_or_else(|_| trigger_movement());
    enter_life_state(&mut movement, 0, profile, 0);
    registry.set_component(pawn, movement).unwrap();
}

fn health(current: f32) -> HealthComponent {
    let mut health = HealthComponent::from_descriptor(&postretro_entities::HealthDescriptor {
        max: 100.0,
        hitbox: None,
        zone_multipliers: Default::default(),
    });
    health.current = current;
    health
}

fn provenance(name: &str) -> DescriptorProvenance {
    DescriptorProvenance {
        canonical_name: name.into(),
        owned_components: Default::default(),
        map_overrides: Default::default(),
        spawn_path: DescriptorSpawnPath::PlayerSpawn,
    }
}

fn pickup(registry: &mut EntityRegistry) -> EntityId {
    let item = registry.spawn(Transform::default());
    registry
        .set_component(item, weapon_component("pickup"))
        .unwrap();
    registry.set_component(item, provenance("pickup")).unwrap();
    registry
        .set_component(
            item,
            TouchableComponent {
                mode: TouchMode::Auto,
                radius: 1.0,
            },
        )
        .unwrap();
    item
}

fn volume(registry: &mut EntityRegistry, activation: TriggerActivation, on_fire: &str) -> EntityId {
    let entity = registry.spawn(Transform::default());
    let mut component = TriggerVolumeComponent::new(
        activation,
        String::new(),
        on_fire.into(),
        String::new(),
        TriggerCommand::Start,
        TriggerFireMode::Multiple,
        0.0,
        true,
    );
    component.on_exit = "exit".into();
    registry.set_component(entity, component).unwrap();
    entity
}

#[test]
fn life_state_switch_capability_gates_host_local_and_client_equip_at_zero_hp() {
    for client in [false, true] {
        for allowed in [false, true] {
            let registry = Rc::new(RefCell::new(EntityRegistry::new()));
            let (pawn, weapon) = spawn_reload_pair(&mut registry.borrow_mut(), 10, 10, 100, 5);
            {
                let mut r = registry.borrow_mut();
                let mut component = r.get_component::<WeaponComponent>(weapon).unwrap().clone();
                component.lower_ms = 0;
                component.raise_ms = 0;
                r.set_component(weapon, component.clone()).unwrap();
                let second = r.spawn(Transform::default());
                r.set_component(second, component).unwrap();
                let mut inventory = r.get_component::<Inventory>(pawn).unwrap().clone();
                inventory.wieldables[1] = Some(second);
                r.set_component(pawn, inventory).unwrap();
                r.set_component(pawn, health(0.0)).unwrap();
                stamp(
                    &mut r,
                    pawn,
                    &LifeStateProfile {
                        switch_weapon: allowed,
                        ..Default::default()
                    },
                );
                r.mark_local_player_pawn(pawn).unwrap();
            }
            let mut command = sim_command(false, false);
            command.select_slot = Some(1);
            if client {
                let (accepted, _) = simulate_client_wieldable_tick(
                    registry.clone(),
                    &CollisionWorld::new(),
                    &HitZoneStore::new(),
                    Some(pawn),
                    false,
                    Some(1),
                    command.fire_button,
                    false,
                    0.0,
                    1.0 / 60.0,
                );
                assert_eq!(accepted, allowed);
            } else {
                run_local_only_tick(registry.clone(), weapon, &command, 1.0 / 60.0);
            }
            assert_eq!(
                registry
                    .borrow()
                    .get_component::<Inventory>(pawn)
                    .unwrap()
                    .active_slot,
                usize::from(allowed)
            );
        }
    }
}

#[test]
fn life_state_reload_refusal_consumes_raw_hold_on_local_and_remote_paths() {
    for remote in [false, true] {
        let registry = Rc::new(RefCell::new(EntityRegistry::new()));
        let (pawn, weapon) = spawn_reload_pair(&mut registry.borrow_mut(), 10, 20, 100, 0);
        registry
            .borrow_mut()
            .set_component(pawn, health(0.0))
            .unwrap();
        stamp(
            &mut registry.borrow_mut(),
            pawn,
            &LifeStateProfile {
                reload: false,
                ..Default::default()
            },
        );
        if !remote {
            registry.borrow_mut().mark_local_player_pawn(pawn).unwrap();
        } else {
            let mut r = registry.borrow_mut();
            let host = r.spawn(Transform {
                position: Vec3::splat(100.0),
                ..Default::default()
            });
            r.set_component(host, trigger_movement()).unwrap();
            r.mark_local_player_pawn(host).unwrap();
        }
        let tick = |reload: bool, tick| {
            if remote {
                run_remote_only_tick(
                    registry.clone(),
                    &[remote_command(pawn, Some(weapon), 1, tick, false, reload)],
                )
            } else {
                run_local_only_tick(
                    registry.clone(),
                    weapon,
                    &sim_command(false, reload),
                    1.0 / 60.0,
                )
            }
        };
        assert!(tick(true, 1).reload_deliveries.is_empty());
        assert!(
            registry
                .borrow()
                .get_component::<WeaponComponent>(weapon)
                .unwrap()
                .reload_press_consumed
        );
        stamp(
            &mut registry.borrow_mut(),
            pawn,
            &LifeStateProfile::default(),
        );
        assert!(
            tick(true, 2).reload_deliveries.is_empty(),
            "opening reload must not synthesize a held press"
        );
        tick(false, 3);
        assert!(
            tick(true, 4)
                .reload_deliveries
                .iter()
                .any(|delivery| delivery.outcome == ReloadOutcome::Started)
        );
    }
}

#[test]
fn life_state_refused_reload_preserves_host_local_and_remote_charges() {
    for remote in [false, true] {
        let registry = Rc::new(RefCell::new(EntityRegistry::new()));
        let (pawn, weapon) = spawn_reload_pair(&mut registry.borrow_mut(), 10, 20, 100, 10);
        let descriptor: postretro_foundation::WeaponDescriptor = serde_json::from_value(json!({"damage":5,"range":20,"resolution":"hitscan","primary":{"trigger":"press","recoveryMs":10,"charge":{"minMs":10,"fullMs":2000},"steps":[{"kind":"shot"}]}})).unwrap();
        registry
            .borrow_mut()
            .set_component(weapon, WeaponComponent::from_descriptor(&descriptor))
            .unwrap();
        registry
            .borrow_mut()
            .set_component(pawn, health(0.0))
            .unwrap();
        stamp(
            &mut registry.borrow_mut(),
            pawn,
            &LifeStateProfile {
                reload: false,
                ..Default::default()
            },
        );
        if remote {
            let mut r = registry.borrow_mut();
            let host = r.spawn(Transform {
                position: Vec3::splat(100.0),
                ..Default::default()
            });
            r.set_component(host, trigger_movement()).unwrap();
            r.mark_local_player_pawn(host).unwrap();
        } else {
            registry.borrow_mut().mark_local_player_pawn(pawn).unwrap();
        }
        let tick = |tick, pressed, reload| {
            if remote {
                let mut command = remote_command(pawn, Some(weapon), 1, tick, pressed, reload);
                command.command.fire_button.active = true;
                run_remote_only_tick(registry.clone(), &[command]);
            } else {
                let mut command = super::tests::sim_command_at(tick, pressed, reload);
                command.fire_button.active = true;
                run_local_only_tick(registry.clone(), weapon, &command, 1.0 / 60.0);
            }
        };
        tick(1, true, false);
        assert!(
            registry
                .borrow()
                .get_component::<WeaponComponent>(weapon)
                .unwrap()
                .state
                .activation_cursor()
                .is_some()
        );
        tick(2, false, true);
        assert!(
            registry
                .borrow()
                .get_component::<WeaponComponent>(weapon)
                .unwrap()
                .state
                .activation_cursor()
                .is_some(),
            "a refused reload never cancels charge"
        );
        stamp(
            &mut registry.borrow_mut(),
            pawn,
            &LifeStateProfile::default(),
        );
        tick(3, false, true);
        assert!(
            registry
                .borrow()
                .get_component::<WeaponComponent>(weapon)
                .unwrap()
                .state
                .activation_cursor()
                .is_some(),
            "the refused held edge was consumed"
        );
        tick(4, false, false);
        tick(5, false, true);
        assert!(
            registry
                .borrow()
                .get_component::<WeaponComponent>(weapon)
                .unwrap()
                .state
                .activation_cursor()
                .is_none()
        );
    }
}

#[test]
fn life_state_use_touch_and_occupancy_gate_independently_without_false_exit() {
    use crate::trigger_system::{TriggerDispatchInputs, TriggerEventEdge, TriggerTickInputs};
    let mut registry = EntityRegistry::new();
    let pawn = registry.spawn(Transform::default());
    registry.set_component(pawn, health(0.0)).unwrap();
    stamp(&mut registry, pawn, &LifeStateProfile::default());
    let player = PlayerId::Local(pawn);
    let players = [AuthoritativePlayer { id: player, pawn }];
    let touch = volume(&mut registry, TriggerActivation::Touch, "touch");
    let use_volume = volume(&mut registry, TriggerActivation::Use, "use");
    let mut bridge = TriggerVolumeBridge::new();
    for trigger in [touch, use_volume] {
        bridge.insert_for_test(trigger, Vec3::splat(-4.0), Vec3::splat(4.0));
    }
    let mut system = TriggerSystem::default();
    let uses = HashMap::from([(player, true)]);
    let bound = HashSet::new();
    let mut tick = |registry: &mut EntityRegistry| {
        let capture = player_life::TriggerStageCapabilities::capture(registry, &players);
        let mut observed = Vec::new();
        system.run_authoritative_tick_with_capabilities(
            registry,
            &bridge,
            TriggerTickInputs {
                players: &players,
                use_pressed: &uses,
                tick_dt: 1.0 / 60.0,
            },
            TriggerDispatchInputs {
                alive_players: &capture.occupants,
                bound_edges: &bound,
            },
            &capture,
            |event, count, _, _| observed.push((event.fire.trigger, event.edge, count)),
        );
        observed
    };
    let permitted = tick(&mut registry);
    assert!(permitted.contains(&(touch, TriggerEventEdge::Enter, 1)));
    assert!(permitted.contains(&(use_volume, TriggerEventEdge::Enter, 1)));
    stamp(
        &mut registry,
        pawn,
        &LifeStateProfile {
            use_enabled: false,
            touch: false,
            occupancy: false,
            ..Default::default()
        },
    );
    assert!(
        tick(&mut registry).is_empty(),
        "closed gates retain existing membership without emitting Exit"
    );
    let mut transform = *registry.get_component::<Transform>(pawn).unwrap();
    transform.position.x = 20.0;
    registry.set_component(pawn, transform).unwrap();
    let leaving = tick(&mut registry);
    assert!(leaving.contains(&(touch, TriggerEventEdge::Exit, 0)));
    assert!(leaving.contains(&(use_volume, TriggerEventEdge::Exit, 0)));
    transform.position = Vec3::ZERO;
    registry.set_component(pawn, transform).unwrap();
    assert!(
        tick(&mut registry).is_empty(),
        "false touch/use blocks new entries"
    );
}

#[test]
fn life_state_o16_o35_trigger_transition_blocks_weapons_but_keeps_stage_pickup_profile() {
    for remote in [false, true] {
        let ctx = ScriptCtx::new();
        let (pawn, weapon) = spawn_reload_pair(&mut ctx.registry.borrow_mut(), 10, 20, 100, 10);
        let trigger;
        {
            let mut r = ctx.registry.borrow_mut();
            stamp(&mut r, pawn, &LifeStateProfile::default());
            r.set_component(pawn, provenance("player")).unwrap();
            r.set_component(pawn, health(0.0)).unwrap();
            if !remote {
                r.mark_local_player_pawn(pawn).unwrap();
            } else {
                let host = r.spawn(Transform {
                    position: Vec3::splat(100.0),
                    ..Default::default()
                });
                r.set_component(host, trigger_movement()).unwrap();
                r.mark_local_player_pawn(host).unwrap();
            }
            trigger = volume(&mut r, TriggerActivation::Touch, "close");
        }
        let descriptor = EntityTypeDescriptor {
            canonical_name: Some("player".into()),
            ghost: false,
            faction: None,
            tolerance: None,
            inventory: None,
            light: None,
            emitter: None,
            movement: None,
            weapon: None,
            touchable: None,
            mesh: None,
            health: None,
            behavior: None,
            life_states: Some(LifeStatesDescriptor {
                initial: "alive".into(),
                states: BTreeMap::from([
                    ("alive".into(), LifeStateProfile::default()),
                    (
                        "closed".into(),
                        LifeStateProfile {
                            movement: postretro_foundation::LifeStateMovementProfile {
                                speed_scale: 0.0,
                                ..Default::default()
                            },
                            look: false,
                            fire: false,
                            use_enabled: false,
                            touch: false,
                            occupancy: false,
                            ..Default::default()
                        },
                    ),
                ]),
                rejoin: Default::default(),
                state_ids: Default::default(),
                table_revision: 0,
            }),
        };
        {
            let mut data = ctx.data_registry.borrow_mut();
            data.replace_entity_types(vec![descriptor]);
            data.populate_level(
                vec![NamedReaction {
                    name: "close".into(),
                    descriptor: ReactionDescriptor::Primitive(PrimitiveDescriptor {
                        primitive: "setLifeState".into(),
                        target: Some("@activators".into()),
                        kind: None,
                        tag: None,
                        on_complete: None,
                        args: json!({"state":"closed"}),
                    }),
                }],
                vec![],
                &[],
            );
        }
        let first = pickup(&mut ctx.registry.borrow_mut());
        let bindings = TriggerBindingTable::build_with_script_ctx(
            &ctx.registry.borrow(),
            &ctx.data_registry.borrow(),
            &ctx,
        );
        let mut bridge = TriggerVolumeBridge::new();
        bridge.insert_for_test(trigger, Vec3::splat(-4.0), Vec3::splat(4.0));
        let mut system = TriggerSystem::default();
        let mut touch = TouchSystem::default();
        let mut progress = ProgressTracker::new();
        let mut movers = MoverTickStateTable::default();
        let world = CollisionWorld::new();
        let hit_zones = HitZoneStore::new();
        let edges = HashMap::new();
        let remote_commands = if remote {
            let mut remote = remote_command(pawn, Some(weapon), 1, 1, true, false);
            remote.command.movement.wish_dir = glam::Vec2::Y;
            vec![remote]
        } else {
            Vec::new()
        };
        let mut command = sim_command(!remote, false);
        command.movement.wish_dir = glam::Vec2::Y;
        let mut run = |touch: &mut TouchSystem, context: bool| {
            simulate_tick_with_presentation_aim(
                ctx.registry.clone(),
                &world,
                &hit_zones,
                None,
                postretro_collision::EnvironmentView::level_only(0.0),
                false,
                0.0,
                (0.0, 0.0),
                &mut progress,
                |_, _, _, _| AiTickResult {
                    events: vec![],
                    projectile_spawns: vec![],
                },
                &[],
                &mut movers,
                &remote_commands,
                &command,
                |_| PostMovementCommand {
                    aim_origin: Vec3::ZERO,
                    aim_direction: Vec3::NEG_Z,
                },
                1.0 / 60.0,
                touch,
                &mut Default::default(),
                &[],
                0,
                &FactionRegistry::default(),
                &ctx.faction_sentiment,
                None,
                &edges,
                &edges,
                context.then_some(TriggerTickContext {
                    teleporters: None,
                    system: &mut system,
                    bridge: &bridge,
                    bindings: &bindings,
                    slot_table: ctx.slot_table.clone(),
                    script_ctx: Some(ctx.clone()),
                    auto_close_timers: None,
                    use_edges: &edges,
                }),
                |_, _| {},
                |_| {},
                TimingGate::OFF,
            )
        };
        let tick = run(&mut touch, true);
        assert!(
            tick.weapon.is_empty() && tick.authorized_shots.is_empty(),
            "the later Weapons stage reads fire:false"
        );
        assert!(
            ctx.registry
                .borrow()
                .get_component::<Inventory>(pawn)
                .unwrap()
                .wieldables
                .contains(&Some(first)),
            "same-stage pickup reads the captured touch:true"
        );
        let (after_transition, closed) = {
            let registry = ctx.registry.borrow();
            (
                registry.get_component::<Transform>(pawn).unwrap().position,
                registry
                    .get_component::<PlayerMovementComponent>(pawn)
                    .unwrap()
                    .clone(),
            )
        };
        assert!(
            after_transition.length() > 0.0,
            "T's movement used the entry profile before Triggers"
        );
        assert_eq!(closed.life_speed_scale, 0.0);
        let project = |scale| {
            let mut movement = closed.clone();
            movement.life_speed_scale = scale;
            crate::movement::tick_with_immersion_and_armed(
                &mut movement,
                &command.movement,
                &world,
                postretro_collision::EnvironmentView::level_only(0.0),
                1.0 / 60.0,
                after_transition,
                |_| false,
            )
            .0
        };
        let next_closed = project(0.0);
        let next_old = project(1.0);
        assert!(
            next_closed.distance(next_old) > 1e-6,
            "the two profiles have observably different next-step movement"
        );
        let later = pickup(&mut ctx.registry.borrow_mut());
        run(&mut touch, false);
        let actual = ctx
            .registry
            .borrow()
            .get_component::<Transform>(pawn)
            .unwrap()
            .position;
        assert!(
            actual.distance(next_closed) < 1e-6,
            "T+1 uses the trigger's new speed profile, including the remote path"
        );
        assert!(
            !ctx.registry
                .borrow()
                .get_component::<Inventory>(pawn)
                .unwrap()
                .wieldables
                .contains(&Some(later)),
            "next-stage pickup reads touch:false even without trigger_context"
        );
    }
}

#[test]
fn life_state_damageable_replaces_only_spatial_health_and_preserves_ai_brain_gates() {
    use crate::scripting_systems::health::{is_damage_target_eligible, is_quiescent};
    let mut registry = EntityRegistry::new();
    let pawn = registry.spawn(Transform::default());
    for hp in [0.0, 100.0] {
        registry.set_component(pawn, health(hp)).unwrap();
        for allowed in [false, true] {
            stamp(
                &mut registry,
                pawn,
                &LifeStateProfile {
                    damageable: allowed,
                    ..Default::default()
                },
            );
            assert_eq!(is_damage_target_eligible(&registry, pawn), allowed);
            assert_eq!(player_life::ai_is_quiescent(&registry, pawn), !allowed);
            assert_eq!(
                is_quiescent(&registry, pawn),
                hp == 0.0,
                "movement/steering keep their own health semantics"
            );
        }
    }
    registry.set_component(pawn, health(0.0)).unwrap();
    let graph: postretro_entities::BehaviorGraphDescriptor = serde_json::from_value(json!({"initial":"idle", "activities":{"idle":{"motion":"hold"}}, "transitions":{}, "moveSpeed":0})).unwrap();
    registry
        .set_component(
            pawn,
            postretro_entities::components::brain::BrainComponent::from_graph(&graph),
        )
        .unwrap();
    assert!(
        player_life::ai_is_quiescent(&registry, pawn),
        "a brained actor never bypasses its depleted-HP gate"
    );
    registry
        .set_component(
            pawn,
            postretro_entities::DeferredEffectComponent {
                inert: true,
                ..Default::default()
            },
        )
        .unwrap();
    assert!(!is_damage_target_eligible(&registry, pawn));
    assert!(player_life::ai_is_quiescent(&registry, pawn));
}

#[test]
fn life_state_undeclared_zero_hp_keeps_switch_reload_use_touch_and_spatial_rules() {
    let mut registry = EntityRegistry::new();
    let pawn = registry.spawn(Transform::default());
    registry.set_component(pawn, trigger_movement()).unwrap();
    registry.set_component(pawn, health(0.0)).unwrap();
    assert!(player_life::permits(
        &registry,
        pawn,
        postretro_foundation::Capability::SwitchWeapon
    ));
    assert!(player_life::permits(
        &registry,
        pawn,
        postretro_foundation::Capability::Reload
    ));
    let player = PlayerId::Local(pawn);
    let capture = player_life::TriggerStageCapabilities::capture(
        &registry,
        &[AuthoritativePlayer { id: player, pawn }],
    );
    assert!(capture.permits_use(player) && capture.permits_touch(player));
    assert!(!capture.occupants.contains(&player));
    assert!(!crate::scripting_systems::health::is_damage_target_eligible(&registry, pawn));
}
