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
    let (app, ctx, pawns, records) = install_script(level_script(body));
    for record in &records {
        assert!(
            !(record.level == log::Level::Error
                && (record.message.contains("Dropping the reaction")
                    || record.message.contains("drops address"))),
            "{}",
            record.message
        );
    }
    (app, ctx, pawns)
}

/// Install `script` as the level's data script over the dev mod with the
/// `shield` layer and the `shock` type (×2 on shields); returns every record
/// the install logged.
fn install_script(
    script: String,
) -> (
    crate::App,
    ScriptCtx,
    Vec<EntityId>,
    Vec<postretro_test_log_capture::CapturedRecord>,
) {
    let mut app = test_app();
    let ctx = script_ctx(&app);
    let mut primitives = PrimitiveRegistry::new();
    register_all(&mut primitives, ctx.clone());
    initialize_dev_mod(&mut app, &primitives);

    postretro_scripting_core::defense_vocabulary::commit_defense_vocabulary(
        &ctx,
        DefenseVocabulary {
            layers: vec!["shield".to_string()],
            damage_types: vec![postretro_foundation::defense::DamageTypeDeclaration {
                name: "shock".to_string(),
                matrix: std::collections::BTreeMap::from([(
                    "shield".to_string(),
                    postretro_foundation::defense::PoolMultiplier::Scale(2.0),
                )]),
            }],
            default_damage_type: None,
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
        compiled_bytes: script.into_bytes(),
        source_path: "e16-grant-layer.luau".to_string(),
    });
    let capture = LogCapture::start();
    install_capture_world(&mut app, &world);
    app.rebuild_active_system_reaction_bindings();
    app.rebuild_player_events(false);
    let records = capture.records();

    let pawns = SEATS.iter().map(|seat| spawn_pawn(&ctx, *seat)).collect();
    ctx.system_commands.discard_all();
    (app, ctx, pawns, records)
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

/// A level script over the `e16` stores and the `shield` / `shock` handles,
/// with `prelude` Luau ahead of `setupLevel` returning `manifest`.
fn handle_script(prelude: &str, manifest: &str) -> String {
    format!(
        r#"
local Postretro = require("postretro")
local e16 = Postretro.defineStore("e16", {{
  trip = {{ type = "number", default = 0, perOwner = true }},
  cap = {{ type = "number", default = 100 }},
  bonus = {{ type = "number", default = 0, perOwner = true }},
}})
local shield = Postretro.defineDefenseLayer("shield")
local shock = Postretro.defineDamageType("shock", {{ shield = 2 }})
local read = Postretro.read
local tripped = Postretro.becomes(read(e16.trip):ge(1))
{prelude}
function setupLevel(_ctx)
  return {manifest}
end
"#
    )
}

fn enemy(ctx: &ScriptCtx) -> EntityId {
    let mut registry = ctx.registry.borrow_mut();
    let enemy = registry.spawn(postretro_entities::Transform::default());
    registry
        .set_component(
            enemy,
            HealthComponent::from_descriptor(&postretro_foundation::HealthDescriptor {
                max: 100.0,
                ..Default::default()
            }),
        )
        .unwrap();
    registry.set_tags(enemy, vec!["enemy".to_string()]).unwrap();
    enemy
}

fn shocked_hit(app: &mut crate::App, ctx: &ScriptCtx, target: EntityId) {
    let shock = ctx.registry.borrow().damage_type_index(Some("shock"));
    let mut registry = ctx.registry.borrow_mut();
    apply_damage_with_context(
        &mut registry,
        target,
        &DamagePayload {
            amount: 5.0,
            damage_type: shock,
            ..Default::default()
        },
        DamageContext::new("e16.shocked", DamageProducer::InTick),
    );
    app.session
        .as_mut()
        .unwrap()
        .scripting
        .impact_policy_runtime
        .evaluate_pending_in_registry(&mut registry);
}

fn state(ctx: &ScriptCtx, entity: EntityId, name: &str) -> f32 {
    ctx.registry
        .borrow()
        .get_component::<postretro_entities::EntityStateComponent>(entity)
        .map_or(0.0, |state| state.get(name))
}

// D2: a level script's own handles reference the registered vocabulary:
// `impact.type(shock)` in a policy and `.damage(n, { type })` in a reaction.
#[test]
fn level_script_handles_reference_registered_vocabulary() {
    let script = handle_script(
        r#"
local body = Postretro.defineReaction("e16.body", function(on)
  return { on.player:damage(20, { type = shock }) }
end)
local shocked = Postretro.defineImpactEvent("e16.shocked", { tag = "enemy" }, function(impact)
  return { { when = impact.type(shock), ["do"] = { impact.target:setState("shocked", 1) } } }
end)
"#,
        "{ reactions = { body }, events = { shocked }, playerEvents = { Postretro.players():on(tripped, { body }) } }",
    );
    let (mut app, ctx, pawns, records) = install_script(script);
    let loud: Vec<_> = records
        .iter()
        .filter(|record| {
            record.level <= log::Level::Warn && !record.message.starts_with("[HitZones]")
        })
        .collect();
    assert!(loud.is_empty(), "{loud:#?}");
    trip(&ctx);
    player_event_frame(&mut app);
    for &pawn in &pawns {
        assert_eq!(shield(&ctx, pawn), (60.0, 100.0), "shock is ×2 on shields");
        assert_eq!(health(&ctx, pawn), 100.0);
    }
    let target = enemy(&ctx);
    shocked_hit(&mut app, &ctx, target);
    assert_eq!(state(&ctx, target, "shocked"), 1.0);
}

// D6: a bad defense reference drops only the definition holding it, on each
// install surface; its siblings install and run.
#[test]
fn bad_defense_reference_drops_only_its_definition_at_install() {
    let script = handle_script(
        r#"
local armor = Postretro.defineDefenseLayer("armor")
local good = Postretro.defineReaction("e16.good", function(on)
  return { on.player:damage(5) }
end)
local grantType = Postretro.defineReaction("e16.grantType", function(on)
  return { on.player:grantLayer(shock, 5) }
end)
local damageLayer = Postretro.defineReaction("e16.damageLayer", function(on)
  return { on.player:damage(5, { type = shield }) }
end)
local grantArmor = Postretro.defineReaction("e16.grantArmor", function(on)
  return { on.player:grantLayer(armor) }
end)
local goodPolicy = Postretro.defineImpactEvent("e16.goodPolicy", { tag = "enemy" }, function(impact)
  return { impact.target:setState("good", 1) }
end)
local badPolicy = Postretro.defineImpactEvent("e16.badPolicy", { tag = "enemy" }, function(impact)
  return { impact.target:setState("bad", impact.target:defense(shock).current) }
end)
"#,
        "{ reactions = { good, grantType, damageLayer, grantArmor }, events = { goodPolicy, badPolicy }, playerEvents = { Postretro.players():on(tripped, { good }), Postretro.players():on(Postretro.becomes(armor.current:le(0)), { good }) } }",
    );
    let (mut app, ctx, pawns, records) = install_script(script);
    for name in ["e16.grantType", "e16.damageLayer", "e16.grantArmor"] {
        assert!(
            records
                .iter()
                .any(|record| record.level == log::Level::Error
                    && record.message.contains(&format!("reaction `{name}`"))
                    && record.message.contains("Dropping the reaction")),
            "{name} drops at install"
        );
    }
    let names: Vec<_> = ctx
        .data_registry
        .borrow()
        .reactions
        .iter()
        .filter(|reaction| !matches!(
            &reaction.descriptor,
            postretro_scripting_core::data_descriptors::ReactionDescriptor::Sequence(steps) if steps.is_empty()
        ))
        .map(|reaction| reaction.name.clone())
        .collect();
    assert!(names.contains(&"e16.good".to_string()), "{names:?}");
    trip(&ctx);
    player_event_frame(&mut app);
    for &pawn in &pawns {
        assert_eq!(
            shield(&ctx, pawn),
            (95.0, 100.0),
            "the good event fires once"
        );
    }
    let target = enemy(&ctx);
    shocked_hit(&mut app, &ctx, target);
    assert_eq!(state(&ctx, target, "good"), 1.0, "the sibling policy runs");
    assert_eq!(state(&ctx, target, "bad"), 0.0, "the bad policy dropped");
}
