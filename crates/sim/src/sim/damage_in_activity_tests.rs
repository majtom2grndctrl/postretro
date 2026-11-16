// `@brain.damageInActivity` fed at the damage chokepoint: every producer's hits
// count across defense layers and health, and the fixed tick's stage order
// decides which activity a hit counts toward.
// See: context/lib/scripting.md §11 · context/lib/entity_model.md §7c

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::rc::Rc;

use glam::Vec3;
use postretro_entities::components::brain::BrainComponent;
use postretro_entities::components::defense_layers::{DefenseLayer, DefenseLayersComponent};
use postretro_entities::components::health::{HealthComponent, Hitbox};
use postretro_entities::components::inventory::Inventory;
use postretro_entities::components::projectile::ProjectileComponent;
use postretro_entities::{EntityId, EntityRegistry, Transform};
use postretro_foundation::{
    BRAIN_DAMAGE_IN_ACTIVITY_INPUT, BehaviorActivityDescriptor, BehaviorGraphDescriptor,
    BehaviorGraphEnvelope, GuardedRow, IrNode, IrValue, MotionVerb, SplashDescriptor,
};

use super::tests::{run_local_only_tick, sim_command, trigger_movement, weapon_component};

const TICK_DT: f32 = 1.0 / 60.0;
/// Where every target stands: straight down the shooter's `-Z` aim.
const TARGET_POSITION: Vec3 = Vec3::new(0.0, 0.0, -5.0);
const TARGET_HEALTH: f32 = 100.0;

fn hold(name: &str) -> (String, BehaviorActivityDescriptor) {
    (
        name.to_string(),
        BehaviorActivityDescriptor {
            sound: None,
            animation: None,
            motion: Some(MotionVerb::Hold),
            action: None,
            on_enter: None,
            facing: None,
            reposition: None,
            attack_turn: None,
            speed: None,
            layers: BTreeMap::new(),
        },
    )
}

fn damage_at_least(points: f32) -> IrNode {
    IrNode::Ge {
        a: Box::new(IrNode::Input {
            name: BRAIN_DAMAGE_IN_ACTIVITY_INPUT.to_string(),
            owner: None,
        }),
        b: Box::new(IrNode::Const {
            value: IrValue::Number(points),
        }),
    }
}

/// A planted brain over `activities` (the first is initial), leaving each by
/// `rows`: `(from, to, damage threshold)`.
fn graph(activities: &[&str], rows: &[(&str, &str, f32)]) -> BehaviorGraphDescriptor {
    let mut transitions: BTreeMap<String, Vec<GuardedRow>> = BTreeMap::new();
    for (from, to, points) in rows {
        transitions
            .entry(from.to_string())
            .or_default()
            .push(GuardedRow {
                to: to.to_string(),
                when: damage_at_least(*points),
            });
    }
    BehaviorGraphDescriptor {
        knockback: Default::default(),
        envelope: BehaviorGraphEnvelope {
            initial: activities[0].to_string(),
            activities: activities.iter().map(|name| hold(name)).collect(),
            transitions,
        },
        candidate_filter: None,
        retaliation: None,
        patrol: None,
        attacks: BTreeMap::new(),
        engagement_radius: None,
        move_speed: 1.0,
    }
}

/// A brained target down the aim line with `armor` points of armor ahead of
/// its health.
fn spawn_target(registry: &mut EntityRegistry, brain: BrainComponent, armor: f32) -> EntityId {
    let target = registry.spawn(Transform {
        position: TARGET_POSITION,
        ..Transform::default()
    });
    registry
        .set_component(
            target,
            HealthComponent {
                max: TARGET_HEALTH,
                current: TARGET_HEALTH,
                hitbox: Some(Hitbox {
                    half_extents: Vec3::splat(0.5),
                    offset: Vec3::ZERO,
                }),
                death_handled: false,
                pending_kill_credit: None,
                zone_multipliers: Default::default(),
                contributor_ledger: Default::default(),
            },
        )
        .unwrap();
    registry.set_component(target, brain).unwrap();
    if armor > 0.0 {
        registry
            .set_component(
                target,
                DefenseLayersComponent {
                    descriptor: None,
                    layers: vec![DefenseLayer {
                        name: "armor".to_string(),
                        value: armor,
                        capacity: armor,
                        recharge_delay_ms: 3_000.0,
                        recharge_rate: 0.0,
                        resist: Vec::new(),
                        refill: Default::default(),
                    }],
                    health_resist: Vec::new(),
                    last_hit_tick: None,
                },
            )
            .unwrap();
    }
    target
}

/// The local shooter at the origin, aiming `-Z`, holding a hitscan weapon that
/// fires `pellets` pellets of 10 damage each.
fn spawn_shooter(registry: &mut EntityRegistry, pellets: u32) -> EntityId {
    let pawn = registry.spawn(Transform::default());
    let weapon = registry.spawn(Transform::default());
    let mut component = weapon_component("weapon.test.damage-in-activity");
    component.pellet_count = pellets;
    component.spread_degrees = 0.0;
    registry.set_component(weapon, component).unwrap();
    let mut inventory = Inventory::default();
    inventory.wieldables[0] = Some(weapon);
    registry.set_component(pawn, inventory).unwrap();
    registry.set_component(pawn, trigger_movement()).unwrap();
    registry.mark_local_player_pawn(pawn).unwrap();
    weapon
}

/// A projectile already in flight toward the target: it reaches it on the
/// next Projectiles stage. `splash` detonates it there instead.
fn spawn_projectile(registry: &mut EntityRegistry, damage: f32, splash: Option<SplashDescriptor>) {
    let owner_pawn = registry.spawn(Transform::default());
    let owner_weapon = registry.spawn(Transform::default());
    let projectile = registry.spawn(Transform {
        position: Vec3::new(0.0, 0.0, -3.0),
        ..Transform::default()
    });
    registry
        .set_component(
            projectile,
            ProjectileComponent {
                source_sounds: None,
                predicted_visible: true,
                source_action: None,
                source_shot: None,
                knockback_impulse: [0.0; 3],
                direction: Vec3::NEG_Z.to_array(),
                speed: 180.0,
                radius: 0.0,
                remaining_range: 10.0,
                remaining_lifetime: 10.0,
                damage,
                credit_source: "test.projectile".to_string(),
                owner_pawn,
                owner_weapon,
                attackerless: false,
                spawned: false,
                predicted_shot_id: None,
                elapsed_flight_age: 0.0,
                flipbook_active: false,
                impact_light: None,
                splash,
                source_weapon: None,
                activation: None,
                damage_type: None,
            },
        )
        .unwrap();
}

/// One fixed host tick: Projectiles, then AI, then Weapons, with the local
/// shooter firing when `fire` is set.
fn tick(registry: &Rc<RefCell<EntityRegistry>>, weapon: EntityId, fire: bool) {
    run_local_only_tick(registry.clone(), weapon, &sim_command(fire, false), TICK_DT);
}

fn armor(registry: &EntityRegistry, id: EntityId) -> f32 {
    registry
        .get_component::<DefenseLayersComponent>(id)
        .map_or(0.0, |layers| layers.layers[0].value)
}

fn health(registry: &EntityRegistry, id: EntityId) -> f32 {
    registry
        .get_component::<HealthComponent>(id)
        .unwrap()
        .current
}

fn brain(registry: &EntityRegistry, id: EntityId) -> &BrainComponent {
    registry.get_component::<BrainComponent>(id).unwrap()
}

fn idle_brain() -> BrainComponent {
    BrainComponent::from_graph(&graph(&["idle"], &[]))
}

/// Run one tick against a fresh armored target and check its sum is exactly
/// what the tick drained from its armor and health, and that the hit crossed
/// both.
fn assert_counts_across_armor_and_health(
    producer: &str,
    armor_before: f32,
    setup: impl FnOnce(&mut EntityRegistry) -> (EntityId, bool),
) -> f32 {
    let registry = Rc::new(RefCell::new(EntityRegistry::new()));
    let target = spawn_target(&mut registry.borrow_mut(), idle_brain(), armor_before);
    let (weapon, fire) = setup(&mut registry.borrow_mut());
    tick(&registry, weapon, fire);

    let registry = registry.borrow();
    let absorbed = armor_before - armor(&registry, target);
    let lost = TARGET_HEALTH - health(&registry, target);
    assert!(absorbed > 0.0, "{producer}: the hit drains the armor");
    assert!(lost > 0.0, "{producer}: and spills into health");
    let sum = brain(&registry, target)
        .damage_in_activity(0)
        .expect("idle is active");
    assert!(
        (sum - (absorbed + lost)).abs() <= 1.0e-4,
        "{producer}: the sum {sum} is absorbed {absorbed} plus health loss {lost}"
    );
    sum
}

// D1 (hitscan): eight pellets landing on one tick each count, across armor
// and health.
#[test]
fn hitscan_pellets_on_one_tick_each_count_across_armor_and_health() {
    let sum = assert_counts_across_armor_and_health("hitscan", 30.0, |registry| {
        (spawn_shooter(registry, 8), true)
    });
    assert_eq!(sum, 80.0, "all eight 10-point pellets count");
}

// D1 (projectile): a direct projectile hit counts across armor and health.
#[test]
fn projectile_hit_counts_across_armor_and_health() {
    let sum = assert_counts_across_armor_and_health("projectile", 10.0, |registry| {
        spawn_projectile(registry, 25.0, None);
        (spawn_shooter(registry, 1), false)
    });
    assert_eq!(sum, 25.0);
}

// D1 (splash): a splash detonation counts across armor and health.
#[test]
fn splash_hit_counts_across_armor_and_health() {
    assert_counts_across_armor_and_health("splash", 10.0, |registry| {
        spawn_projectile(
            registry,
            25.0,
            Some(SplashDescriptor {
                knockback: None,
                radius: 2.0,
                min_fraction: 1.0,
                self_damage: true,
            }),
        );
        (spawn_shooter(registry, 1), false)
    });
}

// D1 (`applyDamage`): script damage carries no attacker and still counts;
// two dispatches on one tick both count, across armor and health.
#[test]
fn apply_damage_reactions_on_one_tick_both_count_across_armor_and_health() {
    use crate::health::reactions::{ApplyDamageArgs, dispatch};

    let mut registry = EntityRegistry::new();
    let target = spawn_target(&mut registry, idle_brain(), 10.0);
    for amount in [6.0, 9.0] {
        dispatch(
            &mut registry,
            &[target],
            &ApplyDamageArgs {
                amount,
                damage_type: None,
            },
        )
        .unwrap();
    }

    assert_eq!(armor(&registry, target), 0.0);
    assert_eq!(health(&registry, target), TARGET_HEALTH - 5.0);
    assert_eq!(brain(&registry, target).damage_in_activity(0), Some(15.0));
}

// D2 (pin P2): on one tick a projectile lands before the AI pass and a
// hitscan after it. The first counts toward `a` only, whose row it passes; the
// second counts toward `b`, which that pass entered, and is read on `b`'s first
// evaluation, the next tick.
#[test]
fn a_hit_before_the_ai_pass_counts_toward_the_activity_left_and_one_after_toward_the_one_entered() {
    let registry = Rc::new(RefCell::new(EntityRegistry::new()));
    let (target, weapon) = {
        let mut registry = registry.borrow_mut();
        let brain = BrainComponent::from_graph(&graph(
            &["a", "b", "c"],
            &[("a", "b", 20.0), ("b", "c", 10.0)],
        ));
        let target = spawn_target(&mut registry, brain, 0.0);
        spawn_projectile(&mut registry, 25.0, None);
        (target, spawn_shooter(&mut registry, 1))
    };

    tick(&registry, weapon, true);
    {
        let registry = registry.borrow();
        assert_eq!(
            health(&registry, target),
            TARGET_HEALTH - 35.0,
            "both hits landed on tick N"
        );
        let brain = brain(&registry, target);
        assert_eq!(
            brain.active_path_names(),
            ["b"],
            "the projectile counted toward `a` before the AI pass, passing its row"
        );
        assert_eq!(
            brain.damage_in_activity(0),
            Some(10.0),
            "only the hitscan after the AI pass counts toward `b`"
        );
    }

    tick(&registry, weapon, false);
    let registry = registry.borrow();
    let brain = brain(&registry, target);
    assert_eq!(
        brain.active_path_names(),
        ["c"],
        "`b`'s first evaluation reads the post-pass hit"
    );
    assert_eq!(brain.damage_in_activity(0), Some(0.0));
}
