//! App-side capture proof through shipped dispatch and sim handlers.
//! Origins passed here are explicit test inputs; actual frame producer origins
//! and the conditioned host/client route remain P15 coverage.
use super::reaction_scheduler::{ReactionScheduler, register_reaction_control_primitives};
use mlua::LuaSerdeExt;
use postretro_entities::components::agent::AgentComponent;
use postretro_entities::components::billboard_emitter::BillboardEmitterComponent;
use postretro_entities::components::brain::BrainComponent;
use postretro_entities::components::light::LightComponent;
use postretro_entities::presentation_overrides::{PresentationComponent, PresentationOverride};
use postretro_entities::provenance::{DescriptorProvenance, DescriptorSpawnPath};
use postretro_entities::{
    ComponentKind, DataRegistry, EntityId, FireOrigin, FireSubject, FogVolumeComponent, GroupKind,
    NamedReaction, PrimitiveDescriptor, ReactionDescriptor, ScriptCtx, SequenceStep, Transform,
};
use postretro_scripting_core::reaction_dispatch::{
    NamedEventDispatchContext, PrepartitionedReactionStep, ResidualOrigin,
    dispatch_deferred_named_events_with_sequences, fire_named_event_with_sequences,
    fire_prepartitioned_reactions_with_sequences,
};
use postretro_scripting_core::reaction_registry::{
    ReactionPrimitiveRegistry, SystemReactionRegistry,
};
use postretro_scripting_core::sequence::SequencedPrimitiveRegistry;
use postretro_sim::scripting::reactions::registry::{
    register_emitter_reaction_primitives, register_fog_reaction_primitives,
    register_sequenced_fog_primitives,
};
use serde_json::{Value, json};

struct Fixture {
    ctx: ScriptCtx,
    data: DataRegistry,
    seq: SequencedPrimitiveRegistry,
    reactions: ReactionPrimitiveRegistry,
    systems: SystemReactionRegistry,
    scheduler: ReactionScheduler,
}
impl Fixture {
    fn new() -> Self {
        let ctx = ScriptCtx::new();
        ctx.presentation_networked_predicate
            .set(Some(crate::netcode::is_host_replicated_descriptor_entity));
        ctx.presentation_override_validator
            .set(Some(crate::netcode::validate_presentation_override));
        let scheduler = ReactionScheduler::default();
        scheduler.set_role(crate::scripting_systems::reaction_scheduler::SchedulerRole::Host);
        let mut seq = SequencedPrimitiveRegistry::new();
        register_reaction_control_primitives(&mut seq, scheduler.clone());
        register_sequenced_fog_primitives(&mut seq, ctx.clone());
        postretro_lighting::script_primitives::register_sequenced_light_primitives(
            &mut seq,
            ctx.clone(),
        );
        let mut reactions = ReactionPrimitiveRegistry::new();
        register_emitter_reaction_primitives(&mut reactions);
        register_fog_reaction_primitives(&mut reactions);
        postretro_lighting::script_primitives::register_light_reaction_primitives(&mut reactions);
        Self {
            ctx,
            data: DataRegistry::new(),
            seq,
            reactions,
            systems: SystemReactionRegistry::new(),
            scheduler,
        }
    }
    fn frame(&self, time: f64) {
        self.ctx.presentation_overrides.borrow_mut().begin_frame();
        self.ctx.presentation_script_time.set(time);
    }
    fn install(&mut self, reactions: Vec<NamedReaction>) {
        self.data.populate_level(reactions, Vec::new(), &[]);
    }
    fn fire(&self, name: &str, origin: FireOrigin) -> Vec<(String, FireOrigin)> {
        fire_named_event_with_sequences(
            name,
            &self.data,
            &self.seq,
            &self.reactions,
            &self.systems,
            &self.ctx,
            Some(NamedEventDispatchContext {
                origin,
                source: format!("capture-test:{name}"),
                values: &[],
                emitter: None,
            }),
        )
    }
    fn spawn_emitter(&self, tag: &str, networked_npc: bool) -> EntityId {
        let mut registry = self.ctx.registry.borrow_mut();
        let id = registry.spawn(Transform::default());
        registry.set_tags(id, vec![tag.into()]).unwrap();
        let component: BillboardEmitterComponent = serde_json::from_value(json!({
            "rate": 1.0, "burst": null, "spread": 0.0, "lifetime": 5.0,
            "velocity": [0.0,1.0,0.0], "buoyancy": 0.0, "drag": 0.0,
            "size_over_lifetime": [1.0], "opacity_over_lifetime": [1.0],
            "color": [1.0,1.0,1.0], "sprite": "smoke", "spin_rate": 0.0,
            "spin_animation": null,
        }))
        .unwrap();
        registry.set_component(id, component).unwrap();
        if networked_npc {
            let graph = postretro_foundation::BehaviorGraphDescriptor {
                knockback: Default::default(),
                envelope: postretro_foundation::BehaviorGraphEnvelope {
                    initial: "idle".into(),
                    activities: std::collections::BTreeMap::from([(
                        "idle".into(),
                        postretro_foundation::BehaviorActivityDescriptor {
                            sound: None,
                            animation: None,
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
                move_speed: 1.0,
            };
            registry
                .set_component(id, BrainComponent::from_graph(&graph))
                .unwrap();
            registry
                .set_component(id, AgentComponent::new(0.4, 1.6, 0.3, 3.5))
                .unwrap();
            registry
                .set_component(
                    id,
                    DescriptorProvenance {
                        canonical_name: "capture-enemy".into(),
                        owned_components: Default::default(),
                        map_overrides: Default::default(),
                        spawn_path: DescriptorSpawnPath::MapPlacement,
                    },
                )
                .unwrap();
            assert!(crate::netcode::is_host_replicated_descriptor_entity(
                &registry, id
            ));
        }
        id
    }
    fn spawn_fog(&self) -> EntityId {
        let mut registry = self.ctx.registry.borrow_mut();
        let id = registry.spawn(Transform::default());
        registry.set_tags(id, vec!["mist".into()]).unwrap();
        registry
            .set_component(
                id,
                FogVolumeComponent {
                    density: 0.5,
                    glow: 0.0,
                    edge_softness: 0.0,
                    falloff: 1.0,
                    tint: [1.0; 3],
                    saturation: 1.0,
                    min_brightness: 0.0,
                    light_range: 1.0,
                    animation: None,
                },
            )
            .unwrap();
        id
    }
    fn record(&self, id: EntityId, kind: ComponentKind) -> PresentationOverride {
        self.ctx.presentation_overrides.borrow().records[&(id, kind)].clone()
    }
}
fn host() -> FireOrigin {
    FireOrigin::HostOnly(FireSubject::None)
}
fn primitive(name: &str, tag: &str, verb: &str, args: Value) -> NamedReaction {
    NamedReaction {
        name: name.into(),
        descriptor: ReactionDescriptor::Primitive(PrimitiveDescriptor {
            primitive: verb.into(),
            target: None,
            kind: None,
            tag: Some(tag.into()),
            on_complete: None,
            args,
        }),
    }
}
fn sequence(name: &str, id: EntityId, verb: &str, args: Value) -> NamedReaction {
    NamedReaction {
        name: name.into(),
        descriptor: ReactionDescriptor::Sequence(vec![SequenceStep {
            id: id.into(),
            primitive: verb.into(),
            args,
        }]),
    }
}
fn fog_anim(keys: Vec<f32>) -> Value {
    json!({ "periodMs": 5000.0, "playCount": 1, "density": keys })
}

#[test]
fn typed_fog_member_fade_runs_actual_sdk_builder_then_sequence_capture() {
    let mut fixture = Fixture::new();
    let id = fixture.spawn_fog();
    // Execute the shipping typed Luau member builder; the shared SDK test also
    // checks this wire against the TypeScript member builder byte for byte.
    let lua = mlua::Lua::new();
    let sdk: mlua::Table = lua
        .load(include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../sdk/lib/entities/fog_volumes.luau"
        )))
        .eval()
        .unwrap();
    lua.globals().set("fogSdk", sdk).unwrap();
    lua.globals().set("fogId", id.to_raw()).unwrap();
    let built: mlua::Value = lua.load(r#"
        local fog = fogSdk.wrapFogVolumeEntity({ id = fogId, position = { x=0,y=0,z=0 }, tags={}, component={} })
        return fog:fade({ from=0.6, to=0, periodMs=5000 })
    "#).eval().unwrap();
    let wire: Value = lua.from_value(built).unwrap();
    let manifest = lua
        .to_value(&json!({ "name": "fade", "sequence": wire }))
        .unwrap();
    let reaction = postretro_scripting_core::data_descriptors::named_reaction_from_lua(manifest)
        .expect("shipping builder output passes the production reaction parser");
    let ReactionDescriptor::Sequence(steps) = &reaction.descriptor else {
        panic!("sequence")
    };
    assert_eq!(steps[0].primitive, "setFogAnimation");
    assert_eq!(steps[0].args["playCount"], 1);
    assert_eq!(steps[0].args["density"].as_array().unwrap().len(), 16);
    fixture.install(vec![reaction]);
    fixture.frame(7.0);
    fixture.fire("fade", host());
    let record = fixture.record(id, ComponentKind::FogVolume);
    assert_eq!(record.anim_start, Some(7.0));
    let mut bridge = super::fog_volume_bridge::FogVolumeBridge::new();
    bridge.tick(&mut fixture.ctx.registry.borrow_mut(), 7.0);
    bridge.tick(&mut fixture.ctx.registry.borrow_mut(), 9.0);
    assert_eq!(fixture.record(id, ComponentKind::FogVolume), record);
    let PresentationComponent::Fog(component) = record.component else {
        panic!("fog")
    };
    assert_eq!(component.animation.unwrap().density.unwrap().len(), 16);
}

#[test]
fn primitive_tag_and_npc_group_capture_only_networked_every_machine_writes() {
    let mut fixture = Fixture::new();
    let local = fixture.spawn_emitter("smoke", false);
    let enemy = fixture.spawn_emitter("smoke", true);
    let mut group = primitive("group", "smoke", "setEmitterRate", json!({"rate": 9.0}));
    let ReactionDescriptor::Primitive(p) = &mut group.descriptor else {
        unreachable!()
    };
    p.kind = Some(GroupKind::Npc);
    fixture.install(vec![
        primitive("tag", "smoke", "setEmitterRate", json!({"rate": 4.0})),
        group,
    ]);
    fixture.frame(2.0);
    fixture.fire(
        "tag",
        FireOrigin::EveryMachine(postretro_entities::MachineSource::HostMirrored),
    );
    assert!(
        !fixture
            .ctx
            .presentation_overrides
            .borrow()
            .records
            .contains_key(&(local, ComponentKind::BillboardEmitter))
    );
    assert!(
        fixture
            .ctx
            .presentation_overrides
            .borrow()
            .frame_start
            .contains_key(&(local, ComponentKind::BillboardEmitter))
    );
    let PresentationComponent::Emitter(c) = fixture
        .record(enemy, ComponentKind::BillboardEmitter)
        .component
    else {
        panic!("emitter")
    };
    assert_eq!(c.rate, 4.0);
    fixture.fire(
        "group",
        FireOrigin::EveryMachine(postretro_entities::MachineSource::HostMirrored),
    );
    let PresentationComponent::Emitter(c) = fixture
        .record(enemy, ComponentKind::BillboardEmitter)
        .component
    else {
        panic!("emitter")
    };
    assert_eq!(c.rate, 9.0);
    assert_eq!(
        fixture
            .ctx
            .registry
            .borrow()
            .get_component::<BillboardEmitterComponent>(local)
            .unwrap()
            .rate,
        4.0
    );
    fixture.fire("tag", host());
    assert!(
        fixture
            .ctx
            .presentation_overrides
            .borrow()
            .records
            .contains_key(&(local, ComponentKind::BillboardEmitter))
    );
}

#[test]
fn first_unrecorded_write_then_host_restore_compares_against_frame_start() {
    let mut fixture = Fixture::new();
    let id = fixture.spawn_fog();
    fixture.install(vec![
        sequence("change", id, "setFogAnimation", fog_anim(vec![0.0, 1.0])),
        sequence("restore", id, "setFogAnimation", Value::Null),
    ]);
    fixture.frame(3.0);
    fixture.fire(
        "change",
        FireOrigin::EveryMachine(postretro_entities::MachineSource::HostMirrored),
    );
    assert!(
        fixture
            .ctx
            .presentation_overrides
            .borrow()
            .records
            .is_empty()
    );
    fixture.fire("restore", host());
    let record = fixture.record(id, ComponentKind::FogVolume);
    assert_eq!(record.anim_start, None);
    let PresentationComponent::Fog(c) = record.component else {
        panic!("fog")
    };
    assert!(c.animation.is_none());
}

#[test]
fn scalar_identical_restore_and_double_write_keep_original_animation_start() {
    let mut fixture = Fixture::new();
    let id = fixture.spawn_fog();
    let original = fog_anim(vec![0.1, 0.9]);
    fixture.install(vec![
        sequence("original", id, "setFogAnimation", original),
        sequence("changed", id, "setFogAnimation", fog_anim(vec![0.9, 0.1])),
        sequence("scalar", id, "setFogDensity", json!({"density":0.75})),
    ]);
    fixture.frame(1.0);
    fixture.fire("original", host());
    assert_eq!(
        fixture.record(id, ComponentKind::FogVolume).anim_start,
        Some(1.0)
    );
    fixture.frame(4.0);
    fixture.fire("scalar", host());
    fixture.fire("original", host());
    assert_eq!(
        fixture.record(id, ComponentKind::FogVolume).anim_start,
        Some(1.0)
    );
    fixture.frame(5.0);
    fixture.fire("changed", host());
    fixture.fire("original", host());
    let final_record = fixture.record(id, ComponentKind::FogVolume);
    assert_eq!(final_record.anim_start, Some(1.0));
    let PresentationComponent::Fog(c) = final_record.component else {
        panic!("fog")
    };
    assert_eq!(c.animation.unwrap().density.unwrap(), vec![0.1, 0.9]);
}

#[test]
fn prepartitioned_host_followup_and_real_wait_landing_capture_without_subject() {
    let mut fixture = Fixture::new();
    let id = fixture.spawn_emitter("mist", false);
    let mut first = primitive("first", "mist", "setEmitterRate", json!({"rate":2.0}));
    let ReactionDescriptor::Primitive(p) = &mut first.descriptor else {
        unreachable!()
    };
    p.on_complete = Some("follow".into());
    let follow = primitive("follow", "mist", "setEmitterRate", json!({"rate":3.0}));
    let delayed = NamedReaction {
        name: "delayed".into(),
        descriptor: ReactionDescriptor::Sequence(vec![
            SequenceStep {
                id: postretro_entities::SequenceTarget::Wait,
                primitive: "wait".into(),
                args: json!({"durationMs":1.0}),
            },
            SequenceStep {
                id: postretro_entities::SequenceTarget::Group(postretro_entities::GroupTarget {
                    kind: GroupKind::Npc,
                    tag: Some("unmatched".into()),
                }),
                primitive: "setEmitterRate".into(),
                args: json!({"rate":99.0}),
            },
            // Member emitter steps are not registered sequenced; use a real `fire`
            // control landing to reach the production tag handler.
            SequenceStep {
                id: postretro_entities::SequenceTarget::Fire,
                primitive: "fire".into(),
                args: json!({"event":"follow"}),
            },
        ]),
    };
    fixture.install(vec![first.clone(), follow, delayed]);
    fixture.frame(6.0);
    let previous = fixture.ctx.system_commands.replace_fire_context(
        postretro_entities::SystemCommandFireContext {
            origin: host(),
            ..Default::default()
        },
    );
    let followups = fire_prepartitioned_reactions_with_sequences(
        &[PrepartitionedReactionStep::Descriptor(
            "first".into(),
            0,
            first.descriptor,
        )],
        &fixture.seq,
        &fixture.reactions,
        &fixture.systems,
        &fixture.ctx,
        ResidualOrigin::ResumedTail,
    );
    fixture.ctx.system_commands.replace_fire_context(previous);
    assert_eq!(followups[0].1, host());
    dispatch_deferred_named_events_with_sequences(
        followups,
        &fixture.data,
        &fixture.seq,
        &fixture.reactions,
        &fixture.systems,
        &fixture.ctx,
    );
    let PresentationComponent::Emitter(c) = fixture
        .record(id, ComponentKind::BillboardEmitter)
        .component
    else {
        panic!("emitter")
    };
    assert_eq!(c.rate, 3.0);
    fixture.fire("delayed", host());
    fixture
        .ctx
        .presentation_overrides
        .borrow_mut()
        .records
        .clear();
    fixture.frame(7.0);
    fixture.scheduler.begin_frame();
    fixture.scheduler.evaluate(&[]);
    fixture.scheduler.drain_landings(
        &fixture.data,
        &fixture.seq,
        &fixture.reactions,
        &fixture.systems,
        &fixture.ctx,
    );
    assert!(
        fixture
            .ctx
            .presentation_overrides
            .borrow()
            .records
            .contains_key(&(id, ComponentKind::BillboardEmitter))
    );
}

#[test]
fn oversized_curve_warns_once_and_keeps_last_valid_override() {
    let mut fixture = Fixture::new();
    let id = fixture.spawn_fog();
    fixture.install(vec![
        sequence("valid", id, "setFogAnimation", fog_anim(vec![0.1, 0.9])),
        sequence(
            "oversized",
            id,
            "setFogAnimation",
            fog_anim(vec![1.0; postretro_net::wire::MAX_WIRE_CURVE_KEYS + 1]),
        ),
    ]);
    fixture.frame(2.0);
    fixture.fire("valid", host());
    let valid = fixture.record(id, ComponentKind::FogVolume);
    let capture = crate::scripting::reactions::log_capture::capture(|| {
        fixture.frame(3.0);
        fixture.fire("oversized", host());
        fixture.fire("oversized", host());
        fixture.frame(4.0);
        fixture.fire("oversized", host());
    });
    assert_eq!(fixture.record(id, ComponentKind::FogVolume), valid);
    assert_eq!(
        capture
            .iter()
            .filter(|(level, message)| *level == log::Level::Warn
                && message.contains("cannot be encoded"))
            .count(),
        1
    );
}

#[test]
fn real_light_sequence_captures_component_and_start() {
    let mut fixture = Fixture::new();
    let id = fixture
        .ctx
        .registry
        .borrow_mut()
        .spawn(Transform::default());
    let component: LightComponent = serde_json::from_value(json!({
        "origin":[0.0,0.0,0.0],"lightType":"Point","intensity":1.0,"color":[1.0,1.0,1.0],
        "falloffModel":"InverseSquared","falloffRange":10.0,"coneAngleInner":null,
        "coneAngleOuter":null,"coneDirection":null,"isDynamic":true,
    }))
    .unwrap();
    fixture
        .ctx
        .registry
        .borrow_mut()
        .set_component(id, component)
        .unwrap();
    fixture.install(vec![sequence(
        "light",
        id,
        "setLightAnimation",
        json!({"periodMs":5000.0,"playCount":1,"brightness":[0.0,1.0]}),
    )]);
    fixture.frame(8.0);
    fixture.fire("light", host());
    assert_eq!(
        fixture.record(id, ComponentKind::Light).anim_start,
        Some(8.0)
    );
}

// Regression: authored tag clears became {} before the registered light handler.
#[test]
fn sdk_light_animation_tag_clears_match_member_clear_and_capture_in_both_runtimes() {
    use postretro_level_format::data_script::DataScriptSection;
    use postretro_scripting_core::primitives_registry::PrimitiveRegistry;
    use postretro_scripting_core::runtime::{ScriptRuntime, ScriptRuntimeConfig};
    use postretro_test_log_capture::LogCapture;

    for luau in [false, true] {
        let mut fixture = Fixture::new();
        let id = fixture
            .ctx
            .registry
            .borrow_mut()
            .spawn(Transform::default());
        let component: LightComponent = serde_json::from_value(json!({
            "origin":[0.0,0.0,0.0],"lightType":"Point","intensity":1.0,"color":[1.0,1.0,1.0],
            "falloffModel":"InverseSquared","falloffRange":10.0,"coneAngleInner":null,
            "coneAngleOuter":null,"coneDirection":null,"isDynamic":true,
        }))
        .unwrap();
        {
            let mut registry = fixture.ctx.registry.borrow_mut();
            registry.set_component(id, component).unwrap();
            registry.set_tags(id, vec!["alarm".into()]).unwrap();
        }
        let source = if luau {
            r#"
                local P = require("postretro")
                function setupLevel()
                  return { reactions = {
                    P.defineReaction("start", { primitive = "setLightAnimation", tag = "alarm", args = { periodMs = 5000, playCount = 1, brightness = { 0.2, 0.8 } } }),
                    P.defineReaction("clear", { primitive = "setLightAnimation", tag = "alarm", args = nil }),
                    P.defineReaction("clearMissing", { primitive = "setLightAnimation", tag = "alarm" }),
                    P.defineReaction("clearUndefined", { primitive = "setLightAnimation", tag = "alarm", args = nil }),
                    P.defineReaction("memberClear", { sequence = { { id = __MEMBER_ID__, primitive = "setLightAnimation", args = nil } } }),
                    P.defineReaction("invalid", { primitive = "setLightAnimation", tag = "alarm", args = {} }),
                  } }
                end
            "#
        } else {
            r#"
                import { defineReaction } from "postretro";
                export function setupLevel() {
                  return { reactions: [
                    defineReaction("start", { primitive: "setLightAnimation", tag: "alarm", args: { periodMs: 5000, playCount: 1, brightness: [0.2, 0.8] } }),
                    defineReaction("clear", { primitive: "setLightAnimation", tag: "alarm", args: null }),
                    defineReaction("clearMissing", { primitive: "setLightAnimation", tag: "alarm" }),
                    defineReaction("clearUndefined", { primitive: "setLightAnimation", tag: "alarm", args: undefined }),
                    defineReaction("memberClear", { sequence: [{ id: __MEMBER_ID__, primitive: "setLightAnimation", args: null }] }),
                    defineReaction("invalid", { primitive: "setLightAnimation", tag: "alarm", args: {} }),
                  ] };
                }
            "#
        }
        .replace("__MEMBER_ID__", &id.to_raw().to_string());
        let dir = tempfile::tempdir().unwrap();
        let entry = dir
            .path()
            .join(if luau { "level.luau" } else { "level.ts" });
        std::fs::write(&entry, &source).unwrap();
        let compiled = if luau {
            source
        } else {
            postretro_script_compiler::bundle_entry(&entry).expect("TS fixture bundles")
        };
        let runtime = ScriptRuntime::new(
            &PrimitiveRegistry::new(),
            &ScriptRuntimeConfig::default(),
            &fixture.ctx,
        )
        .unwrap();
        let manifest = runtime.run_data_script(
            &DataScriptSection {
                compiled_bytes: compiled.into_bytes(),
                source_path: entry.to_string_lossy().into_owned(),
            },
            dir.path(),
        );
        assert_eq!(manifest.reactions.len(), 6, "luau={luau}");
        for reaction in &manifest.reactions[1..4] {
            let ReactionDescriptor::Primitive(descriptor) = &reaction.descriptor else {
                panic!("tag clear")
            };
            assert!(descriptor.args.is_null(), "{}: luau={luau}", reaction.name);
        }
        fixture.install(manifest.reactions);

        let capture = LogCapture::start();
        let mut member_clear = None;
        for clear in ["memberClear", "clear", "clearMissing", "clearUndefined"] {
            fixture.frame(8.0);
            fixture.fire("start", host());
            let running = fixture.record(id, ComponentKind::Light);
            let PresentationComponent::Light(light) = &running.component else {
                panic!("light")
            };
            let animation = light
                .animation
                .as_ref()
                .expect("valid tag animation installs");
            assert!((animation.period_ms - 5000.0).abs() < 1e-5);
            assert_eq!(animation.play_count, Some(1));
            assert_eq!(animation.brightness.as_ref().unwrap().len(), 2);

            fixture.frame(8.5);
            capture.clear();
            fixture.fire("invalid", host());
            capture.assert_logged_once(log::Level::Warn, "failed to deserialize args");
            assert_eq!(fixture.record(id, ComponentKind::Light), running);
            assert_eq!(
                fixture
                    .ctx
                    .registry
                    .borrow()
                    .get_component::<LightComponent>(id)
                    .unwrap(),
                light,
                "invalid object must preserve the running component"
            );

            fixture.frame(9.0);
            capture.clear();
            fixture.fire(clear, host());
            capture.assert_not_logged(log::Level::Warn, "setLightAnimation");
            assert!(
                fixture
                    .ctx
                    .registry
                    .borrow()
                    .get_component::<LightComponent>(id)
                    .unwrap()
                    .animation
                    .is_none(),
                "{clear}: luau={luau}"
            );
            let cleared = fixture.record(id, ComponentKind::Light);
            let PresentationComponent::Light(light) = &cleared.component else {
                panic!("light")
            };
            assert!(
                light.animation.is_none(),
                "clear is captured under the host origin"
            );
            assert!((cleared.anim_start.unwrap() - 9.0).abs() < 1e-9);
            if let Some(expected) = &member_clear {
                assert_eq!(
                    &cleared, expected,
                    "tag/member parity: {clear}, luau={luau}"
                );
            } else {
                member_clear = Some(cleared);
            }
        }
    }
}

#[test]
fn actual_cpu_install_starts_level_load_and_already_met_progress_at_zero_clock() {
    use crate::startup::lifecycle::tests::{
        install_capture_world, level_world, script_ctx, test_app,
    };
    use postretro_entities::components::health::HealthComponent;
    use postretro_entities::data_descriptors::HealthDescriptor;
    let mut app = test_app();
    let ctx = script_ctx(&app);
    let emitter: BillboardEmitterComponent = serde_json::from_value(json!({
        "rate":1.0,"burst":null,"spread":0.0,"lifetime":5.0,"velocity":[0.0,1.0,0.0],
        "buoyancy":0.0,"drag":0.0,"size_over_lifetime":[1.0],"opacity_over_lifetime":[1.0],
        "color":[1.0,1.0,1.0],"sprite":"smoke","spin_rate":0.0,"spin_animation":null,
    }))
    .unwrap();
    let mut ids = Vec::new();
    for tag in ["install-load", "install-progress"] {
        let mut registry = ctx.registry.borrow_mut();
        let id = registry.spawn(Transform::default());
        registry.set_tags(id, vec![tag.into()]).unwrap();
        registry.set_component(id, emitter.clone()).unwrap();
        registry
            .set_component(
                id,
                HealthComponent::from_descriptor(&HealthDescriptor {
                    max: 100.0,
                    hitbox: None,
                    zone_multipliers: Default::default(),
                }),
            )
            .unwrap();
        registry
            .set_component(
                id,
                DescriptorProvenance {
                    canonical_name: tag.into(),
                    owned_components: Default::default(),
                    map_overrides: Default::default(),
                    spawn_path: DescriptorSpawnPath::MapPlacement,
                },
            )
            .unwrap();
        ids.push(id);
    }
    let spin = json!({"animation":{"duration":5.0,"rate_curve":[0.0,1.0]}});
    ctx.data_registry
        .borrow_mut()
        .replace_global_reactions(vec![
            postretro_entities::ScopedReaction {
                reaction: primitive("levelLoad", "install-load", "setSpinRate", spin.clone()),
                levels: vec![],
            },
            postretro_entities::ScopedReaction {
                reaction: NamedReaction {
                    name: "already-met".into(),
                    descriptor: ReactionDescriptor::Progress(
                        postretro_entities::ProgressDescriptor {
                            tag: "install-progress".into(),
                            at: 0.0,
                            fire: "progress-spin".into(),
                        },
                    ),
                },
                levels: vec![],
            },
            postretro_entities::ScopedReaction {
                reaction: primitive("progress-spin", "install-progress", "setSpinRate", spin),
                levels: vec![],
            },
        ]);
    register_emitter_reaction_primitives(
        &mut app.session.as_mut().unwrap().scripting.reaction_registry,
    );
    app.script_time = 17.0;
    ctx.presentation_script_time.set(17.0);
    ctx.presentation_overrides
        .borrow_mut()
        .warned_writes
        .insert((ids[0], "old-level".into()));
    install_capture_world(&mut app, &level_world("capture-clock", 1));
    assert!(ctx.presentation_script_time.get().abs() < f64::EPSILON);
    assert!(app.script_time.abs() < f64::EPSILON);
    assert!(ctx.presentation_overrides.borrow().warned_writes.is_empty());
    for &id in &ids {
        let table = ctx.presentation_overrides.borrow();
        let record = &table.records[&(id, ComponentKind::BillboardEmitter)];
        assert_eq!(record.anim_start, Some(0.0));
        let PresentationComponent::Emitter(component) = &record.component else {
            panic!("emitter")
        };
        assert!(component.spin_animation.is_some());
    }
    // Regression: first observation treated an install-time tween as a running-frame
    // write and skipped this delta, although its immutable start was already zero.
    assert_eq!(
        app.session.as_ref().unwrap().particle_pool.len(),
        0,
        "install primes clocks without emitting particles",
    );
    let session = app.session.as_mut().unwrap();
    session.emitter_bridge.update(
        &mut ctx.registry.borrow_mut(),
        0.5,
        0.5,
        0.5,
        &mut session.particle_pool,
        &mut session.particle_render,
    );
    for id in ids {
        assert!(
            (ctx.registry
                .borrow()
                .get_component::<BillboardEmitterComponent>(id)
                .unwrap()
                .spin_rate
                - 0.1)
                .abs()
                < 1e-5
        );
        assert_eq!(
            ctx.presentation_overrides.borrow().records[&(id, ComponentKind::BillboardEmitter)]
                .anim_start,
            Some(0.0)
        );
    }
}

#[test]
fn emitter_rate_and_identical_tween_keep_start_and_bridge_samples_do_not_mutate_capture() {
    let mut fixture = Fixture::new();
    let id = fixture.spawn_emitter("tween", false);
    fixture.install(vec![
        primitive(
            "spin",
            "tween",
            "setSpinRate",
            json!({"animation":{"duration":5.0,"rate_curve":[0.0,1.0]}}),
        ),
        primitive("rate", "tween", "setEmitterRate", json!({"rate":0.0})),
    ]);
    fixture.frame(1.0);
    fixture.fire("spin", host());
    fixture.frame(3.0);
    fixture.fire("rate", host());
    fixture.fire("spin", host());
    let immutable = fixture.record(id, ComponentKind::BillboardEmitter);
    assert_eq!(immutable.anim_start, Some(1.0));
    let mut bridge = super::emitter_bridge::EmitterBridge::new();
    let mut particle_pool = postretro_sim::particle_pool::ParticlePool::new();
    let mut particle_render = super::particle_render::ParticleRenderCollector::new();
    assert!(bridge.seed_replicated_state(&fixture.ctx.registry.borrow(), id, Some(2.0)));
    bridge.update(
        &mut fixture.ctx.registry.borrow_mut(),
        0.1,
        0.1,
        3.0,
        &mut particle_pool,
        &mut particle_render,
    );
    assert_eq!(
        fixture.record(id, ComponentKind::BillboardEmitter),
        immutable
    );
    assert!(
        (fixture
            .ctx
            .registry
            .borrow()
            .get_component::<BillboardEmitterComponent>(id)
            .unwrap()
            .spin_rate
            - 0.4)
            .abs()
            < 1e-5
    );
}

#[test]
fn all_nine_replicated_primitives_apply_and_capture_through_shipping_dispatch() {
    use postretro_scripting_core::reaction_dispatch::presentation_capture::REPLICATED_PRESENTATION_PRIMITIVES;
    let rows = [
        (
            "setLightAnimation",
            ComponentKind::Light,
            json!({"periodMs":5000.0,"playCount":1,"brightness":[0.0,1.0]}),
        ),
        (
            "setFogDensity",
            ComponentKind::FogVolume,
            json!({"density":0.8}),
        ),
        ("setFogGlow", ComponentKind::FogVolume, json!({"glow":0.3})),
        (
            "setFogEdgeSoftness",
            ComponentKind::FogVolume,
            json!({"edgeSoftness":0.4}),
        ),
        (
            "setFogFalloff",
            ComponentKind::FogVolume,
            json!({"falloff":2.0}),
        ),
        (
            "setFogParams",
            ComponentKind::FogVolume,
            json!({"tint":[0.2,0.4,0.6],"saturation":0.7,"minBrightness":0.1,"lightRange":2.0}),
        ),
        (
            "setFogAnimation",
            ComponentKind::FogVolume,
            fog_anim(vec![0.1, 0.9]),
        ),
        (
            "setEmitterRate",
            ComponentKind::BillboardEmitter,
            json!({"rate":8.0}),
        ),
        (
            "setSpinRate",
            ComponentKind::BillboardEmitter,
            json!({"animation":{"duration":5.0,"rate_curve":[0.0,1.0]}}),
        ),
    ];
    let expected: std::collections::BTreeSet<_> = rows.iter().map(|(name, _, _)| *name).collect();
    assert_eq!(expected.len(), 9);
    assert_eq!(
        expected,
        REPLICATED_PRESENTATION_PRIMITIVES.into_iter().collect()
    );
    // Isolate this vocabulary from the fixture's combat/animation helpers.
    // Every registration added by these shipping functions must be accounted for.
    let registration_ctx = ScriptCtx::new();
    let mut tag_registry = ReactionPrimitiveRegistry::new();
    register_fog_reaction_primitives(&mut tag_registry);
    postretro_lighting::script_primitives::register_light_reaction_primitives(&mut tag_registry);
    register_emitter_reaction_primitives(&mut tag_registry);
    let mut sequence_registry = SequencedPrimitiveRegistry::new();
    register_sequenced_fog_primitives(&mut sequence_registry, registration_ctx.clone());
    postretro_lighting::script_primitives::register_sequenced_light_primitives(
        &mut sequence_registry,
        registration_ctx,
    );
    let mut tag_names: std::collections::BTreeSet<_> = tag_registry.names().collect();
    // The public sim emitter-registration wrapper retains these two legacy
    // non-presentation registrations. Account for them explicitly rather than
    // filtering by name patterns: any other added registration must fail here.
    assert!(tag_names.remove("applyDamage"));
    assert!(tag_names.remove("setAnimationState"));
    let sequence_names: std::collections::BTreeSet<_> = sequence_registry.names().collect();
    let expected_tag: std::collections::BTreeSet<_> =
        rows.iter().map(|(name, _, _)| *name).collect();
    let expected_sequence: std::collections::BTreeSet<_> = rows
        .iter()
        .filter(|(_, kind, _)| *kind != ComponentKind::BillboardEmitter)
        .map(|(name, _, _)| *name)
        .collect();
    assert_eq!(
        tag_names, expected_tag,
        "closed shipping fog/emitter tag registration set"
    );
    assert_eq!(
        sequence_names, expected_sequence,
        "closed shipping light/fog sequence registration set"
    );
    assert_eq!(
        tag_names
            .union(&sequence_names)
            .copied()
            .collect::<std::collections::BTreeSet<_>>(),
        expected
    );
    for (verb, kind, args) in rows {
        for sequence_path in [false, true] {
            let mut fixture = Fixture::new();
            let registered = if sequence_path {
                fixture.seq.contains(verb)
            } else {
                fixture.reactions.contains(verb)
            };
            // Light and fog ship both handlers; emitter has tag/group handlers.
            let expected_registered = match kind {
                ComponentKind::Light => true,
                ComponentKind::BillboardEmitter => !sequence_path,
                ComponentKind::FogVolume => true,
                _ => unreachable!(),
            };
            assert_eq!(
                registered, expected_registered,
                "shipping registration {verb}, sequence={sequence_path}"
            );
            if !registered {
                continue;
            }
            let (id, tag) = match kind {
                ComponentKind::FogVolume => (fixture.spawn_fog(), "mist"),
                ComponentKind::BillboardEmitter => {
                    (fixture.spawn_emitter("capture", false), "capture")
                }
                ComponentKind::Light => {
                    let id = fixture
                        .ctx
                        .registry
                        .borrow_mut()
                        .spawn(Transform::default());
                    let component: LightComponent = serde_json::from_value(json!({
                        "origin":[0.0,0.0,0.0],"lightType":"Point","intensity":1.0,"color":[1.0,1.0,1.0],
                        "falloffModel":"InverseSquared","falloffRange":10.0,"coneAngleInner":null,
                        "coneAngleOuter":null,"coneDirection":null,"isDynamic":true,
                    })).unwrap();
                    fixture
                        .ctx
                        .registry
                        .borrow_mut()
                        .set_component(id, component)
                        .unwrap();
                    fixture
                        .ctx
                        .registry
                        .borrow_mut()
                        .set_tags(id, vec!["lamp".into()])
                        .unwrap();
                    (id, "lamp")
                }
                _ => unreachable!(),
            };
            let before =
                PresentationComponent::snapshot(&fixture.ctx.registry.borrow(), id, kind).unwrap();
            let reaction = if sequence_path {
                sequence("apply", id, verb, args.clone())
            } else {
                primitive("apply", tag, verb, args.clone())
            };
            fixture.install(vec![reaction]);
            fixture.frame(12.0);
            fixture.fire("apply", host());
            let after =
                PresentationComponent::snapshot(&fixture.ctx.registry.borrow(), id, kind).unwrap();
            assert_ne!(
                before, after,
                "production handler must mutate {verb}, sequence={sequence_path}"
            );
            let record = fixture.record(id, kind);
            assert_eq!(
                record.component, after,
                "capture must match written {verb}, sequence={sequence_path}"
            );
            assert!(crate::netcode::validate_presentation_override(&record));
        }
    }
}
