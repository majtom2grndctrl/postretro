//! Level unload and reinstall tests: what unload keeps, what it releases, and
//! that a reinstall carries no residue.
//! See: context/lib/boot_sequence.md §4

use super::lifecycle_test_fixtures::*;
use super::*;

use std::collections::BTreeMap;

use postretro_entities::{NamedReaction, PrimitiveDescriptor, ReactionDescriptor};
use postretro_entities::{SlotOwnership, SlotRecord, SlotSchema, SlotType, SlotValue, Transform};
use postretro_foundation::ModMapEntry;

const FIXTURE_MAP_B: &str = "fixture_map_b_combat_lab";

fn named_reaction(name: &str) -> NamedReaction {
    NamedReaction {
        name: name.to_string(),
        descriptor: ReactionDescriptor::Primitive(PrimitiveDescriptor {
            primitive: "testPrimitive".to_string(),
            target: None,
            kind: None,
            tag: None,
            on_complete: None,
            args: serde_json::Value::Object(Default::default()),
        }),
    }
}

struct CpuFixture {
    name: &'static str,
    reaction_name: &'static str,
    light_tag: &'static str,
    fog_tag: &'static str,
    light_count: usize,
    fog_count: usize,
    triangle_count: usize,
}

fn install_cpu_fixture(app: &mut App, fixture: CpuFixture) {
    app.level = Some(level_world(fixture.name, fixture.triangle_count));
    let ctx = script_ctx(app);
    ctx.data_registry.borrow_mut().populate_level(
        vec![named_reaction(fixture.reaction_name)],
        Vec::new(),
        &[],
    );
    app.session
        .as_mut()
        .expect("test app session installed")
        .scripting
        .slot_accumulator_bindings
        .rebuild(&ctx);

    let lights = (0..fixture.light_count)
        .map(|i| map_light(fixture.light_tag, [i as f64, 2.0, 3.0]))
        .collect::<Vec<_>>();
    let fog_records = (0..fixture.fog_count)
        .map(|i| fog_record(fixture.fog_tag, i as f32 * 4.0))
        .collect::<Vec<_>>();
    {
        let session = app.session.as_mut().expect("test app session installed");
        session
            .light_bridge
            .populate_from_level(&lights, &mut ctx.registry.borrow_mut(), 0);
        session
            .fog_volume_bridge
            .populate_from_level(&mut ctx.registry.borrow_mut(), &fog_records);
    }

    if let Some(world) = app.level.as_ref() {
        app.collision_world = crate::collision::CollisionWorld::from_level(world)
            .expect("fixture collision mesh is valid");
    }
}

#[test]
fn unload_level_preserves_slot_table_and_entity_type_registry() {
    let mut app = test_app();
    script_ctx(&app)
        .slot_table
        .borrow_mut()
        .insert_namespace(
            "test.global",
            vec![(
                "score".to_string(),
                SlotRecord::new(SlotSchema {
                    slot_type: SlotType::Number,
                    default: Some(SlotValue::Number(42.0)),
                    range: None,
                    persist: true,
                    readonly: false,
                    ownership: SlotOwnership::Mod,
                    network: postretro_entities::ReplicationScope::None,
                    per_owner: false,
                    accumulate: Some(postretro_foundation::IrNode::Input {
                        name: "@dt".to_string(),
                        owner: None,
                    }),
                }),
            )],
        )
        .unwrap();
    script_ctx(&app)
        .data_registry
        .borrow_mut()
        .upsert_entity_type(descriptor("global_grunt"));

    install_cpu_fixture(
        &mut app,
        CpuFixture {
            name: FIXTURE_MAP_A,
            reaction_name: "reactorWave",
            light_tag: "reactor_only",
            fog_tag: "reactor_fog",
            light_count: 1,
            fog_count: 1,
            triangle_count: 2,
        },
    );
    app.script_time = 12.5;
    app.anim_time = 3.25;
    app.session
        .as_mut()
        .expect("test app session installed")
        .presentation_cells
        .write(
            "level_panel".to_string(),
            "count".to_string(),
            SlotValue::Number(11.0),
        );
    assert!(
        !app.session
            .as_ref()
            .expect("test app session installed")
            .presentation_cells
            .snapshot()
            .is_empty()
    );
    assert!(
        !app.session
            .as_ref()
            .expect("test app session installed")
            .scripting
            .slot_accumulator_bindings
            .is_empty(),
        "level install must bind the declared accumulator"
    );

    let presentation_target = {
        let ctx = script_ctx(&app);
        let mut registry = ctx.registry.borrow_mut();
        let target = registry.spawn(Transform::default());
        registry.push_presentation_spawn(postretro_entities::PresentationSpawn {
            world_anchor: Vec3::ZERO,
            template: "old-level-number".into(),
            facts: BTreeMap::new(),
            presenter: None,
            lifetime_seconds: 1.0,
            motion: postretro_foundation::PresentationMotion::default(),
            fade: postretro_foundation::PresentationFade::default(),
            scatter_radius: 0.0,
        });
        target
    };
    {
        let session = app.session.as_mut().expect("test app session installed");
        let mut registry = session.scripting.script_ctx.registry.borrow_mut();
        let _ = session.presentation_pool.advance_and_collect_inputs(
            &mut registry,
            0.0,
            glam::Mat4::IDENTITY,
            [800, 600],
            crate::presentation_pool::MotionPreference::Full,
        );
        session.presentation_pool.refresh_overlay(
            presentation_target,
            postretro_entities::PresentationTemplateHandle::from("old-level-overlay"),
            1.0,
            1,
            u64::from(presentation_target.to_raw()),
        );
        registry.push_presentation_spawn(postretro_entities::PresentationSpawn {
            world_anchor: Vec3::ZERO,
            template: "queued-old-level-number".into(),
            facts: BTreeMap::new(),
            presenter: None,
            lifetime_seconds: 1.0,
            motion: postretro_foundation::PresentationMotion::default(),
            fade: postretro_foundation::PresentationFade::default(),
            scatter_radius: 0.0,
        });
        assert!(
            session
                .client_overlay_facts
                .receive(postretro_net::wire::WireOverlayRoster {
                    revision: 91,
                    server_script_time: 1.0,
                    entries: Vec::new(),
                },)
        );
    }

    let slots_before = slot_snapshot(&app);
    script_ctx(&app)
        .data_registry
        .borrow_mut()
        .replace_maps(vec![ModMapEntry {
            id: "e1m1".to_string(),
            path: "maps/e1m1.prl".to_string(),
            name: "Entryway".to_string(),
            tags: vec!["campaign".to_string()],
            loading_tree: Vec::new(),
        }]);
    let data_before = {
        let ctx = script_ctx(&app);
        let data_registry = ctx.data_registry.borrow();
        (data_registry.entities.clone(), data_registry.maps.clone())
    };

    app.unload_level();

    assert_eq!(slot_snapshot(&app), slots_before);
    let data_after = {
        let ctx = script_ctx(&app);
        let data_registry = ctx.data_registry.borrow();
        (data_registry.entities.clone(), data_registry.maps.clone())
    };
    assert_eq!(data_after, data_before);
    assert!(app.boot_state == BootState::Frontend);
    assert!(app.level.is_none());
    assert!(
        app.session
            .as_ref()
            .expect("test app session installed")
            .scripting
            .slot_accumulator_bindings
            .is_empty(),
        "level unload must drop every bound accumulator program"
    );
    assert_eq!(app.script_time, 0.0);
    assert_eq!(app.anim_time, 0.0);
    assert!(
        app.session
            .as_ref()
            .expect("test app session installed")
            .presentation_cells
            .snapshot()
            .is_empty()
    );
    let session = app.session.as_ref().expect("test app session installed");
    assert_eq!(session.presentation_pool.live_counts(), (0, 0));
    assert_eq!(session.client_overlay_facts.revision(), None);
    assert!(
        session
            .scripting
            .script_ctx
            .registry
            .borrow_mut()
            .take_presentation_spawns()
            .is_empty(),
        "level unload must discard queued world presentation intake",
    );
}

// Regression: a frame-end removal from the old level could dispatch its
// carried kill event after the next level installed.
#[test]
fn unload_level_clears_pending_death_event_carryover() {
    let mut app = test_app();
    app.session
        .as_mut()
        .expect("test app session installed")
        .pending_death_events
        .push("oldLevelKill".to_string());

    app.unload_level();

    assert!(
        app.session
            .as_ref()
            .expect("test app session installed")
            .pending_death_events
            .is_empty(),
        "level unload must discard deferred death events from the old level",
    );
}

// Regression: unload left the streaming sessions alive, so the level's
// manifest and its retained file lived on until the next install.
#[test]
fn unload_level_releases_the_level_streaming_state() {
    use crate::lightmap_streaming::prl_test_fixture::StreamedLightmapPrl;
    use crate::session::lightmap_residency::{LightmapLevelView, LightmapStreamingSession};

    let mut app = test_app();
    let prl = StreamedLightmapPrl::write();
    let world = prl.load();
    let view = LightmapLevelView::of(&world).expect("the fixture streams its lightmap");
    let manifest = std::sync::Arc::downgrade(view.manifest);
    let session = LightmapStreamingSession::new(view, None).unwrap();
    app.session
        .as_mut()
        .expect("test app session installed")
        .level_streaming
        .install_lightmap(session);
    app.level = Some(world);

    app.unload_level();

    let session = app.session.as_ref().expect("test app session installed");
    assert!(session.level_streaming.lightmap().is_none());
    assert!(!session.level_streaming.is_retiring());
    assert!(
        manifest.upgrade().is_none(),
        "the manifest and its retained file are released at unload"
    );
}

#[test]
fn reinstall_after_unload_leaves_no_fixture_a_cpu_residue() {
    let mut app = test_app();

    install_cpu_fixture(
        &mut app,
        CpuFixture {
            name: FIXTURE_MAP_A,
            reaction_name: "reactorWave",
            light_tag: "reactor_only",
            fog_tag: "reactor_fog",
            light_count: 2,
            fog_count: 2,
            triangle_count: 2,
        },
    );
    assert_eq!(
        app.session
            .as_ref()
            .expect("test app session installed")
            .light_bridge
            .light_count(),
        2
    );
    assert_eq!(
        app.session
            .as_ref()
            .expect("test app session installed")
            .fog_volume_bridge
            .entity_count(),
        2
    );
    assert_eq!(app.collision_world.triangle_count(), 2);

    // A live puff, reusable drain scratch, and a final-tick request all
    // belong to the retiring level, even when the next install is immediate.
    {
        let session = app.session.as_mut().unwrap();
        assert!(
            session
                .particle_pool
                .spawn_impact(glam::Vec3::ZERO, glam::Vec3::Y, 0)
        );
        let request = postretro_foundation::ImpactParticleRequest {
            point: glam::Vec3::ZERO,
            normal: glam::Vec3::Y,
        };
        session.impact_particle_requests.push(request);
        session
            .scripting
            .script_ctx
            .registry
            .borrow_mut()
            .push_impact_particle_request(request);
    }

    app.unload_level();

    install_cpu_fixture(
        &mut app,
        CpuFixture {
            name: FIXTURE_MAP_B,
            reaction_name: "combatWave",
            light_tag: "combat_only",
            fog_tag: "combat_fog",
            light_count: 1,
            fog_count: 1,
            triangle_count: 1,
        },
    );

    {
        let session = app.session.as_mut().unwrap();
        assert!(session.particle_pool.is_empty());
        assert_eq!(
            session.particle_pool.parameter_block_count(),
            crate::weapon::IMPACT_PARTICLE_COUNT
        );
        assert!(session.impact_particle_requests.is_empty());
        let mut registry = session.scripting.script_ctx.registry.borrow_mut();
        crate::weapon::drain_impact_particle_requests(
            &mut registry,
            &mut session.particle_pool,
            &mut session.impact_particle_requests,
        );
        assert!(
            session.particle_pool.is_empty(),
            "the retiring request never reaches the new level"
        );
        crate::weapon::spawn_impact_effect_at(&mut registry, glam::Vec3::X, glam::Vec3::Y);
        crate::weapon::drain_impact_particle_requests(
            &mut registry,
            &mut session.particle_pool,
            &mut session.impact_particle_requests,
        );
        assert_eq!(
            session.particle_pool.len(),
            crate::weapon::IMPACT_PARTICLE_COUNT
        );
    }

    let ctx = script_ctx(&app);
    let data_registry = ctx.data_registry.borrow();
    assert_eq!(data_registry.reactions.len(), 1);
    assert_eq!(data_registry.reactions[0].name, "combatWave");
    assert!(
        data_registry
            .reactions
            .iter()
            .all(|r| r.name != "reactorWave"),
        "{FIXTURE_MAP_A} reaction leaked into {FIXTURE_MAP_B}"
    );
    drop(data_registry);

    let session = app.session.as_ref().expect("test app session installed");
    assert_eq!(session.light_bridge.light_count(), 1);
    let light_id = session.light_bridge.entity_for_map_index(0).unwrap();
    assert_eq!(
        ctx.registry.borrow().get_tags(light_id).unwrap(),
        &["combat_only"],
    );

    assert_eq!(session.fog_volume_bridge.entity_count(), 1);
    assert_eq!(session.fog_volume_bridge.cached_aabb_count(), 1);
    assert!(session.fog_volume_bridge.active_aabbs().is_empty());
    assert_eq!(app.collision_world.triangle_count(), 1);
    assert_eq!(app.collision_world.vertex_count(), 3);
}
