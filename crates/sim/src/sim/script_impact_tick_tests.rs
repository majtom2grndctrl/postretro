// In-tick script-damage evaluation through the authoritative fixed-tick seam:
// a trigger- or teleporter-bound `applyDamage` runs its impact policies right
// after its damage applies, so later commands and later hits see the result.
// See: context/lib/scripting.md §2 (Impact-policy evaluation)

use glam::Vec3;
use postretro_entities::components::health::{
    DamageContext, DamageProducer, HealthComponent, apply_damage_with_context,
};
use postretro_entities::{
    EntityId, EntityStateComponent, NamedReaction, PrimitiveDescriptor, ReactionDescriptor,
    Transform,
};
use postretro_foundation::{DamagePayload, ImpactEventDescriptor};
use serde_json::{Value, json};

use super::teleport_tick_tests::{Rig, classic};
use crate::impact_policy::ImpactPolicyRuntime;

const DEST: Vec3 = Vec3::new(40.0, 0.0, 0.0);
const FAR: Vec3 = Vec3::new(-40.0, 0.0, 0.0);

fn damage(name: &str, amount: f32) -> NamedReaction {
    NamedReaction {
        name: name.into(),
        descriptor: ReactionDescriptor::Primitive(PrimitiveDescriptor {
            primitive: "applyDamage".into(),
            target: None,
            args: json!({ "amount": amount }),
            kind: None,
            tag: Some("enemy".into()),
            on_complete: None,
        }),
    }
}

fn input(name: &str) -> Value {
    json!({ "op": "input", "name": name })
}

fn number(value: f32) -> Value {
    json!({ "op": "const", "value": value })
}

fn state_write(name: &str, value: Value) -> Value {
    json!({
        "primitive": "setState",
        "target": "@impact.target",
        "args": { "name": name, "value": value },
    })
}

/// Recover a lethal hit to 50, count every evaluation, and record what each
/// evaluation read as `healthAfter`.
fn recovery_policy() -> ImpactEventDescriptor {
    ImpactEventDescriptor {
        id: "recover".into(),
        is_override: false,
        levels: Vec::new(),
        filter_tag: Some("enemy".into()),
        policy: vec![
            json!({
                "when": { "op": "le", "a": input("@impact.healthAfter"), "b": number(0.0) },
                "do": [{
                    "primitive": "setHealth", "target": "@impact.target",
                    "args": { "value": number(50.0) },
                }],
            }),
            state_write(
                "hits",
                json!({ "op": "add", "a": input("@state.hits"), "b": number(1.0) }),
            ),
            json!({
                "when": { "op": "eq", "a": input("@impact.healthAfter"), "b": number(40.0) },
                "do": [state_write("sawRecovered", number(1.0))],
            }),
            state_write("lastAfter", input("@impact.healthAfter")),
        ],
    }
}

fn enemy(rig: &Rig) -> EntityId {
    let mut registry = rig.ctx.registry.borrow_mut();
    let enemy = registry.spawn(Transform {
        position: FAR,
        ..Transform::default()
    });
    registry
        .set_component(
            enemy,
            HealthComponent {
                max: 100.0,
                current: 100.0,
                hitbox: None,
                death_handled: false,
                pending_kill_credit: None,
                zone_multipliers: Default::default(),
                contributor_ledger: Default::default(),
            },
        )
        .unwrap();
    registry.set_tags(enemy, vec!["enemy".into()]).unwrap();
    enemy
}

fn state(rig: &Rig, entity: EntityId, name: &str) -> f32 {
    rig.ctx
        .registry
        .borrow()
        .get_component::<EntityStateComponent>(entity)
        .map(|state| state.get(name))
        .unwrap_or(0.0)
}

/// A rocket-like in-tick hit at the AI stage, evaluated at once as every
/// in-tick producer's is.
fn rocket(
    registry: &mut postretro_entities::EntityRegistry,
    target: EntityId,
    attacker: EntityId,
    runtime: &mut ImpactPolicyRuntime,
) {
    let mut context = DamageContext::new("rocket", DamageProducer::InTick);
    context.attacker = Some(attacker);
    apply_damage_with_context(
        registry,
        target,
        &DamagePayload {
            amount: 5.0,
            impulse: Vec3::ZERO,
            damage_type: None,
        },
        context,
    );
    runtime.evaluate_pending_in_registry(registry);
}

fn ledger_sources(rig: &Rig, entity: EntityId) -> Vec<String> {
    rig.ctx
        .registry
        .borrow()
        .get_component::<HealthComponent>(entity)
        .unwrap()
        .contributor_ledger
        .entries()
        .iter()
        .map(|entry| entry.source_id.clone())
        .collect()
}

/// Which in-tick site binds the two `applyDamage` steps.
#[derive(Clone, Copy, Debug)]
enum Site {
    Trigger,
    Teleporter,
}

/// Bind `[lethal 200, then 10]` to `site`, fire it on the first tick, and
/// optionally land a rocket from the pawn later that tick.
fn run(site: Site, with_rocket: bool) -> (Rig, EntityId, EntityId, super::TickEvents) {
    let mut rig = Rig::new(vec![classic(Vec3::ZERO, 2.0, DEST, true)]);
    // Triggers bind from the registry, so place the volume before binding.
    let pawn = match site {
        // A pawn standing in a trigger far from the teleporter.
        Site::Trigger => {
            let pawn = rig.local_pawn(Vec3::new(0.0, 0.0, 20.0));
            rig.trigger(Vec3::new(0.0, 0.0, 20.0), 2.0);
            pawn
        }
        Site::Teleporter => rig.local_pawn(Vec3::ZERO),
    };
    match site {
        Site::Trigger => rig.bind(
            vec![damage("edge", 200.0), damage("edge", 10.0)],
            &[],
            Vec::new(),
        ),
        Site::Teleporter => rig.bind(
            vec![damage("hurt", 200.0), damage("hurt", 10.0)],
            &[(0, &["hurt"])],
            Vec::new(),
        ),
    }
    let target = enemy(&rig);
    let mut runtime = ImpactPolicyRuntime::new(rig.ctx.clone());
    runtime.replace_global_events(vec![recovery_policy()]);
    let runtime = std::cell::RefCell::new(runtime);
    let events = rig.tick_evaluating(
        &[],
        |registry| {
            if with_rocket {
                rocket(registry, target, pawn, &mut runtime.borrow_mut());
            }
        },
        |registry| runtime.borrow_mut().evaluate_pending_in_registry(registry),
    );
    (rig, pawn, target, events)
}

// P13, P31: the lethal script hit recovers before the sweep and before the
// list's next step, which sees the recovered health.
#[test]
fn in_tick_script_impact_recovers_before_sweep_and_next_command() {
    for site in [Site::Trigger, Site::Teleporter] {
        let (rig, _pawn, target, events) = run(site, false);
        assert_eq!(
            rig.health(target),
            40.0,
            "{site:?}: recovered to 50, then 10"
        );
        assert!(events.death.is_empty(), "{site:?}: the sweep never latches");
        assert_eq!(
            state(&rig, target, "hits"),
            2.0,
            "{site:?}: both impacts ran"
        );
        assert_eq!(
            state(&rig, target, "sawRecovered"),
            1.0,
            "{site:?}: the second step saw the recovered health"
        );
        assert!(
            rig.ctx
                .registry
                .borrow_mut()
                .take_impact_dispatches()
                .is_empty(),
            "{site:?}: no script impact is left for a later evaluator"
        );
    }
}

// P31: a later in-tick hit lands on the recovered target, records its
// contributor and reads `healthAfter` from the recovered health.
#[test]
fn in_tick_script_impact_evaluates_before_following_rocket() {
    for site in [Site::Trigger, Site::Teleporter] {
        let (rig, _pawn, target, events) = run(site, true);
        assert_eq!(rig.health(target), 35.0, "{site:?}");
        assert!(events.death.is_empty(), "{site:?}");
        assert_eq!(state(&rig, target, "hits"), 3.0, "{site:?}");
        assert_eq!(state(&rig, target, "lastAfter"), 35.0, "{site:?}");
        assert!(
            ledger_sources(&rig, target).contains(&"rocket".to_string()),
            "{site:?}: the rocket's contributor is recorded"
        );
    }
}

// P32: a script impact pending when the tick starts (a UI press, `levelLoad`,
// the previous tick's player events) evaluates first, in landing order, and a
// later hit's evaluator neither drops nor reorders it.
#[test]
fn script_impact_before_weapon_hit_evaluates_in_landing_order() {
    let mut rig = Rig::new(vec![classic(Vec3::ZERO, 2.0, DEST, true)]);
    rig.bind(Vec::new(), &[], Vec::new());
    let pawn = rig.local_pawn(Vec3::new(0.0, 0.0, 20.0));
    let target = enemy(&rig);
    let mut runtime = ImpactPolicyRuntime::new(rig.ctx.clone());
    runtime.replace_global_events(vec![recovery_policy()]);
    let runtime = std::cell::RefCell::new(runtime);
    crate::health::reactions::dispatch(
        &mut rig.ctx.registry.borrow_mut(),
        &[target],
        &crate::health::reactions::ApplyDamageArgs {
            amount: 200.0,
            damage_type: None,
        },
    )
    .unwrap();

    let events = rig.tick_evaluating(
        &[],
        |registry| rocket(registry, target, pawn, &mut runtime.borrow_mut()),
        |registry| runtime.borrow_mut().evaluate_pending_in_registry(registry),
    );

    assert_eq!(rig.health(target), 45.0, "recovered to 50, then the rocket");
    assert!(events.death.is_empty());
    assert_eq!(state(&rig, target, "hits"), 2.0);
    assert_eq!(state(&rig, target, "lastAfter"), 45.0);
}
