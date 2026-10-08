// Binary orchestration exercised without a window or GPU.
// See: context/lib/networking.md §Combat authority · context/lib/testing_guide.md
use super::ClientWeaponFrame;
use crate::{sim, weapon};
use glam::Vec3;
use postretro_entities::components::{
    health::HealthComponent, inventory::Inventory, weapon::WeaponComponent,
};
use postretro_entities::{EntityId, EntityRegistry, Transform};
use postretro_foundation::{
    ActivationLane, ActivationToken, HealthDescriptor, PlayerMovementComponent, WeaponDescriptor,
    WeaponPlacementDescriptor,
};

fn fixture(primary: serde_json::Value, projectile: bool) -> (EntityRegistry, EntityId, EntityId) {
    fixture_with_resource(primary, projectile, None)
}
fn fixture_with_resource(
    primary: serde_json::Value,
    projectile: bool,
    resource: Option<serde_json::Value>,
) -> (EntityRegistry, EntityId, EntityId) {
    let mut raw = serde_json::json!({"damage":5.0,"range":20.0,"resolution":"hitscan","primary":primary,"bloomPerShotDegrees":2.0,"bloomMaxDegrees":20.0});
    if projectile {
        raw["resolution"] = serde_json::json!("projectile");
        raw["projectile"] = serde_json::json!({"speed":4.0,"radius":0.1,"lifetimeMs":60000.0,"visual":{"body":{"kind":"sprite","sprite":"sprites/projectiles/plasma.png","size":0.2}}});
    }
    if let Some(resource) = resource {
        raw["resource"] = resource;
    }
    let descriptor: WeaponDescriptor = serde_json::from_value(raw).unwrap();
    let mut registry = EntityRegistry::new();
    let pawn = registry.spawn(Transform::default());
    registry
        .set_component(
            pawn,
            PlayerMovementComponent::from_descriptor(&crate::tests::minimal_player_descriptor()),
        )
        .unwrap();
    registry
        .set_component(
            pawn,
            HealthComponent::from_descriptor(&HealthDescriptor {
                max: 100.0,
                hitbox: None,
                zone_multipliers: Default::default(),
            }),
        )
        .unwrap();
    let weapon = registry.spawn(Transform::default());
    registry
        .set_component(weapon, WeaponComponent::from_descriptor(&descriptor))
        .unwrap();
    let mut inventory = Inventory::default();
    inventory.wieldables[0] = Some(weapon);
    registry.set_component(pawn, inventory).unwrap();
    (registry, pawn, weapon)
}
fn command(pressed: bool, active: bool) -> sim::SimCommand {
    let mut command = crate::build_sim_command(
        &crate::input::ActionSnapshot::neutral(),
        &crate::camera::Camera::new(Vec3::ZERO, 0.0, 0.0),
        false,
        false,
        false,
        false,
        false,
        false,
        false,
    );
    command.fire_button = weapon::FireButtonState { pressed, active };
    command
}
fn predict(
    frame: &mut ClientWeaponFrame,
    registry: &mut EntityRegistry,
    command: &mut sim::SimCommand,
    tick: u32,
) {
    let shared = std::rc::Rc::new(std::cell::RefCell::new(std::mem::replace(
        registry,
        EntityRegistry::new(),
    )));
    let pawn = shared.borrow().local_player_movement_pawn();
    sim::simulate_client_wieldable_tick(
        shared.clone(),
        &crate::collision::CollisionWorld::new(),
        &crate::scripting_systems::hit_zones::HitZoneStore::new(),
        pawn,
        false,
        command.select_slot,
        command.fire_button,
        command.reload,
        0.0,
        1.0 / 60.0,
    );
    *registry = std::rc::Rc::try_unwrap(shared)
        .unwrap_or_else(|_| panic!("equip released registry handle"))
        .into_inner();
    frame.predict(
        registry,
        command,
        tick,
        77,
        1000.0 / 60.0,
        WeaponPlacementDescriptor::default(),
        &Default::default(),
    );
}

fn presented_catchup_app(projectile: bool) -> crate::App {
    let action = serde_json::json!({"trigger":"press","recoveryMs":50.0,"steps":[{"kind":"shot"},{"kind":"wait","durationMs":16.0},{"kind":"shot"},{"kind":"wait","durationMs":16.0},{"kind":"shot"}]});
    let (mut registry, _, weapon_id) = fixture(action, projectile);
    if let Ok(postretro_entities::ComponentValue::Weapon(component)) =
        registry.get_component_value_mut(weapon_id, postretro_entities::ComponentKind::Weapon)
    {
        // This fixture isolates the camera choice, rather than bloom dispersion.
        component.bloom_per_shot_degrees = 0.0;
        component.bloom_max_degrees = 0.0;
    }
    let mut frame = ClientWeaponFrame::default();
    predict(&mut frame, &mut registry, &mut command(true, true), 10);
    for tick in 11..15 {
        predict(&mut frame, &mut registry, &mut command(false, false), tick);
    }
    assert_eq!(frame.due.len(), 3);
    let mut app = crate::startup::lifecycle::tests::test_app();
    let session = app.session.as_mut().unwrap();
    *session.scripting.script_ctx.registry.borrow_mut() = registry;
    // No polling or handshake is needed: this exercises the connected App lane.
    session.net_endpoint = crate::netcode::NetEndpoint::from_role(
        &crate::netcode::NetRole::Connect {
            addr: std::net::SocketAddr::from((std::net::Ipv4Addr::LOCALHOST, 1)),
        },
        None,
    )
    .unwrap();
    app.client_weapon = frame;
    app
}

fn half_tick_presented_eye(app: &mut crate::App, previous: Vec3, current: Vec3) -> f32 {
    use crate::frame_timing::{FrameTiming, InterpolableState};
    use std::time::Duration;

    app.camera.position = current;
    app.frame_timing = FrameTiming::new(InterpolableState::new(previous));
    app.frame_timing.tick_duration = Duration::from_millis(16);
    app.frame_timing.push_state(InterpolableState::new(current));
    let frame = app.frame_timing.accumulate(Duration::from_millis(8));
    assert_eq!(frame.ticks, 0);
    frame.alpha
}

fn assert_catchup_uses_presented_aim(app: &mut crate::App, alpha: f32, projectile: bool) {
    let aim = app.presented_aim_pose(alpha);
    // Use the same pose adapter and eye assembly as the actual render stage.
    // No copied interpolation/yaw formula supplies the expected ray.
    let rendered = crate::frame_eye::assemble_frame_eye(
        aim.frame_eye_inputs(app.camera.aspect(), None, &[], 0.0, 1.0),
        crate::frame_eye::ViewFeelTracking {
            state: &mut app.view_feel_state,
            followed_pawn: &mut app.view_feel_followed_pawn,
            descriptor: &mut app.view_feel_descriptor,
        },
    )
    .camera;
    let (tick_eye, tick_direction) = app.camera.aim_ray();
    assert!(
        tick_eye.distance(rendered.eye_position) > 0.05
            || tick_direction.distance(rendered.forward) > 0.05,
        "the settled camera must differ enough to expose the old selection"
    );
    let ctx = app.session.as_ref().unwrap().scripting.script_ctx.clone();
    let target = {
        let mut registry = ctx.registry.borrow_mut();
        let target = registry.spawn(Transform {
            position: rendered.eye_position + rendered.forward * 5.0,
            ..Default::default()
        });
        registry
            .set_component(
                target,
                HealthComponent::from_descriptor(
                    &serde_json::from_value(serde_json::json!({
                        "max":100.0,"hitbox":{"halfExtents":[0.025,0.025,0.025]}
                    }))
                    .unwrap(),
                ),
            )
            .unwrap();
        target
    };
    let mut emissions = Vec::new();
    // Exercise the actual App drain, correction, resolution and launch seam.
    app.run_client_fire_path_post_loop(0.0, 0.0, aim, &mut emissions);
    assert!(app.client_weapon.due.is_empty());
    assert_eq!(app.client_fire_resolutions.len(), 3);
    for resolution in &app.client_fire_resolutions {
        if projectile {
            let launch = resolution.projectile_launch.as_ref().unwrap();
            assert!(launch.origin.distance(rendered.eye_position) < 1.0e-5);
            assert!(launch.direction.distance(rendered.forward) < 1.0e-5);
        } else {
            assert_eq!(resolution.hits.len(), 1);
            assert_eq!(resolution.hits[0].target, target);
        }
    }
    app.run_client_fire_path_post_loop(0.0, 0.0, aim, &mut emissions);
    assert!(
        app.client_fire_resolutions.is_empty(),
        "due shots drain once"
    );
}

// Regression: catch-up shots started at the latest tick eye rather than the interpolated view.
#[test]
fn client_weapon_catchup_uses_fractional_presented_eye_in_app_resolver() {
    for projectile in [false, true] {
        let mut app = presented_catchup_app(projectile);
        let alpha =
            half_tick_presented_eye(&mut app, Vec3::new(0.0, 1.7, 0.0), Vec3::new(0.2, 1.7, 0.0));
        assert_catchup_uses_presented_aim(&mut app, alpha, projectile);
    }
}

// Regression: shot direction omitted the rotating mover's displayed fractional yaw carry.
#[test]
fn client_weapon_catchup_uses_presented_mover_yaw_in_app_resolver() {
    for projectile in [false, true] {
        let mut app = presented_catchup_app(projectile);
        let eye = Vec3::new(0.0, 1.7, 0.0);
        let alpha = half_tick_presented_eye(&mut app, eye, eye);
        app.camera.yaw = 0.3;
        app.mover_yaw_carry_ground = postretro_foundation::GroundRef::Mover(7);
        app.kinematic_mover_tick_states.publish(
            7,
            crate::kinematic_mover::MoverTickState {
                entity: EntityId::from_raw(0),
                transform: Transform::default(),
                linear_velocity: Vec3::ZERO,
                tick_delta: Vec3::ZERO,
                angular_velocity: Vec3::Y,
                tick_rotation_delta: glam::Quat::from_rotation_y(0.4),
                carry_yaw: true,
                tick_dt: 1.0 / 60.0,
            },
        );
        assert_catchup_uses_presented_aim(&mut app, alpha, projectile);
    }
}

#[test]
fn client_weapon_catchup_resolves_every_due_ordinal_at_rendered_pose() {
    let action = serde_json::json!({"trigger":"press","recoveryMs":50.0,"steps":[{"kind":"shot"},{"kind":"wait","durationMs":16.0},{"kind":"shot"},{"kind":"wait","durationMs":16.0},{"kind":"shot"}]});
    for projectile in [false, true] {
        let (mut registry, pawn, weapon_id) = fixture(action.clone(), projectile);
        let mut frame = ClientWeaponFrame::default();
        let mut first = command(true, true);
        predict(&mut frame, &mut registry, &mut first, 10);
        for tick in 11..15 {
            predict(&mut frame, &mut registry, &mut command(false, false), tick);
        }
        assert_eq!(frame.due.len(), 3);
        assert_eq!(
            frame
                .due
                .iter()
                .map(|q| q.shot.activation.shot_id.ordinal)
                .collect::<Vec<_>>(),
            vec![0, 1, 2]
        );
        assert_eq!(
            registry
                .get_component::<WeaponComponent>(weapon_id)
                .unwrap()
                .shells_fired,
            3
        );
        if !projectile {
            assert_eq!(
                frame
                    .due
                    .iter()
                    .map(|q| q.shot.activation.bloom_degrees)
                    .collect::<Vec<_>>(),
                vec![0.0, 2.0, 4.0]
            );
        }
        let target = registry.spawn(Transform {
            position: Vec3::new(2.0, 0.0, -5.0),
            ..Default::default()
        });
        registry
            .set_component(
                target,
                HealthComponent::from_descriptor(
                    &serde_json::from_value(
                        serde_json::json!({"max":100.0,"hitbox":{"halfExtents":[0.5,0.5,0.5]}}),
                    )
                    .unwrap(),
                ),
            )
            .unwrap();
        for queued in frame.due {
            let resolution = weapon::resolve_client_shot(
                Some(pawn),
                queued.component,
                &queued.pellet_salt,
                0,
                Vec3::new(2.0, 0.0, 0.0),
                Vec3::NEG_Z,
                &queued.placement,
                &crate::collision::CollisionWorld::new(),
                &registry,
                &crate::scripting_systems::hit_zones::HitZoneStore::new(),
                0.0,
                queued.shot,
            );
            if projectile {
                assert!(resolution.projectile_launch.is_some());
            } else {
                assert_eq!(resolution.hits[0].target, target);
            }
        }
        assert_eq!(
            registry
                .get_component::<WeaponComponent>(weapon_id)
                .unwrap()
                .shells_fired,
            3,
            "spatial resolution cannot reserve again"
        );
    }
}
#[test]
fn client_weapon_hold_restarts_are_attached_to_real_commands() {
    let (mut registry, _, _) = fixture(
        serde_json::json!({"trigger":"hold","recoveryMs":16.0,"steps":[{"kind":"shot"}]}),
        false,
    );
    let mut frame = ClientWeaponFrame::default();
    let mut requests = Vec::new();
    for tick in 0..5 {
        let mut command = command(tick == 0, true);
        predict(&mut frame, &mut registry, &mut command, tick);
        if let Some(token) = command.activation.initiation {
            assert_eq!(token.start_tick, tick);
            requests.push(token);
        }
    }
    assert!(requests.len() >= 2);
    assert_eq!(
        requests.len(),
        frame.due.len(),
        "every held restart has a transmitted initiation"
    );
}
#[test]
fn client_weapon_zero_tick_suspend_refocus_then_fixed_cancel_produces_no_shot() {
    let (mut registry, _, id) = fixture(
        serde_json::json!({"trigger":"press","recoveryMs":16.0,"charge":{"minMs":0.0,"fullMs":100.0},"steps":[{"kind":"shot"}]}),
        false,
    );
    let mut frame = ClientWeaponFrame::default();
    let mut capture = crate::input::ActivationInputCapture::default();
    let edge = crate::input::ActionSnapshot::with_button_state(
        crate::input::Action::Shoot,
        crate::input::ButtonState::Pressed,
    );
    capture.observe(&edge);
    let mut start = command(true, true);
    start.activation = capture.command(8);
    predict(&mut frame, &mut registry, &mut start, 8);
    let token = start.activation.initiation.unwrap();
    capture.set_active(Some(token));
    let before = *registry
        .get_component::<WeaponComponent>(id)
        .unwrap()
        .state
        .activation_cursor()
        .as_ref()
        .unwrap();
    frame.suspend(&registry);
    capture.suspend();
    assert_eq!(capture.take_cancel(), Some(token));
    assert_eq!(
        *registry
            .get_component::<WeaponComponent>(id)
            .unwrap()
            .state
            .activation_cursor()
            .as_ref()
            .unwrap(),
        before,
        "no semantic advance on zero-tick frames"
    );
    assert_eq!(capture.take_cancel(), None); // refocus still has no fixed tick
    let mut next = command(false, false);
    next.activation = capture.command(9);
    predict(&mut frame, &mut registry, &mut next, 9);
    assert_eq!(next.activation.cancel, Some(token));
    assert!(frame.due.is_empty());
    assert!(frame.suppressed.is_none());
    assert!(
        registry
            .get_component::<WeaponComponent>(id)
            .unwrap()
            .state
            .activation_cursor()
            .is_none()
    );
}
#[test]
fn client_weapon_outcomes_require_token_host_identity_and_captured_instance() {
    use postretro_net::wire::{ActivationOutcome as O, NetworkId, WireActivationToken};
    let (mut registry, pawn, weapon) = fixture(
        serde_json::json!({"trigger":"press","recoveryMs":50.0,"steps":[{"kind":"shot"}]}),
        false,
    );
    let token = ActivationToken {
        start_tick: 10,
        lane: ActivationLane::Primary,
    };
    let wire = WireActivationToken {
        start_tick: 10,
        lane: 0,
    };
    let mut frame = ClientWeaponFrame::default();
    frame.records.request(token, weapon);
    let accepted = O::ExecutionAccepted {
        token: wire,
        weapon: NetworkId(991),
        charge_millionths: 500000,
        recovery_ticks: 4,
    };
    assert!(
        frame.records.outcome(&mut registry, accepted).is_none(),
        "execution cannot precede bound acceptance"
    );
    frame.records.outcome(
        &mut registry,
        O::InitiationAccepted {
            token: wire,
            weapon: NetworkId(991),
        },
    );
    assert!(
        frame
            .records
            .outcome(
                &mut registry,
                O::ExecutionAccepted {
                    token: wire,
                    weapon: NetworkId(992),
                    charge_millionths: 500000,
                    recovery_ticks: 4
                }
            )
            .is_none()
    );
    assert_eq!(
        frame
            .records
            .outcome(&mut registry, accepted)
            .unwrap()
            .weapon,
        weapon
    );
    let replacement = registry.spawn(Transform::default());
    let (replacement_registry, _, source_weapon) = fixture(
        serde_json::json!({"trigger":"press","recoveryMs":900.0,"steps":[
            {"kind":"shot"},{"kind":"wait","durationMs":100},{"kind":"shot"}
        ]}),
        false,
    );
    registry
        .set_component(
            replacement,
            replacement_registry
                .get_component::<WeaponComponent>(source_weapon)
                .unwrap()
                .clone(),
        )
        .unwrap();
    let mut inventory = registry.get_component::<Inventory>(pawn).unwrap().clone();
    inventory.wieldables[1] = Some(replacement);
    inventory.active_slot = 1;
    registry.set_component(pawn, inventory).unwrap();
    let mut replacement_input = command(true, true);
    predict(&mut frame, &mut registry, &mut replacement_input, 30);
    if let postretro_entities::ComponentValue::Weapon(component) = registry
        .get_component_value_mut(replacement, postretro_entities::ComponentKind::Weapon)
        .unwrap()
    {
        component.cooldown_remaining_ms = 876.0;
    }
    let before = registry
        .get_component::<WeaponComponent>(replacement)
        .unwrap()
        .clone();
    assert_eq!(
        registry
            .get_component::<Inventory>(pawn)
            .unwrap()
            .active_wieldable(),
        Some(replacement)
    );
    assert_eq!(
        before.state.activation_cursor().unwrap().token.start_tick,
        30
    );
    assert_ne!(weapon, replacement);
    let old_cancel = O::Cancelled {
        token: wire,
        weapon: NetworkId(991),
        recovery_ticks: 3,
    };
    let effect = frame
        .records
        .outcome(&mut registry, old_cancel)
        .expect("a live captured instance still receives its correctly bound outcome");
    assert_eq!(effect.weapon, weapon);
    assert!(
        (registry
            .get_component::<WeaponComponent>(weapon)
            .unwrap()
            .cooldown_remaining_ms
            - 50.0)
            .abs()
            < 0.001
    );
    assert_eq!(
        registry
            .get_component::<WeaponComponent>(replacement)
            .unwrap(),
        &before,
        "the active B instance cannot receive A's cancellation or recovery"
    );

    registry.despawn(weapon).unwrap();
    let mut inventory = registry.get_component::<Inventory>(pawn).unwrap().clone();
    inventory.wieldables[0] = Some(replacement);
    inventory.wieldables[1] = None;
    inventory.active_slot = 0;
    registry.set_component(pawn, inventory).unwrap();
    assert_eq!(
        registry
            .get_component::<Inventory>(pawn)
            .unwrap()
            .active_wieldable(),
        Some(replacement)
    );
    assert!(
        frame
            .records
            .outcome(
                &mut registry,
                O::Cancelled {
                    token: wire,
                    weapon: NetworkId(991),
                    recovery_ticks: 3
                }
            )
            .is_none(),
        "current/reused slot cannot redirect an old correction"
    );
    assert_eq!(
        registry
            .get_component::<WeaponComponent>(replacement)
            .unwrap(),
        &before,
        "reused active slot B keeps its live cursor and recovery after old A feedback"
    );
}

#[test]
fn client_weapon_hidden_stale_resource_flights_still_contact_after_history_eviction() {
    use weapon::{ClientPullPresentation, SlotSample};
    let resources = [
        (
            serde_json::json!({"kind":"ammo","type":"bullets.test","magazine":10,"reserve":10,"reloadMs":100}),
            weapon::ReplicatedWeaponProjection {
                magazine: Some(SlotSample {
                    slot: 0,
                    value: Some(0.0),
                }),
                reload_active: Some(SlotSample {
                    slot: 0,
                    value: false,
                }),
                ..Default::default()
            },
            ClientPullPresentation::DryFire,
        ),
        (
            serde_json::json!({"kind":"cell","capacity":100.0,"costPerShot":5.0,"regenPerSecond":10.0,"regenDelayMs":100}),
            weapon::ReplicatedWeaponProjection {
                cell: Some(SlotSample {
                    slot: 0,
                    value: Some(0.0),
                }),
                ..Default::default()
            },
            ClientPullPresentation::DryFire,
        ),
        (
            serde_json::json!({"kind":"heat","heatPerShot":5.0,"overheatAt":100.0,"coolPerSecond":10.0}),
            weapon::ReplicatedWeaponProjection {
                overheated: Some(SlotSample {
                    slot: 0,
                    value: true,
                }),
                ..Default::default()
            },
            ClientPullPresentation::Silent,
        ),
    ];
    for (resource, projection, expected) in resources {
        for visible in [false, true] {
            let (mut registry, pawn, weapon_id) = fixture_with_resource(
                serde_json::json!({"trigger":"press","recoveryMs":50.0,"steps":[{"kind":"shot"}]}),
                true,
                Some(resource.clone()),
            );
            let target = registry.spawn(Transform {
                position: Vec3::new(0.0, 0.0, -2.0),
                ..Default::default()
            });
            registry
                .set_component(
                    target,
                    HealthComponent::from_descriptor(
                        &serde_json::from_value(
                            serde_json::json!({"max":100.0,"hitbox":{"halfExtents":[0.2,0.2,0.2]}}),
                        )
                        .unwrap(),
                    ),
                )
                .unwrap();
            let mut frame = ClientWeaponFrame::default();
            frame.predict(
                &mut registry,
                &mut command(true, true),
                9,
                77,
                1000.0 / 60.0,
                WeaponPlacementDescriptor::default(),
                &projection,
            );
            let queued = frame.due.pop().unwrap();
            assert_eq!(queued.presentation, expected);
            let shot_id = queued.shot.activation.shot_id;
            let result = weapon::resolve_client_shot(
                Some(pawn),
                queued.component,
                &queued.pellet_salt,
                0,
                Vec3::ZERO,
                Vec3::NEG_Z,
                &queued.placement,
                &crate::collision::CollisionWorld::new(),
                &registry,
                &crate::scripting_systems::hit_zones::HitZoneStore::new(),
                0.0,
                queued.shot,
            );
            let launch = result.projectile_launch.unwrap();
            assert!(weapon::client_pull_effects(expected, true, false).spawn_projectile);
            let projectile = sim::spawn_projectile(
                &mut registry,
                pawn,
                weapon_id,
                launch,
                Some(shot_id),
                sim::ProjectileSource::default(),
            )
            .unwrap();
            sim::set_predicted_projectile_visible(&mut registry, projectile, visible);
            // Bounded reconciliation history can evict this activation while its flight lives.
            for tick in 10..80 {
                let token = ActivationToken {
                    start_tick: tick,
                    lane: ActivationLane::Primary,
                };
                frame.records.request(token, weapon_id);
                frame.records.local_terminal(token);
            }
            let registry = std::rc::Rc::new(std::cell::RefCell::new(registry));
            let mut resolutions = Vec::new();
            for _ in 0..3 {
                sim::advance_predicted(
                    &registry,
                    &crate::collision::CollisionWorld::new(),
                    &crate::scripting_systems::hit_zones::HitZoneStore::new(),
                    0.0,
                    0.25,
                    &mut |event| resolutions.push(event),
                );
            }
            assert!(
                matches!(&resolutions[..],[sim::PredictedProjectileResolution::Impact {shot_id:id,impact,visible:permission,..}] if *id==shot_id && impact.target==Some(target) && *permission==visible),
                "semantic contact and original visibility survive eviction"
            );
            assert!(!registry.borrow().exists(projectile));
            assert_eq!(
                registry
                    .borrow()
                    .get_component::<HealthComponent>(target)
                    .unwrap()
                    .current,
                100.0,
                "client declaration does not apply authoritative damage"
            );
        }
    }
}

#[test]
fn client_weapon_charge_correction_uses_frozen_zero_scale_and_preserves_live_flight() {
    use postretro_entities::components::{
        projectile::ProjectileComponent, sprite_visual::SpriteVisual,
    };
    use postretro_foundation::{
        ActivationRelease, ActivationStepDescriptor, IrNode, IrValue, NumberOrIr,
    };
    use postretro_net::wire::{ActivationOutcome as O, NetworkId, WireActivationToken};
    fn scale(multiplier: f32, plus_one: bool) -> NumberOrIr {
        let times = IrNode::Mul {
            a: Box::new(IrNode::Input {
                name: "charge".into(),
                owner: None,
            }),
            b: Box::new(IrNode::Const {
                value: IrValue::Number(multiplier),
            }),
        };
        NumberOrIr::Ir(if plus_one {
            IrNode::Add {
                a: Box::new(times),
                b: Box::new(IrNode::Const {
                    value: IrValue::Number(1.0),
                }),
            }
        } else {
            times
        })
    }
    let (mut registry, pawn, weapon_id) = fixture(
        serde_json::json!({"trigger":"press","recoveryMs":50.0,"charge":{"minMs":0,"fullMs":32},"steps":[{"kind":"shot"}]}),
        true,
    );
    let mut component = registry
        .get_component::<WeaponComponent>(weapon_id)
        .unwrap()
        .clone();
    component.sounds = Some(std::sync::Arc::new(postretro_foundation::WeaponSounds {
        impact: Some("sfx/original_hit".into()),
        ..Default::default()
    }));
    let ActivationStepDescriptor::Shot { scale: axes } =
        &mut std::sync::Arc::make_mut(&mut component.primary).steps[0]
    else {
        unreachable!()
    };
    axes.damage = scale(6.0, false);
    axes.range = scale(3.0, true);
    axes.projectile_speed = scale(3.0, true);
    axes.projectile_radius = scale(2.0, true);
    axes.projectile_size = scale(5.0, true);
    component.activation_programs =
        postretro_foundation::WeaponActivationPrograms::install(&component.primary, None);
    registry.set_component(weapon_id, component).unwrap();
    let token = ActivationToken {
        start_tick: 5,
        lane: ActivationLane::Primary,
    };
    let mut start = command(true, true);
    start.activation = postretro_foundation::ActivationInput {
        initiation: Some(token),
        release: Some(ActivationRelease {
            token,
            release_tick: 5,
        }),
        cancel: None,
    };
    let mut frame = ClientWeaponFrame::default();
    predict(&mut frame, &mut registry, &mut start, 5);
    let queued = frame.due.pop().unwrap();
    assert_eq!(queued.shot.activation.values.damage, 0.0);
    let frozen = queued.shot.clone();
    let launch = weapon::resolve_client_shot(
        Some(pawn),
        queued.component,
        &queued.pellet_salt,
        0,
        Vec3::ZERO,
        Vec3::NEG_Z,
        &queued.placement,
        &crate::collision::CollisionWorld::new(),
        &registry,
        &crate::scripting_systems::hit_zones::HitZoneStore::new(),
        0.0,
        queued.shot,
    )
    .projectile_launch
    .unwrap();
    let projectile = sim::spawn_projectile(
        &mut registry,
        pawn,
        weapon_id,
        launch,
        Some(frozen.activation.shot_id),
        sim::ProjectileSource::default(),
    )
    .unwrap();
    let mut flight = registry
        .get_component::<ProjectileComponent>(projectile)
        .unwrap()
        .clone();
    flight.remaining_range -= 3.0;
    registry.set_component(projectile, flight).unwrap();
    let mut pose = *registry.get_component::<Transform>(projectile).unwrap();
    pose.position = Vec3::new(0.0, 0.0, -3.0);
    registry.set_component(projectile, pose).unwrap();
    let replacement:WeaponDescriptor=serde_json::from_value(serde_json::json!({"damage":99.0,"range":99.0,"resolution":"hitscan","primary":{"trigger":"press","recoveryMs":500.0,"steps":[{"kind":"shot"}]},"sounds":{"impact":"sfx/replacement_hit"}})).unwrap();
    let mut current = registry
        .get_component::<WeaponComponent>(weapon_id)
        .unwrap()
        .clone();
    current.refresh_from_descriptor(&replacement);
    registry.set_component(weapon_id, current).unwrap();
    let wire = WireActivationToken {
        start_tick: 5,
        lane: 0,
    };
    frame.records.outcome(
        &mut registry,
        O::InitiationAccepted {
            token: wire,
            weapon: NetworkId(999),
        },
    );
    let accepted = O::ExecutionAccepted {
        token: wire,
        weapon: NetworkId(999),
        charge_millionths: 500000,
        recovery_ticks: 3,
    };
    assert!(frame.records.outcome(&mut registry, accepted).is_some());
    let flight = registry
        .get_component::<ProjectileComponent>(projectile)
        .unwrap();
    assert_eq!(flight.damage, 15.0);
    assert_eq!(flight.speed, 10.0);
    assert_eq!(flight.radius, 0.2);
    assert_eq!(flight.remaining_range, 47.0);
    assert_eq!(
        flight.source_sounds.as_ref().unwrap().impact.as_deref(),
        Some("sfx/original_hit")
    );
    assert!(
        (registry
            .get_component::<SpriteVisual>(projectile)
            .unwrap()
            .size
            - 0.7)
            .abs()
            < 1e-6
    );
    assert_eq!(
        *registry.get_component::<Transform>(projectile).unwrap(),
        pose
    );
    assert_eq!(
        registry
            .get_component::<WeaponComponent>(weapon_id)
            .unwrap()
            .damage,
        99.0
    );
    registry.despawn(projectile).unwrap();
    let count = registry
        .iter_with_kind(postretro_entities::ComponentKind::Projectile)
        .count();
    frame.records.outcome(&mut registry, accepted);
    assert_eq!(
        registry
            .iter_with_kind(postretro_entities::ComponentKind::Projectile)
            .count(),
        count,
        "correction cannot resurrect a resolved projectile"
    );
}

#[test]
fn client_weapon_equip_pass_preserves_reload_latch_and_newer_recovery_after_old_rejection() {
    use postretro_net::wire::{ActivationOutcome as O, WireActivationToken};
    let (mut registry, _, id) = fixture(
        serde_json::json!({"trigger":"hold","recoveryMs":50.0,"steps":[{"kind":"shot"}]}),
        false,
    );
    let mut frame = ClientWeaponFrame::default();
    let mut predicted = weapon::ClientPredictedShots::new();
    let mut first = command(true, true);
    predict(&mut frame, &mut registry, &mut first, 0);
    let a = frame.due.pop().unwrap();
    let resolution = weapon::ClientFireResolution {
        client_tick: 0,
        hits: vec![],
        world_contacts: vec![],
        projectile_launch: None,
    };
    predicted.predict(
        a.shot.activation.shot_id,
        id,
        &resolution,
        a.cooldown_before,
        a.cooldown_after,
        a.presentation,
    );
    for tick in 1..8 {
        let mut next = command(false, true);
        predict(&mut frame, &mut registry, &mut next, tick);
        if !frame.due.is_empty() {
            break;
        }
    }
    let b = frame
        .due
        .pop()
        .expect("later held restart advances after equip pass");
    assert_ne!(
        a.shot.activation.shot_id.start_tick,
        b.shot.activation.shot_id.start_tick
    );
    predicted.predict(
        b.shot.activation.shot_id,
        id,
        &resolution,
        b.cooldown_before,
        b.cooldown_after,
        b.presentation,
    );
    let before = registry
        .get_component::<WeaponComponent>(id)
        .unwrap()
        .cooldown_remaining_ms;
    let mut stale = postretro_entities::SlotTable::new();
    stale.get_mut("player.weaponCooldownMs").unwrap().value =
        Some(postretro_entities::SlotValue::Number(0.0));
    assert!(!crate::reconcile_client_weapon_cooldown_from_slot_table(
        &mut predicted,
        &mut registry,
        &stale,
        Some(0)
    ));
    assert_eq!(
        registry
            .get_component::<WeaponComponent>(id)
            .unwrap()
            .cooldown_remaining_ms,
        before,
        "slot-only snapshot cannot rewrite predicted B recovery"
    );
    let effect = frame
        .records
        .outcome(
            &mut registry,
            O::InitiationRejected {
                token: WireActivationToken {
                    start_tick: a.shot.activation.shot_id.start_tick,
                    lane: 0,
                },
                recovery_ticks: 0,
            },
        )
        .unwrap();
    assert!(effect.recovery_ms.is_none());
    predicted.apply_verdict(&mut registry, a.shot.activation.shot_id, false, false);
    assert_eq!(
        registry
            .get_component::<WeaponComponent>(id)
            .unwrap()
            .cooldown_remaining_ms,
        before,
        "old shot rejection cannot rewind B recovery"
    );
    let mut reload = command(false, false);
    reload.reload = true;
    predict(&mut frame, &mut registry, &mut reload, 20);
    assert!(
        registry
            .get_component::<WeaponComponent>(id)
            .unwrap()
            .reload_press_consumed
    );
    let mut held = command(false, false);
    held.reload = true;
    predict(&mut frame, &mut registry, &mut held, 21);
    assert!(
        registry
            .get_component::<WeaponComponent>(id)
            .unwrap()
            .reload_press_consumed,
        "neutral equip cannot reset held reload"
    );
    assert_eq!(
        registry
            .get_component::<WeaponComponent>(id)
            .unwrap()
            .last_activation_tick,
        Some(21)
    );
}

#[test]
fn client_weapon_equip_then_prediction_preserves_real_charge_release() {
    let (mut registry, _, id) = fixture(
        serde_json::json!({"trigger":"press","recoveryMs":50.0,"charge":{"minMs":16,"fullMs":48},"steps":[{"kind":"shot"}]}),
        false,
    );
    let mut frame = ClientWeaponFrame::default();
    let mut first = command(true, true);
    predict(&mut frame, &mut registry, &mut first, 10);
    let token = first.activation.initiation.unwrap();
    for tick in 11..13 {
        predict(&mut frame, &mut registry, &mut command(false, true), tick);
        assert!(frame.due.is_empty());
    }
    let mut release = command(false, false);
    release.activation.release = Some(postretro_foundation::ActivationRelease {
        token,
        release_tick: 13,
    });
    predict(&mut frame, &mut registry, &mut release, 13);
    assert_eq!(frame.due.len(), 1);
    assert_eq!(frame.due[0].shot.activation.charge, 1.0);
    assert_eq!(
        registry
            .get_component::<WeaponComponent>(id)
            .unwrap()
            .last_activation_tick,
        Some(13)
    );
}

#[test]
fn client_weapon_owner_death_projection_clears_charge_hud_without_a_tick() {
    let (mut registry, pawn, id) = fixture(
        serde_json::json!({"trigger":"press","recoveryMs":50,"charge":{"minMs":0,"fullMs":100},"steps":[{"kind":"shot"}]}),
        false,
    );
    let mut frame = ClientWeaponFrame::default();
    let mut first = command(true, true);
    predict(&mut frame, &mut registry, &mut first, 10);
    let token = first.activation.initiation.unwrap();
    let mut capture = crate::input::ActivationInputCapture::default();
    capture.set_active(Some(token));
    let ctx = postretro_entities::ctx::ScriptCtx::new();
    *ctx.registry.borrow_mut() = registry;
    let mut publisher =
        crate::scripting_systems::ui_proxy::PlayerHudStatePublisher::new(ctx.clone());
    publisher.tick_for_role_and_report_sampled_weapon(true, None);
    assert_eq!(
        ctx.slot_table
            .borrow()
            .get("player.weaponCharging")
            .unwrap()
            .value,
        Some(postretro_entities::SlotValue::Boolean(true))
    );
    ctx.slot_table
        .borrow_mut()
        .get_mut("player.health")
        .unwrap()
        .value = Some(postretro_entities::SlotValue::Number(0.0));
    let clock = ctx
        .registry
        .borrow()
        .get_component::<WeaponComponent>(id)
        .unwrap()
        .activation_clock;
    assert!(!frame.apply_owner_liveness(
        &mut ctx.registry.borrow_mut(),
        &ctx.slot_table.borrow(),
        &mut capture
    ));
    publisher.tick_for_role_and_report_sampled_weapon(true, None);
    assert_eq!(
        ctx.slot_table
            .borrow()
            .get("player.weaponCharging")
            .unwrap()
            .value,
        Some(postretro_entities::SlotValue::Boolean(false))
    );
    assert_eq!(
        ctx.slot_table
            .borrow()
            .get("player.weaponChargeProgress")
            .unwrap()
            .value,
        Some(postretro_entities::SlotValue::Number(0.0))
    );
    assert_eq!(
        ctx.registry
            .borrow()
            .get_component::<WeaponComponent>(id)
            .unwrap()
            .activation_clock,
        clock
    );
    assert_eq!(
        ctx.registry
            .borrow()
            .get_component::<HealthComponent>(pawn)
            .unwrap()
            .current,
        100.0,
        "owner projection does not replace local health authority"
    );
    assert!(
        ctx.registry
            .borrow()
            .get_component::<WeaponComponent>(id)
            .unwrap()
            .state
            .activation_cursor()
            .is_none()
    );
    let mut edges = crate::input::ActionSnapshot::with_button_state(
        crate::input::Action::Shoot,
        crate::input::ButtonState::Pressed,
    );
    capture.observe(&edges);
    edges = crate::input::ActionSnapshot::with_button_state(
        crate::input::Action::Shoot,
        crate::input::ButtonState::Released,
    );
    capture.observe(&edges);
    let fresh = capture.command(11);
    assert_eq!(
        fresh.release.unwrap().token,
        fresh.initiation.unwrap(),
        "dead token cannot capture a fresh release"
    );
    let mut next = command(true, true);
    next.activation = fresh;
    frame.predict_with_liveness(
        &mut ctx.registry.borrow_mut(),
        &mut next,
        11,
        77,
        1000.0 / 60.0,
        Default::default(),
        &Default::default(),
        false,
    );
    assert!(
        frame.due.is_empty(),
        "dead owner cannot restart or produce later ordinals"
    );
}

#[test]
fn client_weapon_held_reload_does_not_cancel_later_burst_ordinals() {
    let (mut registry, _, id) = fixture(
        serde_json::json!({"trigger":"press","recoveryMs":50,"steps":[{"kind":"shot"},{"kind":"wait","durationMs":16},{"kind":"shot"}]}),
        false,
    );
    let mut frame = ClientWeaponFrame::default();
    let mut reload = command(false, false);
    reload.reload = true;
    predict(&mut frame, &mut registry, &mut reload, 0);
    let mut start = command(true, true);
    start.reload = true;
    predict(&mut frame, &mut registry, &mut start, 1);
    for tick in 2..5 {
        let mut held = command(false, false);
        held.reload = true;
        predict(&mut frame, &mut registry, &mut held, tick);
    }
    assert_eq!(
        frame.due.len(),
        2,
        "equip-only pass cannot turn a held reload into repeated cancelling edges"
    );
    assert_eq!(
        registry
            .get_component::<WeaponComponent>(id)
            .unwrap()
            .shells_fired,
        2
    );
}

#[test]
fn client_weapon_invalid_old_charge_correction_retracts_only_that_activation() {
    use postretro_foundation::{
        ActivationRelease, ActivationStepDescriptor, IrNode, IrValue, NumberOrIr,
    };
    use postretro_net::wire::{ActivationOutcome as O, NetworkId, WireActivationToken};
    let (mut registry, pawn, id) = fixture(
        serde_json::json!({"trigger":"press","recoveryMs":0,"charge":{"minMs":0,"fullMs":100},"steps":[{"kind":"shot"}]}),
        true,
    );
    let mut component = registry
        .get_component::<WeaponComponent>(id)
        .unwrap()
        .clone();
    let ActivationStepDescriptor::Shot { scale } =
        &mut std::sync::Arc::make_mut(&mut component.primary).steps[0]
    else {
        unreachable!()
    };
    scale.damage = NumberOrIr::Ir(IrNode::Add {
        a: Box::new(IrNode::Mul {
            a: Box::new(IrNode::Mul {
                a: Box::new(IrNode::Input {
                    name: "charge".into(),
                    owner: None,
                }),
                b: Box::new(IrNode::Const {
                    value: IrValue::Number(3e38),
                }),
            }),
            b: Box::new(IrNode::Const {
                value: IrValue::Number(2.0),
            }),
        }),
        b: Box::new(IrNode::Const {
            value: IrValue::Number(1.0),
        }),
    });
    component.secondary = Some(std::sync::Arc::new(serde_json::from_value(serde_json::json!({"trigger":"press","recoveryMs":90,"steps":[{"kind":"shot"},{"kind":"wait","durationMs":100},{"kind":"shot"}]})).unwrap()));
    component.activation_programs = postretro_foundation::WeaponActivationPrograms::install(
        &component.primary,
        component.secondary.as_deref(),
    );
    registry.set_component(id, component).unwrap();
    let mut frame = ClientWeaponFrame::default();
    let token_a = ActivationToken {
        start_tick: 10,
        lane: ActivationLane::Primary,
    };
    let mut first = command(true, true);
    first.activation.initiation = Some(token_a);
    first.activation.release = Some(ActivationRelease {
        token: token_a,
        release_tick: 10,
    });
    predict(&mut frame, &mut registry, &mut first, 10);
    let a = frame.due.pop().unwrap();
    assert_eq!(a.shot.activation.charge, 0.0);
    let resolution = weapon::resolve_client_shot(
        Some(pawn),
        a.component,
        &a.pellet_salt,
        0,
        Vec3::ZERO,
        Vec3::NEG_Z,
        &a.placement,
        &crate::collision::CollisionWorld::new(),
        &registry,
        &crate::scripting_systems::hit_zones::HitZoneStore::new(),
        0.0,
        a.shot.clone(),
    );
    let mut predicted = weapon::ClientPredictedShots::new();
    predicted.predict(
        a.shot.activation.shot_id,
        id,
        &resolution,
        a.cooldown_before,
        a.cooldown_after,
        a.presentation,
    );
    let flight = sim::spawn_projectile(
        &mut registry,
        pawn,
        id,
        resolution.projectile_launch.unwrap(),
        Some(a.shot.activation.shot_id),
        sim::ProjectileSource {
            weapon: None,
            activation: None,
        },
    )
    .unwrap();
    let mut next = command(false, false);
    next.secondary_button = weapon::FireButtonState {
        pressed: true,
        active: true,
    };
    predict(&mut frame, &mut registry, &mut next, 11);
    let token_b = next.activation.initiation.unwrap();
    let before = registry
        .get_component::<WeaponComponent>(id)
        .unwrap()
        .cooldown_remaining_ms;
    let wire_a = WireActivationToken {
        start_tick: 10,
        lane: 0,
    };
    frame.records.outcome(
        &mut registry,
        O::InitiationAccepted {
            token: wire_a,
            weapon: NetworkId(17),
        },
    );
    let effect = frame
        .records
        .outcome(
            &mut registry,
            O::ExecutionAccepted {
                token: wire_a,
                weapon: NetworkId(17),
                charge_millionths: 1_000_000,
                recovery_ticks: 0,
            },
        )
        .unwrap();
    assert!(
        effect.terminal && effect.rejected,
        "strict overflow cannot leave incorrect prediction live"
    );
    assert!(
        effect.recovery_ms.is_none(),
        "old correction cannot overwrite B recovery"
    );
    let mut old = a.shot.clone();
    assert!(
        !frame.records.correct_new_shot(&mut old),
        "pending old snapshots also abort"
    );
    predicted.apply_verdict(&mut registry, a.shot.activation.shot_id, false, false);
    assert!(!registry.exists(flight));
    let component = registry.get_component::<WeaponComponent>(id).unwrap();
    assert_eq!(component.state.activation_cursor().unwrap().token, token_b);
    assert_eq!(component.cooldown_remaining_ms, before);
    assert_eq!(frame.due.len(), 1, "new B remains queued");
}
