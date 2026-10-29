use super::*;
use postretro_entities::data_descriptors::{EntityTypeDescriptor, LifeStatesDescriptor};
use postretro_entities::provenance::DescriptorSpawnPath;
use postretro_entities::{EntityId, Transform};
use postretro_foundation::*;
use std::collections::BTreeMap;

fn descriptor(name: &str, states: Option<&[&str]>) -> EntityTypeDescriptor {
    let mut descriptor = base_descriptor(name);
    descriptor.life_states = states.map(|names| LifeStatesDescriptor {
        initial: names[0].into(),
        states: names
            .iter()
            .map(|name| (name.to_string(), LifeStateProfile::default()))
            .collect(),
        rejoin: BTreeMap::new(),
        state_ids: BTreeMap::new(),
        table_revision: 0,
    });
    descriptor
}

fn base_descriptor(name: &str) -> EntityTypeDescriptor {
    EntityTypeDescriptor {
        life_states: None,
        ghost: false,
        faction: None,
        tolerance: None,
        canonical_name: Some(name.to_string()),
        inventory: None,
        light: None,
        emitter: None,
        movement: Some(PlayerMovementDescriptor {
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
                friction: GroundParams::DEFAULT_FRICTION,
                stop_speed: GroundParams::DEFAULT_STOP_SPEED,
            },
            air: AirParams {
                forward_steer: 0.3,
                accel: 2.0,
                max_control_speed: 4.0,
                bunny_hop: true,
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
            vault: None,
            view_feel: None,
        }),
        weapon: None,
        touchable: None,
        mesh: None,
        health: None,
        behavior: None,
    }
}

fn pawn(ctx: &ScriptCtx) -> EntityId {
    let mut registry = ctx.registry.borrow_mut();
    let pawn = registry.spawn(Transform {
        position: glam::Vec3::new(0.0, 2.0, 0.0),
        ..Default::default()
    });
    registry
        .set_component(
            pawn,
            DescriptorProvenance {
                canonical_name: "player".into(),
                owned_components: Default::default(),
                map_overrides: Default::default(),
                spawn_path: DescriptorSpawnPath::PlayerSpawn,
            },
        )
        .unwrap();
    let data = ctx.data_registry.borrow();
    let descriptor = data
        .entities
        .iter()
        .find(|d| d.canonical_name.as_deref() == Some("player"))
        .unwrap();
    let mut movement =
        PlayerMovementComponent::from_descriptor(descriptor.movement.as_ref().unwrap());
    if let Some(block) = descriptor.life_states.as_ref() {
        let (id, profile) = block.state("down").unwrap();
        enter_life_state(&mut movement, id, profile, block.table_revision);
        movement.life_state_elapsed_ticks = 123;
    }
    registry.set_component(pawn, movement).unwrap();
    pawn
}

fn read(ctx: &ScriptCtx, pawn: EntityId) -> PlayerMovementComponent {
    ctx.registry
        .borrow()
        .get_component::<PlayerMovementComponent>(pawn)
        .unwrap()
        .clone()
}

#[test]
fn life_state_o8_kept_name_remaps_shifted_id_and_new_profile_without_reset() {
    let ctx = ScriptCtx::new();
    ctx.data_registry
        .borrow_mut()
        .replace_entity_types(vec![descriptor("player", Some(&["alive", "down"]))]);
    let pawn = pawn(&ctx);
    let names = ctx.data_registry.borrow().life_state_names.clone();
    let mut next = descriptor("player", Some(&["alive", "down"]));
    next.life_states
        .as_mut()
        .unwrap()
        .states
        .get_mut("down")
        .unwrap()
        .fire = false;
    ctx.data_registry
        .borrow_mut()
        .replace_entity_types(vec![next, descriptor("other", Some(&["aaa"]))]);
    remap_live_life_states(&ctx, &names);
    let movement = read(&ctx, pawn);
    assert_eq!(
        ctx.data_registry.borrow().life_state_names[movement.life_state as usize],
        "down"
    );
    assert_eq!(movement.life_state_elapsed_ticks, 123);
    assert_eq!(player_capability(&movement, Capability::Fire), Some(false));
    assert_eq!(
        movement.life_state_table_revision,
        ctx.data_registry.borrow().life_state_table_revision
    );
}

#[test]
fn life_state_o8_removed_name_enters_initial_then_undeclared_with_one_warning_each() {
    let logs = postretro_test_log_capture::LogCapture::start();
    let ctx = ScriptCtx::new();
    ctx.data_registry
        .borrow_mut()
        .replace_entity_types(vec![descriptor("player", Some(&["alive", "down"]))]);
    let pawn = pawn(&ctx);
    let names = ctx.data_registry.borrow().life_state_names.clone();
    ctx.data_registry
        .borrow_mut()
        .replace_entity_types(vec![descriptor("player", Some(&["alive"]))]);
    remap_live_life_states(&ctx, &names);
    assert_eq!(read(&ctx, pawn).life_state_elapsed_ticks, 0);
    assert_eq!(read(&ctx, pawn).life_state, 0);
    let names = ctx.data_registry.borrow().life_state_names.clone();
    ctx.data_registry
        .borrow_mut()
        .replace_entity_types(vec![descriptor("player", None)]);
    remap_live_life_states(&ctx, &names);
    let movement = read(&ctx, pawn);
    assert_eq!(movement.life_state, UNDECLARED_LIFE_STATE);
    assert_eq!(movement.life_speed_scale, 1.0);
    assert_eq!(player_capability(&movement, Capability::Fire), None);
    let warnings = logs
        .records()
        .iter()
        .filter(|r| r.level == log::Level::Warn && r.message.contains("[LifeState]"))
        .count();
    assert_eq!(warnings, 2);
}

#[test]
fn life_state_o34_new_declaration_enters_initial_and_o37_client_remaps_nothing() {
    let ctx = ScriptCtx::new();
    ctx.data_registry
        .borrow_mut()
        .replace_entity_types(vec![descriptor("player", None)]);
    let pawn = pawn(&ctx);
    ctx.data_registry
        .borrow_mut()
        .replace_entity_types(vec![descriptor("player", Some(&["alive", "down"]))]);
    remap_live_life_states(&ctx, &[]);
    assert_eq!(read(&ctx, pawn).life_state, 0);
    assert_eq!(read(&ctx, pawn).life_state_elapsed_ticks, 0);
    let before = read(&ctx, pawn);
    ctx.owner_slot_writes_enabled.set(false);
    ctx.data_registry
        .borrow_mut()
        .replace_entity_types(vec![descriptor("player", None)]);
    remap_live_life_states(&ctx, &["alive".into(), "down".into()]);
    let after = read(&ctx, pawn);
    assert_eq!(after.life_state, before.life_state);
    assert_eq!(
        after.life_state_table_revision,
        before.life_state_table_revision
    );
    assert_eq!(after.life_fire, before.life_fire);
}

fn restrictive_profile() -> LifeStateProfile {
    LifeStateProfile {
        movement: LifeStateMovementProfile {
            speed_scale: 0.3,
            abilities: false,
            posture: LifeStatePosture::Crouch,
        },
        look: false,
        fire: false,
        switch_weapon: false,
        reload: false,
        use_enabled: false,
        touch: false,
        occupancy: false,
        damageable: false,
    }
}

fn restrictive_descriptor() -> EntityTypeDescriptor {
    let mut descriptor = descriptor("player", Some(&["alive", "down"]));
    descriptor.movement.as_mut().unwrap().crouch = Some(CrouchParams {
        half_height: 0.4,
        eye_height: 0.3,
        transition_rate: 8.0,
    });
    *descriptor
        .life_states
        .as_mut()
        .unwrap()
        .states
        .get_mut("down")
        .unwrap() = restrictive_profile();
    descriptor
}

/// Exercise the descriptor-owned movement lane, rather than a remap-only pawn
/// whose empty provenance lets the refresh planner skip every replacement.
fn descriptor_owned_pawn(ctx: &ScriptCtx) -> EntityId {
    let pawn = pawn(ctx);
    let mut registry = ctx.registry.borrow_mut();
    let mut provenance = registry
        .get_component::<DescriptorProvenance>(pawn)
        .unwrap()
        .clone();
    provenance
        .owned_components
        .insert(postretro_entities::provenance::DescriptorComponentKind::Movement);
    registry.set_component(pawn, provenance).unwrap();
    pawn
}

fn assert_carried_life_state(before: &PlayerMovementComponent, after: &PlayerMovementComponent) {
    assert_eq!(after.life_state, before.life_state);
    assert_eq!(
        after.life_state_elapsed_ticks,
        before.life_state_elapsed_ticks
    );
    assert_eq!(
        after.life_state_table_revision,
        before.life_state_table_revision
    );
    for capability in [
        Capability::MovementAbilities,
        Capability::Look,
        Capability::Fire,
        Capability::SwitchWeapon,
        Capability::Reload,
        Capability::Use,
        Capability::Touch,
        Capability::Occupancy,
        Capability::Damageable,
    ] {
        assert_eq!(
            player_capability(after, capability),
            player_capability(before, capability),
            "{capability:?}"
        );
    }
    assert!((after.life_speed_scale - before.life_speed_scale).abs() < f32::EPSILON);
    assert_eq!(after.life_posture, before.life_posture);
    assert_eq!(after.vault_barred, before.vault_barred);
}

#[test]
fn life_state_o37_actual_movement_refresh_carries_fields_and_stamps_before_client_no_remap() {
    use crate::refresh_plan::{apply_descriptor_refresh_plan, plan_descriptor_refresh};
    for removed_block in [false, true] {
        let ctx = ScriptCtx::new();
        ctx.owner_slot_writes_enabled.set(false);
        ctx.data_registry
            .borrow_mut()
            .replace_entity_types(vec![restrictive_descriptor()]);
        let pawn = descriptor_owned_pawn(&ctx);
        let before = read(&ctx, pawn);
        let old = ctx.data_registry.borrow().entities.clone();
        let previous_names = ctx.data_registry.borrow().life_state_names.clone();
        let mut replacement = descriptor(
            "player",
            if removed_block {
                None
            } else {
                Some(&["alive", "down"])
            },
        );
        replacement.movement.as_mut().unwrap().ground.accel = 20.0;
        let next = vec![replacement, descriptor("other", Some(&["aaa"]))];
        let plan = plan_descriptor_refresh(&old, &next, &ctx.registry.borrow());
        let summary = apply_descriptor_refresh_plan(&plan, &mut ctx.registry.borrow_mut()).unwrap();
        assert_eq!(
            summary.changed_movement_entities,
            [pawn],
            "must take the actual movement replacement lane"
        );
        let refreshed = read(&ctx, pawn);
        assert!((refreshed.ground_params.accel - 20.0).abs() < f32::EPSILON);
        assert_carried_life_state(&before, &refreshed);
        ctx.data_registry.borrow_mut().replace_entity_types(next);
        remap_live_life_states(&ctx, &previous_names);
        assert_carried_life_state(&before, &read(&ctx, pawn));
    }
}

#[test]
fn life_state_actual_movement_refresh_carries_old_identity_until_host_kept_or_removed_state_remap()
{
    use crate::refresh_plan::{apply_descriptor_refresh_plan, plan_descriptor_refresh};
    for keep_down in [false, true] {
        let logs = postretro_test_log_capture::LogCapture::start();
        let ctx = ScriptCtx::new();
        ctx.data_registry
            .borrow_mut()
            .replace_entity_types(vec![restrictive_descriptor()]);
        let pawn = descriptor_owned_pawn(&ctx);
        let before = read(&ctx, pawn);
        let old = ctx.data_registry.borrow().entities.clone();
        let names = ctx.data_registry.borrow().life_state_names.clone();
        let mut replacement = descriptor(
            "player",
            Some(if keep_down {
                &["alive", "down"]
            } else {
                &["alive"]
            }),
        );
        replacement.movement.as_mut().unwrap().ground.accel = 20.0;
        let next = vec![replacement, descriptor("other", Some(&["aaa"]))];
        let plan = plan_descriptor_refresh(&old, &next, &ctx.registry.borrow());
        let summary = apply_descriptor_refresh_plan(&plan, &mut ctx.registry.borrow_mut()).unwrap();
        assert_eq!(summary.changed_movement_entities, [pawn]);
        assert_carried_life_state(&before, &read(&ctx, pawn));
        ctx.data_registry.borrow_mut().replace_entity_types(next);
        remap_live_life_states(&ctx, &names);
        let after = read(&ctx, pawn);
        let data = ctx.data_registry.borrow();
        assert_eq!(
            data.life_state_names[after.life_state as usize],
            if keep_down { "down" } else { "alive" }
        );
        assert_eq!(
            after.life_state_elapsed_ticks,
            if keep_down { 123 } else { 0 }
        );
        assert_eq!(
            after.life_state_table_revision,
            data.life_state_table_revision
        );
        assert_eq!(player_capability(&after, Capability::Fire), Some(true));
        assert!((after.life_speed_scale - 1.0).abs() < f32::EPSILON);
        assert_eq!(after.life_posture, LifeStatePosture::Stand);
        assert!(!after.vault_barred);
        assert_eq!(
            logs.records()
                .iter()
                .filter(|record| record.level == log::Level::Warn
                    && record.message.contains("[LifeState]"))
                .count(),
            usize::from(!keep_down)
        );
    }
}

#[test]
fn life_state_host_remap_runs_when_movement_refresh_keeps_old_or_skips_missing_snapshot() {
    use crate::refresh_plan::{apply_descriptor_refresh_plan, plan_descriptor_refresh};
    for no_previous_snapshot in [false, true] {
        let ctx = ScriptCtx::new();
        ctx.data_registry
            .borrow_mut()
            .replace_entity_types(vec![restrictive_descriptor()]);
        let pawn = descriptor_owned_pawn(&ctx);
        let before = read(&ctx, pawn);
        let old = if no_previous_snapshot {
            Vec::new()
        } else {
            ctx.data_registry.borrow().entities.clone()
        };
        let names = ctx.data_registry.borrow().life_state_names.clone();
        let mut replacement = descriptor("player", Some(&["alive", "down"]));
        // The pawn's centre is y=2: this new capsule would embed it and must
        // KeepOld, while the independent committed profile still takes effect.
        replacement.movement.as_mut().unwrap().capsule.half_height = 4.0;
        let next = vec![replacement, descriptor("other", Some(&["aaa"]))];
        let plan = plan_descriptor_refresh(&old, &next, &ctx.registry.borrow());
        let summary = apply_descriptor_refresh_plan(&plan, &mut ctx.registry.borrow_mut()).unwrap();
        assert!(summary.changed_movement_entities.is_empty());
        assert_eq!(summary.kept_old_actions, usize::from(!no_previous_snapshot));
        assert_carried_life_state(&before, &read(&ctx, pawn));
        ctx.data_registry.borrow_mut().replace_entity_types(next);
        remap_live_life_states(&ctx, &names);
        let after = read(&ctx, pawn);
        assert_eq!(after.life_state_elapsed_ticks, 123);
        assert_eq!(player_capability(&after, Capability::Fire), Some(true));
        assert!((after.capsule.half_height - before.capsule.half_height).abs() < f32::EPSILON);
    }
}
