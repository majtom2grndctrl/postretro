// A host restart of the client's active catalog map, followed through the
// client's lifecycle: teardown, reinstall, `levelLoad` again.
// See: context/lib/networking.md §Slot lifecycle

use super::tests::{
    catalog_map, connect_test_app_as_client, drop_in_flight_worker, install_capture_world,
    level_world, script_ctx, test_app,
};
use super::*;
use crate::scripting_systems::reaction_scheduler::SchedulerRole;
use crate::startup::LevelRequest;
use postretro_entities::{
    NamedReaction, ReactionDescriptor, ScopedReaction, SequenceStep, SequenceTarget,
    SystemReactionCommand,
};
use serde_json::json;
use std::collections::VecDeque;

fn played(app: &App) -> Vec<String> {
    script_ctx(app)
        .system_commands
        .take()
        .into_iter()
        .filter_map(|command| match command {
            SystemReactionCommand::PlaySound { sound, .. } => Some(sound),
            _ => None,
        })
        .collect()
}

/// The client installs the catalog map as the production install does: the
/// in-flight entry becomes the active source, then segment B fires `levelLoad`.
fn install_e1m1(app: &mut App) {
    app.active_level_source = Some(LevelSource::Catalog("e1m1".to_string()));
    app.boot_state = BootState::Running;
    install_capture_world(app, &level_world("e1m1", 1));
}

// Regression: a client treated the host's same-map restart as a duplicate
// relevel, so it kept the previous life: `levelLoad` never re-fired and the
// scheduler kept that life's parked tails and join tick.
#[test]
fn host_restart_relevel_reinstalls_the_client_and_refires_level_load() {
    let mut app = test_app();
    connect_test_app_as_client(&mut app);
    let ctx = script_ctx(&app);
    ctx.data_registry
        .borrow_mut()
        .replace_maps(vec![catalog_map("e1m1", "maps/e1m1.prl", "Entryway", &[])]);
    // An every-machine intro cue, then a klaxon a minute later.
    let play = |sound: &str| SequenceStep {
        id: SequenceTarget::System,
        primitive: "playSound".into(),
        args: json!({ "sound": sound }),
    };
    ctx.data_registry
        .borrow_mut()
        .replace_global_reactions(vec![ScopedReaction {
            reaction: NamedReaction {
                name: "levelLoad".into(),
                descriptor: ReactionDescriptor::Sequence(vec![
                    play("intro"),
                    SequenceStep {
                        id: SequenceTarget::Wait,
                        primitive: "wait".into(),
                        args: json!({ "durationMs": 60_000.0 }),
                    },
                    play("klaxon"),
                ]),
            },
            levels: Vec::new(),
        }]);
    let scheduler = {
        let scripting = &mut app.session.as_mut().unwrap().scripting;
        crate::scripting_systems::system_reactions::register_system_reaction_primitives(
            &mut scripting.system_registry,
        );
        crate::scripting_systems::reaction_scheduler::register_reaction_control_primitives(
            &mut scripting.sequence_registry,
            scripting.scheduler.clone(),
        );
        scripting.scheduler.set_role(SchedulerRole::Client);
        scripting.scheduler.clone()
    };

    install_e1m1(&mut app);
    assert_eq!(played(&app), ["intro"], "the first life's levelLoad");
    assert_eq!(scheduler.pending_len(), 1, "the klaxon tail parks");
    for _ in 0..3 {
        scheduler.evaluate(&[]);
    }
    scheduler.advance_host_clock(Some(100.0), 0.0);
    assert_eq!(scheduler.level_tick(), 3);
    assert_eq!(scheduler.join_host_tick(), Some(100));

    // Admission names the life the client joined; it already runs that map.
    app.follow_relevel_catalog("e1m1".to_string(), 1);
    app.follow_relevel_catalog("e1m1".to_string(), 1);
    assert!(
        app.level_requests.is_empty(),
        "the joined life and its duplicate keep the active map"
    );

    // The host restarts the same map: a new life of it.
    app.follow_relevel_catalog("e1m1".to_string(), 2);
    assert_eq!(
        app.level_requests,
        VecDeque::from([LevelRequest::Load(LevelSource::Catalog("e1m1".to_string()))]),
        "a host restart reloads the active map"
    );
    app.drain_level_requests();
    assert_eq!(
        app.boot_state,
        BootState::Loading,
        "torn down and reloading"
    );
    assert_eq!(
        app.level_load
            .as_ref()
            .and_then(|load| load.entry.catalog_id.as_deref()),
        Some("e1m1")
    );
    assert_eq!(scheduler.pending_len(), 0, "the old life's tail is gone");
    assert_eq!(scheduler.level_tick(), 0, "the level clock restarts");
    assert_eq!(scheduler.join_host_tick(), None, "the join tick refixes");
    app.follow_relevel_catalog("e1m1".to_string(), 2);
    assert!(
        app.level_requests.is_empty(),
        "a duplicate of the restart does not restart the reload"
    );

    // The worker cannot load a fixture path; install its payload directly.
    drop_in_flight_worker(&mut app);
    install_e1m1(&mut app);
    assert_eq!(played(&app), ["intro"], "levelLoad fires for the new life");
    assert_eq!(
        scheduler.pending_len(),
        1,
        "only the new life's tail is parked"
    );
    assert_eq!(scheduler.join_host_tick(), None);
}
