// Weapon activation regression coverage.
// See: context/lib/entity_model.md §4, §5 · context/lib/networking.md
use super::activation_prediction::advance_predicted_weapon_tick;
use super::execution::*;
use super::{FireButtonState, WeaponFireAuthorization, freeze_weapon_shot};
use postretro_entities::components::weapon::WeaponComponent;
use postretro_entities::components::wieldable_state::WieldableState;
use postretro_foundation::{
    ActivationInput, ActivationLane, ActivationRelease, ActivationToken, WeaponDescriptor,
};
use serde_json::{Value, json};

fn action(trigger: &str, recovery: f32, count: usize) -> Value {
    let mut steps = Vec::new();
    for ordinal in 0..count {
        if ordinal > 0 {
            steps.push(json!({"kind":"wait","durationMs":16.0}));
        }
        steps.push(json!({"kind":"shot"}));
    }
    json!({"trigger":trigger,"recoveryMs":recovery,"steps":steps})
}
fn weapon(primary: Value, secondary: Option<Value>, resource: Value) -> WeaponComponent {
    let descriptor:WeaponDescriptor=serde_json::from_value(json!({"damage":10.0,"range":100.0,"resolution":"hitscan","primary":primary,"secondary":secondary,"resource":resource})).unwrap();
    WeaponComponent::from_descriptor_with_canonical(
        &descriptor.validate().unwrap(),
        Some("execution-test"),
    )
}
fn ammo(capacity: u32) -> Value {
    json!({"kind":"ammo","type":"bullets","magazine":capacity,"reserve":0,"costPerShot":1,"reloadMs":100})
}
fn cell() -> Value {
    json!({"kind":"cell","capacity":20.0,"costPerShot":5.0,"regenPerSecond":10.0,"regenDelayMs":100.0})
}
fn input(tick: u32, lane: ActivationLane) -> ActivationCommand {
    ActivationCommand {
        tick,
        pawn: 7,
        real_command: true,
        input: ActivationInput {
            initiation: Some(ActivationToken {
                start_tick: tick,
                lane,
            }),
            ..Default::default()
        },
        controller_starts: false,
        primary: FireButtonState {
            pressed: false,
            active: false,
        },
        secondary: FireButtonState {
            pressed: false,
            active: false,
        },
    }
}
fn next(tick: u32) -> ActivationCommand {
    let mut command = input(tick, ActivationLane::Primary);
    command.input = Default::default();
    command
}
#[test]
fn activation_controller_consumes_blocked_press_and_hold_names_restart() {
    let mut press = weapon(action("press", 100.0, 1), None, Value::Null);
    press.cooldown_remaining_ms = 50.0;
    let mut command = next(0);
    command.controller_starts = true;
    command.primary = FireButtonState {
        pressed: true,
        active: true,
    };
    let blocked = advance_weapon_activation(&mut press, command, 10.0, false, true);
    assert!(blocked.rejected.is_some());
    assert!(blocked.shot.is_none());
    command.tick = 1;
    assert!(
        advance_weapon_activation(&mut press, command, 100.0, false, true)
            .shot
            .is_none()
    );
    command.tick = 2;
    command.primary = FireButtonState {
        pressed: false,
        active: false,
    };
    advance_weapon_activation(&mut press, command, 0.0, false, true);
    command.tick = 3;
    command.primary = FireButtonState {
        pressed: true,
        active: true,
    };
    assert!(
        advance_weapon_activation(&mut press, command, 0.0, false, true)
            .shot
            .is_some()
    );
    let mut hold = weapon(action("hold", 100.0, 1), None, Value::Null);
    command.tick = 0;
    command.primary = FireButtonState {
        pressed: true,
        active: true,
    };
    let first = advance_weapon_activation(&mut hold, command, 0.0, false, true)
        .shot
        .unwrap();
    command.tick = 1;
    command.primary.pressed = false;
    assert!(
        advance_weapon_activation(&mut hold, command, 50.0, false, true)
            .shot
            .is_none()
    );
    command.tick = 2;
    let restart = advance_weapon_activation(&mut hold, command, 50.0, false, true)
        .shot
        .unwrap();
    assert_ne!(first.shot_id.start_tick, restart.shot_id.start_tick);
    command.tick = 3;
    command.controller_starts = false;
    command.real_command = false;
    assert!(
        advance_weapon_activation(&mut hold, command, 100.0, false, true)
            .shot
            .is_none()
    );
}
#[test]
fn activation_secondary_wins_and_sequence_continues_after_release() {
    let mut weapon = weapon(
        action("hold", 0.0, 1),
        Some(action("press", 300.0, 3)),
        ammo(8),
    );
    let mut command = next(10);
    command.controller_starts = true;
    command.primary = FireButtonState {
        pressed: true,
        active: true,
    };
    command.secondary = command.primary;
    let first = advance_weapon_activation(&mut weapon, command, 0.0, false, true)
        .shot
        .unwrap();
    assert_eq!(first.shot_id.lane, ActivationLane::Secondary);
    let token = first.shot_id.activation().token;
    let mut ids = [first.shot_id.ordinal, 0, 0];
    for ordinal in 1..3 {
        let mut command = next(10 + ordinal);
        command.input.release = Some(ActivationRelease {
            token,
            release_tick: 10 + ordinal,
        });
        let shot = advance_weapon_activation(&mut weapon, command, 16.0, false, true)
            .shot
            .unwrap();
        ids[ordinal as usize] = shot.shot_id.ordinal;
        assert_eq!(shot.shot_id.start_tick, 10);
    }
    assert_eq!(ids, [0, 1, 2]);
    assert_eq!(weapon.magazine, 5);
    assert_eq!(weapon.cooldown_remaining_ms, 300.0);
    assert_eq!(weapon.state, WieldableState::Idle);
}
#[test]
fn activation_partial_resource_exhaustion_settles_attempt_and_preserves_recovery() {
    let mut weapon = weapon(action("press", 300.0, 3), None, ammo(1));
    let first = advance_weapon_activation(
        &mut weapon,
        input(0, ActivationLane::Primary),
        0.0,
        false,
        true,
    );
    assert!(first.shot.is_some());
    let empty = advance_weapon_activation(&mut weapon, next(1), 16.0, false, true);
    assert_eq!(empty.attempted.unwrap().ordinal, 1);
    assert_eq!(empty.authorization, Some(WeaponFireAuthorization::Empty));
    assert!(empty.terminal.is_some());
    assert!(empty.shot.is_none());
    assert_eq!(weapon.magazine, 0);
    assert_eq!(weapon.cooldown_remaining_ms, 300.0);
    assert!(
        advance_weapon_activation(&mut weapon, next(2), 16.0, false, true)
            .attempted
            .is_none()
    );
}
#[test]
fn activation_charge_explicit_release_early_cancel_and_independent_scalars() {
    for multiplier in [3.0, 6.0] {
        let mut secondary = action("press", 400.0, 1);
        secondary["charge"] = json!({"minMs":200.0,"fullMs":1000.0});
        secondary["steps"][0]["scale"] = json!({"damage":multiplier});
        let mut host = weapon(action("hold", 110.0, 1), Some(secondary), cell());
        let mut predicted = host.clone();
        let command = input(0, ActivationLane::Secondary);
        let token = command.input.initiation.unwrap();
        assert!(
            advance_weapon_activation(&mut host, command, 0.0, false, true)
                .shot
                .is_none()
        );
        assert!(
            advance_predicted_weapon_tick(&mut predicted, command, false, 0.0, true)
                .shot
                .is_none()
        );
        for tick in 1..60 {
            assert!(
                advance_weapon_activation(&mut host, next(tick), 16.0, false, true)
                    .shot
                    .is_none()
            );
            advance_predicted_weapon_tick(&mut predicted, next(tick), false, 16.0, true);
        }
        let mut release = next(60);
        release.input.release = Some(ActivationRelease {
            token,
            release_tick: 60,
        });
        let hs = advance_weapon_activation(&mut host, release, 16.0, false, true)
            .shot
            .unwrap();
        let ps = advance_predicted_weapon_tick(&mut predicted, release, false, 16.0, true)
            .shot
            .unwrap();
        assert_eq!(hs.values, ps.values);
        assert_eq!(hs.values.damage, 10.0 * multiplier);
        assert_eq!(
            hs.values.resource_cost,
            postretro_foundation::ShotResourceCost::Cell(5.0)
        );
        assert_eq!(host.cell.unwrap().charge, 15.0);
        assert_eq!(predicted.cell.unwrap().charge, 20.0);
    }
    let mut primary = action("press", 400.0, 1);
    primary["charge"] = json!({"minMs":200.0,"fullMs":1000.0});
    let mut weapon = weapon(primary, None, ammo(10));
    let command = input(0, ActivationLane::Primary);
    let token = command.input.initiation.unwrap();
    advance_weapon_activation(&mut weapon, command, 0.0, false, true);
    let mut early = next(1);
    early.input.release = Some(ActivationRelease {
        token,
        release_tick: 1,
    });
    let result = advance_weapon_activation(&mut weapon, early, 16.0, false, true);
    assert!(result.terminal.is_some());
    assert_eq!(weapon.magazine, 10);
    assert_eq!(weapon.cooldown_remaining_ms, 0.0);
}
#[test]
fn activation_heat_crossing_fires_then_cancels_and_cell_cost_keeps_fraction() {
    let mut heated = weapon(
        action("press", 80.0, 3),
        None,
        json!({"kind":"heat","heatPerShot":60.0,"overheatAt":100.0,"coolPerSecond":20.0,"coolDelayMs":100.0}),
    );
    advance_weapon_activation(
        &mut heated,
        input(0, ActivationLane::Primary),
        0.0,
        false,
        true,
    );
    let crossing = advance_weapon_activation(&mut heated, next(1), 16.0, false, true);
    assert!(crossing.shot.is_some());
    assert!(crossing.overheat);
    assert!(crossing.terminal.is_some());
    assert_eq!(heated.heat.unwrap().heat, 100.0);
    assert!(
        advance_weapon_activation(&mut heated, next(2), 16.0, false, true)
            .attempted
            .is_none()
    );
    let mut primary = action("press", 80.0, 1);
    primary["steps"][0]["scale"] = json!({"resourceCost":0.25});
    let mut fractional = weapon(primary, None, cell());
    advance_weapon_activation(
        &mut fractional,
        input(0, ActivationLane::Primary),
        0.0,
        false,
        true,
    );
    assert_eq!(fractional.cell.unwrap().charge, 18.75);
}
#[test]
fn activation_prediction_attempts_continue_without_spending_stale_resources() {
    let mut weapon = weapon(action("press", 300.0, 3), None, ammo(1));
    weapon.magazine = 0;
    for tick in 0..3 {
        let command = if tick == 0 {
            input(tick, ActivationLane::Primary)
        } else {
            next(tick)
        };
        let result = advance_predicted_weapon_tick(&mut weapon, command, false, 16.0, true);
        assert_eq!(result.attempted.unwrap().ordinal, tick as u8);
        assert!(result.shot.is_some());
    }
    assert_eq!(weapon.magazine, 0);
}
#[test]
fn activation_fresh_impossible_reload_cancels_held_reload_does_not_repeat() {
    let mut weapon = weapon(action("press", 300.0, 3), None, Value::Null);
    advance_predicted_weapon_tick(
        &mut weapon,
        input(0, ActivationLane::Primary),
        false,
        0.0,
        true,
    );
    let cancelled = advance_predicted_weapon_tick(&mut weapon, next(1), true, 16.0, true);
    assert!(cancelled.terminal.is_some());
    assert_eq!(weapon.state, WieldableState::Idle);
    assert_eq!(weapon.cooldown_remaining_ms, 284.0);
    let blocked = advance_predicted_weapon_tick(
        &mut weapon,
        input(2, ActivationLane::Primary),
        true,
        400.0,
        true,
    );
    assert!(blocked.shot.is_some());
    assert!(blocked.terminal.is_none());
}
#[test]
fn activation_immutable_shot_keeps_action_and_scaled_tuning_after_replace() {
    let mut weapon = weapon(action("press", 100.0, 1), None, Value::Null);
    let result = advance_weapon_activation(
        &mut weapon,
        input(0, ActivationLane::Primary),
        0.0,
        false,
        true,
    );
    let shot = freeze_weapon_shot(&weapon, result.shot.unwrap());
    let old = shot.activation.action.clone();
    let descriptor:WeaponDescriptor=serde_json::from_value(json!({"damage":99.0,"range":10.0,"resolution":"hitscan","primary":action("press",500.0,1)})).unwrap();
    weapon.refresh_from_descriptor(&descriptor);
    assert_eq!(shot.activation.values.damage, 10.0);
    assert_eq!(shot.activation.values.range, 100.0);
    assert_eq!(shot.activation.recovery_ms, 100.0);
    assert!(!std::sync::Arc::ptr_eq(&old, &weapon.primary));
}
#[test]
fn activation_valid_advance_and_scaling_allocates_nothing_after_install() {
    let mut weapon = weapon(action("press", 300.0, 3), None, ammo(10));
    let probe = crate::alloc_probe::AllocSnapshot::arm();
    for tick in 0..3 {
        let command = if tick == 0 {
            input(tick, ActivationLane::Primary)
        } else {
            next(tick)
        };
        let result = advance_weapon_activation(&mut weapon, command, 16.0, false, true);
        assert!(result.shot.is_some());
    }
    assert_eq!(probe.allocs_since(), 0);
}

#[test]
fn activation_invalid_product_cancels_before_debit_warns_per_descriptor_and_retries_allocate_nothing()
 {
    let capture = postretro_test_log_capture::LogCapture::start();
    let mut primary = action("press", 300.0, 1);
    primary["steps"][0]["scale"] = json!({"damage":2.0});
    let mut first = weapon(primary.clone(), None, ammo(10));
    first.damage = f32::MAX;
    first.descriptor_identity = "invalid-overflow-first".into();
    first.credit_source = "shared-credit".into();
    let denied = advance_weapon_activation(
        &mut first,
        input(0, ActivationLane::Primary),
        0.0,
        false,
        true,
    );
    assert_eq!(denied.attempted.unwrap().ordinal, 0);
    assert!(denied.shot.is_none());
    assert!(denied.terminal.is_some());
    assert_eq!(first.magazine, 10);
    assert_eq!(first.cooldown_remaining_ms, 0.0);
    let probe = crate::alloc_probe::AllocSnapshot::arm();
    let denied = advance_weapon_activation(
        &mut first,
        input(1, ActivationLane::Primary),
        0.0,
        false,
        true,
    );
    assert!(denied.shot.is_none());
    assert_eq!(probe.allocs_since(), 0);
    let mut second = weapon(primary, None, ammo(10));
    second.damage = f32::MAX;
    second.descriptor_identity = "invalid-overflow-second".into();
    second.credit_source = "shared-credit".into();
    advance_weapon_activation(
        &mut second,
        input(0, ActivationLane::Primary),
        0.0,
        false,
        true,
    );
    let warnings = capture
        .records()
        .into_iter()
        .filter(|record| {
            record.level == log::Level::Warn && record.message.contains("invalid activation value")
        })
        .collect::<Vec<_>>();
    assert_eq!(warnings.len(), 2);
    assert!(
        warnings[0]
            .message
            .contains("invalid-overflow-first.damage")
    );
    assert!(
        warnings[1]
            .message
            .contains("invalid-overflow-second.damage")
    );
}

#[test]
fn activation_predicted_shell_interruption_ignores_empty_projected_resources() {
    let mut weapon = weapon(action("hold", 100.0, 1), None, ammo(10));
    weapon.magazine = 0;
    weapon.state = WieldableState::ShellLoading;
    let mut command = next(0);
    command.controller_starts = true;
    command.primary = FireButtonState {
        pressed: false,
        active: true,
    };
    let result = advance_predicted_weapon_tick(&mut weapon, command, false, 16.0, true);
    assert!(result.initiated.is_some());
    assert!(result.shot.is_some());
    assert_eq!(weapon.magazine, 0);
    assert_eq!(weapon.state, WieldableState::Idle);
}

#[test]
fn activation_fixed_tick_reservation_preserves_bloom_decay_before_later_spatial_resolution() {
    let mut weapon = weapon(action("press", 300.0, 3), None, ammo(10));
    weapon.bloom_per_shot_degrees = 2.0;
    weapon.bloom_max_degrees = 10.0;
    weapon.bloom_decay_degrees_per_second = 10.0;
    weapon.bloom_decay_delay_ms = 0.0;
    let first = advance_predicted_weapon_tick(
        &mut weapon,
        input(0, ActivationLane::Primary),
        false,
        0.0,
        true,
    )
    .shot
    .unwrap();
    let second = advance_predicted_weapon_tick(&mut weapon, next(1), false, 100.0, true)
        .shot
        .unwrap();
    let third = advance_predicted_weapon_tick(&mut weapon, next(2), false, 100.0, true)
        .shot
        .unwrap();
    assert_eq!(
        [
            first.shell_counter,
            second.shell_counter,
            third.shell_counter
        ],
        [0, 1, 2]
    );
    assert_eq!(
        [
            first.bloom_degrees,
            second.bloom_degrees,
            third.bloom_degrees
        ],
        [0.0, 1.0, 2.0]
    );
    assert_eq!(weapon.shells_fired, 3);
    assert_eq!(weapon.bloom_accumulator_degrees, 4.0);
    let registry = postretro_entities::EntityRegistry::new();
    let world = crate::collision::CollisionWorld::new();
    let zones = crate::scripting_systems::hit_zones::HitZoneStore::new();
    let command = super::WeaponFireCommand {
        button: FireButtonState {
            pressed: false,
            active: false,
        },
        aim_origin: glam::Vec3::ZERO,
        aim_direction: glam::Vec3::NEG_Z,
        can_fire: true,
    };
    let shots = [
        freeze_weapon_shot(&weapon, first),
        freeze_weapon_shot(&weapon, second),
        freeze_weapon_shot(&weapon, third),
    ];
    for shot in shots {
        super::resolve_activation_shot(
            &registry,
            None,
            &mut weapon,
            "execution-test",
            0,
            &command,
            &Default::default(),
            &world,
            &zones,
            0.0,
            shot,
        );
    }
    assert_eq!(weapon.shells_fired, 3);
    assert_eq!(weapon.bloom_accumulator_degrees, 4.0);
}

#[test]
fn activation_projectile_damage_parity_keeps_cost_and_size_independent_and_freezes_launch() {
    for multiplier in [3.0, 6.0] {
        let mut primary = action("press", 300.0, 1);
        primary["steps"][0]["scale"] = json!({"damage":multiplier});
        let descriptor:WeaponDescriptor=serde_json::from_value(json!({"damage":10,"range":100,"resolution":"projectile","primary":primary,"projectile":{"speed":20,"radius":0.1,"lifetimeMs":1000,"visual":{"body":{"kind":"sprite","sprite":"sprites/test.png","size":0.4}}},"resource":ammo(10)})).unwrap();
        let mut host = WeaponComponent::from_descriptor_with_canonical(
            &descriptor.validate().unwrap(),
            Some("projectile-parity"),
        );
        let mut predicted = host.clone();
        let authoritative = advance_weapon_activation(
            &mut host,
            input(0, ActivationLane::Primary),
            0.0,
            false,
            true,
        )
        .shot
        .unwrap();
        let prediction = advance_predicted_weapon_tick(
            &mut predicted,
            input(0, ActivationLane::Primary),
            false,
            0.0,
            true,
        )
        .shot
        .unwrap();
        assert_eq!(authoritative.values, prediction.values);
        assert_eq!(authoritative.values.damage, 10.0 * multiplier);
        assert_eq!(
            authoritative.values.resource_cost,
            postretro_foundation::ShotResourceCost::Ammo(1)
        );
        assert_eq!(authoritative.values.projectile_size, Some(0.4));
        assert_eq!(host.magazine, 9);
        assert_eq!(predicted.magazine, 10);
        let shot = freeze_weapon_shot(&host, authoritative);
        let mut registry = postretro_entities::EntityRegistry::new();
        let pawn = registry.spawn(Default::default());
        let weapon_id = registry.spawn(Default::default());
        let world = crate::collision::CollisionWorld::new();
        let zones = crate::scripting_systems::hit_zones::HitZoneStore::new();
        let command = super::WeaponFireCommand {
            button: FireButtonState {
                pressed: false,
                active: false,
            },
            aim_origin: glam::Vec3::ZERO,
            aim_direction: glam::Vec3::NEG_Z,
            can_fire: true,
        };
        let events = super::resolve_activation_shot(
            &registry,
            Some(pawn),
            &mut host,
            "projectile-parity",
            0,
            &command,
            &Default::default(),
            &world,
            &zones,
            0.0,
            shot.clone(),
        );
        let launch = events.projectile_launches.into_iter().next().unwrap();
        assert_eq!(launch.damage, 10.0 * multiplier);
        assert_eq!(launch.model_scale, 1.0);
        assert!(std::sync::Arc::ptr_eq(
            launch.action.as_ref().unwrap(),
            shot.action()
        ));
        let projectile = crate::sim::spawn_projectile(
            &mut registry,
            pawn,
            weapon_id,
            launch,
            None,
            Default::default(),
        )
        .unwrap();
        let visual = registry
            .get_component::<postretro_entities::components::sprite_visual::SpriteVisual>(
                projectile,
            )
            .unwrap();
        assert_eq!(visual.size, 0.4);
        let component = registry
            .get_component::<postretro_entities::components::projectile::ProjectileComponent>(
                projectile,
            )
            .unwrap();
        assert_eq!(component.source_shot, Some(shot.activation.shot_id));
        assert_eq!(component.damage, 10.0 * multiplier);
    }
}

#[test]
fn activation_level_teardown_cancels_retained_instances_preserving_paid_debt_and_reload() {
    let mut registry = postretro_entities::EntityRegistry::new();
    let id = registry.spawn(Default::default());
    let mut pending = weapon(action("press", 300.0, 3), None, ammo(10));
    advance_weapon_activation(
        &mut pending,
        input(0, ActivationLane::Primary),
        0.0,
        false,
        true,
    );
    registry.set_component(id, pending).unwrap();
    let reloading = registry.spawn(Default::default());
    let mut reload = weapon(action("press", 100.0, 1), None, ammo(10));
    reload.state = WieldableState::Reloading;
    registry.set_component(reloading, reload).unwrap();
    cancel_weapon_activations(&mut registry);
    let pending = registry.get_component::<WeaponComponent>(id).unwrap();
    assert_eq!(pending.state, WieldableState::Idle);
    assert_eq!(pending.magazine, 9);
    assert_eq!(pending.cooldown_remaining_ms, 300.0);
    assert_eq!(
        registry
            .get_component::<WeaponComponent>(reloading)
            .unwrap()
            .state,
        WieldableState::Reloading
    );
}

#[test]
fn activation_charge_correction_uses_original_program_and_bases_even_after_zero_scale_and_replace()
{
    let mut charged = action("press", 100.0, 1);
    charged["charge"] = json!({"minMs":0,"fullMs":32});
    let mut weapon = weapon(charged, None, ammo(10));
    if let postretro_foundation::ActivationStepDescriptor::Shot { scale } =
        &mut std::sync::Arc::make_mut(&mut weapon.primary).steps[0]
    {
        scale.damage = postretro_foundation::NumberOrIr::Ir(postretro_foundation::IrNode::Mul {
            a: Box::new(postretro_foundation::IrNode::Input {
                name: "charge".into(),
                owner: None,
            }),
            b: Box::new(postretro_foundation::IrNode::Const {
                value: postretro_foundation::IrValue::Number(6.0),
            }),
        });
    }
    weapon.activation_programs =
        postretro_foundation::WeaponActivationPrograms::install(&weapon.primary, None);
    let mut command = input(0, ActivationLane::Primary);
    command.input.release = Some(ActivationRelease {
        token: command.input.initiation.unwrap(),
        release_tick: 0,
    });
    let original = advance_predicted_weapon_tick(&mut weapon, command, false, 0.0, true)
        .shot
        .unwrap();
    assert_eq!(original.values.damage, 0.0);
    let snapshot = freeze_weapon_shot(&weapon, original);
    let replacement: WeaponDescriptor = serde_json::from_value(
        json!({"damage":99,"range":10,"resolution":"hitscan","primary":action("press",500.0,1)}),
    )
    .unwrap();
    weapon.refresh_from_descriptor(&replacement);
    let corrected = snapshot.with_authoritative_charge(0.5).unwrap();
    assert_eq!(corrected.activation.values.damage, 30.0);
    assert_eq!(corrected.activation.values.range, 100.0);
    assert_eq!(
        corrected.activation.values.resource_cost,
        postretro_foundation::ShotResourceCost::Ammo(1)
    );
    assert!(std::sync::Arc::ptr_eq(
        snapshot.action(),
        corrected.action()
    ));
}
