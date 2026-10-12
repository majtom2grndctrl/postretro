//! Real producer outputs enter the same source-origin drain used by redraw.
use super::*;
use glam::Vec3;
use postretro_entities::components::{
    brain::BrainComponent, health::HealthComponent, light::LightComponent,
};
use postretro_entities::{
    NamedReaction, PrimitiveDescriptor, ReactionDescriptor, Recipients, SequenceStep,
    SequenceTarget, Transform,
};
use serde_json::json;
use std::collections::BTreeMap;

fn fire(name: &str) -> NamedReaction {
    NamedReaction {
        name: name.into(),
        descriptor: ReactionDescriptor::Sequence(vec![SequenceStep {
            id: SequenceTarget::Fire,
            primitive: "fire".into(),
            args: json!({"event":format!("{name}.follow")}),
        }]),
    }
}
fn health(hp: f32) -> HealthComponent {
    serde_json::from_value(json!({"max":100.0,"current":hp,"hitbox":null})).unwrap()
}
fn movement_descriptor() -> postretro_foundation::PlayerMovementDescriptor {
    use postretro_foundation::{
        AirParams, CapsuleParams, FallParams, GroundParams, PlayerMovementDescriptor, SpeedParams,
    };
    PlayerMovementDescriptor {
        sounds: None,
        knockback: Default::default(),
        capsule: CapsuleParams {
            radius: 0.4,
            half_height: 0.8,
            eye_height: 0.5,
        },
        ground: GroundParams {
            speed: SpeedParams {
                walk: 7.0,
                run: 11.0,
                crouch: 3.0,
            },
            accel: 10.0,
            step_height: 0.3,
            max_slope: 45.0,
        },
        air: AirParams {
            forward_steer: 0.0,
            accel: 0.7,
            max_control_speed: 0.5,
            bunny_hop: false,
            jumps: 1,
            jump_velocity: 6.0,
            jump_ceiling: 0.0,
        },
        fall: FallParams {
            terminal_velocity: 40.0,
        },
        stuck_stop_enabled: PlayerMovementDescriptor::DEFAULT_STUCK_STOP_ENABLED,
        stuck_stop_threshold: PlayerMovementDescriptor::DEFAULT_STUCK_STOP_THRESHOLD,
        dash: None,
        forgiveness: None,
        crouch: None,
        slide: None,
        swim: None,
        view_feel: None,
    }
}

#[test]
fn actual_movement_ai_mover_and_death_producers_select_frame_origins() {
    use postretro_entities::data_descriptors::{
        BehaviorActivityDescriptor, BehaviorGraphDescriptor, BehaviorGraphEnvelope, GuardedRow,
        MotionVerb,
    };
    use postretro_foundation::PlayerMovementComponent;
    use postretro_foundation::ir::{IrNode, IrValue};
    use postretro_sim::ai_host::{AiTickInputs, SimAiHost};
    let mut app = crate::startup::lifecycle::tests::test_app();
    let ctx = crate::startup::lifecycle::tests::script_ctx(&app);
    let session = app.session.as_mut().unwrap();
    postretro_sim::scripting_systems::reaction_scheduler::register_reaction_control_primitives(
        &mut session.scripting.sequence_registry,
        session.scripting.scheduler.clone(),
    );
    ctx.presentation_networked_predicate
        .set(Some(netcode::is_host_replicated_descriptor_entity));
    ctx.presentation_override_validator
        .set(Some(netcode::validate_presentation_override));
    postretro_lighting::script_primitives::register_sequenced_light_primitives(
        &mut session.scripting.sequence_registry,
        ctx.clone(),
    );
    let mut registry = ctx.registry.borrow_mut();
    let lamp = registry.spawn(Transform::default());
    let light: LightComponent = serde_json::from_value(json!({
        "origin":[0.0,0.0,0.0],"lightType":"Point","intensity":1.0,"color":[1.0,1.0,1.0],
        "falloffModel":"InverseSquared","falloffRange":10.0,"coneAngleInner":null,
        "coneAngleOuter":null,"coneDirection":null,"isDynamic":true
    }))
    .unwrap();
    registry.set_component(lamp, light).unwrap();
    let pawn = registry.spawn(Transform {
        position: Vec3::new(0.0, 1.21, 0.0),
        ..Default::default()
    });
    registry
        .set_component(
            pawn,
            PlayerMovementComponent::from_descriptor(&movement_descriptor()),
        )
        .unwrap();
    registry.set_component(pawn, health(100.0)).unwrap();
    let floor = crate::collision::CollisionWorld::from_triangles_for_test(
        vec![
            Vec3::new(-50.0, 0.0, -50.0),
            Vec3::new(50.0, 0.0, -50.0),
            Vec3::new(50.0, 0.0, 50.0),
            Vec3::new(-50.0, 0.0, 50.0),
        ],
        vec![[0, 2, 1], [0, 3, 2]],
    );
    let idle = postretro_physics::movement::MovementInput {
        wish_dir: glam::Vec2::ZERO,
        jump_pressed: false,
        dash_pressed: false,
        running: false,
        crouch_intent: false,
        facing_yaw: 0.0,
        use_pressed: false,
        drop_pressed: false,
    };
    let dt = 1.0 / 60.0;
    let _ = postretro_sim::sim::run_host_movement_tick(
        &mut registry,
        &floor,
        postretro_collision::EnvironmentView::level_only(-20.0),
        &[(pawn, idle.clone())],
        dt,
    );
    let input = postretro_physics::movement::MovementInput {
        jump_pressed: true,
        ..idle
    };
    let movement_ticks = postretro_sim::sim::run_host_movement_tick(
        &mut registry,
        &floor,
        postretro_collision::EnvironmentView::level_only(-20.0),
        &[(pawn, input)],
        dt,
    );
    let mut movement = Vec::new();
    for (pawn, events) in movement_ticks {
        let mut addresses = Vec::new();
        events.append_named_events(&mut addresses);
        movement.extend(addresses.into_iter().map(|address| MovementEmission {
            address,
            emitter: postretro_sim::emission::entity_emitter(&registry, pawn),
        }));
    }
    assert!(movement.iter().any(|event| event.address == "jumped"));
    let activity = |on_enter| BehaviorActivityDescriptor {
        sound: None,
        animation: None,
        motion: Some(MotionVerb::Hold),
        action: None,
        on_enter,
        layers: BTreeMap::new(),
    };
    let edge = |to: &str| GuardedRow {
        to: to.into(),
        when: IrNode::Const {
            value: IrValue::Bool(true),
        },
    };
    let graph = BehaviorGraphDescriptor {
        knockback: Default::default(),
        envelope: BehaviorGraphEnvelope {
            initial: "a".into(),
            activities: BTreeMap::from([
                ("a".into(), activity(None)),
                ("b".into(), activity(None)),
                ("c".into(), activity(Some("ai.entered".into()))),
            ]),
            transitions: BTreeMap::from([
                ("a".into(), vec![edge("b")]),
                ("b".into(), vec![edge("c")]),
            ]),
        },
        candidate_filter: None,
        retaliation: None,
        patrol: None,
        attacks: Default::default(),
        engagement_radius: None,
        move_speed: 0.0,
    };
    let enemy = registry.spawn(Transform::default());
    registry
        .set_component(enemy, BrainComponent::from_graph(&graph))
        .unwrap();
    registry
        .set_component(
            enemy,
            postretro_entities::components::agent::AgentComponent::new(0.35, 1.8, 0.4, 0.0),
        )
        .unwrap();
    registry.set_component(enemy, health(100.0)).unwrap();
    let mut runtime = postretro_ai::AiRuntime::new();
    let factions = postretro_entities::FactionRegistry::default();
    let sentiment = std::cell::RefCell::new(postretro_entities::FactionSentimentState::default());
    let mut callback = |_: &mut postretro_entities::EntityRegistry| {};
    let mut host = SimAiHost::new(&mut callback);
    let inputs = || AiTickInputs {
        nav_graph: None,
        collision_world: None,
        descriptors: &[],
        descriptor_generation: 0,
        factions: &factions,
        faction_sentiment: &sentiment,
    };
    let _ =
        postretro_ai::run_ai_tick_with_host(&mut registry, &mut runtime, dt, inputs(), &mut host);
    let ai =
        postretro_ai::run_ai_tick_with_host(&mut registry, &mut runtime, dt, inputs(), &mut host)
            .events;
    assert!(
        ai.iter()
            .any(|event| event.address.as_deref() == Some("ai.entered"))
    );
    let mover_id = registry.spawn(Transform {
        position: Vec3::new(4.0, 0.0, 0.0),
        ..Default::default()
    });
    let mut mover = postretro_entities::KinematicMoverComponent::new(
        17,
        postretro_entities::KinematicMoverConfig {
            waypoints: vec![Vec3::ZERO, Vec3::X],
            waypoint_names: vec!["closed".into(), "open".into()],
            speed_mps: 1.0,
            wait_ms: 0.0,
            mode: postretro_entities::KinematicMoverMode::PingPong,
            started: true,
            spin_axis: Vec3::ZERO,
            initial_spin_rate_rad_s: 0.0,
            spin_accel_rad_s2: 0.0,
            carry_yaw: false,
        },
    );
    mover.open_event = Some("door.open".into());
    registry.set_component(mover_id, mover).unwrap();
    let mut collector = crate::runtime_movers::KinematicMoverRenderCollector::new();
    let mut scene = crate::sound_events::AnchorScene {
        registry: &registry,
        world: None,
        movers: &mut collector,
    };
    let movers: Vec<_> = crate::sound_events::resolve_mover_edges(
        &[(crate::kinematic_mover::MoverEventKind::Opened, 17)],
        &mut scene,
    )
    .into_iter()
    .map(|edge| {
        let emitter = edge.reaction_emitter();
        (edge.address.unwrap(), emitter)
    })
    .collect();
    assert_eq!(movers.len(), 1);
    registry.set_component(pawn, health(0.0)).unwrap();
    drop(registry);
    let deaths = postretro_sim::sim::run_death_sweep(&ctx.registry);
    assert_eq!(deaths, vec!["playerDied"]);
    let names: Vec<_> = movement
        .iter()
        .map(|event| event.address)
        .chain(["ai.entered", "door.open", "playerDied"])
        .collect();
    ctx.data_registry.borrow_mut().populate_level(
        names.iter().map(|name| fire(name)).collect(),
        vec![],
        &[],
    );
    let follow = drain_frame_named_events(
        NamedFrameEvents {
            movement: &movement,
            ai: &ai,
            observer: &[],
            weapon: &[],
            movers: &movers,
            deaths: &deaths,
        },
        &session.scripting,
        &ctx,
    );
    for (name, origin) in follow {
        let expected = if movement
            .iter()
            .any(|event| format!("{}.follow", event.address) == name)
        {
            FireOrigin::EveryMachine
        } else {
            FireOrigin::HostOnly(postretro_entities::FireSubject::None)
        };
        assert_eq!(origin, expected, "{name}");
    }
    assert!(ctx.system_commands.take_ordered().is_empty());
    // Each actual producer enters the production drain independently. A local
    // presentation target records only the host-only rows of the source table.
    let changes = names
        .iter()
        .map(|name| NamedReaction {
            name: (*name).into(),
            descriptor: ReactionDescriptor::Sequence(vec![SequenceStep {
                id: lamp.into(),
                primitive: "setLightAnimation".into(),
                args: json!({"periodMs":5000.0,"brightness":[0.0,1.0]}),
            }]),
        })
        .collect();
    ctx.data_registry
        .borrow_mut()
        .populate_level(changes, vec![], &[]);
    for (row, expected) in [(0, false), (1, true), (2, true), (3, true)] {
        ctx.presentation_overrides.borrow_mut().clear();
        let _ = drain_frame_named_events(
            NamedFrameEvents {
                movement: if row == 0 { &movement } else { &[] },
                ai: if row == 1 { &ai } else { &[] },
                observer: &[],
                weapon: &[],
                movers: if row == 2 { &movers } else { &[] },
                deaths: if row == 3 { &deaths } else { &[] },
            },
            &session.scripting,
            &ctx,
        );
        assert_eq!(
            ctx.presentation_overrides
                .borrow()
                .records
                .contains_key(&(lamp, postretro_entities::ComponentKind::Light)),
            expected,
            "source row {row}"
        );
    }
}

#[test]
fn observer_delivery_and_weapon_aliases_remain_every_machine_through_followups() {
    use postretro_net::wire;
    let mut app = crate::startup::lifecycle::tests::test_app();
    let ctx = crate::startup::lifecycle::tests::script_ctx(&app);
    let session = app.session.as_mut().unwrap();
    postretro_sim::scripting_systems::reaction_scheduler::register_reaction_control_primitives(
        &mut session.scripting.sequence_registry,
        session.scripting.scheduler.clone(),
    );
    postretro_sim::scripting_systems::system_reactions::register_system_reaction_primitives(
        &mut session.scripting.system_registry,
    );
    ctx.presentation_networked_predicate
        .set(Some(netcode::is_host_replicated_descriptor_entity));
    ctx.presentation_override_validator
        .set(Some(netcode::validate_presentation_override));
    postretro_lighting::script_primitives::register_sequenced_light_primitives(
        &mut session.scripting.sequence_registry,
        ctx.clone(),
    );
    let lamp = ctx.registry.borrow_mut().spawn(Transform::default());
    let light: LightComponent = serde_json::from_value(json!({
        "origin":[0.0,0.0,0.0],"lightType":"Point","intensity":1.0,"color":[1.0,1.0,1.0],
        "falloffModel":"InverseSquared","falloffRange":10.0,"coneAngleInner":null,
        "coneAngleOuter":null,"coneDirection":null,"isDynamic":true
    }))
    .unwrap();
    ctx.registry
        .borrow_mut()
        .set_component(lamp, light)
        .unwrap();
    let cue = netcode::weapon_cues::materialize_observer_weapon_cue(
        wire::WeaponCue {
            shot_id: wire::WireShotId {
                pawn: 1,
                start_tick: 1,
                lane: 0,
                ordinal: 0,
            },
            kind: wire::WeaponCueKind::Impact,
            sound: None,
            additional_sound: None,
            alias: Some("observer.alias".into()),
            anchor: wire::WeaponCueAnchor::Entity {
                entity: wire::NetworkId(1),
                origin: [3.0, 4.0, 5.0],
            },
        },
        &netcode::ClientReplication::default(),
        &ctx.registry.borrow(),
    )
    .unwrap();
    let mut action = postretro_foundation::data_descriptors::WeaponActivationDescriptor::single(
        postretro_foundation::data_descriptors::ActivationTrigger::Press,
        1.0,
    );
    action.emits = Some(postretro_foundation::data_descriptors::ActivationEmits {
        activate: Some("weapon.alias".into()),
        impact: None,
    });
    let weapon = WeaponEmission {
        sounds: None,
        action: Some(std::sync::Arc::new(action.clone())),
        shot_id: None,
        address: "activate",
        emitter: Emitter::Contacts(vec![postretro_entities::ImpactContact::new(
            Vec3::ZERO,
            Vec3::ZERO,
            None,
        )]),
        weapon: None,
    };
    // AI's committed shot cue uses the same frozen action alias as a local shot.
    let graph:postretro_entities::BehaviorGraphDescriptor=serde_json::from_value(json!({"initial":"idle","activities":{"idle":{"motion":"hold"}},"transitions":{},"moveSpeed":0.0})).unwrap();
    let ai = AiEmission {
        sounds: None,
        action: Some(std::sync::Arc::new(action)),
        shot_id: Some(postretro_foundation::ShotId::from_parts(
            1,
            1,
            postretro_foundation::ActivationLane::Primary,
            0,
        )),
        address: None,
        emitter: Emitter::Contacts(vec![postretro_entities::ImpactContact::new(
            Vec3::ZERO,
            Vec3::ZERO,
            None,
        )]),
        cue: postretro_entities::AiCue::Attack {
            graph: std::sync::Arc::new(graph),
            attack: "shot".into(),
        },
    };
    let mut reactions = Vec::new();
    for name in ["impact", "observer.alias", "activate", "weapon.alias"] {
        let mut reaction = fire(name);
        let ReactionDescriptor::Sequence(steps) = &mut reaction.descriptor else {
            unreachable!()
        };
        steps.insert(
            0,
            SequenceStep {
                id: lamp.into(),
                primitive: "setLightAnimation".into(),
                args: json!({"periodMs":5000.0,"brightness":[0.0,1.0]}),
            },
        );
        reactions.push(reaction);
        reactions.push(NamedReaction {
            name: format!("{name}.follow"),
            descriptor: ReactionDescriptor::Primitive(PrimitiveDescriptor {
                primitive: "playSound".into(),
                target: None,
                kind: None,
                tag: None,
                on_complete: None,
                args: json!({"sound":name}),
            }),
        });
    }
    ctx.data_registry
        .borrow_mut()
        .populate_level(reactions, vec![], &[]);
    let follow = drain_frame_named_events(
        NamedFrameEvents {
            movement: &[],
            ai: &[ai],
            observer: &[cue],
            weapon: &[weapon],
            movers: &[],
            deaths: &[],
        },
        &session.scripting,
        &ctx,
    );
    assert_eq!(follow.len(), 6);
    assert!(
        ctx.presentation_overrides.borrow().records.is_empty(),
        "every-machine AI activation, observer and weapon writes stay local"
    );
    assert!(
        follow
            .iter()
            .all(|(_, origin)| *origin == FireOrigin::EveryMachine)
    );
    postretro_scripting_core::reaction_dispatch::dispatch_deferred_named_events_with_sequences(
        follow,
        &ctx.data_registry.borrow(),
        &session.scripting.sequence_registry,
        &session.scripting.reaction_registry,
        &session.scripting.system_registry,
        &ctx,
    );
    let queued = ctx.system_commands.take_ordered();
    assert_eq!(queued.len(), 6);
    assert!(
        queued
            .iter()
            .all(|(recipients, _)| *recipients == Recipients::Local)
    );
}
