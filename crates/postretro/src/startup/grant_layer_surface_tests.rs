// `grantLayer` in a crossing-fired body, through the production level-load
// pipeline: the body's store writes, grants and damage land at dispatch, in
// body order, and each grant evaluates the capacity in force when it applies.
// See: context/lib/scripting.md §11 (Defense layers), §12 (Player events)

use postretro_entities::components::defense_layers::DefenseLayersComponent;
use postretro_entities::components::health::{
    DamageContext, DamageProducer, HealthComponent, apply_damage_with_context,
};
use postretro_entities::{
    EntityId, ReplicationScope, ScriptCtx, SlotOwnership, SlotRecord, SlotSchema, SlotType,
    SlotValue, Transform,
};
use postretro_foundation::defense::{DefenseVocabulary, HandleRef};
use postretro_foundation::{
    AuthoredHandle, DamagePayload, DefenseLayerEntry, IrNode, NumberOrIr, Seat,
};
use postretro_level_format::data_script::DataScriptSection;
use postretro_scripting_core::primitives_registry::PrimitiveRegistry;
use postretro_sim::residual_drain::{ResidualRegistries, drain_frame_residuals};
use postretro_test_log_capture::LogCapture;

use crate::coop_fixture_tests::initialize_dev_mod;
use crate::scripting::primitives::register_all;
use crate::startup::lifecycle::tests::{install_capture_world, level_world, script_ctx, test_app};

const PAWN: &str = "e16_pawn";
const SEATS: [Seat; 2] = [Seat(1), Seat(2)];

/// A level script whose one player event fires `body` for each player whose
/// `e16.trip` reaches 1.
fn level_script(body: &str) -> String {
    format!(
        r#"
local Postretro = require("postretro")
local UI = require("postretro/ui")
local e16 = Postretro.defineStore("e16", {{
  trip = {{ type = "number", default = 0, perOwner = true }},
  cap = {{ type = "number", default = 100 }},
  bonus = {{ type = "number", default = 0, perOwner = true }},
}})
local shield = Postretro.defineDefenseLayer("shield")
local read = Postretro.read
local body = Postretro.defineReaction("e16.body", function(on)
  return {{ {body} }}
end)
function setupLevel(_ctx)
  return {{
    reactions = {{ body }},
    playerEvents = {{
      Postretro.players():on(Postretro.becomes(read(e16.trip):ge(1)), {{ body }}),
    }},
  }}
end
"#
    )
}

fn number_slot(per_owner: bool) -> SlotRecord {
    SlotRecord::new(SlotSchema {
        slot_type: SlotType::Number,
        default: Some(SlotValue::Number(if per_owner { 0.0 } else { 100.0 })),
        range: None,
        persist: false,
        readonly: false,
        ownership: SlotOwnership::Mod,
        // A host event's global write must replicate; per-owner slots here
        // are host-side only.
        network: if per_owner {
            ReplicationScope::None
        } else {
            ReplicationScope::SharedGlobal
        },
        per_owner,
        accumulate: None,
    })
}

fn input(name: &str) -> IrNode {
    IrNode::Input {
        name: name.to_string(),
        owner: None,
    }
}

/// The dev mod, plus a `shield` vocabulary, the `e16.*` slots and a player
/// descriptor whose shield capacity reads `e16.cap + e16.bonus`; then the
/// level install and one shielded pawn per seat, spawned full.
fn install(body: &str) -> (crate::App, ScriptCtx, Vec<EntityId>) {
    let mut app = test_app();
    let ctx = script_ctx(&app);
    let mut primitives = PrimitiveRegistry::new();
    register_all(&mut primitives, ctx.clone());
    initialize_dev_mod(&mut app, &primitives);

    postretro_scripting_core::defense_vocabulary::commit_defense_vocabulary(
        &ctx,
        DefenseVocabulary {
            layers: vec!["shield".to_string()],
            ..Default::default()
        },
    );
    {
        let mut slots = ctx.slot_table.borrow_mut();
        slots.insert("e16.trip".into(), number_slot(true)).unwrap();
        slots.insert("e16.cap".into(), number_slot(false)).unwrap();
        slots.insert("e16.bonus".into(), number_slot(true)).unwrap();
    }
    {
        let mut data = ctx.data_registry.borrow_mut();
        let mut pawn = data
            .entities
            .iter()
            .find(|descriptor| descriptor.movement.is_some() && descriptor.health.is_some())
            .cloned()
            .expect("the dev mod declares a player descriptor");
        pawn.canonical_name = Some(PAWN.to_string());
        let health = pawn.health.as_mut().unwrap();
        health.max = 100.0;
        health.layers = vec![DefenseLayerEntry {
            layer: AuthoredHandle::from_handle(&HandleRef::DefenseLayer {
                name: "shield".to_string(),
            }),
            capacity: NumberOrIr::Ir(IrNode::Add {
                a: Box::new(input("e16.cap")),
                b: Box::new(input("e16.bonus")),
            }),
            recharge_delay_ms: None,
            recharge_rate: None,
            resist: Default::default(),
        }];
        data.upsert_entity_type(pawn);
    }

    let mut world = level_world("e16-grant-layer", 1);
    world.data_script = Some(DataScriptSection {
        compiled_bytes: level_script(body).into_bytes(),
        source_path: "e16-grant-layer.luau".to_string(),
    });
    let capture = LogCapture::start();
    install_capture_world(&mut app, &world);
    app.rebuild_active_system_reaction_bindings();
    app.rebuild_player_events(false);
    capture.assert_not_logged(log::Level::Error, "Dropping the reaction");
    capture.assert_not_logged(log::Level::Error, "drops address");

    let pawns = SEATS.iter().map(|seat| spawn_pawn(&ctx, *seat)).collect();
    ctx.system_commands.discard_all();
    (app, ctx, pawns)
}

/// Spawn a pawn the way the data-archetype spawn does: evaluate, then fill.
fn spawn_pawn(ctx: &ScriptCtx, seat: Seat) -> EntityId {
    let health = ctx
        .data_registry
        .borrow()
        .entities
        .iter()
        .find(|descriptor| descriptor.canonical_name.as_deref() == Some(PAWN))
        .and_then(|descriptor| descriptor.health.clone())
        .unwrap();
    let mut registry = ctx.registry.borrow_mut();
    let pawn = registry.spawn(Transform::default());
    // Bound first, so the capacity's per-owner read sees the pawn's seat.
    registry.bind_pawn_seat(pawn, seat);
    registry
        .set_component(pawn, HealthComponent::from_descriptor(&health))
        .unwrap();
    let mut layers = DefenseLayersComponent::from_descriptor(&health).unwrap();
    layers.descriptor = Some(PAWN.to_string());
    if let Some(source) = registry.defense_parameters().cloned() {
        source.evaluate(&registry, pawn, &mut layers);
    }
    layers.fill();
    registry.set_component(pawn, layers).unwrap();
    pawn
}

fn trip(ctx: &ScriptCtx) {
    let mut slots = ctx.slot_table.borrow_mut();
    let trip = slots.get_mut("e16.trip").unwrap();
    for seat in SEATS {
        trip.set_per_seat_value(seat, SlotValue::Number(1.0));
    }
}

/// One host frame's player-event work, in `frame_loop` order.
fn player_event_frame(app: &mut crate::App) {
    let ctx = script_ctx(app);
    let mut residuals = Vec::new();
    app.player_events.run_tick(&ctx, &[], &mut residuals);
    let session = app.session.as_ref().expect("test app session installed");
    drain_frame_residuals(
        &[],
        &app.trigger_bindings,
        &session.trigger_system,
        &residuals,
        &app.player_events,
        &session.scripting.scheduler,
        ResidualRegistries {
            sequence: &session.scripting.sequence_registry,
            reaction: &session.scripting.reaction_registry,
            system: &session.scripting.system_registry,
        },
        &ctx,
        &mut Vec::new(),
        |_, _| None,
    );
}

fn shield(ctx: &ScriptCtx, pawn: EntityId) -> (f32, f32) {
    let registry = ctx.registry.borrow();
    let layer = &registry
        .get_component::<DefenseLayersComponent>(pawn)
        .unwrap()
        .layers[0];
    (layer.value, layer.capacity)
}

fn health(ctx: &ScriptCtx, pawn: EntityId) -> f32 {
    ctx.registry
        .borrow()
        .get_component::<HealthComponent>(pawn)
        .unwrap()
        .current
}

fn break_shield(ctx: &ScriptCtx, pawn: EntityId) {
    apply_damage_with_context(
        &mut ctx.registry.borrow_mut(),
        pawn,
        &DamagePayload {
            amount: 100.0,
            ..Default::default()
        },
        DamageContext::new("e16.break", DamageProducer::InTick),
    );
}

// G6 / R-P26: grant 25 then damage 20 against shield 0 / 100 and health 100
// leaves 5 / 100 and health 100; the reverse order leaves 25 / 100 and 80.
#[test]
fn body_grant_and_damage_apply_in_body_order() {
    for (body, expected_shield, expected_health) in [
        (
            "on.player:grantLayer(shield, 25), on.player:damage(20)",
            5.0,
            100.0,
        ),
        (
            "on.player:damage(20), on.player:grantLayer(shield, 25)",
            25.0,
            80.0,
        ),
    ] {
        let (mut app, ctx, pawns) = install(body);
        for &pawn in &pawns {
            break_shield(&ctx, pawn);
            assert_eq!(shield(&ctx, pawn), (0.0, 100.0));
        }
        trip(&ctx);
        player_event_frame(&mut app);
        for &pawn in &pawns {
            assert_eq!(shield(&ctx, pawn), (expected_shield, 100.0), "{body}");
            assert_eq!(health(&ctx, pawn), expected_health, "{body}");
        }
    }
}

// G3: the crossing player's own capacity raise, then its fill, in one body.
#[test]
fn crossing_body_raises_capacity_then_grants_in_body_order() {
    let (mut app, ctx, pawns) =
        install("on.player:addSlot(e16.bonus, 50), on.player:grantLayer(shield)");
    for &pawn in &pawns {
        break_shield(&ctx, pawn);
    }
    trip(&ctx);
    player_event_frame(&mut app);
    for &pawn in &pawns {
        assert_eq!(
            shield(&ctx, pawn),
            (150.0, 150.0),
            "the grant reads the raise written earlier in the body"
        );
    }
}

// G4: a host crossing's global `setState`, then `players().grantLayer`.
#[test]
fn crossing_set_state_then_grant_layer_reads_new_capacity() {
    let (mut app, ctx, pawns) =
        install("UI.updateState(e16.cap, 150), Postretro.players():grantLayer(shield)");
    trip(&ctx);
    player_event_frame(&mut app);
    for &pawn in &pawns {
        assert_eq!(shield(&ctx, pawn), (150.0, 150.0));
    }
}
