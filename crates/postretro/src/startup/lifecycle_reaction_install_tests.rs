// Reaction validation through fresh CPU install and the production reload stage.
// See: context/lib/scripting.md §12 (Dispatch scopes)

use super::tests::{level_world, script_ctx, test_app};
use super::*;
use postretro_entities::{
    NamedReaction, PrimitiveDescriptor, ReactionDescriptor, ScopedReaction, SequenceStep,
    SequenceTarget, SlotOwnership, SlotRecord, SlotSchema, SlotType, SlotValue,
    SystemReactionCommand,
};
use postretro_scripting_core::reaction_dispatch::{
    dispatch_deferred_named_events_with_sequences, fire_named_event_with_sequences,
};
use postretro_test_log_capture::LogCapture;
use serde_json::json;

// Regression: reload retained IR scope requirements from an audience-invalid
// setState sibling, so Pass B rejected a valid caller that fresh install kept.
#[test]
fn fresh_install_and_staged_reload_keep_callers_of_surviving_same_address_bodies() {
    let mut app = test_app();
    let ctx = script_ctx(&app);
    ctx.slot_table
        .borrow_mut()
        .insert(
            "fixture.flag".into(),
            SlotRecord::new(SlotSchema {
                slot_type: SlotType::Boolean,
                default: Some(SlotValue::Boolean(false)),
                range: None,
                persist: false,
                readonly: false,
                ownership: SlotOwnership::Mod,
                network: Default::default(),
                per_owner: false,
                accumulate: None,
            }),
        )
        .expect("fixture slot is vacant");
    let primitive = |primitive: &str, args| NamedReaction {
        name: "cue".into(),
        descriptor: ReactionDescriptor::Primitive(PrimitiveDescriptor {
            primitive: primitive.into(),
            target: None,
            kind: None,
            tag: None,
            on_complete: None,
            args,
        }),
    };
    let fire = |name: &str, event: &str| NamedReaction {
        name: name.into(),
        descriptor: ReactionDescriptor::Sequence(vec![SequenceStep {
            id: SequenceTarget::Fire,
            primitive: "fire".into(),
            args: json!({"event":event}),
        }]),
    };
    ctx.data_registry.borrow_mut().replace_global_reactions(
        vec![
            primitive("playSound", json!({"sound":"sfx/test_tone"})),
            primitive(
                "setState",
                json!({
                    "slot":"fixture.flag", "value":{"op":"input","name":"@rising"},
                    "audience":"everyone",
                }),
            ),
            fire("caller", "cue"),
            fire("levelLoad", "caller"),
        ]
        .into_iter()
        .map(|reaction| ScopedReaction {
            reaction,
            levels: Vec::new(),
        })
        .collect(),
    );
    let world = level_world("reaction_install", 1);
    let capture = LogCapture::start();
    {
        let session = app.session.as_mut().expect("fixture session");
        crate::scripting_systems::system_reactions::register_system_reaction_primitives(
            &mut session.scripting.system_registry,
        );
        crate::scripting_systems::reaction_scheduler::register_reaction_control_primitives(
            &mut session.scripting.sequence_registry,
            session.scripting.scheduler.clone(),
        );
        let handles = WorldInstallHandles {
            command_diagnostics: Default::default(),
            mover_auto_close_ms: crate::runtime_movers::ENGINE_AUTO_CLOSE_MS,
            spawn_context: session.scripting.spawn_context.clone(),
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
            suppress_host_replicated: false,
            suppress_boot_pawn: false,
            local_carried_loadout: None,
        };
        let _ = install_world_cpu(
            handles,
            &mut StartupTimings::new(),
            |_, _| crate::scripting_systems::hit_zones::ModelLoadWarningOwner::GameSide,
            |_| {},
        );
    }
    capture.assert_logged_once(log::Level::Error, "`setState` has invalid audience");
    assert_eq!(ctx.system_commands.take().iter().filter(|command| {
        matches!(command, SystemReactionCommand::PlaySound { sound, .. } if sound == "sfx/test_tone")
    }).count(), 1, "fresh levelLoad fires the valid sibling through caller");

    // This is the same CPU stage poll_staged_manifest_results invokes after a
    // successful commit. Recomposition restores the original invalid body.
    for _ in 0..2 {
        app.rebuild_reactions_after_staged_commit();
        let data = ctx.data_registry.borrow();
        assert!(data.reactions.iter().any(|reaction| reaction.name == "caller"
            && matches!(&reaction.descriptor, ReactionDescriptor::Sequence(steps) if steps.len() == 1)));
        assert!(
            matches!(&data.reactions[1].descriptor, ReactionDescriptor::Sequence(steps) if steps.is_empty())
        );
        let session = app.session.as_ref().expect("fixture session");
        let chained = fire_named_event_with_sequences(
            "caller",
            &data,
            &session.scripting.sequence_registry,
            &session.scripting.reaction_registry,
            &session.scripting.system_registry,
            &ctx,
            None,
        );
        dispatch_deferred_named_events_with_sequences(
            chained,
            &data,
            &session.scripting.sequence_registry,
            &session.scripting.reaction_registry,
            &session.scripting.system_registry,
            &ctx,
        );
        assert_eq!(ctx.system_commands.take().iter().filter(|command| {
            matches!(command, SystemReactionCommand::PlaySound { sound, .. } if sound == "sfx/test_tone")
        }).count(), 1, "reload keeps the valid sibling reachable through caller");
    }
    assert_eq!(
        ctx.slot_table
            .borrow()
            .get("fixture.flag")
            .and_then(|slot| slot.value.clone()),
        Some(SlotValue::Boolean(false)),
        "rejected setState never runs"
    );
}
