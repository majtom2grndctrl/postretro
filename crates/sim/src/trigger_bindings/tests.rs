use super::*;
use crate::trigger_commands::{BoundStoreValue, BoundTarget};
use postretro_entities::components::ammo_reserve::AmmoReserve;
use postretro_entities::components::brain::{BrainComponent, attach_brain_graph};
use postretro_entities::components::health::HealthComponent;
use postretro_entities::{MoverCommand, TriggerVolumeComponent};
use postretro_entities::{
    NumericRange, ReplicationScope, SlotOwnership, SlotRecord, SlotSchema, SlotType, SlotValue,
    Transform, TriggerActivation, TriggerFireMode,
};
use postretro_foundation::Seat;
use postretro_scripting_core::data_descriptors::{
    NamedReaction, PrimitiveDescriptor, ProgressDescriptor, ReactionDescriptor, SequenceStep,
    SequenceTarget,
};
use postretro_scripting_core::data_registry::DataRegistry;

fn primitive(
    name: &str,
    primitive: &str,
    tag: Option<&str>,
    args: serde_json::Value,
    on_complete: Option<&str>,
) -> NamedReaction {
    NamedReaction {
        name: name.to_string(),
        descriptor: ReactionDescriptor::Primitive(PrimitiveDescriptor {
            primitive: primitive.to_string(),
            target: None,
            kind: None,
            tag: tag.map(str::to_string),
            on_complete: on_complete.map(str::to_string),
            args,
        }),
    }
}

fn spawn_trigger(registry: &mut EntityRegistry, on_fire: &str) -> EntityId {
    let id = registry.spawn(Transform::default());
    registry
        .set_component(
            id,
            TriggerVolumeComponent::new(
                TriggerActivation::Touch,
                String::new(),
                on_fire.to_string(),
                String::new(),
                MoverCommand::Start,
                TriggerFireMode::Multiple,
                0.0,
                true,
            ),
        )
        .unwrap();
    id
}

fn spawn_brain(registry: &mut EntityRegistry, tag: &str) -> EntityId {
    let entity = registry.spawn(Transform::default());
    registry.set_tags(entity, vec![tag.into()]).unwrap();
    let graph = postretro_foundation::BehaviorGraphDescriptor {
        knockback: Default::default(),
        envelope: postretro_foundation::BehaviorGraphEnvelope {
            initial: "idle".to_string(),
            activities: std::collections::BTreeMap::from([(
                "idle".to_string(),
                postretro_foundation::BehaviorActivityDescriptor {
                    sound: None,
                    animation: Some("idle".to_string()),
                    motion: Some(postretro_foundation::MotionVerb::Hold),
                    action: None,
                    on_enter: None,
                    layers: Default::default(),
                },
            )]),
            transitions: Default::default(),
        },
        candidate_filter: None,
        retaliation: None,
        patrol: None,
        attacks: Default::default(),
        engagement_radius: None,
        move_speed: 3.5,
    };
    attach_brain_graph(registry, entity, &graph).unwrap();
    entity
}

fn per_owner_number_slot(value: f32) -> SlotRecord {
    SlotRecord::new(SlotSchema {
        slot_type: SlotType::Number,
        default: Some(SlotValue::Number(value)),
        range: Some(NumericRange {
            min: -10_000.0,
            max: 10_000.0,
        }),
        persist: false,
        readonly: false,
        ownership: SlotOwnership::Mod,
        network: ReplicationScope::None,
        per_owner: true,
        accumulate: None,
    })
}

#[test]
fn add_slot_trigger_command_applies_to_activator_seat_and_zero_activators_is_silent() {
    let mut registry = EntityRegistry::new();
    let pawn = registry.spawn(Transform::default());
    registry.bind_pawn_seat(pawn, Seat(6));
    let mut slots = SlotTable::new();
    slots
        .insert("currency.xp".to_string(), per_owner_number_slot(10.0))
        .unwrap();
    let command = bind_command(
        "addSlot",
        Some(BoundTarget::Activators),
        &serde_json::json!({ "slot": "currency.xp", "delta": 2.0 }),
        &slots,
        None,
    )
    .expect("owner-slot add binds");
    assert_eq!(command.kind(), BoundTriggerCommandKind::AddOwnerSlot);

    command.execute(
        &mut registry,
        &mut slots,
        &MoverCommandDiagnostics::default(),
        &crate::spawner::SpawnContext::default(),
        &TriggerFireContext {
            activator: Some(pawn),
            ..Default::default()
        },
    );
    assert_eq!(
        slots
            .get("currency.xp")
            .and_then(|record| record.per_seat_value(Seat(6))),
        Some(&SlotValue::Number(12.0)),
    );

    command.execute(
        &mut registry,
        &mut slots,
        &MoverCommandDiagnostics::default(),
        &crate::spawner::SpawnContext::default(),
        &TriggerFireContext::default(),
    );
    assert_eq!(
        slots
            .get("currency.xp")
            .and_then(|record| record.per_seat_value(Seat(6))),
        Some(&SlotValue::Number(12.0)),
        "zero activators leave the slot untouched without needing a warning path",
    );
}

#[test]
fn add_slot_missing_target_diagnostic_names_parsed_slot() {
    let mut slots = SlotTable::new();
    slots
        .insert("currency.xp".to_string(), per_owner_number_slot(0.0))
        .unwrap();

    let captured = crate::scripting::reactions::log_capture::capture(|| {
        assert!(
            bind_command(
                "addSlot",
                None,
                &serde_json::json!({ "slot": "currency.xp", "delta": 2.0 }),
                &slots,
                None,
            )
            .is_none()
        );
    });

    assert!(captured.iter().any(|(level, message)| {
        *level == log::Level::Warn
            && message.contains("addSlot")
            && message.contains("currency.xp")
            && message.contains("no target")
    }));
}

#[test]
fn legacy_set_state_rejects_per_owner_literal_and_ir_without_blocking_global_sibling() {
    let ctx = ScriptCtx::new();
    {
        let mut slots = ctx.slot_table.borrow_mut();
        slots
            .insert("currency.xp".to_string(), per_owner_number_slot(10.0))
            .unwrap();
        slots
            .insert_namespace(
                "trigger",
                vec![(
                    "flag".to_string(),
                    SlotRecord::new(SlotSchema {
                        slot_type: SlotType::Number,
                        default: Some(SlotValue::Number(0.0)),
                        range: None,
                        persist: false,
                        readonly: false,
                        ownership: SlotOwnership::Mod,
                        network: ReplicationScope::None,
                        per_owner: false,
                        accumulate: None,
                    }),
                )],
            )
            .unwrap();
    }

    let slots = ctx.slot_table.borrow();
    assert!(
        bind_command(
            "setState",
            None,
            &serde_json::json!({ "slot": "currency.xp", "value": 99.0 }),
            &slots,
            Some(&ctx),
        )
        .is_none(),
        "literal setState must not bind a per-owner slot"
    );
    assert!(
        bind_command(
            "setState",
            None,
            &serde_json::json!({
                "slot": "currency.xp",
                "value": { "op": "const", "value": 99.0 }
            }),
            &slots,
            Some(&ctx),
        )
        .is_none(),
        "IR setState must not bind a per-owner slot"
    );
    let global = bind_command(
        "setState",
        None,
        &serde_json::json!({ "slot": "trigger.flag", "value": 1.0 }),
        &slots,
        Some(&ctx),
    )
    .expect("global sibling setState still binds");
    drop(slots);

    let defensive_per_owner = BoundTriggerCommand::StoreSlot {
        slot: "currency.xp".to_string(),
        value: BoundStoreValue::Literal(SlotValue::Number(99.0)),
    };
    let mut registry = EntityRegistry::new();
    let mut dispatch_scope = DispatchScope::script(ctx.clone(), TRIGGER_EVENT_INPUTS);
    for command in [&defensive_per_owner, &global] {
        command.execute_with_script_ctx(
            &mut registry,
            &ctx,
            &mut dispatch_scope,
            &MoverCommandDiagnostics::default(),
            &crate::spawner::SpawnContext::default(),
            &TriggerFireContext::default(),
        );
    }

    let slots = ctx.slot_table.borrow();
    assert_eq!(
        slots
            .get("currency.xp")
            .and_then(|record| record.value.as_ref()),
        Some(&SlotValue::Number(10.0)),
        "defensive trigger execution must not mutate the retained scalar projection"
    );
    assert_eq!(
        slots
            .get("trigger.flag")
            .and_then(|record| record.value.as_ref()),
        Some(&SlotValue::Number(1.0)),
        "rejected per-owner setState must not block a valid global sibling"
    );
}

#[test]
fn update_npc_state_rejects_tagless_and_unknown_key_bindings() {
    assert!(
        bind_command(
            "updateNpcState",
            None,
            &serde_json::json!({ "aggro": true }),
            &SlotTable::new(),
            None,
        )
        .is_none()
    );
    assert!(
        bind_command(
            "updateNpcState",
            Some(BoundTarget::Tag("closet".into())),
            &serde_json::json!({ "unknown": true }),
            &SlotTable::new(),
            None,
        )
        .is_none()
    );
}

#[test]
fn spawn_from_spawner_binds_a_member_or_tag_target_and_rejects_others() {
    let args = serde_json::json!({});
    let slots = SlotTable::new();

    for target in [
        BoundTarget::Tag("closet".into()),
        BoundTarget::Entity(postretro_entities::EntityId::from_raw(7)),
    ] {
        let command = bind_command("spawnFromSpawner", Some(target), &args, &slots, None)
            .expect("a spawner member or raw tag target binds");
        assert_eq!(command.kind(), BoundTriggerCommandKind::Spawn);
    }

    for target in [
        None,
        Some(BoundTarget::Tag(String::new())),
        Some(BoundTarget::Activators),
        Some(BoundTarget::FiredTrigger),
        Some(BoundTarget::Group(postretro_entities::GroupTarget {
            kind: postretro_entities::GroupKind::Npc,
            tag: None,
        })),
    ] {
        assert!(
            bind_command("spawnFromSpawner", target, &args, &slots, None).is_none(),
            "spawnFromSpawner must reject an absent, empty-tag, group or special target"
        );
    }
}

#[test]
fn update_npc_state_resolves_later_added_brains_at_fire_time() {
    let mut registry = EntityRegistry::new();
    let trigger = spawn_trigger(&mut registry, "release");
    let mut data = DataRegistry::new();
    data.populate_level(
        vec![primitive(
            "release",
            "updateNpcState",
            Some("closet"),
            serde_json::json!({ "aggro": false }),
            None,
        )],
        Vec::new(),
        &[],
    );
    let table = TriggerBindingTable::build(&registry, &data, &SlotTable::new());
    let enemy = spawn_brain(&mut registry, "closet");

    let execution = table.execute(
        trigger,
        TriggerEventEdge::Enter,
        &mut registry,
        &mut SlotTable::new(),
        &TriggerFireContext::default(),
    );

    assert_eq!(
        execution.commands,
        vec![BoundTriggerCommandKind::UpdateNpcState]
    );
    assert!(
        !registry
            .get_component::<BrainComponent>(enemy)
            .unwrap()
            .aggro_armed
    );
}

#[test]
fn update_npc_state_empty_tag_is_debug_noop_and_keeps_fanout_work() {
    let mut registry = EntityRegistry::new();
    let trigger = spawn_trigger(&mut registry, "fanout");
    let mut data = DataRegistry::new();
    data.populate_level(
        vec![
            primitive(
                "fanout",
                "updateNpcState",
                Some("unspawned"),
                serde_json::json!({ "aggro": true }),
                None,
            ),
            primitive(
                "fanout",
                "setState",
                None,
                serde_json::json!({ "slot": "trigger.flag", "value": 1.0 }),
                None,
            ),
        ],
        Vec::new(),
        &[],
    );
    let table = TriggerBindingTable::build(&registry, &data, &writable_slots());
    let mut slots = writable_slots();
    let captured = crate::scripting::reactions::log_capture::capture(|| {
        let execution = table.execute(
            trigger,
            TriggerEventEdge::Enter,
            &mut registry,
            &mut slots,
            &TriggerFireContext::default(),
        );
        assert_eq!(
            execution.commands,
            vec![
                BoundTriggerCommandKind::UpdateNpcState,
                BoundTriggerCommandKind::StoreSlot,
            ]
        );
    });
    assert_eq!(
        slots.get("trigger.flag").unwrap().value,
        Some(SlotValue::Number(1.0)),
    );
    assert!(captured.iter().any(|(level, message)| {
        *level == log::Level::Debug && message.contains("empty Brain match")
    }));
}

#[test]
fn update_npc_state_special_target_logs_and_skips() {
    let command = bind_command(
        "updateNpcState",
        Some(BoundTarget::Activators),
        &serde_json::json!({ "aggro": false }),
        &SlotTable::new(),
        None,
    )
    .expect("special target remains bound so the fixed-tick executor can reject it");
    let mut registry = EntityRegistry::new();
    let enemy = spawn_brain(&mut registry, "closet");
    let captured = crate::scripting::reactions::log_capture::capture(|| {
        command.execute(
            &mut registry,
            &mut SlotTable::new(),
            &Default::default(),
            &Default::default(),
            &TriggerFireContext {
                activator: Some(enemy),
                ..Default::default()
            },
        );
    });
    assert!(
        registry
            .get_component::<BrainComponent>(enemy)
            .unwrap()
            .aggro_armed
    );
    assert!(captured.iter().any(|(level, message)| {
        *level == log::Level::Warn && message.contains("requires a tag or group target")
    }));
}

fn writable_slots() -> SlotTable {
    let mut slots = SlotTable::new();
    slots
        .insert_namespace(
            "trigger",
            vec![(
                "flag".to_string(),
                SlotRecord::new(SlotSchema {
                    slot_type: SlotType::Number,
                    default: Some(SlotValue::Number(0.0)),
                    range: Some(NumericRange { min: 0.0, max: 1.0 }),
                    persist: false,
                    readonly: false,
                    ownership: SlotOwnership::Mod,
                    network: Default::default(),
                    per_owner: false,
                    accumulate: None,
                }),
            )],
        )
        .unwrap();
    slots
}

#[test]
fn manifest_trigger_events_append_after_brush_bindings() {
    let mut registry = EntityRegistry::new();
    let trigger = spawn_trigger(&mut registry, "brush");
    registry.set_tags(trigger, vec!["plate".into()]).unwrap();
    let ctx = ScriptCtx::new();
    let mut data = DataRegistry::new();
    data.populate_level_with_trigger_events(
        vec![
            primitive(
                "brush",
                "setState",
                None,
                serde_json::json!({"slot":"trigger.flag","value":0.0}),
                None,
            ),
            primitive(
                "script",
                "setState",
                None,
                serde_json::json!({"slot":"trigger.flag","value":1.0}),
                None,
            ),
        ],
        Vec::new(),
        vec![
            postretro_scripting_core::data_descriptors::TriggerEventDescriptor {
                tag: "plate".into(),
                event: "enter".into(),
                fire: vec!["script".into()],
                levels: Vec::new(),
            },
        ],
        Vec::new(),
        &[],
    );
    ctx.slot_table
        .borrow_mut()
        .insert_namespace(
            "trigger",
            vec![(
                "flag".into(),
                SlotRecord::new(SlotSchema {
                    slot_type: SlotType::Number,
                    default: Some(SlotValue::Number(0.0)),
                    range: Some(NumericRange { min: 0.0, max: 1.0 }),
                    persist: false,
                    readonly: false,
                    ownership: SlotOwnership::Mod,
                    network: Default::default(),
                    per_owner: false,
                    accumulate: None,
                }),
            )],
        )
        .unwrap();
    let mut table = TriggerBindingTable::build_with_script_ctx(&registry, &data, &ctx);
    table.install_manifest_events(&registry, &data, &ctx);

    let execution = table.execute_with_script_ctx(
        trigger,
        TriggerEventEdge::Enter,
        &mut registry,
        &ctx,
        &TriggerFireContext::default(),
    );
    assert_eq!(execution.commands.len(), 2);
    assert_eq!(
        ctx.slot_table
            .borrow()
            .get("trigger.flag")
            .and_then(|r| r.value.as_ref()),
        Some(&SlotValue::Number(1.0))
    );
    assert!(
        table
            .bound_edges()
            .contains(&(trigger, TriggerEventEdge::Enter))
    );
}

#[test]
fn manifest_trigger_event_with_zero_tag_matches_is_inert() {
    let mut registry = EntityRegistry::new();
    let unrelated = spawn_trigger(&mut registry, "");
    registry
        .set_tags(unrelated, vec!["other-plate".into()])
        .unwrap();
    let ctx = ScriptCtx::new();
    let mut data = DataRegistry::new();
    data.populate_level_with_trigger_events(
        vec![primitive(
            "never",
            "setState",
            None,
            serde_json::json!({"slot":"trigger.flag","value":1.0}),
            None,
        )],
        Vec::new(),
        vec![
            postretro_scripting_core::data_descriptors::TriggerEventDescriptor {
                tag: "missing-plate".into(),
                event: "enter".into(),
                fire: vec!["never".into()],
                levels: Vec::new(),
            },
        ],
        Vec::new(),
        &[],
    );
    *ctx.slot_table.borrow_mut() = writable_slots();
    let mut table = TriggerBindingTable::build_with_script_ctx(&registry, &data, &ctx);
    table.install_manifest_events(&registry, &data, &ctx);

    assert!(table.bound_edges().is_empty());
    let execution = table.execute_with_script_ctx(
        unrelated,
        TriggerEventEdge::Enter,
        &mut registry,
        &ctx,
        &TriggerFireContext::default(),
    );
    assert!(execution.commands.is_empty());
    assert!(execution.residual().is_none());
    assert_eq!(
        ctx.slot_table.borrow().get("trigger.flag").unwrap().value,
        Some(SlotValue::Number(0.0)),
    );
}

#[test]
fn presentation_sequence_step_with_sentinel_target_rejects_without_residual() {
    let mut registry = EntityRegistry::new();
    let trigger = spawn_trigger(&mut registry, "bad-presentation");
    let mut data = DataRegistry::new();
    data.populate_level(
        vec![NamedReaction {
            name: "bad-presentation".into(),
            descriptor: ReactionDescriptor::Sequence(vec![SequenceStep {
                id: SequenceTarget::Activators,
                primitive: "flashScreen".into(),
                args: serde_json::json!({"color":[1,0,0],"durationMs":20}),
            }]),
        }],
        Vec::new(),
        &[],
    );
    let table = TriggerBindingTable::build(&registry, &data, &SlotTable::new());
    assert!(
        table.binding(trigger, TriggerEventEdge::Enter).is_none(),
        "a presentation-only sentinel step must leave the edge inert"
    );
    assert!(
        !table
            .bound_edges()
            .contains(&(trigger, TriggerEventEdge::Enter)),
        "a rejected presentation-only binding must not turn a nameless edge into a script-owned edge"
    );
    let execution = table.execute(
        trigger,
        TriggerEventEdge::Enter,
        &mut registry,
        &mut SlotTable::new(),
        &TriggerFireContext {
            fired_trigger: Some(trigger),
            activator: Some(trigger),
            occupancy: 1,
        },
    );
    assert!(execution.commands.is_empty());
    assert!(execution.residual().is_none());
}

fn health(registry: &mut EntityRegistry) -> EntityId {
    let id = registry.spawn(Transform::default());
    registry
        .set_component(
            id,
            HealthComponent {
                max: 100.0,
                current: 100.0,
                hitbox: None,
                death_handled: false,
                pending_kill_credit: None,
                zone_multipliers: Default::default(),
                contributor_ledger: Default::default(),
            },
        )
        .unwrap();
    id
}

fn ammo_reserve(registry: &mut EntityRegistry) -> EntityId {
    let id = registry.spawn(Transform::default());
    registry.set_component(id, AmmoReserve::new()).unwrap();
    id
}

#[test]
fn grant_commands_bind_negative_amounts_for_the_chokepoint_to_reject() {
    let slots = SlotTable::new();
    let health = bind_command(
        "grantHealth",
        Some(BoundTarget::Activators),
        &serde_json::json!({ "amount": -1.0 }),
        &slots,
        None,
    )
    .expect("negative health grant must reach the chokepoint");
    assert_eq!(health.kind(), BoundTriggerCommandKind::GrantHealth);

    let ammo = bind_command(
        "grantAmmo",
        Some(BoundTarget::Activators),
        &serde_json::json!({ "type": "bullets.light", "amount": -1.0 }),
        &slots,
        None,
    )
    .expect("negative ammo grant must reach the chokepoint");
    assert_eq!(ammo.kind(), BoundTriggerCommandKind::GrantAmmo);
}

#[test]
fn activator_grant_commands_mutate_only_the_current_activator() {
    let mut registry = EntityRegistry::new();
    let trigger = spawn_trigger(&mut registry, "grant");
    let health_recipient = health(&mut registry);
    let mut health_component = registry
        .get_component::<HealthComponent>(health_recipient)
        .unwrap()
        .clone();
    health_component.current = 60.0;
    registry
        .set_component(health_recipient, health_component)
        .unwrap();
    let ammo_recipient = ammo_reserve(&mut registry);
    let bystander = health(&mut registry);
    let mut data = DataRegistry::new();
    data.populate_level(
        vec![
            NamedReaction {
                name: "grant".into(),
                descriptor: ReactionDescriptor::Primitive(PrimitiveDescriptor {
                    primitive: "grantHealth".into(),
                    target: Some("@activators".into()),
                    kind: None,
                    tag: None,
                    on_complete: None,
                    args: serde_json::json!({ "amount": 25.0 }),
                }),
            },
            NamedReaction {
                name: "grant".into(),
                descriptor: ReactionDescriptor::Primitive(PrimitiveDescriptor {
                    primitive: "grantAmmo".into(),
                    target: Some("@activators".into()),
                    kind: None,
                    tag: None,
                    on_complete: None,
                    args: serde_json::json!({ "type": "bullets.light", "amount": 8.0 }),
                }),
            },
        ],
        Vec::new(),
        &[],
    );
    let table = TriggerBindingTable::build(&registry, &data, &SlotTable::new());

    let health_execution = table.execute(
        trigger,
        TriggerEventEdge::Enter,
        &mut registry,
        &mut SlotTable::new(),
        &TriggerFireContext {
            activator: Some(health_recipient),
            ..Default::default()
        },
    );
    assert_eq!(
        health_execution.commands,
        vec![
            BoundTriggerCommandKind::GrantHealth,
            BoundTriggerCommandKind::GrantAmmo,
        ]
    );
    assert_eq!(
        registry
            .get_component::<HealthComponent>(health_recipient)
            .unwrap()
            .current,
        85.0,
        "the first activator receives the health grant"
    );
    assert_eq!(
        registry
            .get_component::<HealthComponent>(bystander)
            .unwrap()
            .current,
        100.0
    );

    table.execute(
        trigger,
        TriggerEventEdge::Enter,
        &mut registry,
        &mut SlotTable::new(),
        &TriggerFireContext {
            activator: Some(ammo_recipient),
            ..Default::default()
        },
    );
    assert_eq!(
        registry
            .get_component::<AmmoReserve>(ammo_recipient)
            .unwrap()
            .available("bullets.light"),
        8
    );
}

#[test]
fn tag_grant_skips_missing_components_without_aborting_other_targets() {
    let mut registry = EntityRegistry::new();
    let trigger = spawn_trigger(&mut registry, "grant");
    let bare = registry.spawn(Transform::default());
    let recipient = ammo_reserve(&mut registry);
    registry.set_tags(bare, vec!["pickup".into()]).unwrap();
    registry.set_tags(recipient, vec!["pickup".into()]).unwrap();
    let mut data = DataRegistry::new();
    data.populate_level(
        vec![primitive(
            "grant",
            "grantAmmo",
            Some("pickup"),
            serde_json::json!({ "type": "bullets.light", "amount": 6.0 }),
            None,
        )],
        Vec::new(),
        &[],
    );
    let table = TriggerBindingTable::build(&registry, &data, &SlotTable::new());
    let captured = crate::scripting::reactions::log_capture::capture(|| {
        table.execute(
            trigger,
            TriggerEventEdge::Enter,
            &mut registry,
            &mut SlotTable::new(),
            &TriggerFireContext::default(),
        );
    });

    assert_eq!(
        registry
            .get_component::<AmmoReserve>(recipient)
            .unwrap()
            .available("bullets.light"),
        6
    );
    let warnings: Vec<_> = captured
        .iter()
        .filter(|(level, _)| *level == log::Level::Warn)
        .map(|(_, message)| message.clone())
        .collect();
    assert_eq!(
        warnings,
        vec![format!(
            "[Grant] grantAmmo: entity {bare} has no AmmoReserve; skipping"
        )]
    );
}

#[test]
fn activator_target_damages_each_edge_presser_once_and_leaves_bystander_untouched() {
    let mut registry = EntityRegistry::new();
    let trigger = spawn_trigger(&mut registry, "presser");
    let first = health(&mut registry);
    let second = health(&mut registry);
    let bystander = health(&mut registry);
    let mut data = DataRegistry::new();
    data.populate_level(
        vec![NamedReaction {
            name: "presser".into(),
            descriptor: ReactionDescriptor::Primitive(PrimitiveDescriptor {
                primitive: "applyDamage".into(),
                target: Some("@activators".into()),
                kind: None,
                tag: None,
                on_complete: None,
                args: serde_json::json!({"amount": 25}),
            }),
        }],
        Vec::new(),
        &[],
    );
    let table = TriggerBindingTable::build(&registry, &data, &SlotTable::new());

    for entrant in [first, second] {
        table.execute(
            trigger,
            TriggerEventEdge::Enter,
            &mut registry,
            &mut SlotTable::new(),
            &TriggerFireContext {
                fired_trigger: Some(trigger),
                activator: Some(entrant),
                occupancy: 2,
            },
        );
    }

    assert_eq!(
        registry
            .get_component::<HealthComponent>(first)
            .unwrap()
            .current,
        75.0
    );
    assert_eq!(
        registry
            .get_component::<HealthComponent>(second)
            .unwrap()
            .current,
        75.0
    );
    assert_eq!(
        registry
            .get_component::<HealthComponent>(bystander)
            .unwrap()
            .current,
        100.0
    );
}

#[test]
fn fired_trigger_target_disarms_only_its_plate_and_can_rearm_it() {
    let mut registry = EntityRegistry::new();
    let first = spawn_trigger(&mut registry, "shared");
    let second = spawn_trigger(&mut registry, "shared");
    let mut data = DataRegistry::new();
    data.populate_level(
        vec![NamedReaction {
            name: "shared".into(),
            descriptor: ReactionDescriptor::Sequence(vec![SequenceStep {
                id: SequenceTarget::FiredTrigger,
                primitive: "disarmTrigger".into(),
                args: serde_json::json!({}),
            }]),
        }],
        Vec::new(),
        &[],
    );
    let disarm = TriggerBindingTable::build(&registry, &data, &SlotTable::new());
    disarm.execute(
        first,
        TriggerEventEdge::Enter,
        &mut registry,
        &mut SlotTable::new(),
        &TriggerFireContext {
            fired_trigger: Some(first),
            ..Default::default()
        },
    );
    assert!(
        !registry
            .get_component::<TriggerVolumeComponent>(first)
            .unwrap()
            .armed
    );
    assert!(
        registry
            .get_component::<TriggerVolumeComponent>(second)
            .unwrap()
            .armed
    );

    let mut arm_data = DataRegistry::new();
    arm_data.populate_level(
        vec![NamedReaction {
            name: "shared".into(),
            descriptor: ReactionDescriptor::Sequence(vec![SequenceStep {
                id: SequenceTarget::FiredTrigger,
                primitive: "armTrigger".into(),
                args: serde_json::json!({}),
            }]),
        }],
        Vec::new(),
        &[],
    );
    let arm = TriggerBindingTable::build(&registry, &arm_data, &SlotTable::new());
    arm.execute(
        first,
        TriggerEventEdge::Enter,
        &mut registry,
        &mut SlotTable::new(),
        &TriggerFireContext {
            fired_trigger: Some(first),
            ..Default::default()
        },
    );
    assert!(
        registry
            .get_component::<TriggerVolumeComponent>(first)
            .unwrap()
            .armed
    );
}

#[test]
fn occupancy_input_records_enter_and_exit_effective_counts() {
    let mut registry = EntityRegistry::new();
    let trigger = spawn_trigger(&mut registry, "occupancy");
    let ctx = ScriptCtx::new();
    ctx.slot_table
        .borrow_mut()
        .insert(
            "trigger.occupancy".into(),
            SlotRecord::new(SlotSchema {
                slot_type: SlotType::Number,
                default: Some(SlotValue::Number(0.0)),
                range: None,
                persist: false,
                readonly: false,
                ownership: SlotOwnership::Mod,
                network: Default::default(),
                per_owner: false,
                accumulate: None,
            }),
        )
        .unwrap();
    let mut data = DataRegistry::new();
    data.populate_level(
        vec![primitive(
            "occupancy",
            "setState",
            None,
            serde_json::json!({
                "slot": "trigger.occupancy",
                "value": {"op":"input", "name":"@occupancy"}
            }),
            None,
        )],
        Vec::new(),
        &[],
    );
    let table = TriggerBindingTable::build_with_script_ctx(&registry, &data, &ctx);
    for count in [2, 1] {
        table.execute_with_script_ctx(
            trigger,
            TriggerEventEdge::Enter,
            &mut registry,
            &ctx,
            &TriggerFireContext {
                fired_trigger: Some(trigger),
                occupancy: count,
                ..Default::default()
            },
        );
        assert_eq!(
            ctx.slot_table
                .borrow()
                .get("trigger.occupancy")
                .unwrap()
                .value,
            Some(SlotValue::Number(count as f32))
        );
    }
}

#[test]
fn runtime_set_state_accumulates_in_trigger_enqueue_order() {
    let mut registry = EntityRegistry::new();
    let trigger = spawn_trigger(&mut registry, "increment");
    let ctx = ScriptCtx::new();
    ctx.slot_table
        .borrow_mut()
        .insert(
            "trigger.count".to_string(),
            SlotRecord::new(SlotSchema {
                slot_type: SlotType::Number,
                default: Some(SlotValue::Number(0.0)),
                range: Some(NumericRange {
                    min: 0.0,
                    max: 100.0,
                }),
                persist: false,
                readonly: false,
                ownership: SlotOwnership::Mod,
                network: Default::default(),
                per_owner: false,
                accumulate: None,
            }),
        )
        .unwrap();
    let mut data = DataRegistry::new();
    let increment = |name: &str| {
        primitive(
            name,
            "setState",
            None,
            serde_json::json!({
                "slot": "trigger.count",
                "value": {
                    "op": "add",
                    "a": { "op": "input", "name": "trigger.count" },
                    "b": { "op": "const", "value": 1.0 }
                }
            }),
            None,
        )
    };
    data.populate_level(
        vec![increment("increment"), increment("increment")],
        Vec::new(),
        &[],
    );
    let table = TriggerBindingTable::build_with_script_ctx(&registry, &data, &ctx);
    table.execute_with_script_ctx(
        trigger,
        TriggerEventEdge::Enter,
        &mut registry,
        &ctx,
        &TriggerFireContext::default(),
    );
    assert_eq!(
        ctx.slot_table
            .borrow()
            .get("trigger.count")
            .and_then(|record| record.value.as_ref()),
        Some(&SlotValue::Number(2.0)),
        "same-tick IR writes must evaluate and commit in trigger command order"
    );
}

#[test]
fn trigger_bind_rejects_crossing_dispatch_input_without_blocking_other_work() {
    let mut registry = EntityRegistry::new();
    let trigger = spawn_trigger(&mut registry, "mixed");
    let ctx = ScriptCtx::new();
    ctx.slot_table
        .borrow_mut()
        .insert_namespace(
            "trigger",
            vec![
                (
                    "direction".to_string(),
                    SlotRecord::new(SlotSchema {
                        slot_type: SlotType::Number,
                        default: Some(SlotValue::Number(7.0)),
                        range: None,
                        persist: false,
                        readonly: false,
                        ownership: SlotOwnership::Mod,
                        network: Default::default(),
                        per_owner: false,
                        accumulate: None,
                    }),
                ),
                (
                    "unrelated".to_string(),
                    SlotRecord::new(SlotSchema {
                        slot_type: SlotType::Number,
                        default: Some(SlotValue::Number(0.0)),
                        range: None,
                        persist: false,
                        readonly: false,
                        ownership: SlotOwnership::Mod,
                        network: Default::default(),
                        per_owner: false,
                        accumulate: None,
                    }),
                ),
            ],
        )
        .unwrap();
    let mut data = DataRegistry::new();
    data.populate_level(
        vec![
            primitive(
                "mixed",
                "setState",
                None,
                serde_json::json!({
                    "slot": "trigger.direction",
                    "value": {
                        "op": "select",
                        "cond": { "op": "input", "name": "@rising" },
                        "a": { "op": "const", "value": 1 },
                        "b": { "op": "const", "value": 0 }
                    }
                }),
                None,
            ),
            primitive(
                "mixed",
                "setState",
                None,
                serde_json::json!({ "slot": "trigger.unrelated", "value": 1 }),
                None,
            ),
        ],
        Vec::new(),
        &[],
    );

    let table = TriggerBindingTable::build_with_script_ctx(&registry, &data, &ctx);
    let execution = table.execute_with_script_ctx(
        trigger,
        TriggerEventEdge::Enter,
        &mut registry,
        &ctx,
        &TriggerFireContext::default(),
    );

    assert_eq!(execution.commands, vec![BoundTriggerCommandKind::StoreSlot]);
    assert_eq!(
        ctx.slot_table
            .borrow()
            .get("trigger.direction")
            .unwrap()
            .value,
        Some(SlotValue::Number(7.0)),
        "the trigger site has no @rising vocabulary, so its scoped write is rejected"
    );
    assert_eq!(
        ctx.slot_table
            .borrow()
            .get("trigger.unrelated")
            .unwrap()
            .value,
        Some(SlotValue::Number(1.0)),
        "one rejected reaction must not suppress unrelated trigger work"
    );
}

#[test]
#[should_panic(expected = "IR-valued trigger setState must execute with a ScriptCtx")]
fn execute_without_script_ctx_rejects_ir_set_state() {
    let mut registry = EntityRegistry::new();
    let trigger = spawn_trigger(&mut registry, "increment");
    let ctx = ScriptCtx::new();
    ctx.slot_table
        .borrow_mut()
        .insert(
            "trigger.count".to_string(),
            SlotRecord::new(SlotSchema {
                slot_type: SlotType::Number,
                default: Some(SlotValue::Number(0.0)),
                range: None,
                persist: false,
                readonly: false,
                ownership: SlotOwnership::Mod,
                network: Default::default(),
                per_owner: false,
                accumulate: None,
            }),
        )
        .unwrap();
    let mut data = DataRegistry::new();
    data.populate_level(
        vec![primitive(
            "increment",
            "setState",
            None,
            serde_json::json!({
                "slot": "trigger.count",
                "value": {
                    "op": "add",
                    "a": { "op": "input", "name": "trigger.count" },
                    "b": { "op": "const", "value": 1.0 }
                }
            }),
            None,
        )],
        Vec::new(),
        &[],
    );
    let table = TriggerBindingTable::build_with_script_ctx(&registry, &data, &ctx);
    let mut slots = writable_slots();

    table.execute(
        trigger,
        TriggerEventEdge::Enter,
        &mut registry,
        &mut slots,
        &TriggerFireContext::default(),
    );
}

#[test]
fn bind_partitions_direct_consequential_steps_and_retains_presentation() {
    let mut registry = EntityRegistry::new();
    let trigger = spawn_trigger(&mut registry, "open");
    let mut data = DataRegistry::new();
    data.populate_level(
        vec![
            primitive(
                "open",
                "moverStart",
                Some("door"),
                serde_json::json!({}),
                None,
            ),
            primitive(
                "open",
                "moverSetSpinRate",
                Some("door"),
                serde_json::json!({ "rate": -90.0 }),
                None,
            ),
            primitive(
                "open",
                "setState",
                None,
                serde_json::json!({ "slot": "trigger.flag", "value": 1 }),
                None,
            ),
            primitive(
                "open",
                "flashScreen",
                None,
                serde_json::json!({ "color": [1, 0, 0], "durationMs": 20 }),
                None,
            ),
        ],
        Vec::new(),
        &[],
    );

    let table = TriggerBindingTable::build(&registry, &data, &writable_slots());
    let binding = table
        .binding(trigger, TriggerEventEdge::Enter)
        .expect("named trigger event binds");
    assert_eq!(binding.commands.len(), 3);
    assert!(matches!(
        binding.commands[0],
        BoundTriggerCommand::Mover {
            command: MoverCommand::Start,
            ..
        }
    ));
    assert!(matches!(
        binding.commands[1],
        BoundTriggerCommand::Mover {
            command: MoverCommand::SetSpinRate(rate),
            ..
        } if (rate + 90.0).abs() < f32::EPSILON
    ));
    assert!(matches!(
        binding.commands[2],
        BoundTriggerCommand::StoreSlot { ref slot, .. } if slot == "trigger.flag"
    ));
    let residual = table
        .residual(binding.residual.expect("presentation is residual"))
        .unwrap();
    assert!(matches!(
        residual.steps(),
        [PrepartitionedReactionStep::Descriptor(_, _, ReactionDescriptor::Primitive(
            PrimitiveDescriptor { primitive, .. }
        ))] if primitive == "flashScreen"
    ));
}

// O50: a pre-wait consequential step stays in-tick; only the wait and the
// steps after it reach the residual. The E18-A guarantee the binder amendment
// delivers.
#[test]
fn pre_wait_consequential_step_binds_in_tick_and_wait_tail_goes_to_residual() {
    let mut registry = EntityRegistry::new();
    let trigger = spawn_trigger(&mut registry, "reveal");
    let door = registry.spawn(Transform::default());
    let post = registry.spawn(Transform::default());
    let mut data = DataRegistry::new();
    data.populate_level(
        vec![NamedReaction {
            name: "reveal".into(),
            descriptor: ReactionDescriptor::Sequence(vec![
                SequenceStep {
                    id: SequenceTarget::Entity(door),
                    primitive: "moverStart".into(),
                    args: serde_json::json!({}),
                },
                SequenceStep {
                    id: SequenceTarget::Wait,
                    primitive: "wait".into(),
                    args: serde_json::json!({ "durationMs": 500, "interruptible": false }),
                },
                SequenceStep {
                    id: SequenceTarget::Entity(post),
                    primitive: "setLightAnimation".into(),
                    args: serde_json::json!({}),
                },
            ]),
        }],
        Vec::new(),
        &[],
    );
    let table = TriggerBindingTable::build(&registry, &data, &SlotTable::new());
    let binding = table
        .binding(trigger, TriggerEventEdge::Enter)
        .expect("reveal binds its Enter edge");
    assert_eq!(
        binding.commands.len(),
        1,
        "moverStart runs in the fixed tick"
    );
    assert!(matches!(
        binding.commands[0],
        BoundTriggerCommand::Mover {
            command: MoverCommand::Start,
            ..
        }
    ));
    match table
        .residual(binding.residual.expect("wait tail is a residual"))
        .unwrap()
        .steps()
    {
        [PrepartitionedReactionStep::Descriptor(_, _, ReactionDescriptor::Sequence(tail))] => {
            assert_eq!(tail.len(), 2, "wait plus the post-wait step");
            assert!(matches!(tail[0].id, SequenceTarget::Wait));
            assert_eq!(tail[1].primitive, "setLightAnimation");
        }
        other => panic!("expected a single wait-tail residual, got {other:?}"),
    }
}

// O49: `[fire(alarm), wait, moverStart, fire(release)]` — the pre-wait `fire`
// lowers to a deferred event; the wait, `moverStart`, and the trailing `fire`
// all land in the residual in authored order; `moverStart` is NOT hoisted
// in-tick because it is past the wait.
#[test]
fn trigger_bound_wait_body_keeps_wait_and_tail_and_pre_wait_fire_defers() {
    let mut registry = EntityRegistry::new();
    let trigger = spawn_trigger(&mut registry, "reveal");
    let door = registry.spawn(Transform::default());
    let mut data = DataRegistry::new();
    data.populate_level(
        vec![
            NamedReaction {
                name: "reveal".into(),
                descriptor: ReactionDescriptor::Sequence(vec![
                    SequenceStep {
                        id: SequenceTarget::Fire,
                        primitive: "fire".into(),
                        args: serde_json::json!({ "event": "alarm" }),
                    },
                    SequenceStep {
                        id: SequenceTarget::Wait,
                        primitive: "wait".into(),
                        args: serde_json::json!({ "durationMs": 800, "interruptible": true }),
                    },
                    SequenceStep {
                        id: SequenceTarget::Entity(door),
                        primitive: "moverStart".into(),
                        args: serde_json::json!({}),
                    },
                    SequenceStep {
                        id: SequenceTarget::Fire,
                        primitive: "fire".into(),
                        args: serde_json::json!({ "event": "release" }),
                    },
                ]),
            },
            primitive("alarm", "flashScreen", None, serde_json::json!({}), None),
            primitive("release", "flashScreen", None, serde_json::json!({}), None),
        ],
        Vec::new(),
        &[],
    );
    let table = TriggerBindingTable::build(&registry, &data, &SlotTable::new());
    let binding = table
        .binding(trigger, TriggerEventEdge::Enter)
        .expect("reveal binds its Enter edge");
    assert!(
        binding.commands.is_empty(),
        "no post-wait consequential step is hoisted into the fixed tick",
    );
    match table
        .residual(binding.residual.expect("wait tail residual"))
        .unwrap()
        .steps()
    {
        [
            PrepartitionedReactionStep::DeferredEvent(alarm),
            PrepartitionedReactionStep::Descriptor(_, _, ReactionDescriptor::Sequence(tail)),
        ] => {
            assert_eq!(
                alarm.as_str(),
                "alarm",
                "the pre-wait fire lowers to a deferred event"
            );
            assert_eq!(tail.len(), 3);
            assert!(matches!(tail[0].id, SequenceTarget::Wait));
            assert_eq!(tail[1].primitive, "moverStart");
            assert!(matches!(tail[2].id, SequenceTarget::Fire));
        }
        other => panic!("unexpected residual composition: {other:?}"),
    }
}

// O46: a body whose FIRST step is the wait still binds its Enter edge — the
// wait and its tail make `steps` non-empty, so `bind_event` does not early-
// return, and no `bind_edge_only` is needed for the Enter edge.
#[test]
fn wait_first_body_still_binds_its_enter_edge() {
    let mut registry = EntityRegistry::new();
    let trigger = spawn_trigger(&mut registry, "reveal");
    let door = registry.spawn(Transform::default());
    let mut data = DataRegistry::new();
    data.populate_level(
        vec![NamedReaction {
            name: "reveal".into(),
            descriptor: ReactionDescriptor::Sequence(vec![
                SequenceStep {
                    id: SequenceTarget::Wait,
                    primitive: "wait".into(),
                    args: serde_json::json!({ "durationMs": 800, "interruptible": false }),
                },
                SequenceStep {
                    id: SequenceTarget::Entity(door),
                    primitive: "moverStart".into(),
                    args: serde_json::json!({}),
                },
            ]),
        }],
        Vec::new(),
        &[],
    );
    let table = TriggerBindingTable::build(&registry, &data, &SlotTable::new());
    let binding = table
        .binding(trigger, TriggerEventEdge::Enter)
        .expect("Enter edge binds even when the first step is the wait");
    assert!(binding.commands.is_empty());
    assert!(
        table
            .bound_edges()
            .contains(&(trigger, TriggerEventEdge::Enter))
    );
    match table
        .residual(binding.residual.expect("wait tail residual"))
        .unwrap()
        .steps()
    {
        [PrepartitionedReactionStep::Descriptor(_, _, ReactionDescriptor::Sequence(tail))] => {
            assert_eq!(tail.len(), 2);
            assert!(matches!(tail[0].id, SequenceTarget::Wait));
            assert_eq!(tail[1].primitive, "moverStart");
        }
        other => panic!("expected a single wait-tail residual, got {other:?}"),
    }
}

#[test]
fn bind_defers_consequential_on_complete_as_later_residual_hop() {
    let mut registry = EntityRegistry::new();
    let trigger = spawn_trigger(&mut registry, "open");
    let mut data = DataRegistry::new();
    data.populate_level(
        vec![
            primitive(
                "open",
                "moverStart",
                Some("door"),
                serde_json::json!({}),
                Some("after_open"),
            ),
            primitive(
                "after_open",
                "applyDamage",
                Some("enemy"),
                serde_json::json!({ "amount": 5 }),
                None,
            ),
        ],
        Vec::new(),
        &[],
    );

    let table = TriggerBindingTable::build(&registry, &data, &writable_slots());
    let binding = table
        .binding(trigger, TriggerEventEdge::Enter)
        .expect("named trigger event binds");
    assert_eq!(binding.commands.len(), 1);
    let residual = table
        .residual(binding.residual.expect("chain is residual"))
        .unwrap();
    assert!(matches!(
        residual.steps(),
        [PrepartitionedReactionStep::DeferredEvent(event_name)] if event_name == "after_open"
    ));
}

#[test]
fn bind_rejects_readonly_set_state_at_install() {
    let mut registry = EntityRegistry::new();
    let trigger = spawn_trigger(&mut registry, "set_readonly");
    let mut data = DataRegistry::new();
    data.populate_level(
        vec![primitive(
            "set_readonly",
            "setState",
            None,
            serde_json::json!({ "slot": "player.health", "value": 1 }),
            None,
        )],
        Vec::new(),
        &[],
    );

    let table = TriggerBindingTable::build(&registry, &data, &SlotTable::new());
    assert!(
        table.binding(trigger, TriggerEventEdge::Enter).is_none(),
        "rejected work must not bind the trigger edge"
    );
}

// Regression: tagged setState bypassed the normal system-only dispatch contract.
#[test]
fn bind_rejects_tagged_set_state_without_an_in_tick_write() {
    let mut registry = EntityRegistry::new();
    let trigger = spawn_trigger(&mut registry, "set_tagged");
    let mut data = DataRegistry::new();
    data.populate_level(
        vec![primitive(
            "set_tagged",
            "setState",
            Some("not-a-system-target"),
            serde_json::json!({ "slot": "trigger.flag", "value": 1 }),
            None,
        )],
        Vec::new(),
        &[],
    );
    let mut slots = writable_slots();

    let table = TriggerBindingTable::build(&registry, &data, &slots);
    assert!(
        table.binding(trigger, TriggerEventEdge::Enter).is_none(),
        "rejected work must not bind the trigger edge"
    );

    assert!(
        table
            .execute(
                trigger,
                TriggerEventEdge::Enter,
                &mut registry,
                &mut slots,
                &TriggerFireContext::default(),
            )
            .residual()
            .is_none(),
        "a rejected tagged setState must not leave residual work"
    );
    assert_eq!(
        slots
            .get("trigger.flag")
            .and_then(|record| record.value.as_ref()),
        Some(&SlotValue::Number(0.0)),
        "tagged setState must not perform an in-tick write"
    );
}

// Regression: a sentinel target bypassed the tag-only validation and was
// discarded by bind_command while still allowing the system write.
#[test]
fn bind_rejects_sentinel_targeted_set_state_without_an_in_tick_write() {
    let mut registry = EntityRegistry::new();
    let trigger = spawn_trigger(&mut registry, "set_sentinel_targeted");
    let mut data = DataRegistry::new();
    data.populate_level(
        vec![NamedReaction {
            name: "set_sentinel_targeted".to_string(),
            descriptor: ReactionDescriptor::Primitive(PrimitiveDescriptor {
                primitive: "setState".to_string(),
                target: Some("@activators".to_string()),
                kind: None,
                tag: None,
                on_complete: None,
                args: serde_json::json!({ "slot": "trigger.flag", "value": 1 }),
            }),
        }],
        Vec::new(),
        &[],
    );
    let mut slots = writable_slots();

    let table = TriggerBindingTable::build(&registry, &data, &slots);
    assert!(
        table.binding(trigger, TriggerEventEdge::Enter).is_none(),
        "rejected work must not bind the trigger edge"
    );

    table.execute(
        trigger,
        TriggerEventEdge::Enter,
        &mut registry,
        &mut slots,
        &TriggerFireContext::default(),
    );
    assert_eq!(
        slots
            .get("trigger.flag")
            .and_then(|record| record.value.as_ref()),
        Some(&SlotValue::Number(0.0)),
        "sentinel-targeted setState must not perform an in-tick write"
    );
}

// Regression: sequence setState silently discarded its entity target and
// performed a system write despite the malformed authoring shape.
#[test]
fn bind_rejects_entity_targeted_sequence_set_state_without_an_in_tick_write() {
    let mut registry = EntityRegistry::new();
    let trigger = spawn_trigger(&mut registry, "set_sequence_tagged");
    let mut data = DataRegistry::new();
    data.populate_level(
        vec![NamedReaction {
            name: "set_sequence_tagged".to_string(),
            descriptor: ReactionDescriptor::Sequence(vec![SequenceStep {
                id: trigger.into(),
                primitive: "setState".to_string(),
                args: serde_json::json!({ "slot": "trigger.flag", "value": 1 }),
            }]),
        }],
        Vec::new(),
        &[],
    );
    let mut slots = writable_slots();

    let table = TriggerBindingTable::build(&registry, &data, &slots);
    assert!(
        table.binding(trigger, TriggerEventEdge::Enter).is_none(),
        "rejected work must not bind the trigger edge"
    );

    assert!(
        table
            .execute(
                trigger,
                TriggerEventEdge::Enter,
                &mut registry,
                &mut slots,
                &TriggerFireContext::default(),
            )
            .residual()
            .is_none(),
        "a rejected sequence setState must not leave residual work"
    );
    assert_eq!(
        slots
            .get("trigger.flag")
            .and_then(|record| record.value.as_ref()),
        Some(&SlotValue::Number(0.0)),
        "entity-targeted sequence setState must not perform an in-tick write"
    );
}

// Regression: the binding retained a Progress descriptor in its residual, which
// fired the progress target on the app drain with zero kills — and then again
// when ProgressTracker hit the real threshold.
#[test]
fn bind_leaves_no_residual_for_a_progress_reaction() {
    let mut registry = EntityRegistry::new();
    let trigger = spawn_trigger(&mut registry, "wave_started");
    let mut data = DataRegistry::new();
    data.populate_level(
        vec![
            NamedReaction {
                name: "wave_started".to_string(),
                descriptor: ReactionDescriptor::Progress(ProgressDescriptor {
                    tag: "wave1".to_string(),
                    at: 1.0,
                    fire: "open_vault".to_string(),
                }),
            },
            primitive(
                "open_vault",
                "moverStart",
                Some("vault"),
                serde_json::json!({}),
                None,
            ),
        ],
        Vec::new(),
        &[],
    );

    let table = TriggerBindingTable::build(&registry, &data, &writable_slots());
    assert!(
        table.binding(trigger, TriggerEventEdge::Enter).is_none(),
        "ProgressTracker owns the progress target, so it must not bind the trigger edge"
    );
}

// A reaction whose only content is an `onComplete`-style hop must still bind,
// and firing the trigger must reach the chained reaction through the
// dispatch entry points the app's residual drain uses. Two shapes carry nothing but hops: a sequence of one `fire`
// step, and a consequential primitive that fails to bind but names an
// `onComplete`.
#[test]
fn residual_hop_only_reactions_bind_and_run_the_chained_reaction() {
    use postretro_scripting_core::reaction_dispatch::{
        ResidualOrigin, dispatch_deferred_named_events_with_sequences,
        fire_prepartitioned_reactions_with_sequences,
    };
    use postretro_scripting_core::reaction_registry::{
        ReactionPrimitiveRegistry, SystemReactionRegistry,
    };
    use postretro_scripting_core::sequence::SequencedPrimitiveRegistry;
    use std::sync::{Arc, Mutex};

    let fire_only = NamedReaction {
        name: "relay".into(),
        descriptor: ReactionDescriptor::Sequence(vec![SequenceStep {
            id: SequenceTarget::Fire,
            primitive: "fire".into(),
            args: serde_json::json!({ "event": "target" }),
        }]),
    };
    // Unknown slot: the command is rejected at bind, leaving only the hop.
    let rejected_with_hop = primitive(
        "relay",
        "setState",
        None,
        serde_json::json!({ "slot": "no.such.slot", "value": 1 }),
        Some("target"),
    );
    for (shape, relay) in [
        ("fire-only sequence", fire_only),
        ("rejected primitive with onComplete", rejected_with_hop),
    ] {
        let mut registry = EntityRegistry::new();
        let trigger = spawn_trigger(&mut registry, "relay");
        let mut data = DataRegistry::new();
        data.populate_level(
            vec![
                relay,
                primitive(
                    "target",
                    "record",
                    None,
                    serde_json::json!({ "label": "target" }),
                    None,
                ),
            ],
            Vec::new(),
            &[],
        );

        let table = TriggerBindingTable::build(&registry, &data, &writable_slots());
        let binding = table
            .binding(trigger, TriggerEventEdge::Enter)
            .unwrap_or_else(|| panic!("{shape}: a hop-only reaction still binds its edge"));
        assert!(binding.commands.is_empty(), "{shape}: no direct commands");
        let residual = table
            .residual(
                binding
                    .residual
                    .unwrap_or_else(|| panic!("{shape}: hop survives as residual")),
            )
            .unwrap();

        let calls = Arc::new(Mutex::new(Vec::new()));
        let calls_for_handler = Arc::clone(&calls);
        let mut system_registry = SystemReactionRegistry::new();
        system_registry.register("record", move |args, _queue| {
            calls_for_handler.lock().unwrap().push(
                args.get("label")
                    .and_then(serde_json::Value::as_str)
                    .unwrap()
                    .to_string(),
            );
            Ok(())
        });
        let sequence_registry = SequencedPrimitiveRegistry::new();
        let reaction_registry = ReactionPrimitiveRegistry::new();
        let script_ctx = ScriptCtx::new();
        let follow_ups = fire_prepartitioned_reactions_with_sequences(
            residual.steps(),
            &sequence_registry,
            &reaction_registry,
            &system_registry,
            &script_ctx,
            ResidualOrigin::TriggerBinding,
        );
        assert_eq!(follow_ups, vec!["target".to_string()], "{shape}: follow-up");
        dispatch_deferred_named_events_with_sequences(
            follow_ups,
            &data,
            &sequence_registry,
            &reaction_registry,
            &system_registry,
            &script_ctx,
        );
        assert_eq!(
            calls.lock().unwrap().as_slice(),
            ["target".to_string()],
            "{shape}: chained reaction ran"
        );
    }
}
