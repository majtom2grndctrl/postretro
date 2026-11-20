//! Proofs for the seam `arrive` binding table: brush and script bindings both
//! run in-tick, presentation leaves a residual drained at frame end, scope
//! rejections mirror a teleport event's, and a client binds nothing.
//! See: context/lib/entity_model.md §7e (Level Seams) · context/lib/scripting.md §12

use super::*;
use crate::seam_commands::apply_pending_seam_mutations;
use crate::teleporter_system::TeleporterBindingTable;
use postretro_entities::components::health::HealthComponent;
use postretro_entities::{
    NamedReaction, PrimitiveDescriptor, Recipients, SeamEventDescriptor, SequenceStep,
    SystemReactionCommand, TeleportEventDescriptor, TeleporterComponent, Transform,
};

fn seam(ctx: &ScriptCtx, armed: bool, on_arrive: &str) -> EntityId {
    let mut registry = ctx.registry.borrow_mut();
    let id = registry.spawn(Transform::default());
    registry
        .set_component(
            id,
            SeamComponent {
                index: 0,
                source_entity: 7,
                name: "airlock".into(),
                armed,
                on_arrive: on_arrive.into(),
            },
        )
        .unwrap();
    id
}

fn seam_armed(ctx: &ScriptCtx, id: EntityId) -> bool {
    ctx.registry
        .borrow()
        .get_component::<SeamComponent>(id)
        .unwrap()
        .armed
}

fn pawn(ctx: &ScriptCtx, current: f32) -> EntityId {
    let mut registry = ctx.registry.borrow_mut();
    let id = registry.spawn(Transform::default());
    registry
        .set_component(
            id,
            HealthComponent {
                max: 100.0,
                current,
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

fn health(ctx: &ScriptCtx, id: EntityId) -> f32 {
    ctx.registry
        .borrow()
        .get_component::<HealthComponent>(id)
        .unwrap()
        .current
}

fn primitive(
    name: &str,
    primitive: &str,
    target: Option<&str>,
    args: serde_json::Value,
) -> NamedReaction {
    NamedReaction {
        name: name.into(),
        descriptor: ReactionDescriptor::Primitive(PrimitiveDescriptor {
            primitive: primitive.into(),
            target: target.map(str::to_owned),
            args,
            kind: None,
            tag: None,
            on_complete: None,
        }),
    }
}

fn heal_activators() -> NamedReaction {
    primitive(
        "heal",
        "grantHealth",
        Some("@activators"),
        serde_json::json!({ "amount": 10.0 }),
    )
}

fn flash() -> NamedReaction {
    primitive(
        "fx",
        "flashScreen",
        None,
        serde_json::json!({ "color": [1.0, 1.0, 1.0, 1.0], "durationMs": 30.0 }),
    )
}

fn arrive(seam: EntityId, fire: &[&str]) -> SeamEventDescriptor {
    SeamEventDescriptor {
        seam,
        event: "arrive".into(),
        fire: fire.iter().map(|name| (*name).to_owned()).collect(),
    }
}

fn drain_one(ctx: &ScriptCtx, bindings: &SeamBindingTable, residual: SeamArriveResidual) {
    let mut system = postretro_scripting_core::reaction_registry::SystemReactionRegistry::new();
    crate::scripting_systems::system_reactions::register_system_reaction_primitives(&mut system);
    drain_seam_residuals(
        &[residual],
        bindings,
        &ReactionScheduler::default(),
        ResidualRegistries {
            sequence: &postretro_scripting_core::sequence::SequencedPrimitiveRegistry::new(),
            reaction: &postretro_scripting_core::reaction_registry::ReactionPrimitiveRegistry::new(
            ),
            system: &system,
        },
        ctx,
        &mut Vec::new(),
    );
}

#[test]
fn arrive_runs_the_brush_reaction_and_every_script_binding_once_per_execute() {
    let ctx = ScriptCtx::new();
    let gate = seam(&ctx, true, "heal");
    let arrived = pawn(&ctx, 50.0);
    let bystander = pawn(&ctx, 50.0);
    ctx.data_registry
        .borrow_mut()
        .populate_level(vec![heal_activators()], vec![], &[]);
    // A script `seamEvents` binding naming the same reaction binds beside the
    // brush `on_arrive`; a duplicate member binding binds once.
    ctx.data_registry
        .borrow_mut()
        .set_level_seam_events(vec![arrive(gate, &["heal"]), arrive(gate, &["heal"])]);
    let host = SeamBindingTable::build(&ctx, Default::default(), Default::default());
    assert!(!host.is_empty());
    assert!(
        host.execute(&ctx, 0, arrived, PlayerId::Local(arrived))
            .is_none(),
        "no presentation, no residual"
    );
    assert_eq!(
        health(&ctx, arrived),
        70.0,
        "the brush `on_arrive` and the script binding each run once"
    );
    assert_eq!(
        health(&ctx, bystander),
        50.0,
        "activators is the arriving pawn only"
    );
    host.execute(&ctx, 0, arrived, PlayerId::Local(arrived));
    assert_eq!(health(&ctx, arrived), 90.0, "one fire per arrival");

    // A connected client: the host-built table refuses, and a client-built
    // table binds nothing.
    ctx.owner_slot_writes_enabled.set(false);
    assert!(
        host.execute(&ctx, 0, arrived, PlayerId::Local(arrived))
            .is_none()
    );
    let client = SeamBindingTable::build(&ctx, Default::default(), Default::default());
    assert!(client.is_empty());
    assert!(
        client
            .execute(&ctx, 0, arrived, PlayerId::Local(arrived))
            .is_none()
    );
    assert_eq!(health(&ctx, arrived), 90.0);
}

#[test]
fn a_disarmed_destination_seam_still_fires_arrive() {
    let ctx = ScriptCtx::new();
    let _gate = seam(&ctx, false, "heal");
    let arrived = pawn(&ctx, 50.0);
    ctx.data_registry
        .borrow_mut()
        .populate_level(vec![heal_activators()], vec![], &[]);
    let host = SeamBindingTable::build(&ctx, Default::default(), Default::default());
    host.execute(&ctx, 0, arrived, PlayerId::Local(arrived));
    assert_eq!(health(&ctx, arrived), 60.0);
}

#[test]
fn arrive_presentation_leaves_a_residual_that_drains_to_the_arriving_seat() {
    let ctx = ScriptCtx::new();
    let gate = seam(&ctx, true, "");
    let arrived = {
        let mut registry = ctx.registry.borrow_mut();
        let pawn = registry.spawn(Transform::default());
        registry.bind_pawn_seat(pawn, Seat(3));
        pawn
    };
    ctx.data_registry
        .borrow_mut()
        .populate_level(vec![flash()], vec![], &[]);
    ctx.data_registry
        .borrow_mut()
        .set_level_seam_events(vec![arrive(gate, &["fx"])]);
    let bindings = SeamBindingTable::build(&ctx, Default::default(), Default::default());
    let residual = bindings
        .execute(&ctx, 0, arrived, PlayerId::Remote(5))
        .expect("presentation leaves a residual");
    assert_eq!(
        residual,
        SeamArriveResidual {
            index: 0,
            seam: gate,
            pawn: arrived,
            player: PlayerId::Remote(5),
            seat: Some(Seat(3)),
        }
    );
    assert!(
        ctx.system_commands.take_ordered().is_empty(),
        "presentation waits for the frame-end drain"
    );
    drain_one(&ctx, &bindings, residual);
    let queued = ctx.system_commands.take_ordered();
    assert_eq!(queued.len(), 1);
    assert_eq!(queued[0].0, Recipients::Seats(vec![Seat(3)]));
    assert!(matches!(
        queued[0].1,
        SystemReactionCommand::FlashScreen { .. }
    ));

    // A seated player who left before the drain is skipped, not routed to
    // the host screen.
    ctx.registry.borrow_mut().clear_pawn_seat(arrived);
    drain_one(&ctx, &bindings, residual);
    assert!(ctx.system_commands.take_ordered().is_empty());
}

#[test]
fn on_seam_disarm_queues_for_the_next_tick_and_keeps_no_edge() {
    let ctx = ScriptCtx::new();
    let gate = seam(&ctx, true, "");
    let arrived = pawn(&ctx, 100.0);
    ctx.data_registry.borrow_mut().populate_level(
        vec![primitive(
            "airlock.sealed",
            "disarmSeam",
            Some("@seam"),
            serde_json::json!({}),
        )],
        vec![],
        &[],
    );
    ctx.data_registry
        .borrow_mut()
        .set_level_seam_events(vec![arrive(gate, &["airlock.sealed"])]);
    let bindings = SeamBindingTable::build(&ctx, Default::default(), Default::default());
    bindings.execute(&ctx, 0, arrived, PlayerId::Local(arrived));
    assert!(seam_armed(&ctx, gate), "a disarm waits for the next tick");
    assert_eq!(
        ctx.pending_seam_mutations.borrow().as_slice(),
        [(gate, false)]
    );
    let mut newly_armed = Vec::new();
    apply_pending_seam_mutations(&ctx, &mut newly_armed);
    assert!(!seam_armed(&ctx, gate));
    assert!(newly_armed.is_empty());
}

#[test]
fn arrive_rejects_what_a_teleport_event_rejects() {
    let step = |id: SequenceTarget, primitive: &str, args: serde_json::Value| SequenceStep {
        id,
        primitive: primitive.into(),
        args,
    };
    let wait = |interruptible: bool| {
        step(
            SequenceTarget::Wait,
            "wait",
            serde_json::json!({ "durationMs": 100.0, "interruptible": interruptible }),
        )
    };
    let occupancy = ReactionDescriptor::Primitive(PrimitiveDescriptor {
        primitive: "setState".into(),
        target: None,
        args: serde_json::json!({ "value": { "op": "input", "name": "@occupancy" } }),
        kind: None,
        tag: None,
        on_complete: None,
    });
    let rejected = [
        occupancy,
        primitive("t", "armTrigger", Some("@trigger"), serde_json::json!({})).descriptor,
        primitive(
            "t",
            "armTeleporter",
            Some("@teleporter"),
            serde_json::json!({}),
        )
        .descriptor,
        ReactionDescriptor::Sequence(vec![step(
            SequenceTarget::FiredTrigger,
            "disarmTrigger",
            serde_json::json!({}),
        )]),
        ReactionDescriptor::Sequence(vec![step(
            SequenceTarget::FiredTeleporter,
            "disarmTeleporter",
            serde_json::json!({}),
        )]),
        ReactionDescriptor::Sequence(vec![step(
            SequenceTarget::EventPlayer,
            "grantHealth",
            serde_json::json!({ "amount": 1.0 }),
        )]),
        ReactionDescriptor::Sequence(vec![
            wait(false),
            step(
                SequenceTarget::FiredSeam,
                "disarmSeam",
                serde_json::json!({}),
            ),
        ]),
        ReactionDescriptor::Sequence(vec![
            wait(false),
            step(
                SequenceTarget::Activators,
                "grantHealth",
                serde_json::json!({ "amount": 1.0 }),
            ),
        ]),
        ReactionDescriptor::Sequence(vec![wait(true)]),
    ];
    for descriptor in &rejected {
        assert!(invalid_scope(descriptor), "must reject {descriptor:?}");
    }
    let accepted = [
        primitive("s", "disarmSeam", Some("@seam"), serde_json::json!({})).descriptor,
        primitive(
            "h",
            "grantHealth",
            Some("@activators"),
            serde_json::json!({ "amount": 1.0 }),
        )
        .descriptor,
        ReactionDescriptor::Sequence(vec![
            step(
                SequenceTarget::FiredSeam,
                "disarmSeam",
                serde_json::json!({}),
            ),
            wait(false),
        ]),
    ];
    for descriptor in &accepted {
        assert!(!invalid_scope(descriptor), "must accept {descriptor:?}");
    }
    assert!(reaction_uses_seam(&accepted[0]));
    assert!(reaction_uses_seam(&accepted[2]));
    assert!(!reaction_uses_seam(&accepted[1]));

    // A rejected reaction binds nothing: executing the seam leaves the
    // teleporter it names untouched.
    let ctx = ScriptCtx::new();
    let _gate = seam(&ctx, true, "cross");
    let arrived = pawn(&ctx, 100.0);
    ctx.data_registry.borrow_mut().populate_level(
        vec![primitive(
            "cross",
            "disarmTeleporter",
            Some("@teleporter"),
            serde_json::json!({}),
        )],
        vec![],
        &[],
    );
    let bindings = SeamBindingTable::build(&ctx, Default::default(), Default::default());
    assert!(
        bindings
            .execute(&ctx, 0, arrived, PlayerId::Local(arrived))
            .is_none()
    );
    assert!(ctx.pending_teleporter_mutations.borrow().is_empty());
}

#[test]
fn a_teleport_event_refuses_the_seam_token() {
    let ctx = ScriptCtx::new();
    let gate = {
        let mut registry = ctx.registry.borrow_mut();
        let gate = registry.spawn(Transform::default());
        registry
            .set_component(
                gate,
                TeleporterComponent {
                    index: 0,
                    source_entity: 1,
                    armed: true,
                    arrival_sound: String::new(),
                    on_teleport: String::new(),
                },
            )
            .unwrap();
        gate
    };
    let target_seam = seam(&ctx, true, "");
    let traveller = pawn(&ctx, 100.0);
    ctx.data_registry.borrow_mut().populate_level(
        vec![primitive(
            "wrong",
            "disarmSeam",
            Some("@seam"),
            serde_json::json!({}),
        )],
        vec![],
        &[],
    );
    ctx.data_registry
        .borrow_mut()
        .set_level_teleport_events(vec![TeleportEventDescriptor {
            teleporter: gate,
            event: "teleport".into(),
            fire: vec!["wrong".into()],
        }]);
    let bindings = TeleporterBindingTable::build(&ctx, Default::default(), Default::default());
    bindings.execute(&ctx, 0, traveller, PlayerId::Local(traveller));
    assert!(ctx.pending_seam_mutations.borrow().is_empty());
    assert!(seam_armed(&ctx, target_seam));
}

#[test]
fn bindings_to_a_missing_seam_or_reaction_are_skipped() {
    let ctx = ScriptCtx::new();
    let gate = seam(&ctx, true, "absent");
    let stranger = ctx.registry.borrow_mut().spawn(Transform::default());
    let arrived = pawn(&ctx, 50.0);
    ctx.data_registry
        .borrow_mut()
        .populate_level(vec![heal_activators()], vec![], &[]);
    ctx.data_registry
        .borrow_mut()
        .set_level_seam_events(vec![arrive(stranger, &["heal"]), arrive(gate, &["absent"])]);
    let bindings = SeamBindingTable::build(&ctx, Default::default(), Default::default());
    bindings.execute(&ctx, 0, arrived, PlayerId::Local(arrived));
    assert_eq!(
        health(&ctx, arrived),
        50.0,
        "a binding keyed to a non-seam entity never reaches this seam"
    );
    assert!(
        bindings
            .execute(&ctx, 9, arrived, PlayerId::Local(arrived))
            .is_none(),
        "an unknown seam index fires nothing"
    );
}
