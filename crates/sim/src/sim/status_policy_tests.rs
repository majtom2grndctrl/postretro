// Statuses through impact policies and reaction verbs on the fixed tick: the
// tick-of read, applier keying, a tick's own policy changing the instance set,
// and trigger-bound script damage and status verbs ordering against the stage.
// See: context/lib/scripting.md §2 (Impact-policy evaluation, Status
// declarations) · context/lib/entity_model.md §5

use std::cell::RefCell;

use glam::Vec3;
use postretro_entities::components::health::{
    DamageContext, DamageProducer, HealthComponent, apply_damage_with_context,
};
use postretro_entities::components::statuses::StatusApplier;
use postretro_entities::{
    EntityId, EntityRegistry, EntityStateComponent, NamedReaction, PrimitiveDescriptor,
    ReactionDescriptor,
};
use postretro_foundation::{DamagePayload, ImpactEventDescriptor};
use serde_json::{Value, json};

use super::status_tick_tests::{BURNING, CORRODING, World, applier, apply, burning, spawn};
use crate::impact_policy::ImpactPolicyRuntime;

fn input(name: &str) -> Value {
    json!({ "op": "input", "name": name })
}

fn number(value: f32) -> Value {
    json!({ "op": "const", "value": value })
}

fn handle(name: &str) -> Value {
    json!({ "kind": "status", "name": name })
}

fn effect(primitive: &str, status: &str) -> Value {
    json!({ "primitive": primitive, "target": "@impact.target", "args": { "status": handle(status) } })
}

fn when(condition: Value, effects: Vec<Value>) -> Value {
    json!({ "when": condition, "do": effects })
}

fn state_write(name: &str, value: Value) -> Value {
    json!({ "primitive": "setState", "target": "@impact.target", "args": { "name": name, "value": value } })
}

fn policy(effects: Vec<Value>) -> ImpactEventDescriptor {
    ImpactEventDescriptor {
        id: "status".into(),
        is_override: false,
        levels: Vec::new(),
        filter_tag: Some("enemy".into()),
        policy: effects,
    }
}

fn policies(world: &World, effects: Vec<Value>) -> RefCell<ImpactPolicyRuntime> {
    let mut runtime = ImpactPolicyRuntime::new(world.rig.ctx.clone());
    runtime.replace_global_events(vec![policy(effects)]);
    RefCell::new(runtime)
}

/// One tick with the in-tick evaluator; `ai` runs at the AI stage.
fn tick(
    world: &mut World,
    runtime: &RefCell<ImpactPolicyRuntime>,
    mut ai: impl FnMut(&mut EntityRegistry),
) -> super::TickEvents {
    world.rig.tick_evaluating(
        &[],
        |registry| ai(registry),
        |registry| runtime.borrow_mut().evaluate_pending_in_registry(registry),
    )
}

fn state(world: &World, entity: EntityId, name: &str) -> f32 {
    world
        .registry()
        .get_component::<EntityStateComponent>(entity)
        .map(|state| state.get(name))
        .unwrap_or(0.0)
}

fn hit(
    registry: &mut EntityRegistry,
    target: EntityId,
    attacker: Option<EntityId>,
    source: &str,
    amount: f32,
) {
    let mut context = DamageContext::new(source, DamageProducer::InTick);
    context.attacker = attacker;
    apply_damage_with_context(
        registry,
        target,
        &DamagePayload {
            amount,
            impulse: Vec3::ZERO,
            damage_type: None,
        },
        context,
    );
}

fn primitive(name: &str, primitive: &str, args: Value) -> NamedReaction {
    NamedReaction {
        name: name.into(),
        descriptor: ReactionDescriptor::Primitive(PrimitiveDescriptor {
            primitive: primitive.into(),
            target: None,
            args,
            kind: None,
            tag: Some("enemy".into()),
            on_complete: None,
        }),
    }
}

/// A pawn standing in a trigger whose enter edge runs `edge`, bound after the
/// volume exists.
fn trigger_world(burning_status: Value, edge: Vec<NamedReaction>) -> World {
    let mut world = World::new(burning_status);
    world.rig.local_pawn(Vec3::new(0.0, 0.0, 20.0));
    world.rig.trigger(Vec3::new(0.0, 0.0, 20.0), 2.0);
    world.rig.bind(edge, &[], Vec::new());
    world
}

// A burn tick runs policies with `impact.source` as the applier and
// `impact.status(burning)` true; a grant on the source heals the applier; a
// hit that is not a tick reads it false.
#[test]
fn status_tick_policy_reads_source_and_tick_of() {
    let mut world = World::new(burning("refresh", 500.0, None));
    let enemy = world.enemy;
    let shooter = spawn(&world.rig, 100.0);
    let mut wounded = world
        .registry()
        .get_component::<HealthComponent>(shooter)
        .unwrap()
        .clone();
    wounded.current = 50.0;
    world.registry().set_component(shooter, wounded).unwrap();
    let runtime = policies(
        &world,
        vec![
            when(
                input("@impact.status.burning"),
                vec![
                    state_write(
                        "ticks",
                        json!({ "op": "add", "a": input("@state.ticks"), "b": number(1.0) }),
                    ),
                    json!({ "primitive": "grantHealth", "target": "@impact.source", "args": { "amount": number(5.0) } }),
                ],
            ),
            when(
                json!({ "op": "not", "x": input("@impact.status.burning") }),
                vec![state_write("other", number(1.0))],
            ),
        ],
    );
    tick(&mut world, &runtime, |registry| {
        let source = registry.intern_status_source("weapon.incendiary");
        registry.apply_status(enemy, BURNING, StatusApplier::Entity(shooter), source);
        hit(registry, enemy, Some(shooter), "weapon.pistol", 1.0);
        runtime.borrow_mut().evaluate_pending_in_registry(registry);
    });
    world.registry().take_impact_dispatches();
    for _ in 0..30 {
        tick(&mut world, &runtime, |_| {});
    }
    assert_eq!(state(&world, enemy, "ticks"), 1.0);
    assert_eq!(
        state(&world, enemy, "other"),
        1.0,
        "the pistol hit read false"
    );
    assert_eq!(
        world.health(shooter),
        55.0,
        "the applier was healed by its burn"
    );
}

// P1: an unguarded refresh policy that reapplies `burning` from its own tick
// on the expiry tick keeps the instance with its phase unmoved; the guarded
// recipe lets the burn expire.
#[test]
fn status_self_refresh_on_expiry_tick_survives_and_guard_expires() {
    for (guarded, survives) in [(false, true), (true, false)] {
        let mut world = World::new(burning("refresh", 500.0, Some(1000.0)));
        let enemy = world.enemy;
        let reapply = effect("applyStatus", "burning");
        let effects = if guarded {
            vec![when(
                json!({ "op": "not", "x": input("@impact.status.burning") }),
                vec![reapply],
            )]
        } else {
            vec![reapply]
        };
        let runtime = policies(&world, effects);
        let a = applier(&world, "weapon.incendiary");
        tick(&mut world, &runtime, |registry| {
            apply(registry, enemy, BURNING, a);
        });
        let applied = world.host_tick();
        while world.host_tick() < applied + 60 {
            tick(&mut world, &runtime, |_| {});
        }
        assert_eq!(
            world.registry().has_status(enemy, BURNING),
            survives,
            "guarded {guarded}"
        );
        if survives {
            let instance = *world.registry().statuses().sets()[0]
                .instances()
                .next()
                .unwrap();
            assert_eq!(
                instance.next_due,
                applied + 90,
                "phase unmoved: one tickMs later"
            );
        }
    }
}

// P8: a tick's own policy changes the instance set at once: clearing
// `burning` leaves none; applying `corroding` adds an instance that does not
// tick this tick; an instance an earlier tick's policy cleared on the same
// tick does not land.
#[test]
fn status_tick_policy_mutations_take_effect_immediately() {
    let mut world = World::new(burning("refresh", 1000.0, None));
    let enemy = world.enemy;
    let runtime = policies(
        &world,
        vec![
            when(
                input("@impact.status.burning"),
                vec![
                    effect("clearStatus", "burning"),
                    effect("clearStatus", "corroding"),
                    effect("applyStatus", "wet"),
                    state_write(
                        "burnTicks",
                        json!({ "op": "add", "a": input("@state.burnTicks"), "b": number(1.0) }),
                    ),
                ],
            ),
            when(
                input("@impact.status.corroding"),
                vec![state_write("corrodeTicks", number(1.0))],
            ),
        ],
    );
    let a = applier(&world, "weapon.incendiary");
    tick(&mut world, &runtime, |registry| {
        apply(registry, enemy, BURNING, a);
        apply(registry, enemy, CORRODING, a);
    });
    world.registry().take_impact_dispatches();
    for _ in 0..60 {
        tick(&mut world, &runtime, |_| {});
    }
    let registry = world.registry();
    assert!(
        !registry.has_status(enemy, BURNING),
        "its own policy cleared it"
    );
    assert!(
        !registry.has_status(enemy, CORRODING),
        "cleared before its due tick"
    );
    assert!(registry.has_status(enemy, super::status_tick_tests::WET));
    drop(registry);
    assert_eq!(
        state(&world, enemy, "burnTicks"),
        1.0,
        "the burn tick landed once"
    );
    assert_eq!(
        state(&world, enemy, "corrodeTicks"),
        0.0,
        "corroding never landed"
    );
}

// P29: a status applied by a policy on an impact with no damager keys to that
// impact's source id, separately from a `script.applyStatus` instance, and its
// ticks credit that source id and no player.
#[test]
fn status_applier_falls_back_to_source_id() {
    let mut world = World::new(burning("refresh", 500.0, None));
    let enemy = world.enemy;
    let runtime = policies(
        &world,
        vec![when(
            json!({ "op": "not", "x": input("@impact.status.burning") }),
            vec![effect("applyStatus", "burning")],
        )],
    );
    tick(&mut world, &runtime, |registry| {
        hit(registry, enemy, None, "script.applyDamage", 1.0);
        runtime.borrow_mut().evaluate_pending_in_registry(registry);
        hit(registry, enemy, None, "fluid.lava", 1.0);
        runtime.borrow_mut().evaluate_pending_in_registry(registry);
        let source = registry.intern_status_source("script.applyStatus");
        registry.apply_status(enemy, BURNING, StatusApplier::Source(source), source);
    });
    let registry = world.registry();
    let mut keyed: Vec<String> = registry.statuses().sets()[0]
        .instances()
        .map(|instance| match instance.applier {
            StatusApplier::Source(source) => registry.statuses().source_id(source).to_string(),
            StatusApplier::Entity(_) => panic!("no damager, so no entity applier"),
        })
        .collect();
    drop(registry);
    keyed.sort();
    assert_eq!(
        keyed,
        ["fluid.lava", "script.applyDamage", "script.applyStatus"]
    );
    world.registry().take_impact_dispatches();
    // No evaluator from here, so the ticks' dispatches stay to be read.
    for _ in 0..30 {
        world.tick();
    }
    let dispatches = world.registry().take_impact_dispatches();
    let mut credited: Vec<&str> = dispatches.iter().map(|d| d.source_id.as_str()).collect();
    credited.sort();
    assert_eq!(
        credited,
        ["fluid.lava", "script.applyDamage", "script.applyStatus"]
    );
    assert!(dispatches.iter().all(|dispatch| dispatch.source.is_none()));
}

// P7, P23: two appliers' instances due on the lethal tick land in creation
// order and the first credits its applier; a rejoined applier's new pawn is a
// new applier with its own instance.
#[test]
fn status_due_ticks_land_in_creation_order_and_rejoin_makes_new_instance() {
    let mut world = World::new(burning("refresh", 500.0, None));
    let target = spawn(&world.rig, 6.0);
    let (first, second) = (spawn(&world.rig, 100.0), spawn(&world.rig, 100.0));
    let runtime = policies(&world, Vec::new());
    tick(&mut world, &runtime, |registry| {
        let source = registry.intern_status_source("weapon.incendiary");
        registry.apply_status(target, BURNING, StatusApplier::Entity(first), source);
        registry.apply_status(target, BURNING, StatusApplier::Entity(second), source);
    });
    world.registry().take_impact_dispatches();
    for _ in 0..30 {
        world.tick();
    }
    let dispatches = world.registry().take_impact_dispatches();
    let sources: Vec<_> = dispatches.iter().map(|d| d.source).collect();
    assert_eq!(sources, [Some(first), Some(second)], "creation order");
    assert!(
        dispatches[1].health_after <= 0.0,
        "the second tick was lethal"
    );

    // Rejoin: the same player returns with a new pawn and reapplies.
    let mut world = World::new(burning("refresh", 500.0, None));
    let enemy = world.enemy;
    let old_pawn = spawn(&world.rig, 100.0);
    let runtime = policies(&world, Vec::new());
    tick(&mut world, &runtime, |registry| {
        let source = registry.intern_status_source("weapon.incendiary");
        registry.apply_status(enemy, BURNING, StatusApplier::Entity(old_pawn), source);
    });
    world.registry().despawn(old_pawn).unwrap();
    let new_pawn = spawn(&world.rig, 100.0);
    tick(&mut world, &runtime, |registry| {
        let source = registry.intern_status_source("weapon.incendiary");
        registry.apply_status(enemy, BURNING, StatusApplier::Entity(new_pawn), source);
    });
    assert_eq!(world.registry().statuses().sets()[0].instances().count(), 2);
}

// P30, P31: a trigger-bound lethal `applyDamage` whose policy recovers the
// target, then `applyStatus(burning)` in the same list, leaves an instance;
// a trigger-bound recovery on a target whose burn tick is due lands that tick
// on the recovered target; a trigger-bound policy that clears `burning` leaves
// no tick.
#[test]
fn trigger_script_damage_and_status_verbs_order_against_the_stage() {
    let status_args = json!({ "status": handle("burning") });
    let recover = vec![when(
        json!({ "op": "le", "a": input("@impact.healthAfter"), "b": number(0.0) }),
        vec![
            json!({ "primitive": "setHealth", "target": "@impact.target", "args": { "value": number(50.0) } }),
        ],
    )];

    let mut world = trigger_world(
        burning("refresh", 500.0, None),
        vec![
            primitive("edge", "applyDamage", json!({ "amount": 2_000_000.0 })),
            primitive("edge", "applyStatus", status_args.clone()),
        ],
    );
    let enemy = world.enemy;
    let runtime = policies(&world, recover.clone());
    tick(&mut world, &runtime, |_| {});
    assert_eq!(world.health(enemy), 50.0);
    assert!(
        world.registry().has_status(enemy, BURNING),
        "applied after the recovery"
    );

    // A burn due on the trigger's tick lands on the recovered target.
    let mut world = World::new(burning("refresh", 500.0, None));
    let enemy = world.enemy;
    let a = applier(&world, "weapon.incendiary");
    world.tick_applying(|registry| {
        apply(registry, enemy, BURNING, a);
    });
    let applied = world.host_tick();
    while world.host_tick() + 1 < applied + 30 {
        world.tick();
    }
    world.rig.local_pawn(Vec3::new(0.0, 0.0, 20.0));
    world.rig.trigger(Vec3::new(0.0, 0.0, 20.0), 2.0);
    world.rig.bind(
        vec![primitive(
            "edge",
            "applyDamage",
            json!({ "amount": 2_000_000.0 }),
        )],
        &[],
        Vec::new(),
    );
    let runtime = policies(&world, recover);
    tick(&mut world, &runtime, |_| {});
    assert_eq!(world.host_tick(), applied + 30);
    assert_eq!(
        world.health(enemy),
        46.0,
        "recovered to 50, then the burn tick"
    );

    // A trigger-bound clear on the due tick removes it before it lands.
    let mut world = World::new(burning("refresh", 500.0, None));
    let enemy = world.enemy;
    world.tick_applying(|registry| {
        apply(registry, enemy, BURNING, a);
    });
    let applied = world.host_tick();
    while world.host_tick() + 1 < applied + 30 {
        world.tick();
    }
    world.rig.local_pawn(Vec3::new(0.0, 0.0, 20.0));
    world.rig.trigger(Vec3::new(0.0, 0.0, 20.0), 2.0);
    world.rig.bind(
        vec![primitive("edge", "clearStatus", status_args)],
        &[],
        Vec::new(),
    );
    let before = world.health(enemy);
    world.tick();
    assert_eq!(world.health(enemy), before, "cleared before its due tick");
    assert!(!world.registry().has_status(enemy, BURNING));
}

// P31, P32: a status applied by a policy on trigger-bound script damage, and
// one applied by a policy on script damage that landed before the tick, are
// both live by the AI stage, where a brain guard reads them.
#[test]
fn script_impact_status_visible_at_ai_stage_same_tick() {
    let apply_burn = vec![effect("applyStatus", "burning")];
    let mut world = trigger_world(
        burning("refresh", 500.0, None),
        vec![primitive("edge", "applyDamage", json!({ "amount": 1.0 }))],
    );
    let enemy = world.enemy;
    let runtime = policies(&world, apply_burn.clone());
    let mut seen = false;
    tick(&mut world, &runtime, |registry| {
        seen = registry.has_status(enemy, BURNING)
    });
    assert!(seen, "trigger-bound: live at the AI stage of its tick");

    let mut world = World::new(burning("refresh", 500.0, None));
    let enemy = world.enemy;
    let runtime = policies(&world, apply_burn);
    world.tick();
    // A UI press: script damage outside a tick.
    crate::health::reactions::dispatch(
        &mut world.registry(),
        &[enemy],
        &crate::health::reactions::ApplyDamageArgs {
            amount: 1.0,
            damage_type: None,
        },
    )
    .unwrap();
    let mut seen = false;
    tick(&mut world, &runtime, |registry| {
        seen = registry.has_status(enemy, BURNING)
    });
    assert!(seen, "off-tick: evaluated and applied at the tick start");
}

// P32: a UI press running `[applyDamage(lethal), applyStatus(burning)]` on an
// enemy whose impact policy recovers it with `setHealth` leaves a `burning`
// instance on the next tick: the application latches though health reads 0 at
// the press, and is judged at the tick start after the damage's policy has
// evaluated. Without a recovery the enemy stays dead and takes none.
#[test]
fn ui_press_recovery_then_status_applies_next_tick() {
    let recover = vec![when(
        json!({ "op": "le", "a": input("@impact.healthAfter"), "b": number(0.0) }),
        vec![
            json!({ "primitive": "setHealth", "target": "@impact.target", "args": { "value": number(50.0) } }),
        ],
    )];
    for (effects, recovers) in [(recover, true), (Vec::new(), false)] {
        let mut world = World::new(burning("refresh", 500.0, None));
        let enemy = world.enemy;
        let runtime = policies(&world, effects);
        world.tick();
        // The UI press: both steps outside a tick, the damage's policy not
        // yet evaluated when the status applies.
        crate::health::reactions::dispatch(
            &mut world.registry(),
            &[enemy],
            &crate::health::reactions::ApplyDamageArgs {
                amount: 2_000_000.0,
                damage_type: None,
            },
        )
        .unwrap();
        assert_eq!(world.health(enemy), 0.0, "lethal at the press");
        crate::status_commands::apply_status_to(&mut world.registry(), &[enemy], "burning");
        assert_eq!(
            world.registry().statuses().pending().len(),
            1,
            "latched, not refused at the press"
        );
        tick(&mut world, &runtime, |_| {});
        assert_eq!(
            world.registry().has_status(enemy, BURNING),
            recovers,
            "recovered {recovers}"
        );
        if recovers {
            assert_eq!(world.health(enemy), 50.0);
        }
    }
}

// A connected client applies no status from a reaction verb, and a subject
// without health warns and is skipped.
#[test]
fn reaction_status_verbs_skip_healthless_subjects_and_clients() {
    use postretro_entities::Transform;
    use postretro_scripting_core::reaction_registry::ReactionPrimitiveRegistry;
    let world = World::new(burning("refresh", 500.0, None));
    let enemy = world.enemy;
    let prop = world.registry().spawn(Transform::default());
    let authoritative = std::rc::Rc::new(std::cell::Cell::new(true));
    let mut reactions = ReactionPrimitiveRegistry::new();
    crate::status_commands::register_status_reactions(&mut reactions, authoritative.clone());
    let args = json!({ "status": handle("burning") });
    let capture = postretro_test_log_capture::LogCapture::start();
    reactions
        .dispatch("applyStatus", &mut world.registry(), &[prop, enemy], &args)
        .unwrap();
    assert!(
        capture
            .records()
            .iter()
            .any(|record| record.level == log::Level::Warn
                && record.message.contains("applyStatus")
                && record.message.contains("no HealthComponent"))
    );
    assert_eq!(
        world.registry().statuses().pending().len(),
        1,
        "the enemy's latched application"
    );

    authoritative.set(false);
    reactions
        .dispatch("clearStatus", &mut world.registry(), &[enemy], &args)
        .unwrap();
    assert_eq!(
        world.registry().statuses().pending().len(),
        1,
        "a client clears nothing"
    );
}

// A bad status reference drops that one impact policy at install and that one
// reaction at install; a good one passes.
#[test]
fn unregistered_status_handle_drops_policy_and_reaction() {
    let mut world = World::new(burning("refresh", 500.0, None));
    let enemy = world.enemy;
    let capture = postretro_test_log_capture::LogCapture::start();
    let runtime = policies(&world, vec![effect("applyStatus", "ice")]);
    assert!(
        capture
            .records()
            .iter()
            .any(|record| record.level == log::Level::Warn
                && record.message.contains("was skipped during bind")
                && record.message.contains("ice"))
    );
    tick(&mut world, &runtime, |registry| {
        hit(registry, enemy, None, "trap", 1.0)
    });
    assert!(world.registry().statuses().is_empty());

    let vocabulary = world.registry().defense_vocabulary().clone();
    for (primitive, status, ok) in [
        ("applyStatus", handle("burning"), true),
        ("clearStatus", handle("ice"), false),
        ("applyStatus", json!("burning"), false),
        (
            "applyStatus",
            json!({ "kind": "damageType", "name": "fire" }),
            false,
        ),
    ] {
        let result = crate::validate_defense_reference_args(
            primitive,
            &json!({ "status": status }),
            &vocabulary,
        );
        assert_eq!(result.is_ok(), ok, "{primitive} {status}: {result:?}");
    }
}
