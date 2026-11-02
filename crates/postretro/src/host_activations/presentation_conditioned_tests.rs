//! Production transport, installation, residual dispatch and CPU presentation proof.
include!("presentation_conditioned_fixture.rs");

fn client_entity(peer: &Peer, network_id: wire::NetworkId) -> EntityId {
    peer.replication
        .entity_network_ids()
        .into_iter()
        .find_map(|(id, nid)| (nid == network_id).then_some(id))
        .expect("received entity mapping")
}
fn animation(period: f32, finite: bool) -> Value {
    json!({"periodMs":period,"playCount":if finite {Some(1)}else{None},"brightness":[0.2,1.0],"startActive":true})
}
fn install_changes(h: &mut Harness) {
    let ctx = script_ctx(&h.host);
    let mut reactions = vec![
        primitive("plate", "playSound", None, json!({"sound":"alarm"})),
        primitive(
            "plate",
            "flashScreen",
            None,
            json!({"color":[1,1,1,1],"durationMs":20}),
        ),
    ];
    for tag in ["prl", "light_only", "lit_prop"] {
        reactions.push(primitive(
            "changes",
            "setLightAnimation",
            Some(tag),
            animation(5000.0, true),
        ));
    }
    reactions.push(primitive(
        "changes",
        "setLightAnimation",
        Some("loop"),
        animation(1000.0, false),
    ));
    for (verb, args) in [
        ("setFogDensity", json!({"density":0.7})),
        ("setFogGlow", json!({"glow":0.2})),
        ("setFogEdgeSoftness", json!({"edgeSoftness":0.3})),
        ("setFogFalloff", json!({"falloff":1.5})),
        (
            "setFogParams",
            json!({"tint":[0.3,0.4,0.5],"saturation":0.6,"minBrightness":0.1,"lightRange":2.0}),
        ),
    ] {
        reactions.push(primitive("changes", verb, Some("fog"), args));
    }
    let fog = tagged(&ctx, "fog");
    reactions.push(sequence(
        "changes",
        fog,
        "setFogAnimation",
        json!({"periodMs":5000,"playCount":1,"density":[0.7,0.1]}),
    ));
    for tag in ["billboard_emitter", "emitter_only", "lit_prop"] {
        reactions.push(primitive(
            "changes",
            "setEmitterRate",
            Some(tag),
            json!({"rate":0}),
        ));
        reactions.push(primitive(
            "changes",
            "setSpinRate",
            Some(tag),
            json!({"rate":1}),
        ));
        reactions.push(primitive(
            "changes",
            "setSpinRate",
            Some(tag),
            json!({"animation":{"duration":5.0,"rate_curve":[0.0,2.0]}}),
        ));
    }
    reactions.push(primitive(
        "changes",
        "setAnimationState",
        Some("mesh_prop"),
        json!({"state":"attack"}),
    ));
    h.install_reactions(reactions);
}

// Regression: a wait landing spawned a light after the host's final tick enrollment,
// so its finite animation started a later frame than the captured client start.
#[test]
fn conditioned_wait_spawned_light_starts_at_capture_without_another_fixed_tick() {
    use postretro_entities::{GroupKind, GroupTarget, components::spawner::SpawnerComponent};
    use postretro_sim::spawner::{SpawnContext, register_sequenced_spawner_primitives};

    let mut h = Harness::new();
    h.advance(40);
    h.tick = 600;
    h.clock = 10.0;
    let ctx = script_ctx(&h.host);
    let mut descriptor = descriptors()
        .into_iter()
        .find(|descriptor| descriptor.canonical_name.as_deref() == Some("lit_prop"))
        .unwrap();
    descriptor.canonical_name = Some("late_lit_npc".into());
    descriptor.emitter = None;
    descriptor.behavior =
        crate::scripting::builtins::data_archetype_test_fixtures::behavior_enemy_descriptor(
            "late_lit_npc",
        )
        .behavior;
    for app in std::iter::once(&h.host).chain(h.peers.iter().map(|peer| &peer.app)) {
        script_ctx(app)
            .data_registry
            .borrow_mut()
            .entities
            .push(descriptor.clone());
    }
    let spawn_context = SpawnContext::default();
    spawn_context.replace_level_data(
        [("late_lit_npc".into(), descriptor)].into_iter().collect(),
        None,
    );
    let spawner = {
        let mut registry = ctx.registry.borrow_mut();
        let spawner = registry.spawn(Transform::default());
        registry
            .set_component(
                spawner,
                SpawnerComponent {
                    archetype_name: "late_lit_npc".into(),
                    count: 1,
                    spawned_tags: vec!["late_light".into()],
                    resolved: true,
                },
            )
            .unwrap();
        spawner
    };
    register_sequenced_spawner_primitives(
        &mut h.host.session.as_mut().unwrap().scripting.sequence_registry,
        ctx.clone(),
        spawn_context,
    );
    h.install_reactions(vec![NamedReaction {
        name: "plate".into(),
        descriptor: ReactionDescriptor::Sequence(vec![
            SequenceStep {
                id: SequenceTarget::Wait,
                primitive: "wait".into(),
                args: json!({"durationMs":30}),
            },
            SequenceStep {
                id: SequenceTarget::Entity(spawner),
                primitive: "spawnFromSpawner".into(),
                args: json!({}),
            },
            SequenceStep {
                id: SequenceTarget::Group(GroupTarget {
                    kind: GroupKind::Npc,
                    tag: Some("late_light".into()),
                }),
                primitive: "setLightAnimation".into(),
                args: animation(1000.0, true),
            },
        ]),
    }]);
    h.fire_plate(100.0, &[0]);
    let session = h.host.session.as_mut().unwrap();
    assert_eq!(session.scripting.scheduler.pending_len(), 1);
    for _ in 0..3 {
        session.scripting.scheduler.begin_frame();
        session.scripting.scheduler.evaluate(&[]);
    }
    // The final fixed tick sees no spawned light. Only the frame-end landing
    // below creates it, exactly as in the production frame loop.
    session
        .light_bridge
        .absorb_dynamic_lights(&ctx.registry.borrow());
    let enrolled_before = session.light_bridge.light_count();
    session.scripting.scheduler.drain_landings(
        &ctx.data_registry.borrow(),
        &session.scripting.sequence_registry,
        &session.scripting.reaction_registry,
        &session.scripting.system_registry,
        &ctx,
    );
    assert_eq!(session.light_bridge.light_count(), enrolled_before);
    let host_light = tagged(&ctx, "late_light");
    let captured =
        ctx.presentation_overrides.borrow().records[&(host_light, ComponentKind::Light)].clone();
    assert!((captured.anim_start.unwrap() - h.clock).abs() < 1e-9);
    h.publish();
    let network_id = h.allocator.stamp_for_test(host_light);
    // No enrollment helper here: the same bridge boundary the frame loop calls
    // must both discover this entity and start its finite clock now.
    let first = sample(&mut h.host, h.clock);
    assert_eq!(
        h.host.session.as_ref().unwrap().light_bridge.light_count(),
        enrolled_before + 1
    );
    assert!((first.last().copied().unwrap() - 0.2).abs() < 1e-5);

    // Render again without advancing fixed ticks, after a 100 ms delay. A lazy
    // start here would exceed the one-snapshot completion tolerance below.
    h.clock += 0.1;
    let delayed = sample(&mut h.host, h.clock);
    assert!(delayed.last().unwrap() > first.last().unwrap());
    h.advance(18);
    let client_light = client_entity(&h.peers[0], network_id);
    let client_ctx = script_ctx(&h.peers[0].app);
    assert!(
        client_ctx
            .registry
            .borrow()
            .get_component::<LightComponent>(client_light)
            .expect("the installed runtime-spawn descriptor materializes its light")
            .animation
            .is_some(),
        "the captured finite animation reached the materialized client light"
    );
    let mut host_finish = None;
    let mut client_finish = None;
    for _ in 0..60 {
        h.step();
        for (context, id, finish) in [
            (&ctx, host_light, &mut host_finish),
            (&client_ctx, client_light, &mut client_finish),
        ] {
            if context
                .registry
                .borrow()
                .get_component::<LightComponent>(id)
                .unwrap()
                .animation
                .is_none()
            {
                finish.get_or_insert(h.clock);
            }
        }
    }
    assert!((host_finish.unwrap() - 11.0).abs() <= DT + 1e-6);
    assert!((client_finish.unwrap() - host_finish.unwrap()).abs() <= 2.0 * DT + 1e-6);
    let host_final = ctx
        .registry
        .borrow()
        .get_component::<LightComponent>(host_light)
        .unwrap()
        .clone();
    let client_final = client_ctx
        .registry
        .borrow()
        .get_component::<LightComponent>(client_light)
        .unwrap()
        .clone();
    assert!((host_final.intensity - client_final.intensity).abs() < 1e-5);
}

// Regression: a shorter replacement settled on the host using the previous light's start.
#[test]
fn conditioned_bridge_finite_replacement_keeps_capture_and_client_finish_aligned() {
    let mut h = Harness::new();
    h.advance(40);
    h.tick = 600;
    h.clock = 10.0;
    h.install_reactions(vec![primitive(
        "changes",
        "setLightAnimation",
        Some("prl"),
        animation(5000.0, true),
    )]);
    h.fire_plate(200.0, &[1]);
    sample(&mut h.host, h.clock);
    h.advance(180);
    assert!((h.clock - 13.0).abs() < 1e-9);
    h.install_reactions(vec![primitive(
        "changes",
        "setLightAnimation",
        Some("prl"),
        animation(1000.0, true),
    )]);
    h.fire_plate(-100.0, &[]);
    h.fire_plate(200.0, &[1]);
    let host_ctx = script_ctx(&h.host);
    let host_id = tagged(&host_ctx, "prl");
    let recorded =
        host_ctx.presentation_overrides.borrow().records[&(host_id, ComponentKind::Light)].clone();
    assert!((recorded.anim_start.unwrap() - 13.0).abs() < 1e-9);
    h.publish();
    sample(&mut h.host, h.clock);
    assert!(
        host_ctx
            .registry
            .borrow()
            .get_component::<LightComponent>(host_id)
            .unwrap()
            .animation
            .is_some()
    );

    h.advance(18);
    let client_ctx = script_ctx(&h.peers[0].app);
    let client_id = tagged(&client_ctx, "prl");
    assert!(
        client_ctx
            .registry
            .borrow()
            .get_component::<LightComponent>(client_id)
            .unwrap()
            .animation
            .is_some()
    );
    let brightness = |app: &mut App, clock: f64| {
        let ctx = script_ctx(app);
        app.session
            .as_mut()
            .unwrap()
            .light_bridge
            .update(&mut ctx.registry.borrow_mut(), clock as f32, 0.0)
            .unwrap()
            .effective_brightness[0]
    };
    let host_sample = brightness(&mut h.host, h.clock);
    let client_sample = brightness(&mut h.peers[0].app, h.clock);
    assert!((host_sample - client_sample).abs() < 0.05);
    let mut host_finish = None;
    let mut client_finish = None;
    for _ in 0..60 {
        h.step();
        if host_ctx
            .registry
            .borrow()
            .get_component::<LightComponent>(host_id)
            .unwrap()
            .animation
            .is_none()
        {
            host_finish.get_or_insert(h.clock);
        }
        if client_ctx
            .registry
            .borrow()
            .get_component::<LightComponent>(client_id)
            .unwrap()
            .animation
            .is_none()
        {
            client_finish.get_or_insert(h.clock);
        }
    }
    assert!((host_finish.unwrap() - 14.0).abs() <= DT + 1e-6);
    assert!((client_finish.unwrap() - host_finish.unwrap()).abs() <= 2.0 * DT + 1e-6);
    let host_final = host_ctx
        .registry
        .borrow()
        .get_component::<LightComponent>(host_id)
        .unwrap()
        .intensity;
    let client_final = client_ctx
        .registry
        .borrow()
        .get_component::<LightComponent>(client_id)
        .unwrap()
        .intensity;
    assert!((host_final - client_final).abs() < 1e-5);
    assert_eq!(
        host_ctx.presentation_overrides.borrow().records[&(host_id, ComponentKind::Light)],
        recorded
    );
}

// Regression: the host counted a long write-frame delta before the captured emitter start.
#[test]
fn conditioned_bridge_changed_spin_tween_keeps_capture_and_client_finish_aligned() {
    let mut h = Harness::new();
    h.advance(40);
    h.tick = 600;
    h.clock = 10.0;
    sample(&mut h.host, h.clock);
    h.install_reactions(vec![primitive(
        "changes",
        "setSpinRate",
        Some("emitter_only"),
        json!({"animation":{"duration":1.0,"rate_curve":[0.0,1.0]}}),
    )]);
    h.tick = 630;
    h.clock = 10.5;
    h.fire_plate(200.0, &[1]);
    let host_ctx = script_ctx(&h.host);
    let host_id = tagged(&host_ctx, "emitter_only");
    let recorded = host_ctx.presentation_overrides.borrow().records
        [&(host_id, ComponentKind::BillboardEmitter)]
        .clone();
    assert!((recorded.anim_start.unwrap() - 10.5).abs() < 1e-9);
    h.publish();
    sample_delta(&mut h.host, h.clock, 0.5);
    assert!(
        host_ctx
            .registry
            .borrow()
            .get_component::<BillboardEmitterComponent>(host_id)
            .unwrap()
            .spin_rate
            .abs()
            < 1e-5
    );
    h.advance(24);
    let client_ctx = script_ctx(&h.peers[0].app);
    let client_id = tagged(&client_ctx, "emitter_only");
    let rate = |ctx: &ScriptCtx, id| {
        ctx.registry
            .borrow()
            .get_component::<BillboardEmitterComponent>(id)
            .unwrap()
            .spin_rate
    };
    assert!((rate(&host_ctx, host_id) - 0.4).abs() < 1e-5);
    assert!((rate(&client_ctx, client_id) - rate(&host_ctx, host_id)).abs() < 0.04);

    // An identical tween plus scalar write retains the captured start and local age.
    h.install_reactions(vec![
        primitive(
            "changes",
            "setSpinRate",
            Some("emitter_only"),
            json!({"animation":{"duration":1.0,"rate_curve":[0.0,1.0]}}),
        ),
        primitive(
            "changes",
            "setEmitterRate",
            Some("emitter_only"),
            json!({"rate":0.25}),
        ),
    ]);
    h.fire_plate(-100.0, &[]);
    h.fire_plate(200.0, &[1]);
    assert!(
        (host_ctx.presentation_overrides.borrow().records
            [&(host_id, ComponentKind::BillboardEmitter)]
            .anim_start
            .unwrap()
            - 10.5)
            .abs()
            < 1e-9
    );
    let mut host_finish = None;
    let mut client_finish = None;
    for _ in 0..48 {
        h.step();
        if host_ctx
            .registry
            .borrow()
            .get_component::<BillboardEmitterComponent>(host_id)
            .unwrap()
            .spin_animation
            .is_none()
        {
            host_finish.get_or_insert(h.clock);
        }
        if client_ctx
            .registry
            .borrow()
            .get_component::<BillboardEmitterComponent>(client_id)
            .unwrap()
            .spin_animation
            .is_none()
        {
            client_finish.get_or_insert(h.clock);
        }
    }
    assert!((host_finish.unwrap() - 11.5).abs() <= DT + 1e-6);
    assert!((client_finish.unwrap() - host_finish.unwrap()).abs() <= 2.0 * DT + 1e-6);
    assert!((rate(&host_ctx, host_id) - 1.0).abs() < 1e-5);
    assert!((rate(&client_ctx, client_id) - 1.0).abs() < 1e-5);
}

// Regression: frozen host spin used wall time while a late baseline advertised held script time.
#[test]
fn conditioned_frozen_spin_clock_keeps_late_join_and_resumed_completion_aligned() {
    let mut h = Harness::new();
    h.advance(40);
    h.tick = 600;
    h.clock = 10.0;
    h.install_reactions(vec![primitive(
        "changes",
        "setSpinRate",
        Some("emitter_only"),
        json!({"animation":{"duration":5.0,"rate_curve":[0.0,10.0]}}),
    )]);
    h.fire_plate(200.0, &[1]);
    sample_delta(&mut h.host, h.clock, 0.5);
    let host_ctx = script_ctx(&h.host);
    let host_id = tagged(&host_ctx, "emitter_only");
    let recorded = host_ctx.presentation_overrides.borrow().records
        [&(host_id, ComponentKind::BillboardEmitter)]
        .clone();
    assert!((recorded.anim_start.unwrap() - 10.0).abs() < 1e-9);
    let state = |ctx: &ScriptCtx, id| {
        let registry = ctx.registry.borrow();
        let emitter = registry
            .get_component::<BillboardEmitterComponent>(id)
            .unwrap();
        (emitter.spin_rate, emitter.spin_animation.is_some())
    };
    assert!(state(&host_ctx, host_id).0.abs() < 1e-5);
    h.advance(60);
    assert!((h.clock - 11.0).abs() < 1e-9);
    for _ in 0..6 {
        // Six positive raw half-second frames; the published script clock holds at 11.
        sample_deltas(&mut h.host, h.clock, 0.5, 0.0);
        assert!((state(&host_ctx, host_id).0 - 2.0).abs() < 1e-5);
        assert!(state(&host_ctx, host_id).1);
    }
    assert_eq!(
        host_ctx.presentation_overrides.borrow().records
            [&(host_id, ComponentKind::BillboardEmitter)],
        recorded
    );
    h.connect_late();
    // Publish the late baseline with the held clock, before resuming. Ordinary
    // clocks then advance during transport; the local freeze switch is not sent.
    h.last = None;
    h.publish();
    h.advance(12);
    let client_ctx = script_ctx(&h.peers[1].app);
    let client_id = tagged(&client_ctx, "emitter_only");
    let host = state(&host_ctx, host_id);
    let client = state(&client_ctx, client_id);
    assert!(host.1 && client.1);
    assert!((host.0 - 2.4).abs() < 1e-5);
    assert!((host.0 - client.0).abs() < 0.08);
    let mut host_finish = None;
    let mut client_finish = None;
    for _ in 0..240 {
        h.step();
        if !state(&host_ctx, host_id).1 {
            host_finish.get_or_insert(h.clock);
        }
        if !state(&client_ctx, client_id).1 {
            client_finish.get_or_insert(h.clock);
        }
    }
    assert!((host_finish.unwrap() - 15.0).abs() <= DT + 1e-6);
    assert!((host_finish.unwrap() - client_finish.unwrap()).abs() <= 2.0 * DT + 1e-6);
    assert!((state(&host_ctx, host_id).0 - 10.0).abs() < 1e-5);
    assert!((state(&client_ctx, client_id).0 - 10.0).abs() < 1e-5);
}

// Regression: install captured start zero, but all three bridges started at the first render.
#[test]
fn conditioned_install_animations_keep_zero_origin_through_first_frame_and_client_apply() {
    let mut h = Harness::with_install_reactions(
        fixture_world(),
        vec![
            primitive(
                "levelLoad",
                "setLightAnimation",
                Some("lit_prop"),
                animation(1000.0, true),
            ),
            primitive(
                "levelLoad",
                "setSpinRate",
                Some("lit_prop"),
                json!({"animation":{"duration":1.0,"rate_curve":[0.0,1.0]}}),
            ),
            NamedReaction {
                name: "already-met".into(),
                descriptor: ReactionDescriptor::Progress(postretro_entities::ProgressDescriptor {
                    tag: "lit_prop".into(),
                    at: 0.0,
                    fire: "install-progress".into(),
                }),
            },
            primitive(
                "install-progress",
                "setLightAnimation",
                Some("prl"),
                animation(1000.0, true),
            ),
            primitive(
                "install-progress",
                "setFogAnimation",
                Some("fog"),
                json!({"periodMs":1000.0,"playCount":1,"density":[0.0,1.0]}),
            ),
            primitive(
                "install-progress",
                "setSpinRate",
                Some("billboard_emitter"),
                json!({"animation":{"duration":1.0,"rate_curve":[0.0,1.0]}}),
            ),
        ],
    );
    let host_ctx = script_ctx(&h.host);
    let prop = tagged(&host_ctx, "lit_prop");
    let targets = [
        (prop, ComponentKind::Light),
        (prop, ComponentKind::BillboardEmitter),
        (tagged(&host_ctx, "prl"), ComponentKind::Light),
        (tagged(&host_ctx, "fog"), ComponentKind::FogVolume),
        (
            tagged(&host_ctx, "billboard_emitter"),
            ComponentKind::BillboardEmitter,
        ),
    ];
    let records = host_ctx.presentation_overrides.borrow().records.clone();
    for key in targets {
        assert!(records[&key].anim_start.unwrap().abs() < 1e-9);
    }

    // No bridge sample at time zero: this is the first running frame after install.
    h.tick = 30;
    h.clock = 0.5;
    h.publish();
    let brightness = sample_delta(&mut h.host, h.clock, 0.5);
    assert!((brightness[0] - 0.6).abs() < 1e-5);
    let state = |ctx: &ScriptCtx, id, kind| {
        let registry = ctx.registry.borrow();
        match kind {
            ComponentKind::Light => {
                let light = registry.get_component::<LightComponent>(id).unwrap();
                (light.animation.is_some(), light.intensity)
            }
            ComponentKind::FogVolume => {
                let fog = registry
                    .get_component::<postretro_entities::FogVolumeComponent>(id)
                    .unwrap();
                (fog.animation.is_some(), fog.density)
            }
            ComponentKind::BillboardEmitter => {
                let emitter = registry
                    .get_component::<BillboardEmitterComponent>(id)
                    .unwrap();
                (emitter.spin_animation.is_some(), emitter.spin_rate)
            }
            _ => unreachable!("fixture presentation kinds"),
        }
    };
    for (id, kind) in targets {
        let (active, value) = state(&host_ctx, id, kind);
        assert!(active);
        if kind != ComponentKind::Light {
            assert!((value - 0.5).abs() < 1e-5);
        }
    }
    h.advance(12);
    let client_ctx = script_ctx(&h.peers[0].app);
    let client_prop = client_entity(&h.peers[0], h.allocator.stamp_for_test(prop));
    let client_targets = [
        (client_prop, ComponentKind::Light),
        (client_prop, ComponentKind::BillboardEmitter),
        (tagged(&client_ctx, "prl"), ComponentKind::Light),
        (tagged(&client_ctx, "fog"), ComponentKind::FogVolume),
        (
            tagged(&client_ctx, "billboard_emitter"),
            ComponentKind::BillboardEmitter,
        ),
    ];
    for ((host_id, kind), (client_id, _)) in targets.into_iter().zip(client_targets) {
        let host = state(&host_ctx, host_id, kind);
        let client = state(&client_ctx, client_id, kind);
        assert!(host.0 && client.0);
        if kind != ComponentKind::Light {
            assert!((host.1 - 0.7).abs() < 1e-5);
            assert!((host.1 - client.1).abs() < 0.04);
        }
    }
    // Actual light evaluator, including the rebased client seed, at the same instant.
    let brightness = |app: &mut App| {
        let ctx = script_ctx(app);
        app.session
            .as_mut()
            .unwrap()
            .light_bridge
            .update(&mut ctx.registry.borrow_mut(), 0.7, 0.0)
            .unwrap()
            .effective_brightness[0]
    };
    assert!((brightness(&mut h.host) - brightness(&mut h.peers[0].app)).abs() < 0.04);

    let mut host_finish = [None; 5];
    let mut client_finish = [None; 5];
    for _ in 0..36 {
        h.step();
        for (index, ((host_id, kind), (client_id, _))) in
            targets.into_iter().zip(client_targets).enumerate()
        {
            if !state(&host_ctx, host_id, kind).0 {
                host_finish[index].get_or_insert(h.clock);
            }
            if !state(&client_ctx, client_id, kind).0 {
                client_finish[index].get_or_insert(h.clock);
            }
        }
    }
    for (index, ((host_id, kind), (client_id, _))) in
        targets.into_iter().zip(client_targets).enumerate()
    {
        assert!((host_finish[index].unwrap() - 1.0).abs() <= DT + 1e-6);
        assert!(
            (host_finish[index].unwrap() - client_finish[index].unwrap()).abs() <= 2.0 * DT + 1e-6
        );
        assert!((state(&host_ctx, host_id, kind).1 - 1.0).abs() < 1e-5);
        assert!((state(&client_ctx, client_id, kind).1 - 1.0).abs() < 1e-5);
        assert_eq!(
            host_ctx.presentation_overrides.borrow().records[&(host_id, kind)],
            records[&(host_id, kind)]
        );
    }
}

#[test]
fn conditioned_overrides_late_join_sample_finite_loop_fog_spin_and_display_copy_delta() {
    let mut h = Harness::new();
    install_changes(&mut h);
    h.advance(40);
    assert!(h.server.is_participating(CLIENT_A));
    assert!(!h.server.is_participating(CLIENT_B));
    h.fire_plate(100.0, &[1]);
    h.advance(10);
    h.fire_plate(200.0, &[1]);
    let host_ctx = script_ctx(&h.host);
    let animated_prop = tagged(&host_ctx, "mesh_prop");
    let registry = host_ctx.registry.borrow();
    let host_mesh = registry
        .get_component::<MeshComponent>(animated_prop)
        .unwrap();
    assert_eq!(
        host_mesh.animation.as_ref().unwrap().current_state,
        "attack",
        "real resolved host state changed before publication"
    );
    assert!(
        host_mesh
            .animation
            .as_ref()
            .unwrap()
            .states
            .get("attack")
            .unwrap()
            .clip_index
            .is_some(),
        "shipped Walking_A clip resolved"
    );
    drop(registry);
    let start = h.clock;
    assert_eq!(
        script_ctx(&h.host)
            .presentation_overrides
            .borrow()
            .records
            .len(),
        8
    );
    h.advance(120);
    h.connect_late();
    h.advance(30);
    assert!(h.server.is_participating(CLIENT_B));
    let host_ctx = script_ctx(&h.host);
    let lit = tagged(&host_ctx, "lit_prop");
    let mesh = tagged(&host_ctx, "mesh_prop");
    let lit_nid = h.allocator.stamp_for_test(lit);
    let mesh_nid = h.allocator.stamp_for_test(mesh);
    for peer in &mut h.peers {
        let ctx = script_ctx(&peer.app);
        let registry = ctx.registry.borrow();
        let finite = peer
            .app
            .session
            .as_ref()
            .unwrap()
            .light_bridge
            .entity_for_map_index(0)
            .unwrap();
        assert!(
            registry
                .get_component::<LightComponent>(finite)
                .unwrap()
                .animation
                .is_some(),
            "late join is mid finite animation"
        );
        let local_lit = client_entity(peer, lit_nid);
        let local_mesh = client_entity(peer, mesh_nid);
        assert!(
            registry
                .get_component::<LightComponent>(local_lit)
                .unwrap()
                .animation
                .is_some()
        );
        assert_eq!(
            registry
                .get_component::<MeshComponent>(local_mesh)
                .unwrap()
                .animation
                .as_ref()
                .unwrap()
                .current_state,
            "attack"
        );
        assert!(
            !registry
                .has_component_kind(local_lit, ComponentKind::Health)
                .unwrap()
        );
        let fog = peer
            .app
            .session
            .as_ref()
            .unwrap()
            .fog_volume_bridge
            .entity_for_map_index(0)
            .unwrap();
        let fog = registry
            .get_component::<postretro_entities::FogVolumeComponent>(fog)
            .unwrap();
        assert!((fog.glow - 0.2).abs() < 1e-5);
        assert!((fog.edge_softness - 0.3).abs() < 1e-5);
        assert!((fog.falloff - 1.5).abs() < 1e-5);
        assert_eq!(fog.tint, [0.3, 0.4, 0.5]);
        assert_eq!(fog.saturation, 0.6);
        assert_eq!(fog.min_brightness, 0.1);
        assert_eq!(fog.light_range, 2.0);
        for tag in ["billboard_emitter", "emitter_only"] {
            let id = tagged(&ctx, tag);
            let emitter = registry
                .get_component::<BillboardEmitterComponent>(id)
                .unwrap();
            assert_eq!(emitter.rate, 0.0);
            assert!(emitter.spin_rate > 0.5);
        }
    }
    // Regression: shared local origins hid a missing loop-phase rebase; equal
    // brightness also admitted distinct phases on the closed two-key curve.
    assert!(h.clock - h.peers[1].clock > 2.0);
    let loop_phase = |app: &App, clock: f64| {
        let ctx = script_ctx(app);
        let registry = ctx.registry.borrow();
        let id = app
            .session
            .as_ref()
            .unwrap()
            .light_bridge
            .entity_for_map_index(1)
            .unwrap();
        let animation = registry
            .get_component::<LightComponent>(id)
            .unwrap()
            .animation
            .as_ref()
            .unwrap();
        let period = animation.period_ms / 1000.0;
        // The bridge/GPU samples a looping light against its local script time.
        let phase = (clock as f32 / period + animation.phase.unwrap_or(0.0)).rem_euclid(1.0);
        (f64::from(phase), f64::from(period))
    };
    let (host_phase, period) = loop_phase(&h.host, h.clock);
    // Probe the actual evaluator without advancing the spin bridges a second time.
    let host_brightness = sample_delta(&mut h.host, h.clock, 0.0);
    for peer in &mut h.peers {
        let (client_phase, client_period) = loop_phase(&peer.app, peer.clock);
        assert!((client_period - period).abs() < 1e-6);
        let phase_difference = (host_phase - client_phase).abs();
        let phase_time = phase_difference.min(1.0 - phase_difference) * period;
        assert!(
            phase_time <= 2.0 * DT + 1e-6,
            "loop phase differs by {phase_time}s at shared time {}: host {host_phase}, client {client_phase}",
            h.clock
        );
        let sampled = sample_delta(&mut peer.app, peer.clock, 0.0);
        assert!(
            (host_brightness[1] - sampled[1]).abs() < 0.05,
            "loop evaluator agrees at shared instant: host {:?} client {:?}",
            host_brightness,
            sampled
        );
    }
    // Changed override on an already materialized lit prop, with no new entity.
    let original_ids: Vec<_> = h.peers.iter().map(|p| client_entity(p, lit_nid)).collect();
    let session = h.host.session.as_ref().unwrap();
    // The live predicate records even an every-machine change on a networked prop.
    host_ctx.data_registry.borrow_mut().populate_level(
        vec![primitive(
            "lit.delta",
            "setLightAnimation",
            Some("lit_prop"),
            animation(5000.0, false),
        )],
        vec![],
        &[],
    );
    crate::drain_named_events_with_sequences(
        [("lit.delta", None)],
        postretro_entities::FireOrigin::EveryMachine(
            postretro_entities::MachineSource::HostMirrored,
        ),
        &host_ctx.data_registry.borrow(),
        &session.scripting.sequence_registry,
        &session.scripting.reaction_registry,
        &session.scripting.system_registry,
        &host_ctx,
    );
    h.advance(12);
    for (index, peer) in h.peers.iter().enumerate() {
        assert_eq!(client_entity(peer, lit_nid), original_ids[index]);
        let ctx = script_ctx(&peer.app);
        assert_eq!(
            ctx.registry
                .borrow()
                .get_component::<LightComponent>(original_ids[index])
                .unwrap()
                .animation
                .as_ref()
                .unwrap()
                .play_count,
            None
        );
    }
    // Finite PRL/fog completion uses seeded starts, including the late joiner.
    let mut finishes = [None; 3];
    let mut fog_finishes = [None; 3];
    while h.clock < start + 5.2 {
        h.step();
        for (index, app) in std::iter::once(&h.host)
            .chain(h.peers.iter().map(|peer| &peer.app))
            .enumerate()
        {
            let ctx = script_ctx(app);
            let session = app.session.as_ref().unwrap();
            let light = session.light_bridge.entity_for_map_index(0).unwrap();
            if ctx
                .registry
                .borrow()
                .get_component::<LightComponent>(light)
                .unwrap()
                .animation
                .is_none()
                && finishes[index].is_none()
            {
                finishes[index] = Some(h.clock);
            }
            let fog = session.fog_volume_bridge.entity_for_map_index(0).unwrap();
            if ctx
                .registry
                .borrow()
                .get_component::<postretro_entities::FogVolumeComponent>(fog)
                .unwrap()
                .animation
                .is_none()
                && fog_finishes[index].is_none()
            {
                fog_finishes[index] = Some(h.clock);
            }
        }
    }
    for measured in [finishes, fog_finishes] {
        let host = measured[0].expect("host animation finished");
        for client in &measured[1..] {
            assert!(
                (client.expect("client animation finished") - host).abs() <= DT * 2.0 + 1e-6,
                "finish instants {measured:?}"
            );
        }
    }
    let host_fog = h
        .host
        .session
        .as_ref()
        .unwrap()
        .fog_volume_bridge
        .entity_for_map_index(0)
        .unwrap();
    assert!(
        host_ctx
            .registry
            .borrow()
            .get_component::<postretro_entities::FogVolumeComponent>(host_fog)
            .unwrap()
            .animation
            .is_none()
    );
    for peer in &h.peers {
        let ctx = script_ctx(&peer.app);
        let session = peer.app.session.as_ref().unwrap();
        let light = session.light_bridge.entity_for_map_index(0).unwrap();
        assert!(
            ctx.registry
                .borrow()
                .get_component::<LightComponent>(light)
                .unwrap()
                .animation
                .is_none()
        );
        let fog = session.fog_volume_bridge.entity_for_map_index(0).unwrap();
        assert!(
            (ctx.registry
                .borrow()
                .get_component::<postretro_entities::FogVolumeComponent>(fog)
                .unwrap()
                .density
                - 0.1)
                .abs()
                < 1e-5
        );
    }
}

fn presentation_args(verb: &str, marker: u32, audience: Option<&str>) -> Value {
    let mut args = match verb {
        "playSound" => json!({"sound":format!("sound.{marker}")}),
        "rumble" => json!({"strong":0.5,"durationMs":marker}),
        "flashScreen" => json!({"color":[1,0,0,1],"durationMs":marker}),
        "screenShake" => json!({"amplitude":1,"durationMs":marker}),
        "vignette" => json!({"strength":0.5,"durationMs":marker}),
        _ => unreachable!(),
    };
    if let Some(audience) = audience {
        args["audience"] = json!(audience);
    }
    args
}
fn marker(command: &SystemReactionCommand) -> u32 {
    match command {
        SystemReactionCommand::PlaySound { sound, .. } => sound
            .strip_prefix("sound.")
            .and_then(|s| s.parse().ok())
            .unwrap_or(0),
        SystemReactionCommand::Rumble { duration_ms, .. }
        | SystemReactionCommand::FlashScreen { duration_ms, .. }
        | SystemReactionCommand::ScreenShake { duration_ms, .. }
        | SystemReactionCommand::Vignette { duration_ms, .. } => *duration_ms as u32,
        _ => 0,
    }
}
fn count(commands: &[SystemReactionCommand], wanted: u32) -> usize {
    commands
        .iter()
        .filter(|command| marker(command) == wanted)
        .count()
}
#[test]
fn conditioned_trigger_audiences_player_residual_wait_and_positioned_mover_sound() {
    let mut h = Harness::new();
    h.connect_late();
    h.advance(40);
    let verbs = [
        "playSound",
        "rumble",
        "flashScreen",
        "screenShake",
        "vignette",
    ];
    let audiences = [
        None,
        Some("@activators"),
        Some("@trigger"),
        Some("everyone"),
    ];
    let mut reactions = Vec::new();
    for (row, audience) in audiences.into_iter().enumerate() {
        for (column, verb) in verbs.into_iter().enumerate() {
            reactions.push(primitive(
                "plate",
                verb,
                None,
                presentation_args(verb, 100 + row as u32 * 10 + column as u32, audience),
            ));
        }
    }
    // Actual wait enrollment is sourced by the trigger residual; its landed
    // default screen effect must lose the trigger subject and stay host-local.
    let finite = tagged(&script_ctx(&h.host), "prl");
    reactions.push(NamedReaction {
        name: "plate".into(),
        descriptor: ReactionDescriptor::Sequence(vec![
            SequenceStep {
                id: SequenceTarget::Wait,
                primitive: "wait".into(),
                args: json!({"durationMs":30}),
            },
            SequenceStep {
                id: SequenceTarget::Entity(finite),
                primitive: "setLightAnimation".into(),
                args: animation(1000.0, true),
            },
            SequenceStep {
                id: SequenceTarget::Fire,
                primitive: "fire".into(),
                args: json!({"event":"tail.effect"}),
            },
        ]),
    });
    reactions.push(primitive(
        "tail.effect",
        "flashScreen",
        None,
        presentation_args("flashScreen", 250, None),
    ));
    h.install_reactions(reactions);
    h.fire_plate(100.0, &[0, 1]);
    h.advance(12);
    for row in 0..4 {
        for column in 0..5 {
            let id = 100 + row * 10 + column;
            let expected = match row {
                0 if column == 0 => [2, 2, 2],
                0 | 1 => [1, 1, 0],
                2 => [2, 2, 0],
                3 => [2, 2, 2],
                _ => unreachable!(),
            };
            assert_eq!(
                [
                    count(&h.host_commands, id),
                    count(&h.peers[0].commands, id),
                    count(&h.peers[1].commands, id)
                ],
                expected,
                "row {row} verb {}",
                verbs[column as usize]
            );
        }
    }
    let ctx = script_ctx(&h.host);
    let session = h.host.session.as_ref().unwrap();
    for _ in 0..3 {
        session.scripting.scheduler.begin_frame();
        session.scripting.scheduler.evaluate(&[]);
    }
    ctx.presentation_script_time.set(h.clock);
    session.scripting.scheduler.drain_landings(
        &ctx.data_registry.borrow(),
        &session.scripting.sequence_registry,
        &session.scripting.reaction_registry,
        &session.scripting.system_registry,
        &ctx,
    );
    assert!(
        ctx.presentation_overrides
            .borrow()
            .records
            .contains_key(&(finite, ComponentKind::Light))
    );
    h.route();
    h.advance(12);
    assert_eq!(count(&h.host_commands, 250), 2);
    assert_eq!(count(&h.peers[0].commands, 250), 0);
    assert_eq!(count(&h.peers[1].commands, 250), 0);

    // A real player-event table evaluates the three seat-bound pawns; its
    // event-player audience presents only once on each owning machine.
    ctx.data_registry.borrow_mut().populate_level(
        vec![primitive(
            "player.effect",
            "screenShake",
            None,
            presentation_args("screenShake", 300, Some("@player")),
        )],
        vec![],
        &[],
    );
    ctx.data_registry.borrow_mut().set_level_player_events(vec![
        postretro_entities::PlayerEventDescriptor {
            edge: postretro_entities::PlayerEventEdge::Becomes,
            condition: postretro_foundation::IrNode::Const {
                value: postretro_foundation::IrValue::Bool(true),
            },
            fire: vec!["player.effect".into()],
            levels: vec![],
            authored_index: 0,
        },
    ]);
    ctx.data_registry.borrow_mut().recompose(&[]);
    let mut table = PlayerEventTable::build(&ctx, Default::default(), Default::default(), None);
    let mut residuals = Vec::new();
    table.run_tick(&ctx, &mut residuals);
    assert_eq!(residuals.len(), 3);
    let session = h.host.session.as_ref().unwrap();
    let mut follow = Vec::new();
    drain_frame_residuals(
        &[],
        &h.bindings,
        &h.system,
        &residuals,
        &table,
        &session.scripting.scheduler,
        ResidualRegistries {
            sequence: &session.scripting.sequence_registry,
            reaction: &session.scripting.reaction_registry,
            system: &session.scripting.system_registry,
        },
        &ctx,
        &mut follow,
        |player, registry| match player {
            PlayerId::Local(pawn) => registry.seat_for_pawn(pawn),
            PlayerId::Remote(id) => h.seats.seat_for_client(id),
        },
    );
    h.route();
    h.advance(12);
    assert_eq!(
        [
            count(&h.host_commands, 300),
            count(&h.peers[0].commands, 300),
            count(&h.peers[1].commands, 300)
        ],
        [1, 1, 1]
    );

    // The production named mover drain freezes its emitter before the source
    // entity vanishes, and the real presentation intake retains it.
    let door = ctx.registry.borrow_mut().spawn(Transform {
        position: Vec3::new(3.0, 4.0, 5.0),
        ..Default::default()
    });
    ctx.data_registry.borrow_mut().populate_level(
        vec![primitive(
            "door.open",
            "playSound",
            None,
            json!({"sound":"door","at":"@emitter"}),
        )],
        vec![],
        &[],
    );
    let movers = vec![(
        "door.open".into(),
        Some(postretro_entities::Emitter::Entity {
            id: door,
            origin: Vec3::new(3.0, 4.0, 5.0),
        }),
    )];
    let session = h.host.session.as_ref().unwrap();
    let follow = crate::frame_loop::named_drains::drain_frame_named_events(
        crate::frame_loop::named_drains::NamedFrameEvents {
            movement: &[],
            ai: &[],
            observer: &[],
            weapon: &[],
            movers: &movers,
            deaths: &[],
        },
        &session.scripting,
        &ctx,
    );
    dispatch_deferred_named_events_with_sequences(
        follow,
        &ctx.data_registry.borrow(),
        &session.scripting.sequence_registry,
        &session.scripting.reaction_registry,
        &session.scripting.system_registry,
        &ctx,
    );
    ctx.registry.borrow_mut().despawn(door).unwrap();
    h.route();
    h.advance(12);
    for commands in [&h.host_commands, &h.peers[0].commands, &h.peers[1].commands] {
        let sounds: Vec<_> = commands
            .iter()
            .filter_map(|command| match command {
                SystemReactionCommand::PlaySound { sound, at, .. } if sound == "door" => Some(at),
                _ => None,
            })
            .collect();
        assert_eq!(sounds.len(), 1);
        assert!(
            matches!(sounds[0],Some(postretro_entities::Emitter::Entity {origin,..}) if *origin==Vec3::new(3.0,4.0,5.0))
                || matches!(sounds[0],Some(postretro_entities::Emitter::Contacts(contacts)) if contacts.len() == 1 && contacts[0].point == Vec3::new(3.0,4.0,5.0))
        );
    }
}

#[test]
fn conditioned_state_waits_for_clock_empty_release_and_unchanged_baseline_preserves_local_write() {
    let mut h = Harness::new();
    install_changes(&mut h);
    h.advance(40);
    h.peers[0].rtt = None;
    h.fire_plate(200.0, &[1]);
    h.advance(12);
    let ctx = script_ctx(&h.peers[0].app);
    let emitter = tagged(&ctx, "emitter_only");
    assert!(
        ctx.registry
            .borrow()
            .get_component::<BillboardEmitterComponent>(emitter)
            .unwrap()
            .spin_animation
            .is_none()
    );
    h.peers[0].rtt = Some(RTT);
    h.peers[0].receive(&h.world);
    assert!(
        ctx.registry
            .borrow()
            .get_component::<BillboardEmitterComponent>(emitter)
            .unwrap()
            .spin_animation
            .is_some(),
        "an empty receive releases clock-deferred values"
    );
    let finite = h.peers[0]
        .app
        .session
        .as_ref()
        .unwrap()
        .light_bridge
        .entity_for_map_index(0)
        .unwrap();
    let mut local = ctx
        .registry
        .borrow()
        .get_component::<LightComponent>(finite)
        .unwrap()
        .clone();
    local.intensity = 7.0;
    ctx.registry
        .borrow_mut()
        .set_component(finite, local)
        .unwrap();
    // Force refresh of the identical recorded wire value: it is acknowledged,
    // but the last-applied memo keeps the local script edit until host changes.
    let host_finite = h
        .host
        .session
        .as_ref()
        .unwrap()
        .light_bridge
        .entity_for_map_index(0)
        .unwrap();
    let network_id = h.allocator.stamp_for_test(host_finite);
    h.replication.request_refresh(CLIENT_A, network_id.0, 0);
    h.advance(12);
    assert_eq!(
        ctx.registry
            .borrow()
            .get_component::<LightComponent>(finite)
            .unwrap()
            .intensity,
        7.0
    );
    // Regression: equal opposite stalls in one harness cancelled spin drift,
    // while an unchanged payload never exercised the stalled-batch rebase.
    assert_host_stall_spin_alignment();
    assert_client_stall_batch_age_and_completion();
}

fn spin_state(ctx: &ScriptCtx, id: EntityId) -> (f32, bool) {
    let registry = ctx.registry.borrow();
    let emitter = registry
        .get_component::<BillboardEmitterComponent>(id)
        .unwrap();
    (emitter.spin_rate, emitter.spin_animation.is_some())
}

fn assert_host_stall_spin_alignment() {
    let mut h = Harness::new();
    h.advance(40);
    h.install_reactions(vec![primitive(
        "changes",
        "setSpinRate",
        Some("emitter_only"),
        json!({"animation":{"duration":2.0,"rate_curve":[0.0,2.0]}}),
    )]);
    h.fire_plate(200.0, &[1]);
    let start = h.clock;
    sample(&mut h.host, h.clock);
    h.advance(12);
    let host_ctx = script_ctx(&h.host);
    let host_emitter = tagged(&host_ctx, "emitter_only");
    let client_ctx = script_ctx(&h.peers[0].app);
    let client_emitter = tagged(&client_ctx, "emitter_only");
    let last_host_sample = h.clock;
    let before_stall = spin_state(&host_ctx, host_emitter);
    assert!(before_stall.1);
    assert!(spin_state(&client_ctx, client_emitter).1);
    // Transport and client frames continue while the host produces no frame.
    for _ in 0..30 {
        h.clock += DT;
        h.transport();
        let peer = &mut h.peers[0];
        peer.clock = h.clock - peer.clock_origin;
        peer.receive(&h.world);
        peer.sample();
    }
    assert!((spin_state(&host_ctx, host_emitter).0 - before_stall.0).abs() < 1e-6);
    // Resume with the elapsed script delta, even though fixed ticks lag behind.
    h.tick += 1;
    h.clock += DT;
    let resumed_delta = h.clock - last_host_sample;
    assert!(resumed_delta > 0.5);
    h.publish();
    h.transport();
    sample_delta(&mut h.host, h.clock, resumed_delta as f32);
    let peer = &mut h.peers[0];
    peer.clock = h.clock - peer.clock_origin;
    peer.receive(&h.world);
    peer.sample();
    let host = spin_state(&host_ctx, host_emitter);
    let client = spin_state(&client_ctx, client_emitter);
    assert!(host.1 && client.1);
    // This linear curve makes the sampled rate an injective age in seconds.
    assert!((f64::from(host.0) - (h.clock - start)).abs() < 1e-5);
    assert!(
        f64::from((host.0 - client.0).abs()) <= 2.0 * DT + 1e-6,
        "host resume age aligns immediately: host {}, client {}",
        host.0,
        client.0
    );
    let mut host_finish = None;
    let mut client_finish = None;
    for _ in 0..100 {
        h.step();
        if !spin_state(&host_ctx, host_emitter).1 {
            host_finish.get_or_insert(h.clock);
        }
        if !spin_state(&client_ctx, client_emitter).1 {
            client_finish.get_or_insert(h.clock);
        }
    }
    assert!((host_finish.unwrap() - (start + 2.0)).abs() <= DT + 1e-6);
    assert!((host_finish.unwrap() - client_finish.unwrap()).abs() <= 2.0 * DT + 1e-6);
}

fn assert_client_stall_batch_age_and_completion() {
    // Independent initial clocks prevent a preceding host stall hiding drift.
    let mut h = Harness::new();
    install_changes(&mut h);
    h.advance(40);
    h.fire_plate(200.0, &[1]);
    sample(&mut h.host, h.clock);
    h.advance(12);
    let host_ctx = script_ctx(&h.host);
    let host_emitter = tagged(&host_ctx, "emitter_only");
    let host_short = tagged(&host_ctx, "billboard_emitter");
    let client_ctx = script_ctx(&h.peers[0].app);
    let client_emitter = tagged(&client_ctx, "emitter_only");
    let client_short = tagged(&client_ctx, "billboard_emitter");
    let last_client_sample = h.peers[0].clock;
    let before_stall = spin_state(&client_ctx, client_emitter);
    assert!(before_stall.1);
    h.install_reactions(vec![
        primitive(
            "changes",
            "setSpinRate",
            Some("emitter_only"),
            json!({"animation":{"duration":1.0,"rate_curve":[0.0,1.0]}}),
        ),
        primitive(
            "changes",
            "setSpinRate",
            Some("billboard_emitter"),
            json!({"animation":{"duration":0.25,"rate_curve":[0.0,1.0]}}),
        ),
    ]);
    let mut changed_start = None;
    for frame in 0..30 {
        h.tick += 1;
        h.clock += DT;
        if frame == 0 {
            h.fire_plate(-100.0, &[]);
            h.fire_plate(200.0, &[1]);
            changed_start = Some(h.clock);
        }
        // The changed value rides an early snapshot. Later headers advance
        // throughout the stall, even after the short host tween has settled.
        h.publish();
        h.transport();
        sample(&mut h.host, h.clock);
    }
    assert!(
        (spin_state(&client_ctx, client_emitter).0 - before_stall.0).abs() < 1e-6,
        "no client frame sampled during the stall"
    );
    let expected_age = h.clock - changed_start.unwrap();
    assert!(expected_age > 0.4 && expected_age < 1.0);
    let host = spin_state(&host_ctx, host_emitter);
    assert!(host.1);
    assert!((f64::from(host.0) - expected_age).abs() < 1e-5);
    assert!(!spin_state(&host_ctx, host_short).1);
    h.peers[0].clock = h.clock - h.peers[0].clock_origin;
    h.peers[0].receive(&h.world);
    let peer = &mut h.peers[0];
    sample_delta(
        &mut peer.app,
        peer.clock,
        (peer.clock - last_client_sample) as f32,
    );
    let resumed = spin_state(&client_ctx, client_emitter);
    assert!(resumed.1);
    assert!(
        (f64::from(resumed.0) - expected_age).abs() <= 2.0 * DT + 1e-6,
        "first resumed sample uses the newest batch clock: age {}, expected {expected_age}",
        resumed.0
    );
    let short = spin_state(&client_ctx, client_short);
    assert!(
        !short.1,
        "the short tween is already complete on the first resumed sample"
    );
    assert!((short.0 - 1.0).abs() < 1e-5);
}

#[test]
fn conditioned_actual_level_load_wait_and_crossing_capture_authority() {
    let mut world = fixture_world();
    world.data_script=Some(postretro_level_format::data_script::DataScriptSection {source_path:"conditioned-install.luau".into(),compiled_bytes:br#"
function setupLevel()
    return {reactions={
        {name="levelLoad",sequence={{id="@wait",primitive="wait",args={durationMs=30}},{id="@fire",primitive="fire",args={event="installer.tail"}}}},
        {name="installer.tail",primitive="setLightAnimation",tag="prl",args={periodMs=1000,playCount=1,brightness={0.2,1.0}}}
    }}
end
"#.to_vec()});
    let mut h = Harness::with_world(world);
    let ctx = script_ctx(&h.host);
    assert_eq!(
        h.host
            .session
            .as_ref()
            .unwrap()
            .scripting
            .scheduler
            .pending_len(),
        1,
        "installer levelLoad enrolled a real wait"
    );
    assert!(ctx.presentation_overrides.borrow().records.is_empty());
    let session = h.host.session.as_ref().unwrap();
    for _ in 0..3 {
        session.scripting.scheduler.begin_frame();
        session.scripting.scheduler.evaluate(&[]);
    }
    session.scripting.scheduler.drain_landings(
        &ctx.data_registry.borrow(),
        &session.scripting.sequence_registry,
        &session.scripting.reaction_registry,
        &session.scripting.system_registry,
        &ctx,
    );
    assert_eq!(ctx.presentation_overrides.borrow().records.len(), 1);
    h.advance(40);
    let client_ctx = script_ctx(&h.peers[0].app);
    let client_light = h.peers[0]
        .app
        .session
        .as_ref()
        .unwrap()
        .light_bridge
        .entity_for_map_index(0)
        .unwrap();
    assert!(
        client_ctx
            .registry
            .borrow()
            .get_component::<LightComponent>(client_light)
            .unwrap()
            .animation
            .is_some()
    );
    // Watch an actual settled store crossing; its immediate local-light write
    // is every-machine, while the same crossing's networked prop write records.
    use postretro_entities::{
        CrossingCondition, CrossingDescriptor, ReplicationScope, SlotOwnership, SlotRecord,
        SlotSchema, SlotType, SlotValue,
    };
    ctx.slot_table
        .borrow_mut()
        .insert(
            "fixture.health".into(),
            SlotRecord::new(SlotSchema {
                slot_type: SlotType::Number,
                default: Some(SlotValue::Number(100.0)),
                range: None,
                persist: false,
                readonly: false,
                ownership: SlotOwnership::Mod,
                network: ReplicationScope::None,
                per_owner: false,
                accumulate: None,
            }),
        )
        .unwrap();
    let prl = tagged(&ctx, "prl");
    let lit = tagged(&ctx, "lit_prop");
    ctx.presentation_overrides.borrow_mut().clear();
    ctx.data_registry.borrow_mut().populate_level(
        vec![
            primitive(
                "crossing",
                "setLightAnimation",
                Some("prl"),
                animation(2000.0, true),
            ),
            primitive(
                "crossing",
                "setLightAnimation",
                Some("lit_prop"),
                animation(2000.0, true),
            ),
        ],
        vec![CrossingDescriptor {
            slot: Some("fixture.health".into()),
            condition: CrossingCondition::Below { threshold: 50.0 },
            max: 1.0,
            edge: None,
            fire: vec!["crossing".into()],
        }],
        &[],
    );
    let mut detector = postretro_scripting_core::state_crossings::CrossingDetector::new();
    detector.initialize(&ctx.data_registry.borrow(), &ctx.slot_table.borrow(), &ctx);
    ctx.slot_table
        .borrow_mut()
        .get_mut("fixture.health")
        .unwrap()
        .write_value(Some(SlotValue::Number(20.0)));
    let session = h.host.session.as_ref().unwrap();
    let fired = postretro_sim::scripting::reactions::dispatch_state_crossings_with_sequences(
        &mut detector,
        &ctx.slot_table.borrow(),
        &ctx.data_registry.borrow(),
        &session.scripting.sequence_registry,
        &session.scripting.reaction_registry,
        &session.scripting.system_registry,
        &ctx,
    );
    assert_eq!(fired, vec!["crossing"]);
    assert!(
        !ctx.presentation_overrides
            .borrow()
            .records
            .contains_key(&(prl, ComponentKind::Light))
    );
    assert!(
        ctx.presentation_overrides
            .borrow()
            .records
            .contains_key(&(lit, ComponentKind::Light))
    );
    h.advance(12);
    let remote = client_entity(&h.peers[0], h.allocator.stamp_for_test(lit));
    assert_eq!(
        client_ctx
            .registry
            .borrow()
            .get_component::<LightComponent>(remote)
            .unwrap()
            .animation
            .as_ref()
            .unwrap()
            .period_ms,
        2000.0
    );
    assert_ne!(
        client_ctx
            .registry
            .borrow()
            .get_component::<LightComponent>(client_light)
            .unwrap()
            .animation
            .as_ref()
            .map(|animation| animation.period_ms),
        Some(2000.0)
    );
}

fn wieldable_descriptor() -> postretro_foundation::WeaponDescriptor {
    postretro_foundation::WeaponDescriptor {
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

#[test]
fn conditioned_client_dummy_hit_commits_host_damage_owner_credit_and_despawn() {
    use postretro_entities::components::{health::HealthComponent, weapon::WeaponComponent};
    use postretro_entities::slot_table::{
        ReplicationScope, SlotOwnership, SlotRecord, SlotSchema, SlotType, SlotValue,
    };
    use postretro_foundation::{ActivationLane, ImpactEventDescriptor, ShotId};
    let mut world = fixture_world();
    world.map_entities.push(MapEntityRecord {
        classname: "target_dummy".into(),
        origin: [0.0, 0.0, -5.0],
        tags: vec!["dummy".into()],
        ..Default::default()
    });
    let mut h = Harness::with_world(world);
    h.connect_late();
    h.advance(40);
    let ctx = script_ctx(&h.host);
    let target = tagged(&ctx, "dummy");
    let target_net = h.allocator.stamp(target);
    let remote = client_entity(&h.peers[0], target_net);
    let client_ctx = script_ctx(&h.peers[0].app);
    assert!(
        client_ctx
            .registry
            .borrow()
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
        h.tick,
        &[],
        &[],
        &crate::collision::CollisionWorld::new(),
        &client_ctx.registry.borrow(),
        &postretro_sim::scripting_systems::hit_zones::HitZoneStore::new(),
        0.0,
        DT as f32,
    )
    .expect("client resolves its display copy");
    assert_eq!(resolution.hits.len(), 1);
    assert_eq!(resolution.hits[0].target, remote);
    let records = resolution
        .hits
        .iter()
        .map(|hit| wire::HitRecord {
            target: h.peers[0]
                .replication
                .network_id_for_entity(hit.target)
                .unwrap()
                .0,
            point: hit.point.to_array(),
            zone: hit.zone.clone(),
            normal: hit.normal.to_array(),
        })
        .collect();
    let pawn = h.players[1].pawn;
    let weapon;
    {
        let mut registry = ctx.registry.borrow_mut();
        let mut transform = *registry.get_component::<Transform>(pawn).unwrap();
        transform.position = Vec3::ZERO;
        registry.set_component(pawn, transform).unwrap();
        weapon = registry.spawn(Transform::default());
        registry
            .set_component(
                weapon,
                WeaponComponent::from_descriptor(&wieldable_descriptor()),
            )
            .unwrap();
    }
    h.owners.set_for_test(pawn, CLIENT_A);
    let pawn_net = h.allocator.stamp(pawn);
    let shot_id = ShotId::from_parts(pawn_net.0, h.tick, ActivationLane::Primary, 0);
    // This is the host's frozen FIRE authority. Only the client's actual ray result
    // crosses the conditioned transport; no client health or damage is authoritative.
    h.open_shots.record(
        postretro_combat_model::AuthorizedShot {
            sounds: None,
            action: None,
            source_weapon: Some("weapon.test.net".into()),
            knockback: None,
            shot_id,
            pawn,
            weapon,
            fire_tick: h.tick,
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
        },
        CLIENT_A,
    );
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
        id:"dummy-death-reward".into(),is_override:false,levels:vec![],filter_tag:Some("dummy".into()),
        policy:vec![json!({"when":{"op":"le","a":{"op":"input","name":"@impact.healthAfter"},"b":{"op":"const","value":0}},
            "do":[{"primitive":"slot.set","target":"@impact.source","args":{"slot":"progression.xp","value":{"op":"add","a":{"op":"input","name":"progression.xp","owner":"@impact.source"},"b":{"op":"const","value":10}}}},
                  {"primitive":"despawn","target":"@impact.target","args":{}}]})],
    }]);
    h.install_reactions(vec![primitive(
        "impact",
        "flashScreen",
        None,
        presentation_args("flashScreen", 400, None),
    )]);
    let message = wire::encode(&wire::ClientMessage::HitDeclaration(wire::HitDeclaration {
        shot_id: wire::WireShotId {
            pawn: pawn_net.0,
            start_tick: h.tick,
            lane: 0,
            ordinal: 0,
        },
        records,
    }));
    h.peers[0].client.send_input(message.clone());
    h.peers[0].client.send_input(message);
    h.advance(12);
    let mut emissions = Vec::new();
    let accepted = netcode::host_flush_pending_hit_declarations(
        &mut h.server,
        &mut ctx.registry.borrow_mut(),
        &h.host.collision_world,
        &h.host.session.as_ref().unwrap().hit_zone_store,
        &h.allocator,
        &h.owners,
        &h.queues,
        &mut h.open_shots,
        &mut h.pending_hits,
        h.tick,
        h.clock,
        |registry| policy.evaluate_pending_in_registry(registry),
        |_, _| {},
        |emission| emissions.push(emission),
    );
    assert!(accepted);
    assert_eq!(
        ctx.registry
            .borrow()
            .get_component::<HealthComponent>(target)
            .unwrap()
            .current,
        0.0
    );
    assert_eq!(
        ctx.registry
            .borrow()
            .is_marked_for_end_of_frame_removal(target),
        Ok(true)
    );
    assert_eq!(
        emissions.len(),
        1,
        "duplicate HIT consumes no second authority"
    );
    let followups = crate::frame_loop::named_drains::drain_frame_named_events(
        crate::frame_loop::named_drains::NamedFrameEvents {
            movement: &[],
            ai: &[],
            observer: &[],
            weapon: &emissions,
            movers: &[],
            deaths: &[],
        },
        &h.host.session.as_ref().unwrap().scripting,
        &ctx,
    );
    let scripting = &h.host.session.as_ref().unwrap().scripting;
    dispatch_deferred_named_events_with_sequences(
        followups,
        &ctx.data_registry.borrow(),
        &scripting.sequence_registry,
        &scripting.reaction_registry,
        &scripting.system_registry,
        &ctx,
    );
    h.route();
    let removals = ctx.registry.borrow_mut().take_end_of_frame_removals();
    for removed in removals {
        ctx.registry.borrow_mut().despawn(removed).unwrap();
    }
    // Read each seat's committed value before networking projects owner-private rows.
    let table = ctx.slot_table.borrow();
    let xp = table.get("progression.xp").unwrap();
    for (seat, value) in [(Seat(0), 0.0), (Seat(1), 10.0), (Seat(2), 0.0)] {
        assert!(
            matches!(xp.per_seat_value(seat),Some(SlotValue::Number(actual)) if *actual==value)
        );
    }
    drop(table);
    h.advance(12);
    assert!(!ctx.registry.borrow().exists(target));
    for peer in &h.peers {
        assert!(
            !peer
                .replication
                .entity_network_ids()
                .values()
                .any(|id| *id == target_net)
        );
    }
    assert_eq!(count(&h.host_commands, 400), 1);
    assert_eq!(count(&h.peers[0].commands, 400), 0);
    assert_eq!(count(&h.peers[1].commands, 400), 0);
}
