// Attack turns through the AI tick: the budget, turn lifetimes, the hard gate,
// the turn facts, refresh, profiles and reloads, on the AI test fixtures.
// See: context/lib/entity_model.md §7c (Attack budget)

mod budget;
mod facts;
mod lifetime;
mod pools;
mod refresh;
mod release;
mod reload;
mod tokens;

use super::*;
use crate::attack_turns::{AttackBudgets, BudgetTable, TokenBudget, attack_index};
use postretro_entities::components::brain::{
    AttackTokenId, AttackTurnState, ResolvedDraw, ResolvedDraws,
};
use postretro_foundation::attack_budget::{AttackBudgetVocabulary, TokenDraw};
use postretro_foundation::{BRAIN_ATTACK_COOLDOWN_MS_INPUT, BRAIN_HAS_ATTACK_TURN_INPUT};

const DT: f32 = 0.016;
const MELEE: u16 = 0;
const RANGED: u16 = 1;

/// Where the player stands; attackers stand near the origin facing +X.
fn player_spot() -> Vec3 {
    Vec3::new(1.5, 0.0, 0.0)
}

/// An attacker spot `index` steps along Z, all within the facing tolerance of
/// the player.
fn attacker_spot(index: usize) -> Vec3 {
    Vec3::new(0.0, 0.0, -0.3 + 0.2 * index as f32)
}

/// Run every enemy under per-token `limits`, none refreshing.
fn set_limits(runtime: &mut AiRuntime, limits: &[u32]) {
    runtime.turns.budgets = AttackBudgets::new(BudgetTable::new(
        limits
            .iter()
            .map(|&limit| TokenBudget {
                limit,
                refresh_ms: 0.0,
            })
            .collect(),
    ));
}

fn draws(list: &[(u16, u32)]) -> ResolvedDraws {
    let list: Vec<ResolvedDraw> = list
        .iter()
        .map(|&(token, cost)| ResolvedDraw {
            token: AttackTokenId(token),
            cost,
        })
        .collect();
    ResolvedDraws::from_slice(&list).expect("test draw lists fit")
}

/// Make `attack` on `enemy` draw `list`, standing in for authored `draws`.
/// Binds every brain now against `descriptors`, as the next tick would.
fn set_draws(
    reg: &EntityRegistry,
    runtime: &mut AiRuntime,
    descriptors: &[EntityTypeDescriptor],
    enemy: EntityId,
    attack: &str,
    list: &[(u16, u32)],
) {
    let mut warned = HashSet::new();
    runtime.programs.sync(reg, descriptors, 0, &mut warned);
    let graph = &reg.get_component::<BrainComponent>(enemy).unwrap().graph;
    let index = attack_index(graph, attack).expect("the graph declares the attack");
    runtime
        .programs
        .set_attack_draws_for_test(enemy, index, draws(list));
}

fn turns(reg: &EntityRegistry, enemy: EntityId) -> AttackTurnState {
    reg.get_component::<BrainComponent>(enemy)
        .unwrap()
        .attack_turns
}

/// The attacks `enemy` holds turns for, by name.
fn held(reg: &EntityRegistry, enemy: EntityId) -> Vec<String> {
    let brain = reg.get_component::<BrainComponent>(enemy).unwrap();
    brain
        .attack_turns
        .held()
        .filter_map(|turn| crate::attack_turns::attack_name(&brain.graph, turn.attack))
        .map(str::to_string)
        .collect()
}

fn holders(reg: &EntityRegistry, enemies: &[EntityId]) -> usize {
    enemies
        .iter()
        .filter(|&&enemy| turns(reg, enemy).holds_any())
        .count()
}

fn requested(reg: &EntityRegistry, enemy: EntityId) -> Option<String> {
    let brain = reg.get_component::<BrainComponent>(enemy).unwrap();
    brain
        .attack_turns
        .request
        .and_then(|request| crate::attack_turns::attack_name(&brain.graph, request.attack))
        .map(str::to_string)
}

fn wait_since(reg: &EntityRegistry, enemy: EntityId) -> Option<u64> {
    turns(reg, enemy).wait.map(|wait| wait.since_tick)
}

fn contact(damage: f32, cooldown_ms: f32) -> AttackParams {
    AttackParams {
        sound: None,
        weapon: None,
        damage: Some(damage),
        max_range: Some(2.0),
        cooldown_ms: Some(cooldown_ms),
        engagement_radius: None,
        standoff_distance: None,
        damage_type: None,
        draws: None,
    }
}

fn weapon(name: &str) -> AttackParams {
    AttackParams {
        sound: None,
        weapon: Some(name.to_string()),
        damage: None,
        max_range: None,
        cooldown_ms: None,
        engagement_radius: None,
        standoff_distance: None,
        damage_type: None,
        draws: None,
    }
}

/// A standing activity that fires `attack`.
fn striking(attack: &str) -> BehaviorActivityDescriptor {
    authored_state(
        "attack",
        MotionVerb::Hold,
        Some(ActionVerb::Attack(attack.to_string())),
    )
}

fn idle() -> BehaviorActivityDescriptor {
    authored_state("idle", MotionVerb::Hold, None)
}

fn marked(mut activity: BehaviorActivityDescriptor, attack: &str) -> BehaviorActivityDescriptor {
    activity.attack_turn = Some(attack.to_string());
    activity
}

fn graph(
    initial: &str,
    activities: Vec<(&str, BehaviorActivityDescriptor)>,
    transitions: Vec<(&str, Vec<GuardedRow>)>,
    attacks: Vec<(&str, AttackParams)>,
) -> BehaviorGraphDescriptor {
    test_behavior_graph!({
        initial: initial.to_string(),
        activities: activities
            .into_iter()
            .map(|(name, activity)| (name.to_string(), activity))
            .collect(),
        transitions: transitions
            .into_iter()
            .map(|(from, rows)| (from.to_string(), rows))
            .collect(),
        candidate_filter: None,
        retaliation: None,
        patrol: None,
        attacks: attacks
            .into_iter()
            .map(|(name, attack)| (name.to_string(), attack))
            .collect(),
        engagement_radius: None,
        move_speed: TEST_MOVE_SPEED,
    })
}

/// The standing contact attacker: `strike` fires `attack` once per entry.
fn striker(cooldown_ms: f32) -> BehaviorGraphDescriptor {
    graph(
        "strike",
        vec![("strike", striking("attack"))],
        vec![],
        vec![("attack", contact(1.0, cooldown_ms))],
    )
}

/// An attacker that re-enters `strike` 48 ms after each fire, so it keeps
/// asking for turns.
fn cycling_striker(cooldown_ms: f32) -> BehaviorGraphDescriptor {
    graph(
        "strike",
        vec![("strike", striking("attack")), ("rest", idle())],
        vec![
            (
                "strike",
                vec![edge(
                    "rest",
                    IrNode::Ge {
                        a: Box::new(brain_input(BRAIN_ATTACKS_FIRED_IN_ACTIVITY_INPUT)),
                        b: Box::new(IrNode::Const {
                            value: IrValue::Number(1.0),
                        }),
                    },
                )],
            ),
            (
                "rest",
                vec![edge(
                    "strike",
                    IrNode::Ge {
                        a: Box::new(brain_input(BRAIN_TIME_IN_ACTIVITY_MS_INPUT)),
                        b: Box::new(IrNode::Const {
                            value: IrValue::Number(48.0),
                        }),
                    },
                )],
            ),
        ],
        vec![("attack", contact(1.0, cooldown_ms))],
    )
}

fn spawn_at(
    reg: &mut EntityRegistry,
    index: usize,
    graph: &BehaviorGraphDescriptor,
    state: &str,
) -> EntityId {
    spawn_enemy(
        reg,
        attacker_spot(index),
        authored_brain(graph, state),
        50.0,
    )
}

/// Enter root activity `state` as a transition would.
fn enter(reg: &mut EntityRegistry, enemy: EntityId, state: &str) {
    let mut brain = reg.get_component::<BrainComponent>(enemy).unwrap().clone();
    let index = graph_activity_index(&brain.graph, state).expect("the graph declares it");
    assert!(brain.enter_activity_at(0, index));
    reg.set_component(enemy, brain).unwrap();
}

fn state(reg: &EntityRegistry, enemy: EntityId) -> String {
    enemy_state_name(reg, enemy)
}

fn fired(reg: &EntityRegistry, enemy: EntityId) -> u32 {
    reg.get_component::<BrainComponent>(enemy)
        .unwrap()
        .activity_attack_count(0)
        .unwrap_or(0)
}

fn tick(reg: &mut EntityRegistry, runtime: &mut AiRuntime) {
    run_ai_tick(reg, runtime, DT);
}

fn tick_with(
    reg: &mut EntityRegistry,
    runtime: &mut AiRuntime,
    descriptors: &[EntityTypeDescriptor],
) -> AiTickResult {
    run_ai_tick_with_navigation_and_impact(
        reg,
        runtime,
        DT,
        AiTickInputs {
            mover_nav: crate::nav::MoverNav::none(),
            nav_graph: None,
            collision_world: Some(&CollisionWorld::new()),
            descriptors,
            descriptor_generation: 0,
            factions: &FactionRegistry::default(),
            faction_sentiment: &RefCell::new(FactionSentimentState::default()),
        },
        |_| {},
    )
}

/// A projectile weapon whose action fires `shots` shots `wait_ms` apart, then
/// recovers for `recovery_ms`.
fn burst_weapon(name: &str, shots: usize, wait_ms: f32, recovery_ms: f32) -> EntityTypeDescriptor {
    let mut descriptor = projectile_weapon_descriptor(name, 4.0, 1.0, recovery_ms);
    let mut steps = Vec::new();
    for shot in 0..shots {
        if shot > 0 {
            steps.push(serde_json::json!({ "kind": "wait", "durationMs": wait_ms }));
        }
        steps.push(serde_json::json!({ "kind": "shot" }));
    }
    descriptor.weapon.as_mut().unwrap().primary = serde_json::from_value(serde_json::json!({
        "trigger": "press", "recoveryMs": recovery_ms, "steps": steps,
    }))
    .expect("a valid burst action");
    descriptor
}

fn player_hp_lost(reg: &EntityRegistry, pawn: EntityId) -> f32 {
    100.0 - player_hp(reg, pawn)
}

/// Register `tokens` (name, limit) as the mod's `attackTokens`, as a
/// committed manifest does.
fn install_tokens(reg: &mut EntityRegistry, tokens: &[(&str, u32)]) {
    let declarations: Vec<serde_json::Value> = tokens
        .iter()
        .map(|(name, limit)| {
            serde_json::json!({ "kind": "attackToken", "name": name, "budget": { "limit": limit } })
        })
        .collect();
    let vocabulary =
        AttackBudgetVocabulary::from_manifest(Some(&serde_json::json!(declarations)), None)
            .expect("a valid token registration");
    reg.set_attack_budgets(std::sync::Arc::new(vocabulary));
}

/// A draw of one `name`, as a bare handle crosses.
fn token(name: &str) -> TokenDraw {
    TokenDraw(serde_json::json!({ "kind": "attackToken", "name": name }))
}

/// `name.with({ cost })`, as it crosses.
fn costing(name: &str, cost: u32) -> TokenDraw {
    TokenDraw(serde_json::json!({ "token": { "kind": "attackToken", "name": name }, "cost": cost }))
}

fn drawing(mut attack: AttackParams, draws: Vec<TokenDraw>) -> AttackParams {
    attack.draws = Some(draws);
    attack
}

/// A held-forever contact attack drawing `draws`.
fn held_attack(draws: Vec<TokenDraw>) -> AttackParams {
    drawing(contact(1.0, 100_000.0), draws)
}

/// Marked activities, one per attack, plus `idle`, which releases.
fn marked_graph(attacks: Vec<(&str, AttackParams)>) -> BehaviorGraphDescriptor {
    let names: Vec<String> = attacks.iter().map(|(name, _)| name.to_string()).collect();
    let acts: Vec<String> = names.iter().map(|name| format!("{name}_act")).collect();
    let mut activities = vec![("idle", idle())];
    for (name, act) in names.iter().zip(&acts) {
        activities.push((act.as_str(), marked(striking(name), name)));
    }
    graph("idle", activities, vec![], attacks)
}
