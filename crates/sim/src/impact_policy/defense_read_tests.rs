// Layer reads in impact policies and the damaged-enemies overlay: the
// target accessor, the source player's layer, the overlay's shield pair and
// hide-at-full, and the bad-reference rule on each.
// See: context/lib/scripting.md §2 (Impact-policy evaluation), §11 (Defense layers) · context/lib/ui.md

use super::damaged_enemy_overlay::{sample_overlay_fact, shield_pair_full};
use super::tests::{evaluate_pending, event, input, number};
use super::*;
use crate::sim::defense_parameter_tests::{health, layer, lit, session, spawn};
use postretro_entities::components::health::{DamageContext, apply_damage_with_context};
use postretro_foundation::{DamagePayload, Seat};
use postretro_scripting_core::data_descriptors::{DamagedEnemiesOverlay, DamagedEnemiesShield};
use postretro_test_log_capture::LogCapture;
use serde_json::json;

fn world() -> ScriptCtx {
    session(vec![
        ("pawn", true, health(vec![layer(lit(100.0))])),
        ("enemy", false, health(vec![layer(lit(100.0))])),
        ("bare", false, health(vec![layer(lit(0.0))])),
    ])
}

fn strike(ctx: &ScriptCtx, target: EntityId, source: Option<EntityId>, amount: f32) {
    let mut context = DamageContext::new("defense-read-test", DamageProducer::InTick);
    context.attacker = source;
    apply_damage_with_context(
        &mut ctx.registry.borrow_mut(),
        target,
        &DamagePayload {
            amount,
            ..Default::default()
        },
        context,
    );
}

fn set_state(name: &str, value: Value) -> Value {
    json!({
        "primitive": "setState",
        "target": "@impact.target",
        "args": { "name": name, "value": value },
    })
}

fn state(ctx: &ScriptCtx, id: EntityId, name: &str) -> f32 {
    ctx.registry
        .borrow()
        .get_component::<postretro_entities::EntityStateComponent>(id)
        .unwrap()
        .get(name)
}

// Q1: the policy reads the hit's layer facts, the target's own layer and the
// source player's layer.
#[test]
fn impact_policy_reads_layer_facts_and_target_layers() {
    let ctx = world();
    let source = spawn(&ctx, "pawn");
    ctx.registry.borrow_mut().bind_pawn_seat(source, Seat(1));
    let enemy = spawn(&ctx, "enemy");
    ctx.registry
        .borrow_mut()
        .set_tags(enemy, vec!["enemy".to_string()])
        .unwrap();
    strike(&ctx, source, None, 25.0);
    let mut runtime = ImpactPolicyRuntime::new(ctx.clone());
    runtime.replace_global_events(vec![event(
        "read",
        "enemy",
        vec![
            set_state("shield", input("@defense.shield")),
            set_state("capacity", input("@defense.shield.capacity")),
            set_state("absorbed", input("@impact.absorbed")),
            set_state(
                "sourceShield",
                json!({ "op": "input", "name": "player.defense.shield", "owner": "@impact.source" }),
            ),
        ],
    )]);

    strike(&ctx, enemy, Some(source), 30.0);
    evaluate_pending(&ctx, &mut runtime);

    assert_eq!(
        state(&ctx, enemy, "shield"),
        70.0,
        "the target's layer after the hit"
    );
    assert_eq!(state(&ctx, enemy, "capacity"), 100.0);
    assert_eq!(state(&ctx, enemy, "absorbed"), 30.0);
    assert_eq!(
        state(&ctx, enemy, "sourceShield"),
        75.0,
        "the source player's layer"
    );
}

#[test]
fn impact_policy_layer_read_of_an_unregistered_layer_drops_the_policy() {
    let ctx = world();
    let scope = EntityScope::impact(ctx.clone());
    for name in ["@defense.armor", "@defense.@invalid.capacity"] {
        let error = bind_effect(&set_state("x", input(name)), &scope).map(|_| ());
        assert!(error.is_err(), "{name}");
    }
    let owned = json!({ "op": "input", "name": "player.defense.armor", "owner": "@impact.source" });
    assert!(bind_effect(&set_state("x", owned), &scope).is_err());
    assert!(bind_effect(&set_state("x", input("@defense.shield")), &scope).is_ok());
}

fn shield_overlay(value: &str, max: &str) -> BoundPresentationOverlay {
    let ir = |name: &str| postretro_foundation::IrNode::Input {
        name: name.to_string(),
        owner: None,
    };
    damaged_enemy_overlay::bind_damaged_enemies_overlay(
        PresentationTemplateHandle::from("overlay".to_string()),
        PresentationWorldAnchor {
            socket: None,
            offset_y: 0.0,
        },
        2,
        DamagedEnemiesOverlay {
            linger_ms: 500,
            hide_at_full: true,
            shield: Some(DamagedEnemiesShield {
                value: ir(value),
                max: ir(max),
            }),
        },
        vec!["shield".to_string()],
    )
    .expect("the overlay binds")
}

// Q3 (host): the overlay's shield pair reads the entity's own layer, and
// hide-at-full needs that pair full too; a capacity-0 layer counts as full.
#[test]
fn overlay_shield_pair_reads_entity_layer_and_counts_for_hide_at_full() {
    let ctx = world();
    let binding = shield_overlay("@defense.shield", "@defense.shield.capacity");
    let enemy = spawn(&ctx, "enemy");
    let bare = spawn(&ctx, "bare");
    strike(&ctx, enemy, None, 50.0);
    let fact = sample_overlay_fact(&binding, &ctx.registry.borrow(), enemy);
    assert!(fact.has_shield);
    assert_eq!(fact.shield_fraction, 0.5);
    assert_eq!(fact.health_fraction, 1.0);
    assert!(
        !shield_pair_full(&fact),
        "full health, half shield: stays shown"
    );
    postretro_entities::components::grant::grant_layer(
        &mut ctx.registry.borrow_mut(),
        enemy,
        "shield",
        None,
    );
    let fact = sample_overlay_fact(&binding, &ctx.registry.borrow(), enemy);
    assert!(shield_pair_full(&fact));
    let fact = sample_overlay_fact(&binding, &ctx.registry.borrow(), bare);
    assert!(!fact.has_shield, "0 / 0 is no pair");
    assert!(shield_pair_full(&fact), "a capacity-0 layer counts as full");
}

// D6 (overlay): a bad layer reference skips only the shield pair.
#[test]
fn overlay_bad_layer_reference_skips_shield_pair() {
    let capture = LogCapture::start();
    let binding = shield_overlay("@defense.armor", "@defense.armor.capacity");
    assert!(binding.shield.is_none());
    capture.assert_logged_once(
        log::Level::Warn,
        "damaged-enemies overlay shield pair skipped: `armor` names no defense layer",
    );
}

// Q11: a `@state.shield` pair stays author data: hits never drain it, and
// hide-at-full counts it.
#[test]
fn state_shield_overlay_is_author_data_never_drained() {
    let ctx = world();
    let binding = shield_overlay("@state.shield", "@state.shieldMax");
    let enemy = spawn(&ctx, "enemy");
    {
        let mut registry = ctx.registry.borrow_mut();
        let state = registry.entity_state_mut(enemy).unwrap();
        state.set("shield", 5.0);
        state.set("shieldMax", 10.0);
    }
    strike(&ctx, enemy, None, 40.0);
    assert_eq!(
        state(&ctx, enemy, "shield"),
        5.0,
        "no hit drains @state.shield"
    );
    let fact = sample_overlay_fact(&binding, &ctx.registry.borrow(), enemy);
    assert_eq!(fact.shield_fraction, 0.5);
    assert_eq!(fact.health_fraction, 1.0);
    assert!(
        !shield_pair_full(&fact),
        "full health, state shield below max: shown"
    );
    let _ = number(0.0);
}
