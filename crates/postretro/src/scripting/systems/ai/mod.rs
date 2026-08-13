// Engine-owned enemy brain tick: snapshot/compute/apply passes over enemy
// behavior graphs — target steering, damage, animation, and facing.
// See: context/lib/entity_model.md §5 (fixed-tick game logic) ·
//      context/lib/scripting.md §10.5 (the contextual damage chokepoint)

// Mods declare the state graph; Rust executes it. Every brain carries an
// authored `BehaviorGraphDescriptor`, and this module drives exactly one
// evaluator over it. There is no live VM at tick: guards are IR programs bound
// once per graph into the evaluator's side-table (`brain_programs.rs`) and read
// through a refreshed scope (`brain_scope.rs`).
//
// The split of duties: `graph_eval.rs` owns the pure selection and the verb
// vocabulary, `targeting.rs` owns target selection, and this module layers the
// registry reads/writes — steering, damage, facing, animation — on top. The
// engine floor (stride, target selection, hysteresis, combat slots, the aggro
// gate) sits UPSTREAM of guard evaluation and is not authorable.
//
// Exactly ONE thing suppresses guard evaluation: a closed aggro gate, which
// stands the brain down to its graph's `initial` state with steering cleared and
// reads neither targeting nor guards. Everything else — including having no
// target at all — evaluates the whole guard set as usual, with the no-target
// facts (`@brain.hasTarget` false, `@brain.targetDistance` at its sentinel)
// projected into the scope. That is what lets a sealed-closet enemy that gets
// shot flinch on an authored interrupt while it has nobody to chase.

use std::borrow::Cow;
use std::collections::HashSet;

use glam::{Quat, Vec3};

mod brain_programs;
mod brain_scope;
mod candidate_scope;
mod combat_slots;
mod engine_floor;
mod facing;
mod graph_eval;
mod targeting;

#[cfg(test)]
#[path = "../ai_tests.rs"]
mod ai_tests;

use crate::agent_steering;
use crate::collision::CollisionWorld;
use crate::nav::NavGraph;
use brain_programs::BrainPrograms;
use brain_scope::BrainFacts;
use combat_slots::resolve_combat_slots;
use engine_floor::{POSITION_GOAL_ARRIVAL_EPSILON, SteeringIntent};
use facing::{FACING_TURN_RATE, slew_yaw, yaw_from_rotation, yaw_rotation_toward};
use graph_eval::{
    action_for_state, animation_for_state, engages, initial_index, select_transition, state_at,
    steering_for,
};
pub(crate) use graph_eval::{locomotion_animation, rest_animation};
use postretro_entities::components::brain::BrainComponent;
use postretro_entities::components::health::{
    DamageContext, DamageProducer, apply_damage_with_context,
};
use postretro_entities::components::mesh::{
    SwitchResult, restart_animation_clip, switch_animation_state,
};
use postretro_entities::{
    ComponentKind, ComponentValue, DeferredEffectComponent, DeferredEffectKind, EntityId,
    EntityRegistry, Transform,
};
use postretro_foundation::{ActionVerb, DamagePayload, MotionVerb, PatrolMode};
use targeting::{
    TargetPawn, acquisition_due, select_target, selected_target_alive, target_candidate,
    target_distance,
};

/// Event name fired once per enemy attack that lands this tick. Mirrors the
/// weapon-fire event precedent (`"activate"`/`"impact"`): the tick returns the
/// names it raised and the app drains them through `fire_named_event` after the
/// tick loop settles.
pub(crate) const ENEMY_ATTACK_EVENT: &str = "enemyAttack";
const ENEMY_ATTACK_SOURCE_ID: &str = "enemy.attack";

/// Minimum XZ speed (units/sec) the agent must exceed for "moving" behavior:
/// above it the enemy orients to its velocity and a locomotion state plays its
/// own travel animation; at or below it the enemy is treated as stopped, faces
/// its target, and a locomotion state substitutes the graph's rest animation. A
/// shared epsilon keeps facing and locomotion animation in agreement.
const MOVE_SPEED_EPSILON: f32 = 0.05;

#[derive(Debug, Clone, Copy, PartialEq)]
struct LocomotionIntent {
    moving: bool,
    speed_xz_sq: f32,
}

impl LocomotionIntent {
    const STOPPED: Self = Self {
        moving: false,
        speed_xz_sq: 0.0,
    };

    fn from_velocity(velocity: Vec3) -> Self {
        let speed_xz_sq = velocity.x * velocity.x + velocity.z * velocity.z;
        Self {
            moving: speed_xz_sq > MOVE_SPEED_EPSILON * MOVE_SPEED_EPSILON,
            speed_xz_sq,
        }
    }
}

fn should_switch_animation(state_changed: bool, moving: bool, latch: bool) -> bool {
    state_changed || moving != latch
}

/// Resolve a state motion whose destination depends on per-brain state. Unlike
/// [`steering_for`], this runs in the compute pass where the spawn anchor,
/// patrol descriptor, and persistent patrol cursor are all available.
fn position_goal_steering(
    motion: MotionVerb,
    brain: &mut BrainComponent,
    position: Vec3,
) -> SteeringIntent {
    match motion {
        MotionVerb::MoveToAnchor => (crate::nav::distance_xz(position, brain.home_anchor)
            <= POSITION_GOAL_ARRIVAL_EPSILON)
            .then_some(SteeringIntent::Clear)
            .unwrap_or(SteeringIntent::MoveTo(brain.home_anchor)),
        MotionVerb::Patrol => patrol_steering(brain, position),
        // This resolver owns only motion modes with per-brain position goals.
        // Every other mode remains the pure graph evaluator's responsibility.
        MotionVerb::ChaseTarget | MotionVerb::Hold | MotionVerb::Freeze => steering_for(motion),
    }
}

/// Resolve the next patrol point and preserve the route phase on the brain.
/// A malformed hand-built graph degrades to standing still; descriptor
/// validation rejects the same shape before authored data reaches this path.
fn patrol_steering(brain: &mut BrainComponent, position: Vec3) -> SteeringIntent {
    let Some(patrol) = brain.graph.patrol.as_ref() else {
        return SteeringIntent::Clear;
    };
    let point_count = patrol.points.len();
    if point_count == 0 {
        return SteeringIntent::Clear;
    }
    let mode = patrol.mode;

    // A saved brain may outlive a descriptor edit that shortens the route.
    // Preserve its phase rather than resetting it before indexing.
    brain.patrol_cursor %= point_count;
    let mut goal = patrol_goal(brain);
    if crate::nav::distance_xz(position, goal) <= POSITION_GOAL_ARRIVAL_EPSILON {
        advance_patrol_cursor(brain, point_count, mode);
        goal = patrol_goal(brain);
    }
    SteeringIntent::MoveTo(goal)
}

fn patrol_goal(brain: &BrainComponent) -> Vec3 {
    let patrol = brain
        .graph
        .patrol
        .as_ref()
        .expect("patrol goal is only requested for a present non-empty route");
    let [x, z] = patrol.points[brain.patrol_cursor];
    brain.home_anchor + Vec3::new(x, 0.0, z)
}

fn advance_patrol_cursor(brain: &mut BrainComponent, point_count: usize, mode: PatrolMode) {
    if point_count == 1 {
        return;
    }

    match mode {
        PatrolMode::Loop => {
            brain.patrol_cursor = (brain.patrol_cursor + 1) % point_count;
        }
        PatrolMode::PingPong if brain.patrol_direction >= 0 => {
            brain.patrol_direction = 1;
            if brain.patrol_cursor + 1 == point_count {
                brain.patrol_direction = -1;
                brain.patrol_cursor -= 1;
            } else {
                brain.patrol_cursor += 1;
            }
        }
        PatrolMode::PingPong => {
            brain.patrol_direction = -1;
            if brain.patrol_cursor == 0 {
                brain.patrol_direction = 1;
                brain.patrol_cursor = 1;
            } else {
                brain.patrol_cursor -= 1;
            }
        }
    }
}

/// Per-enemy snapshot captured under the immutable iterator borrow so the
/// mutable writes (steering, damage, animation) happen after the walk completes.
/// The compute pass CONSUMES these — the brain is moved into its outcome rather
/// than cloned a second time, which matters because a brain carries its graph.
struct EnemySnapshot {
    id: EntityId,
    position: Vec3,
    brain: BrainComponent,
}

/// One enemy's resolved outcome after evaluating its brain this tick, applied in
/// a second pass under `&mut registry`.
pub(super) struct EnemyOutcome {
    pub(super) id: EntityId,
    /// This enemy's position as snapshotted, carried forward so combat-slot
    /// resolution needs nothing but the outcomes.
    pub(super) position: Vec3,
    pub(super) target: Option<TargetPawn>,
    pub(super) brain: BrainComponent,
    steering: SteeringIntent,
    /// `true` when the selected state is ENGAGED with the target — it chases it
    /// or acts on it (`graph_eval::engages`). Drives facing and combat-slot
    /// participation; the destination writes key on `steering` instead.
    pub(super) engaged: bool,
    pub(super) combat_slot: Option<Vec3>,
    /// The target this brain held BEFORE this tick's evaluation — the incumbency
    /// test for combat-slot retention.
    pub(super) prior_acquired_target: Option<EntityId>,
    /// `true` when the graph state changed this tick; the apply pass uses this
    /// with locomotion intent changes to decide whether to switch animation.
    state_changed: bool,
    /// `true` when an attack landed this tick (damage applied, event raised).
    attacked: bool,
    /// The entered state's authored `on_enter` address, present only on the tick
    /// the brain entered it.
    on_enter: Option<String>,
}

/// The AI tick's run-long state, owned by `App` across ticks.
///
/// Two things outlive a tick: the warn-once latch and the evaluator's bound
/// guard programs. They travel together because both are reconciled at the top
/// of every tick — `sync` binds newly seen graphs, and a guard that fails to
/// bind reports through the same latch that reports an unresolvable animation.
pub(crate) struct AiRuntime {
    /// Warn-once latch for the CONTENT-keyed diagnostics, namespaced so a given
    /// one fires once across the whole run, never each tick: `anim:<name>` for an
    /// animation state that fails to switch (`UnknownState`/`NotAnimated` — the
    /// prior animation is kept and the tick is never aborted),
    /// and `brain-guard:<graph>:<path>:<to>:<reason>` for a transition guard
    /// that failed to bind. Both keys are CONTENT — an animation name, a graph
    /// shape — so they are bounded by the mod's authored content, not by how
    /// many entities the level spawns. Anything keyed by ENTITY belongs in a
    /// typed, prunable set instead (see [`Self::reseat_warned`]).
    pub(crate) warned: HashSet<String>,
    /// Enemies already reported as seated in a behavior state their graph does
    /// not declare.
    ///
    /// Entity-keyed, so it gets the same treatment as [`Self::blocked_warned`]
    /// rather than a per-spawn `format!`ed `String` in the run-long content
    /// latch: an unbounded set of one-string-per-enemy is not something a
    /// wave-spawning level should accumulate for the process lifetime, however
    /// rarely each entry is added. Pruned against the live brains each tick.
    reseat_warned: HashSet<EntityId>,
    /// Enemies already reported as unable to route to their chase destination.
    ///
    /// Entity-keyed, and separate from `warned`, for two reasons a `format!`ed
    /// string key handled badly: the latch check itself must not allocate (a
    /// genuinely unroutable enemy reaches it every tick for the rest of the
    /// run, long after the latch closed), and the set must be prunable, since a
    /// wave-spawning level would otherwise accumulate one entry per enemy that
    /// ever blocked. `run_ai_tick_with_navigation_and_impact` prunes it against
    /// the live brains each tick.
    blocked_warned: HashSet<EntityId>,
    /// Per-entity bound transition guards. Derived data, rebuilt from each
    /// brain's retained graph whenever the entity is (re)seen.
    programs: BrainPrograms,
}

impl AiRuntime {
    pub(crate) fn new() -> Self {
        Self {
            warned: HashSet::new(),
            blocked_warned: HashSet::new(),
            reseat_warned: HashSet::new(),
            programs: BrainPrograms::new(),
        }
    }
}

impl Default for AiRuntime {
    fn default() -> Self {
        Self::new()
    }
}

/// Drive every enemy brain one tick. Returns the event addresses raised this
/// tick — one [`ENEMY_ATTACK_EVENT`] per enemy that attacked, plus each entered
/// state's authored `on_enter` — for the app's post-tick event drain. `tick_dt`
/// is the fixed tick delta in seconds.
///
/// The return is `Cow` so the static attack event costs nothing to raise while
/// an authored address still travels as an owned `String`; the owning clone
/// happens once per state ENTRY, never per tick.
///
/// Ordering inside the tick, PER enemy:
/// 1. Tick the attack cooldown and the time-in-state down/up (every tick).
/// 2. Evaluate the graph's guards — interrupts first, then the current state's
///    transitions, declaration order, first true wins. Every guard is evaluated
///    every armed tick, target or not: nothing latches evaluation off, so a
///    commitment window is an authored `@brain.timeInStateMs` guard rather than
///    an engine rule. A CLOSED aggro gate is the sole exception — it stands the
///    brain down to its `initial` state and skips evaluation entirely.
/// 3. On entering a state, reset the time-in-state and raise its `on_enter`.
/// 4. When the selected state declares the `attack` action, the cooldown has
///    elapsed, and the selected target is inside `attack.range`, apply the
///    graph's damage to that pawn through the chokepoint and raise the attack
///    event.
/// 5. On a state CHANGE or locomotion stop/resume, request the selected
///    animation state.
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn run_ai_tick(
    registry: &mut EntityRegistry,
    runtime: &mut AiRuntime,
    tick_dt: f32,
) -> Vec<Cow<'static, str>> {
    run_ai_tick_with_navigation(registry, runtime, tick_dt, None, None)
}

pub(crate) fn run_ai_tick_with_navigation(
    registry: &mut EntityRegistry,
    runtime: &mut AiRuntime,
    tick_dt: f32,
    nav_graph: Option<&NavGraph>,
    collision_world: Option<&CollisionWorld>,
) -> Vec<Cow<'static, str>> {
    run_ai_tick_with_navigation_and_impact(
        registry,
        runtime,
        tick_dt,
        nav_graph,
        collision_world,
        |_| {},
    )
}

pub(crate) fn run_ai_tick_with_navigation_and_impact(
    registry: &mut EntityRegistry,
    runtime: &mut AiRuntime,
    tick_dt: f32,
    nav_graph: Option<&NavGraph>,
    collision_world: Option<&CollisionWorld>,
    mut on_impact: impl FnMut(&mut EntityRegistry),
) -> Vec<Cow<'static, str>> {
    let dt_ms = tick_dt.max(0.0) * 1000.0;

    // Reconcile the bound-guard side-table with the registry's live brains
    // before anything reads it: this is the single lifecycle hook covering
    // spawn, despawn, and a wholesale deserialize.
    let AiRuntime {
        warned,
        blocked_warned,
        reseat_warned,
        programs,
    } = runtime;
    programs.sync(registry, warned);

    // Bound the blocked-warn latch to entities that still carry a brain: the
    // side-table `sync` just reconciled is the authoritative live set, so this
    // is where the pruning is free. Without it a wave-spawning level accumulates
    // one entry per enemy that ever blocked, for the process lifetime. A reused
    // entity id may report once more, which is the right answer for what is a
    // different enemy.
    blocked_warned.retain(|id| programs.get(*id).is_some());
    reseat_warned.retain(|id| programs.get(*id).is_some());

    // Pass 1: snapshot every brain-bearing enemy under the immutable borrow.
    let snapshots: Vec<EnemySnapshot> = registry
        .iter_with_kind(ComponentKind::Brain)
        .filter_map(|(id, value)| {
            // A terminal impact effect or queued despawn leaves the id live
            // long enough for a same-group playAnim to address it. AI must not
            // overwrite that presentation request or keep steering/attacking.
            if registry
                .get_component::<DeferredEffectComponent>(id)
                .is_ok_and(|effects| {
                    effects.inert
                        || effects
                            .pending
                            .iter()
                            .any(|effect| effect.kind == DeferredEffectKind::Despawn)
                })
                || crate::impact_effects::is_downed_for_recovery(registry, id)
            {
                return None;
            }
            let ComponentValue::Brain(brain) = value else {
                return None;
            };
            let position = registry.get_component::<Transform>(id).ok()?.position;
            Some(EnemySnapshot {
                id,
                position,
                brain: brain.clone(),
            })
        })
        .collect();

    // Pass 2 (compute): evaluate each brain, producing the outcomes to apply.
    let mut outcomes: Vec<EnemyOutcome> = Vec::with_capacity(snapshots.len());
    for snap in snapshots {
        let mut brain = snap.brain;
        // A home-distance guard is about the evaluating enemy alone, not its
        // target or the acquisition stride. Compute it once from this tick's
        // immutable position snapshot before either branch can suppress target
        // work.
        let distance_from_anchor = crate::nav::distance_xz(snap.position, brain.home_anchor);
        let prior_state_index = brain.state_index;
        let prior_acquired_target = brain.acquired_target;
        let (target, evaluate_acquisition) = if brain.aggro_armed {
            // A target is retained across ticks only while the brain is engaged
            // — chasing one, or acting on one. A resting brain re-ranks
            // candidates instead of honoring a stale acquired id.
            let retained_target = engages(&brain.graph, brain.state_index)
                .then_some(brain.acquired_target)
                .flatten();
            let retained = retained_target
                .and_then(|entity| target_candidate(registry, entity, snap.position, None));
            let (target, evaluate_acquisition) = if let Some(retained) = retained {
                // A retained target alone prices the stride from its raw
                // distance. A due tick may still run the normal hysteresis scan
                // below, but neither its candidate filter nor its result can
                // alter this cost input.
                let evaluate_acquisition = acquisition_due(&brain, Some(retained.distance));
                let target = if evaluate_acquisition {
                    let (candidate_filter, candidate_scope) =
                        programs.candidate_filter_context(snap.id);
                    select_target(
                        registry,
                        snap.position,
                        Some(retained.target.entity),
                        None,
                        candidate_filter,
                        candidate_scope,
                    )
                    .1
                } else {
                    Some(retained.target)
                };
                (target, evaluate_acquisition)
            } else {
                let (candidate_filter, candidate_scope) =
                    programs.candidate_filter_context(snap.id);
                let (nearest_for_stride, nearest_selection) = select_target(
                    registry,
                    snap.position,
                    None,
                    None,
                    candidate_filter,
                    candidate_scope,
                );
                let evaluate_acquisition = acquisition_due(
                    &brain,
                    nearest_for_stride.map(|candidate| candidate.distance),
                );
                // The raw candidate only prices the stride. A graph-filtered
                // selection becomes a target only on a due tick; otherwise
                // `BrainFacts` stay untargeted rather than borrowing it.
                (
                    evaluate_acquisition.then_some(nearest_selection).flatten(),
                    evaluate_acquisition,
                )
            };
            (target, evaluate_acquisition)
        } else {
            (None, false)
        };

        // (1) Cooldown ticks down and time-in-state accrues every tick, before
        // any guard reads them: a `@brain.timeInStateMs` commitment window then
        // elapses on the first tick its budget is spent, and never earlier.
        brain.attack_cooldown_remaining_ms = (brain.attack_cooldown_remaining_ms - dt_ms).max(0.0);
        brain.time_in_state_ms += dt_ms;

        // Stride bookkeeping advances every tick so the gate is deterministic.
        brain.think_stride_counter = brain.think_stride_counter.wrapping_add(1);

        let mut attacked = false;
        // Forcing the graph's `initial` state is the engine floor's stand-down,
        // and its re-seat: an unvalidated graph whose `initial` names nothing
        // simply stays put rather than being pushed to an arbitrary state.
        let resting_index = initial_index(&brain.graph).unwrap_or(prior_state_index);
        // The FINALLY selected pawn's identity and distance, or `None` with no
        // target. This one binding feeds the guard facts and attack range gate,
        // so neither can disagree about which target they describe.
        let selected_target =
            target.map(|target| (target.entity, target_distance(target, snap.position)));
        let selected_distance = selected_target.map(|(_, distance)| distance);
        let (next_index, steering) = if !brain.aggro_armed {
            // THE AGGRO GATE, and the only thing that suppresses evaluation. Its
            // v1 disengage policy is hold: a closed brain consults neither target
            // selection nor its guards, and standing down clears steering outright
            // rather than deferring to the resting state's motion verb. Clearing
            // the destination sends the agent through steering's destination-less
            // idle-settle path, which has no separation push.
            (resting_index, SteeringIntent::Clear)
        } else {
            // Re-seat a brain whose index addresses no declared state (a graph
            // swapped under a persisted `state_index`, or a hand-seeded one)
            // instead of leaving it wedged: `select_transition` walks from the
            // current state, so an unaddressable one would answer "stay put"
            // forever. The graph's `initial` is the same state the gate stands
            // brains down to.
            let current_index = if state_at(&brain.graph, brain.state_index).is_some() {
                brain.state_index
            } else {
                if reseat_warned.insert(snap.id) {
                    log::warn!(
                        "[AI] enemy {} sat in behavior state index {} which its graph does not \
                         declare; re-seating it to `{}`. Warned once per enemy.",
                        snap.id,
                        brain.state_index,
                        brain.graph.initial,
                    );
                }
                resting_index
            };

            // The think stride is derived from the CURRENT player distance; the
            // gate fires when the per-enemy counter aligns with the band's
            // divisor, and reaches the guards as `@brain.acquisitionDue` for the
            // edges that opt into it. With no target the facts still refresh —
            // `hasTarget` false, `targetDistance` at its sentinel — because an
            // armed brain evaluates its whole guard set whether or not it has a
            // pawn: that is how an interrupt reaches an enemy nobody is standing
            // in front of.
            programs.scope_mut().refresh(
                registry,
                snap.id,
                BrainFacts {
                    target: selected_target,
                    time_in_state_ms: brain.time_in_state_ms,
                    attack_cooldown_ms: brain.attack_cooldown_remaining_ms,
                    acquisition_due: evaluate_acquisition,
                    distance_from_anchor,
                },
            );
            let next_index = programs
                .get(snap.id)
                .and_then(|bound| {
                    select_transition(&brain.graph, bound, programs.scope(), current_index)
                })
                .unwrap_or(current_index);
            let steering = state_at(&brain.graph, next_index)
                .map(|state| state.motion)
                .map(|motion| position_goal_steering(motion, &mut brain, snap.position))
                .unwrap_or(SteeringIntent::Clear);
            // A chase with nothing to chase degrades to a stand-down: with no
            // target there is nothing to move relative to, and leaving the intent
            // as Chase would keep the agent walking to the last destination it
            // was given.
            let steering = match (steering, target) {
                (SteeringIntent::Chase, None) => SteeringIntent::Clear,
                (steering, _) => steering,
            };
            (next_index, steering)
        };

        // The acquired id is the "this brain is engaged" marker the next tick's
        // retention reads, so it is set by ENGAGEMENT (chasing or acting), not by
        // the steering intent — a state that stands still and swings keeps its
        // pawn.
        brain.acquired_target = match target {
            Some(target) if engages(&brain.graph, next_index) => Some(target.entity),
            _ => None,
        };

        // (4) Attack: the selected state declares the `attack` action, the
        // cooldown has elapsed, the SELECTED target is inside the graph's
        // `attack.range`, and it is still alive — apply the configured damage
        // once and arm the cooldown. Checked every tick.
        // The range gate lets a graph declare the action without making it
        // connect from across the room.
        // A graph with no `attack` block configures no range and no damage, so
        // it never attacks.
        // Gating on the selected target's Health stops attack/event spam against
        // an already-dead but still-present pawn and prevents damaging a
        // different co-op pawn than the one this enemy chose.
        if let (Some(target), Some(distance)) = (target, selected_distance) {
            let in_attack_range = brain
                .graph
                .attack
                .is_some_and(|attack| distance <= attack.range);
            if in_attack_range
                && action_for_state(&brain.graph, next_index) == Some(ActionVerb::Attack)
                && brain.attack_cooldown_remaining_ms <= 0.0
                && selected_target_alive(registry, target.entity)
            {
                attacked = true;
                brain.attack_cooldown_remaining_ms =
                    brain.graph.attack.map_or(0.0, |attack| attack.cooldown_ms);
            }
        }

        let state_changed = next_index != prior_state_index;
        let engaged = target.is_some() && engages(&brain.graph, next_index);
        brain.state_index = next_index;
        let on_enter = if state_changed {
            brain.time_in_state_ms = 0.0;
            state_at(&brain.graph, next_index).and_then(|state| state.on_enter.clone())
        } else {
            None
        };

        outcomes.push(EnemyOutcome {
            id: snap.id,
            position: snap.position,
            target,
            prior_acquired_target,
            state_changed,
            attacked,
            on_enter,
            steering,
            engaged,
            combat_slot: None,
            brain,
        });
    }

    resolve_combat_slots(&mut outcomes, nav_graph, collision_world);

    // Pass 3 (apply): write back brains, drive steering, apply damage, and
    // switch animation. Mutable borrow only; no iterator held.
    let mut events: Vec<Cow<'static, str>> = Vec::new();
    for mut outcome in outcomes {
        // Persist the brain (state + timers + stride counter) BEFORE the damage
        // chokepoint below, so an impact policy, death effect, or `on_impact`
        // callback reacting to this enemy's attack reads the state it is now in
        // rather than last tick's. That ordering is why this write stays and the
        // locomotion latch is folded in by re-reading at the end of the loop
        // instead of writing this snapshot back a second time.
        let _ = registry.set_component(outcome.id, outcome.brain.clone());

        // The entered state's authored entry event. Raised before this tick's
        // action so a reaction reads the state the brain is now IN.
        if let Some(address) = outcome.on_enter.take() {
            events.push(Cow::Owned(address));
        }

        let path_state = agent_steering::path_state(registry, outcome.id);
        let locomotion_intent = if outcome.brain.aggro_armed {
            path_state
                .as_ref()
                .map(|path| LocomotionIntent::from_velocity(path.velocity))
                .unwrap_or(LocomotionIntent::STOPPED)
        } else {
            LocomotionIntent::STOPPED
        };

        // Steering: chase sets the destination to a selected combat slot when
        // one is available, otherwise to the raw target position. Fixed
        // position goals write their resolved destination directly. Clear
        // stands down; hold releases the agent on the tick it takes over and
        // leaves it untouched thereafter.
        // `set_destination`/`clear_destination` no-op when the enemy carries no
        // agent component.
        match outcome.steering {
            SteeringIntent::Chase => {
                if let Some(target) = outcome.target {
                    let destination = outcome.combat_slot.unwrap_or(target.position);
                    agent_steering::set_destination(registry, outcome.id, destination);
                    // Diagnostic read of the steering surface: an agent that
                    // cannot route to the destination it was given AND holds no
                    // previous path to keep following is `blocked`. Surface it
                    // once per enemy so a genuinely unroutable target (a
                    // disconnected region, or a spawn far off the navmesh —
                    // near-wall positions are snap-resolved by pathfinding and
                    // never latch this) is visible without per-tick spam. The
                    // steering tick holds a pathless blocked agent in place and
                    // keeps retrying under its replan cooldown; this only
                    // reports.
                    //
                    // `path_state` was snapshotted BEFORE the `set_destination`
                    // above, so the verdict is the steering tick's answer about
                    // the destination this enemy was chasing LAST tick, not the
                    // one just written. Reading it after the write would not
                    // help — `set_destination` deliberately leaves the plan
                    // intact and `agent_steering::tick` owns the replan — so the
                    // message says which tick it is describing instead.
                    if let Some(state) = path_state.as_ref() {
                        if state.blocked && blocked_warned.insert(outcome.id) {
                            log::warn!(
                                "[AI] enemy {} entered this tick blocked: as of the last \
                                 steering tick its agent had no path to the destination it \
                                 was chasing, so it is holding position. Warned once per \
                                 enemy.",
                                outcome.id
                            );
                        }
                    }
                }
            }
            SteeringIntent::MoveTo(goal) => {
                agent_steering::set_destination(registry, outcome.id, goal);
            }
            SteeringIntent::Clear => {
                agent_steering::clear_destination(registry, outcome.id);
            }
            SteeringIntent::Hold => {
                // `freeze` touches nothing PER TICK, but it cannot touch nothing
                // on the way IN. Entering a freeze state with no action verb
                // makes the brain unengaged, so `resolve_combat_slots` has just
                // surrendered its combat slot — and `set_destination` semantics
                // preserve the existing path, so leaving steering alone would
                // walk the agent into ground another enemy may claim on the very
                // next batch. Releasing the claim and continuing to walk into it
                // are mutually exclusive; the claim is what the slot solver
                // owns, so the walk is what has to stop. Clearing once on ENTRY
                // (not every tick) keeps the verb's contract intact afterwards:
                // a death animation, ragdoll, or scripted mover can drive the
                // frozen entity without this arm fighting it.
                if outcome.state_changed {
                    agent_steering::clear_destination(registry, outcome.id);
                }
            }
        }

        // Facing (yaw-only): nothing else writes the enemy's `Transform` rotation,
        // so without this the model keeps its spawn heading and moonwalks toward
        // its selected target. Orient it believably each tick it is engaged, or
        // while it is travelling under a fixed position goal:
        //   - Moving (XZ speed above the epsilon): face the velocity direction, so
        //     it faces where it is going even when routing around obstacles. The
        //     velocity is read from `path_state` (last tick's resolved velocity) —
        //     a one-tick lag on facing that is imperceptible.
        //   - Stopped but engaged (near-zero XZ speed — arrived/blocked/swinging):
        //     face this enemy's selected target.
        //   - A stopped position-goal mover leaves facing untouched, even when
        //     the target scan happened to find a nearby pawn.
        //   - Standing down: leave facing untouched.
        // The test is ENGAGEMENT, not the chase intent: a state that stands its
        // ground and swings must turn toward what it is hitting, or it lands
        // damage on a pawn behind its back. A state that neither chases nor acts
        // never turns — which is also why a closed aggro gate cannot turn an
        // enemy: it forces the resting state, and resting does neither.
        // Yaw only (model stays upright); a zero-length OR non-finite direction
        // yields `None` and writes nothing, and `slew_yaw` re-seats rather than
        // propagates a non-finite current yaw — between them, no NaN can reach
        // `Transform.rotation`, which the renderer feeds straight into the model
        // matrix and which nothing else re-seats.
        if let Some(path) = path_state.as_ref() {
            let moving = locomotion_intent.speed_xz_sq > MOVE_SPEED_EPSILON * MOVE_SPEED_EPSILON;
            let facing = match outcome.steering {
                SteeringIntent::MoveTo(_) if moving => yaw_rotation_toward(path.velocity),
                SteeringIntent::MoveTo(_) => None,
                _ if outcome.engaged && moving => yaw_rotation_toward(path.velocity),
                _ if outcome.engaged => outcome
                    .target
                    .and_then(|target| yaw_rotation_toward(target.position - path.position)),
                _ => None,
            };
            if let Some(target_rotation) = facing {
                if let Ok(mut transform) = registry.get_component::<Transform>(outcome.id).cloned()
                {
                    let current_yaw = yaw_from_rotation(transform.rotation);
                    let target_yaw = yaw_from_rotation(target_rotation);
                    let slewed_yaw = slew_yaw(current_yaw, target_yaw, FACING_TURN_RATE * tick_dt);
                    transform.rotation = Quat::from_rotation_y(slewed_yaw);
                    let _ = registry.set_component(outcome.id, transform);
                }
            }
        }

        // Damage: route the configured amount through the chokepoint to the
        // SELECTED target id, and raise the attack event. The chokepoint no-ops
        // on a non-health / stale target, but attacks are only marked above
        // after confirming this selected pawn currently has live Health.
        if outcome.attacked {
            if let Some(target) = outcome.target {
                apply_damage_with_context(
                    registry,
                    target.entity,
                    &DamagePayload {
                        amount: outcome
                            .brain
                            .graph
                            .attack
                            .map_or(0.0, |attack| attack.damage),
                    },
                    DamageContext {
                        source_id: ENEMY_ATTACK_SOURCE_ID.to_string(),
                        attacker: Some(outcome.id),
                        weapon: None,
                        zone: None,
                        producer: DamageProducer::InTick,
                    },
                );
                on_impact(registry);
            }
            events.push(Cow::Borrowed(ENEMY_ATTACK_EVENT));

            // Replay the attack clip on every IN-STATE swing. The attack clip is
            // one-shot (`loop:false`) and animation is otherwise switched only on
            // `state_changed`, so a repeated cooldown-gated swing while the enemy
            // STAYS in its attacking state would leave the clip clamped on its
            // last frame — the player cannot tell they are being hit. Restarting
            // it from frame 0 re-fires the swing visually. This is purely
            // cosmetic: damage stays cooldown-gated above (NOT frame-synced).
            //
            // Guard on `!state_changed`: on the entry tick the `state_changed`
            // switch below already plays the clip from zero, so a restart here
            // would double-fire (it would be a harmless re-stamp of a
            // just-stamped pending clip, but skipping it keeps the seam explicit:
            // first swing via the switch, every later in-state swing via restart).
            //
            // An attacking state declares an action, so it is never a locomotion
            // state — its own animation is what plays, never the rest
            // substitution.
            if !outcome.state_changed {
                if let Some(state) = state_at(&outcome.brain.graph, outcome.brain.state_index) {
                    let _ = restart_animation_clip(registry, outcome.id, &state.animation);
                }
            }
        }

        // Animation: on a state change or locomotion stop/resume, request the
        // selected animation name for the new graph/locomotion state. A failed
        // switch (`UnknownState`/`NotAnimated`) warns ONCE per distinct name and
        // keeps the prior animation — it never aborts the tick. The locomotion
        // latch is still persisted after failures so unresolved clips do not
        // re-request the same switch every tick.
        if should_switch_animation(
            outcome.state_changed,
            locomotion_intent.moving,
            outcome.brain.locomotion_moving,
        ) {
            if let Some(name) = animation_for_state(
                &outcome.brain.graph,
                outcome.brain.state_index,
                locomotion_intent.moving,
            ) {
                match switch_animation_state(registry, outcome.id, name) {
                    SwitchResult::Switched | SwitchResult::AlreadyInState => {}
                    SwitchResult::UnknownState | SwitchResult::NotAnimated => {
                        if warned.insert(format!("anim:{name}")) {
                            log::warn!(
                                "[AI] enemy animation state `{name}` could not be switched \
                                 (undeclared/unresolved on the mesh); keeping the prior \
                                 animation. Warned once per distinct name."
                            );
                        }
                    }
                }
            }
        }
        // Fold the locomotion latch into whatever the component NOW holds. The
        // damage chokepoint and `on_impact` ran since the publish above, and
        // either can mutate this entity's brain (`apply_update_enemy_state_to_brain`
        // writes exactly this component); writing the pre-callback snapshot back
        // would silently discard that. The latch is the only field this pass
        // still owns. A missing component means the entity did not survive the
        // callbacks, and there is nothing to update.
        if let Ok(mut brain) = registry
            .get_component::<BrainComponent>(outcome.id)
            .cloned()
        {
            brain.locomotion_moving = locomotion_intent.moving;
            let _ = registry.set_component(outcome.id, brain);
        }
    }

    events
}
