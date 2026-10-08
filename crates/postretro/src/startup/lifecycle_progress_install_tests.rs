// Kill-progress membership through the real renderer-free level install
// (`install_world_cpu`): descriptor NPCs placed on the map materialize in the
// data-archetype sweep, after the data script composes the reaction set, so a
// `progress` over their tag must capture them after placement and before
// `levelLoad` — and an NPC a map spawner releases later, carrying the same tag
// through `spawned_tags`, must neither count nor raise the total.
// See: context/lib/scripting.md §12 (Spawned NPCs and `progress`)

use log::Level;
use postretro_entities::data_descriptors::HealthDescriptor;
use postretro_entities::{
    ComponentKind, EntityId, NamedReaction, ProgressDescriptor, ReactionDescriptor, ScopedReaction,
};
use postretro_level_format::map_entity::MapEntityRecord;
use postretro_test_log_capture::LogCapture;

use super::tests::{descriptor, level_world, script_ctx, test_app};
use super::*;
use crate::scripting::builtins::data_archetype_test_fixtures::behavior_enemy_descriptor;

const DUMMY: &str = "progress_dummy";
const GRUNT: &str = "progress_grunt";
const WAVE: &str = "wave";
const CLOSET: &str = "wave_closet";
const CLEARED: &str = "waveCleared";

fn dummy_placement(x: f32, tags: &[&str]) -> MapEntityRecord {
    MapEntityRecord {
        classname: DUMMY.to_string(),
        origin: [x, 0.0, 0.0],
        angles: [0.0; 3],
        key_values: Vec::new(),
        tags: tags.iter().map(|tag| (*tag).to_string()).collect(),
    }
}

/// A map `entity_spawner` whose own tag addresses it and whose two spawns carry
/// `wave` — the tag the `progress` counts.
fn closet_spawner_placement() -> MapEntityRecord {
    MapEntityRecord {
        classname: crate::scripting::builtins::entity_spawner::CLASSNAME.to_string(),
        origin: [20.0, 0.0, 0.0],
        angles: [0.0; 3],
        key_values: vec![
            ("archetype".to_string(), GRUNT.to_string()),
            ("count".to_string(), "2".to_string()),
            ("spawned_tags".to_string(), WAVE.to_string()),
        ],
        tags: vec![CLOSET.to_string()],
    }
}

#[test]
fn install_captures_map_placed_descriptor_npcs_and_excludes_later_spawner_output() {
    let mut app = test_app();
    let ctx = script_ctx(&app);
    {
        let mut data = ctx.data_registry.borrow_mut();
        let mut dummy = descriptor(DUMMY);
        dummy.health = Some(HealthDescriptor {
            max: 50.0,
            hitbox: None,
            zone_multipliers: Default::default(),
        });
        data.upsert_entity_type(dummy);
        data.upsert_entity_type(behavior_enemy_descriptor(GRUNT));
        data.replace_global_reactions(vec![ScopedReaction {
            reaction: NamedReaction {
                name: "waveProgress".to_string(),
                descriptor: ReactionDescriptor::Progress(ProgressDescriptor {
                    tag: WAVE.to_string(),
                    at: 1.0,
                    fire: CLEARED.to_string(),
                }),
            },
            levels: Vec::new(),
        }]);
    }

    let mut world = level_world("progress_install", 1);
    // One placement repeats its tag (`_tags "wave wave"`): one membership.
    world.map_entities = vec![
        dummy_placement(0.0, &[WAVE, WAVE]),
        dummy_placement(4.0, &[WAVE]),
        dummy_placement(8.0, &[WAVE]),
        closet_spawner_placement(),
    ];

    let spawn_context = crate::spawner::SpawnContext::default();
    let logs = LogCapture::start();
    let mut timings = StartupTimings::new();
    {
        let session = app.session.as_mut().expect("test app session installed");
        crate::scripting::builtins::register_builtins(&mut session.classname_dispatch);
        let handles = WorldInstallHandles {
            command_diagnostics: Default::default(),
            mover_auto_close_ms: crate::runtime_movers::ENGINE_AUTO_CLOSE_MS,
            spawn_context: spawn_context.clone(),
            world: &world,
            script_ctx: &ctx,
            content_root: std::path::Path::new("content/dev"),
            active_level_tags: &[],
            nav_graph: None,
            fog_volume_bridge: &mut session.fog_volume_bridge,
            trigger_volume_bridge: &mut session.trigger_volume_bridge,
            classname_dispatch: &session.classname_dispatch,
            script_runtime: &session.scripting.script_runtime,
            sequence_registry: &session.scripting.sequence_registry,
            reaction_registry: &session.scripting.reaction_registry,
            system_registry: &session.scripting.system_registry,
            modal_stack: &mut session.modal_stack,
            progress_tracker: &mut session.progress_tracker,
            crossing_detector: &mut session.crossing_detector,
            slot_accumulator_bindings: &mut session.scripting.slot_accumulator_bindings,
            impact_policy_runtime: &mut session.scripting.impact_policy_runtime,
            mesh_clip_tables: &mut session.mesh_clip_tables,
            hit_zone_store: &mut session.hit_zone_store,
            trigger_pool_policy: TriggerPoolSeedPolicy::ArmAll,
            suppress_ai_enemies: false,
            suppress_boot_pawn: false,
            local_carried_loadout: None,
        };
        let _ = install_world_cpu(
            handles,
            &mut timings,
            |_models, _clip_tables| {
                crate::scripting_systems::hit_zones::ModelLoadWarningOwner::GameSide
            },
            |_spawn_points| {},
        );
    }
    logs.assert_not_logged(
        Level::Warn,
        "[Scripting] progress on tag `wave` has no members",
    );

    let placed: Vec<EntityId> = ctx
        .registry
        .borrow()
        .query_by_component_and_tag(ComponentKind::Health, Some(WAVE))
        .map(|(id, _)| id)
        .collect();
    assert_eq!(placed.len(), 3, "the archetype sweep placed three dummies");

    // A tick later, the closet releases its two NPCs carrying `wave`.
    let spawner: Vec<EntityId> = ctx
        .registry
        .borrow()
        .query_by_component_and_tag(ComponentKind::Spawner, Some(CLOSET))
        .map(|(id, _)| id)
        .collect();
    assert_eq!(spawner.len(), 1, "the map spawner was dispatched");
    crate::spawner::spawn_from_spawner_targets(
        &mut ctx.registry.borrow_mut(),
        &spawner,
        &spawn_context,
    );
    let released: Vec<EntityId> = ctx
        .registry
        .borrow()
        .query_by_component_and_tag(ComponentKind::Transform, Some(WAVE))
        .map(|(id, _)| id)
        .filter(|id| !placed.contains(id))
        .collect();
    assert_eq!(
        released.len(),
        2,
        "the spawner released two NPCs tagged `wave`"
    );

    let progress = &mut app
        .session
        .as_mut()
        .expect("test app session installed")
        .progress_tracker;
    assert!(
        progress.on_entity_killed(released[0]).is_empty(),
        "a later spawn carrying `wave` never counts"
    );
    assert!(progress.on_entity_killed(placed[0]).is_empty());
    assert!(
        progress.on_entity_killed(placed[1]).is_empty(),
        "two of three members dead: neither the spawn nor the repeated tag counted"
    );
    assert_eq!(
        progress.on_entity_killed(placed[2]),
        vec![CLEARED.to_string()],
        "the third placed kill fires: the living spawn never raised the total"
    );
    assert!(
        progress.on_entity_killed(released[1]).is_empty(),
        "the pair fired once this level"
    );
}
