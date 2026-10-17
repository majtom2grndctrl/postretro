// Level unload, and so a level change or restart, drops the old level's teleporter
// work: arm/disarm mutations still queued for a next tick that never comes, and the
// `teleport` event bindings built against its entities.
// See: context/lib/scripting.md §12 (Teleport events) · context/lib/boot_sequence.md

use postretro_entities::{TeleporterComponent, Transform};

use crate::startup::lifecycle::tests::{script_ctx, test_app};

#[test]
fn teleport_unload_level_clears_queued_mutations_and_event_bindings() {
    let mut app = test_app();
    let ctx = script_ctx(&app);
    let teleporter = {
        let mut registry = ctx.registry.borrow_mut();
        let teleporter = registry.spawn(Transform::default());
        registry
            .set_component(
                teleporter,
                TeleporterComponent {
                    index: 0,
                    source_entity: 1,
                    armed: false,
                    arrival_sound: String::new(),
                    on_teleport: String::new(),
                },
            )
            .unwrap();
        teleporter
    };
    app.teleporter_bindings = postretro_sim::teleporter_system::TeleporterBindingTable::build(
        &ctx,
        Default::default(),
        Default::default(),
    );
    assert!(
        !app.teleporter_bindings.is_empty(),
        "fixture: the installed teleporter is bound"
    );
    // An arm a reaction queued this tick, waiting for the next tick's start.
    ctx.pending_teleporter_mutations
        .borrow_mut()
        .push((teleporter, true));

    app.unload_level();

    assert!(
        ctx.pending_teleporter_mutations.borrow().is_empty(),
        "no queued arm or disarm reaches the next level"
    );
    assert!(
        app.teleporter_bindings.is_empty(),
        "the old level's teleport bindings are dropped"
    );
}
