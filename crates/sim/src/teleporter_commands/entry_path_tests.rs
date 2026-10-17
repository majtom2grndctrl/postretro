//! Host-only proofs for teleporter arm/disarm entry paths and the `teleport` event.
//! See: context/lib/scripting.md §12 (Teleport events)

use super::*;
use crate::teleporter_system::TeleporterBindingTable;
use crate::trigger_commands::{BoundTarget, BoundTriggerCommand, TriggerFireContext};
use crate::trigger_system::PlayerId;
use postretro_entities::components::health::HealthComponent;
use postretro_entities::{
    NamedReaction, PrimitiveDescriptor, ReactionDescriptor, TeleportEventDescriptor, Transform,
};
use postretro_scripting_core::ir_scopes::DispatchScope;

fn teleporter(ctx: &ScriptCtx, armed: bool, on_teleport: &str) -> EntityId {
    let mut registry = ctx.registry.borrow_mut();
    let id = registry.spawn(Transform::default());
    registry
        .set_component(
            id,
            TeleporterComponent {
                index: 0,
                source_entity: 1,
                armed,
                arrival_sound: String::new(),
                on_teleport: on_teleport.into(),
            },
        )
        .unwrap();
    id
}

fn armed(ctx: &ScriptCtx, id: EntityId) -> bool {
    ctx.registry
        .borrow()
        .get_component::<TeleporterComponent>(id)
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

fn heal_activators() -> NamedReaction {
    NamedReaction {
        name: "heal".into(),
        descriptor: ReactionDescriptor::Primitive(PrimitiveDescriptor {
            primitive: "grantHealth".into(),
            target: Some("@activators".into()),
            args: serde_json::json!({ "amount": 10.0 }),
            kind: None,
            tag: None,
            on_complete: None,
        }),
    }
}

#[test]
fn teleport_event_fires_once_per_authoritative_execute_and_never_on_client() {
    let ctx = ScriptCtx::new();
    let gate = teleporter(&ctx, true, "heal");
    let teleported = pawn(&ctx, 50.0);
    let bystander = pawn(&ctx, 50.0);
    ctx.data_registry
        .borrow_mut()
        .populate_level(vec![heal_activators()], vec![], &[]);
    // A member `on("teleport")` binding for the same reaction binds once beside the KVP.
    ctx.data_registry
        .borrow_mut()
        .set_level_teleport_events(vec![TeleportEventDescriptor {
            teleporter: gate,
            event: "teleport".into(),
            fire: vec!["heal".into()],
        }]);
    let host = TeleporterBindingTable::build(&ctx, Default::default(), Default::default());
    host.execute(&ctx, 0, teleported, PlayerId::Local(teleported));
    // `on_teleport` plus the member event name one reaction twice: both run, once each.
    assert_eq!(health(&ctx, teleported), 70.0);
    assert_eq!(
        health(&ctx, bystander),
        50.0,
        "activators is the teleported pawn only"
    );
    host.execute(&ctx, 0, teleported, PlayerId::Local(teleported));
    assert_eq!(
        health(&ctx, teleported),
        90.0,
        "one fire per authoritative teleport"
    );

    // Client: prediction never calls `execute` with authority; the gate refuses it
    // even for a table that was built on the host, and a client-built table is empty.
    ctx.owner_slot_writes_enabled.set(false);
    assert!(
        host.execute(&ctx, 0, teleported, PlayerId::Local(teleported))
            .is_none()
    );
    let client = TeleporterBindingTable::build(&ctx, Default::default(), Default::default());
    assert!(
        client
            .execute(&ctx, 0, teleported, PlayerId::Local(teleported))
            .is_none()
    );
    assert_eq!(health(&ctx, teleported), 90.0);
}

#[test]
fn teleport_client_mutation_role_guard_covers_member_primitive_and_token_paths() {
    for host in [true, false] {
        let ctx = ScriptCtx::new();
        ctx.owner_slot_writes_enabled.set(host);
        let id = teleporter(&ctx, false, "");
        let activator = pawn(&ctx, 100.0);

        // Entry path 1: member step in a sequence.
        let mut steps = SequencedPrimitiveRegistry::new();
        register_sequenced_teleporter_primitives(&mut steps, ctx.clone());
        steps.get("armTeleporter").unwrap()(id, &serde_json::json!({})).unwrap();

        // Entry path 2: reaction primitive (group/target form).
        let mut reactions = ReactionPrimitiveRegistry::new();
        register_teleporter_reaction_primitives(&mut reactions, ctx.clone());
        reactions
            .dispatch(
                "armTeleporter",
                &mut ctx.registry.borrow_mut(),
                &[id],
                &serde_json::json!({}),
            )
            .unwrap();

        // Entry path 3: the `on.teleporter` token, as a bound command.
        let token = BoundTriggerCommand::TeleporterArmed {
            target: BoundTarget::FiredTeleporter,
            armed: true,
        };
        let fire = TriggerFireContext {
            fired_teleporter: Some(id),
            activator: Some(activator),
            ..TriggerFireContext::default()
        };
        let mut scope = DispatchScope::script(ctx.clone(), &[]);
        token.execute_with_script_ctx(
            &mut ctx.registry.borrow_mut(),
            &ctx,
            &mut scope,
            &Default::default(),
            &Default::default(),
            &fire,
        );

        let queued = ctx.pending_teleporter_mutations.borrow().len();
        apply_pending_teleporter_mutations(&ctx);
        assert!(ctx.pending_teleporter_mutations.borrow().is_empty());
        if host {
            assert_eq!(queued, 3, "host queues every entry path");
            assert!(armed(&ctx, id));
        } else {
            assert_eq!(queued, 0, "client queues nothing on any entry path");
            assert!(
                !armed(&ctx, id),
                "client armed state follows replication only"
            );
        }
    }
}
