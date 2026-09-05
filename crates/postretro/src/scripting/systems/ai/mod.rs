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
use std::collections::{HashMap, HashSet};

use glam::{Quat, Vec3};

mod brain_programs;
mod brain_scope;
mod candidate_scope;
mod combat_slots;
mod engine_floor;
mod facing;
mod graph_eval;
mod perception;
mod targeting;

#[cfg(test)]
#[path = "../ai_tests.rs"]
mod ai_tests;

use crate::agent_steering;
use crate::collision::CollisionWorld;
use crate::nav::{NavGraph, find_path};
use brain_programs::BrainPrograms;
use brain_scope::BrainFacts;
use combat_slots::resolve_combat_slots;
use engine_floor::{POSITION_GOAL_ARRIVAL_EPSILON, SteeringIntent};
use facing::{FACING_TURN_RATE, slewed_yaw_toward, yaw_from_rotation, yaw_within_attack_tolerance};
use graph_eval::{
    action_for_path, animation_for_path, engages_active, engages_path, motion_for_path,
    select_transition_path, steering_for,
};
pub(crate) use graph_eval::{locomotion_animation, rest_animation};
use perception::LosGraceState;
use postretro_entities::components::brain::BrainComponent;
use postretro_entities::components::health::{
    DamageContext, DamageProducer, apply_damage_with_context,
};
use postretro_entities::components::mesh::{SwitchResult, switch_animation_state};
use postretro_entities::{
    ComponentKind, ComponentValue, DeferredEffectComponent, DeferredEffectKind, EntityId,
    EntityRegistry, EntityStateComponent, Transform,
};
use postretro_foundation::{ActionVerb, DamagePayload, MotionVerb, PatrolMode};
use targeting::{
    TargetPawn, TargetSelection, acquisition_due, select_target, selected_target_alive,
    target_candidate, target_distance, target_offers,
};

/// Event name fired once per enemy attack that lands this tick. Mirrors the
/// weapon-fire event precedent (`"activate"`/`"impact"`): the tick returns the
/// names it raised and the app drains them through the sequence-aware named
/// dispatcher after the tick loop settles.
pub(crate) const ENEMY_ATTACK_EVENT: &str = "enemyAttack";
const ENEMY_ATTACK_SOURCE_ID: &str = "enemy.attack";

/// Interim `@state` field supplying the engine's fresh-acquisition hostility
/// floor. Guards consume the durable `@brain.targetHostile` fact instead of
/// binding directly to this storage detail.
pub(crate) const FACTION_STATE_FIELD: &str = "faction";
/// Host-owned brain-bearing enemies begin in faction one. Player pawns leave
/// the emergent state field absent and therefore read as faction zero.
pub(crate) const ENEMY_DEFAULT_FACTION: f32 = 1.0;

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

/// Resolve the one direction this tick's apply pass will slew toward. A
/// committed aim holds its movement destination but still turns toward the
/// shared eye-to-target vector; other engaged states keep the established
/// velocity-first facing behavior.
fn facing_direction(
    steering: SteeringIntent,
    engaged: bool,
    committed_aim: bool,
    target_perception: Option<perception::EnemyTargetPerception>,
    path_state: Option<crate::agent_steering::AgentPathState>,
) -> Option<Vec3> {
    let moving_velocity = path_state
        .map(|path| path.velocity)
        .filter(|velocity| LocomotionIntent::from_velocity(*velocity).moving);
    match steering {
        SteeringIntent::MoveTo(_) if let Some(velocity) = moving_velocity => Some(velocity),
        SteeringIntent::MoveTo(_) => None,
        _ if committed_aim => {
            target_perception.map(|perception| perception.target_aim - perception.enemy_eye)
        }
        _ if engaged && let Some(velocity) = moving_velocity => Some(velocity),
        _ if engaged => {
            target_perception.map(|perception| perception.target_aim - perception.enemy_eye)
        }
        _ => None,
    }
}

fn should_switch_animation(state_changed: bool, moving: bool, latch: bool) -> bool {
    state_changed || moving != latch
}

fn entity_faction(registry: &EntityRegistry, entity: EntityId) -> f32 {
    registry
        .get_component::<EntityStateComponent>(entity)
        .map_or(0.0, |state| state.get(FACTION_STATE_FIELD))
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
        MotionVerb::MoveToAnchor => {
            if crate::nav::distance_xz(position, brain.home_anchor) <= POSITION_GOAL_ARRIVAL_EPSILON
            {
                SteeringIntent::Clear
            } else {
                SteeringIntent::MoveTo(brain.home_anchor)
            }
        }
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
        if point_count == 1 {
            return SteeringIntent::Clear;
        }
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
    rotation: Quat,
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
    /// Canonical enemy-eye/target-aim LOS endpoints. Passing them as data keeps
    /// combat positioning registry-decoupled and aligned with the fire gate.
    pub(super) enemy_eye_offset: Vec3,
    pub(super) target_aim: Option<Vec3>,
    pub(super) brain: BrainComponent,
    steering: SteeringIntent,
    /// `true` when the selected state is ENGAGED with the target — it chases it
    /// or acts on it (`graph_eval::engages`). Drives facing and combat-slot
    /// participation; the destination writes key on `steering` instead.
    pub(super) engaged: bool,
    /// The facing direction evaluated in the compute pass and written in apply.
    /// Carrying it across the pass boundary lets the fire gate inspect this
    /// tick's exact post-slew heading rather than the previous tick's rotation.
    facing_direction: Option<Vec3>,
    pub(super) combat_slot: Option<Vec3>,
    /// The target this brain held BEFORE this tick's evaluation — the incumbency
    /// test for combat-slot retention.
    pub(super) prior_acquired_target: Option<EntityId>,
    /// A replacement graph or invalid restored path was reseated this tick.
    /// Its state identity was resolved by name even when the resulting numeric
    /// index stayed equal.
    pub(super) graph_reseated: bool,
    /// `true` when the graph state changed this tick; the apply pass uses this
    /// with locomotion intent changes to decide whether to switch animation.
    state_changed: bool,
    /// `true` when an attack landed this tick (damage applied, event raised).
    attacked: bool,
    /// Damage already resolved at the one fire-latch seam. The apply pass never
    /// re-derives an action from a potentially changed path.
    attack_damage: Option<f32>,
    /// The selected offense action's standoff before and after this tick's
    /// transition. Combat slots are path-relative, not root-graph-relative.
    pub(super) prior_standoff_distance: f32,
    pub(super) standoff_distance: f32,
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
    /// Host-only LOS loss grace, keyed by enemy and pruned with live brains.
    /// It stays out of components and replication because clients never run AI
    /// perception or guard evaluation.
    los_grace: HashMap<EntityId, LosGraceState>,
}

impl AiRuntime {
    pub(crate) fn new() -> Self {
        Self {
            warned: HashSet::new(),
            blocked_warned: HashSet::new(),
            reseat_warned: HashSet::new(),
            programs: BrainPrograms::new(),
            los_grace: HashMap::new(),
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
/// state's authored `on_enter` after a transition/reseat — for the app's
/// post-tick event drain. Fresh initial seating is event-silent. `tick_dt` is
/// the fixed tick delta in seconds.
///
/// The return is `Cow` so the static attack event costs nothing to raise while
/// an authored address still travels as an owned `String`; the owning clone
/// happens once per state ENTRY, never per tick.
///
/// Ordering inside the tick, PER enemy:
/// 1. Tick cooldowns and every active activity clock.
/// 2. Evaluate outer-to-inner transition rows, `"*"` before source-keyed rows,
///    declaration order, first true wins. Every guard is evaluated every armed
///    tick; a closed aggro gate is the sole exception, standing the brain down
///    to `initial` and skipping evaluation entirely.
/// 3. On entry, reset the entered path suffix. Raise its leaf `onEnter` unless
///    this is the fresh spawn's initial seating.
/// 4. Latch-fire an active leaf's action on its first dwell tick that passes its
///    cooldown, range, live-target, LOS, and post-slew-facing gates.
/// 5. On an activity change or locomotion stop/resume, request the one animation
///    state resolved from the active nested path.
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
        los_grace,
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
    los_grace.retain(|id, _| programs.get(*id).is_some());

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
            let transform = registry.get_component::<Transform>(id).ok()?;
            Some(EnemySnapshot {
                id,
                position: transform.position,
                rotation: transform.rotation,
                brain: brain.clone(),
            })
        })
        .collect();

    // Pass 2 (compute): evaluate each brain, producing the outcomes to apply.
    let mut outcomes: Vec<EnemyOutcome> = Vec::with_capacity(snapshots.len());
    for snap in snapshots {
        let mut brain = snap.brain;
        let mut graph_reseated = programs.take_reseat(snap.id);
        if graph_reseated {
            brain.reseat_to_initial();
        }
        // Validate the complete restored path before any consumer walks it.
        // Checking only the root misses a stale child index or a descendant
        // retained beneath a leaf; trusting an over-cap length also panics in
        // timer/counter slice operations.
        if !brain.has_valid_active_path() {
            if reseat_warned.insert(snap.id) {
                log::warn!(
                    "[AI] enemy {} carried an invalid behavior activity path; re-seating it \
                     to `{}`. Warned once per enemy.",
                    snap.id,
                    brain.graph.envelope.initial,
                );
            }
            graph_reseated |= brain.reseat_to_initial();
        }
        // Read the evaluating enemy's mutable faction once for the whole
        // compute pass. Candidate comparison consumes this scalar only on a
        // fresh scan; retained target lookup deliberately does not see it.
        let enemy_faction = entity_faction(registry, snap.id);
        // A home-distance guard is about the evaluating enemy alone, not its
        // target or the acquisition stride. Compute it once from this tick's
        // immutable position snapshot before either branch can suppress target
        // work.
        let distance_from_anchor = crate::nav::distance_xz(snap.position, brain.home_anchor);
        let prior_acquired_target = brain.acquired_target;
        let (target_selection, evaluate_acquisition) = if brain.aggro_armed {
            // A target is retained across ticks only while the brain is engaged
            // — chasing one, or acting on one. A resting brain re-ranks
            // candidates instead of honoring a stale acquired id.
            let retained_target = engages_active(&brain)
                .then_some(brain.acquired_target)
                .flatten();
            let retained = retained_target
                .and_then(|entity| target_candidate(registry, entity, snap.position));
            let (target, evaluate_acquisition) = if let Some(retained) = retained {
                // A retained target alone prices the stride from its raw
                // distance. A due tick may still run the normal hysteresis scan
                // below, but neither its candidate filter nor its result can
                // alter this cost input.
                let evaluate_acquisition = acquisition_due(&brain, Some(retained.distance));
                let target = if evaluate_acquisition {
                    let (candidate_filter, candidate_scope) =
                        programs.candidate_filter_context(snap.id);
                    let offers = target_offers(
                        registry,
                        snap.position,
                        enemy_faction,
                        Some(retained.target.entity),
                    );
                    let enemy_eye =
                        perception::enemy_eye(registry, snap.id, snap.position, nav_graph);
                    let mut candidate_perception = |candidate| {
                        perception::raw_target_perception(
                            registry,
                            enemy_eye,
                            candidate,
                            collision_world,
                        )
                    };
                    select_target(
                        Some(retained),
                        &offers,
                        registry,
                        candidate_filter,
                        candidate_scope,
                        &mut candidate_perception,
                    )
                } else {
                    Some(TargetSelection {
                        target: retained.target,
                        fresh_perception: None,
                    })
                };
                (target, evaluate_acquisition)
            } else {
                let offers = target_offers(registry, snap.position, enemy_faction, None);
                let evaluate_acquisition =
                    acquisition_due(&brain, offers.nearest.map(|candidate| candidate.distance));
                let (candidate_filter, candidate_scope) =
                    programs.candidate_filter_context(snap.id);
                // The raw nearest hostile offer prices the stride. A
                // graph- and LOS-filtered selection becomes a target only on a
                // due tick; otherwise `BrainFacts` stay untargeted rather than
                // borrowing it. The offer set avoids a second registry walk
                // while keeping exact candidate raycasts off non-due ticks.
                let target = evaluate_acquisition.then(|| {
                    let enemy_eye =
                        perception::enemy_eye(registry, snap.id, snap.position, nav_graph);
                    let mut candidate_perception = |candidate| {
                        perception::raw_target_perception(
                            registry,
                            enemy_eye,
                            candidate,
                            collision_world,
                        )
                    };
                    select_target(
                        None,
                        &offers,
                        registry,
                        candidate_filter,
                        candidate_scope,
                        &mut candidate_perception,
                    )
                });
                (target.flatten(), evaluate_acquisition)
            };
            (target, evaluate_acquisition)
        } else {
            (None, false)
        };
        let target = target_selection.map(|selection| selection.target);

        // (1) Every named cooldown ticks down before the aggro gate and before
        // any guard reads its selected attack's value. Entries do not freeze
        // while another attack is current, nor disappear on a graph reseat.
        for remaining_ms in brain.attack_cooldown_remaining_ms.values_mut() {
            *remaining_ms = (*remaining_ms - dt_ms).max(0.0);
        }
        // An entry edge observes a freshly zeroed activity clock. Once it has
        // been consumed, subsequent ticks advance the active clocks before
        // evaluating their transition rows. A transition later in this pass
        // resets its new suffix again, so no newly entered activity inherits a
        // fraction of its predecessor's tick.
        if !brain.entry_pending {
            brain.tick_activity_timers(dt_ms);
        }

        // Stride bookkeeping advances every tick so the gate is deterministic.
        brain.think_stride_counter = brain.think_stride_counter.wrapping_add(1);

        // The FINALLY selected pawn's identity and distance, or `None` with no
        // target. This one binding feeds the guard facts and attack range gate,
        // so neither can disagree about which target they describe.
        let target_perception = target_selection.and_then(|selection| {
            perception::perceive_target(
                registry,
                los_grace,
                perception::TargetPerceptionQuery {
                    enemy: snap.id,
                    enemy_position: snap.position,
                    target: selection.target,
                    nav_graph,
                    collision_world,
                    fresh: selection.fresh_perception,
                },
            )
        });
        if target.is_none() {
            los_grace.remove(&snap.id);
        }
        // The graph fact and the engine-floor fire gate consume this one
        // already-debounced perception result. Keep the fire gate independent
        // of authoring below; it still calls `perception::fire_gate` directly.
        let target_visible = target_perception.is_some_and(|perception| perception.visible);
        let enemy_eye_offset = target_perception
            .map(|perception| perception.enemy_eye - snap.position)
            .unwrap_or(Vec3::ZERO);
        let target_aim = target_perception.map(|perception| perception.target_aim);

        let selected_target = target.map(|target| {
            (
                target.entity,
                target_distance(target, snap.position),
                target.position,
            )
        });
        let target_hostile = selected_target
            .is_some_and(|(target, _, _)| entity_faction(registry, target) != enemy_faction);
        // Reachability is the nav floor's pathfinder verdict, cached on the
        // existing acquisition stride. It deliberately mirrors the same
        // `find_path` capability chase consumes, rather than claiming a
        // stronger ground-truth answer. An absent nav graph has no route query,
        // so it is immediately unreachable even if a restored brain retained a
        // cached result from an earlier map or acquisition stride.
        let target_reachable = match (target, nav_graph) {
            (Some(_), None) => {
                brain.target_reachable = false;
                false
            }
            (Some(target), Some(graph)) if evaluate_acquisition => {
                let reachable = find_path(graph, snap.position, target.position).is_some();
                brain.target_reachable = reachable;
                reachable
            }
            (Some(_), Some(_)) => brain.target_reachable,
            (None, _) => {
                brain.target_reachable = false;
                false
            }
        };
        let selected_distance = selected_target.map(|(_, distance, _)| distance);
        let mut prior_standoff_distance = brain.graph.standoff_distance_for_action(None);
        let (transitioned, motion, steering) = if !brain.aggro_armed {
            // THE AGGRO GATE, and the only thing that suppresses evaluation. Its
            // v1 disengage policy is hold: a closed brain consults neither target
            // selection nor its guards, and standing down clears steering outright
            // rather than deferring to the resting state's motion verb. Clearing
            // the destination sends the agent through steering's destination-less
            // idle-settle path, which has no separation push.
            let transitioned = if !brain.is_seated_at_initial() {
                brain.reseat_to_initial()
            } else {
                false
            };
            (transitioned, None, SteeringIntent::Clear)
        } else {
            // The think stride is derived from the CURRENT player distance; the
            // gate fires when the per-enemy counter aligns with the band's
            // divisor, and reaches the guards as `@brain.acquisitionDue` for the
            // edges that opt into it. With no target the facts still refresh —
            // `hasTarget` false, `targetDistance` at its sentinel — because an
            // armed brain evaluates its whole guard set whether or not it has a
            // pawn: that is how an interrupt reaches an enemy nobody is standing
            // in front of.
            // Selector guards may choose the current action, but their input
            // snapshot has not yet been refreshed for this enemy. Seed a
            // type-correct zero cooldown first, resolve that action without
            // cloning it, then refresh the one action-relative cooldown fact
            // the transition planner is allowed to observe.
            programs.scope_mut().refresh(
                registry,
                snap.id,
                BrainFacts {
                    target: selected_target,
                    attack_cooldown_ms: 0.0,
                    acquisition_due: evaluate_acquisition,
                    distance_from_anchor,
                    target_hostile,
                    target_reachable,
                    target_visible,
                    attacks_fired_in_activity: brain.activity_attack_count(0).unwrap_or(0),
                },
            );
            let attack_cooldown_ms = programs
                .with_entry_scope(snap.id, |bound, scope| {
                    action_for_path(bound, scope, &brain).and_then(|action| match action {
                        ActionVerb::Attack(name) => {
                            brain.attack_cooldown_remaining_ms.get(name).copied()
                        }
                    })
                })
                .flatten()
                .unwrap_or(0.0);
            programs.scope_mut().refresh(
                registry,
                snap.id,
                BrainFacts {
                    target: selected_target,
                    attack_cooldown_ms,
                    acquisition_due: evaluate_acquisition,
                    distance_from_anchor,
                    target_hostile,
                    target_reachable,
                    target_visible,
                    attacks_fired_in_activity: brain.activity_attack_count(0).unwrap_or(0),
                },
            );
            prior_standoff_distance = programs
                .with_entry_scope(snap.id, |bound, scope| {
                    action_for_path(bound, scope, &brain)
                        .map(|action| brain.graph.standoff_distance_for_action(Some(action)))
                })
                .flatten()
                .unwrap_or_else(|| brain.graph.standoff_distance_for_action(None));
            let transitioned = programs
                .with_entry_scope(snap.id, |bound, scope| {
                    select_transition_path(bound, scope, &mut brain)
                })
                .unwrap_or(false);
            let motion = programs
                .with_entry_scope(snap.id, |bound, scope| {
                    motion_for_path(bound, scope, &brain)
                })
                .flatten();
            let steering = motion
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
            (transitioned, motion, steering)
        };

        // The acquired id is the cross-tick retention marker. Keep it while the
        // active path remains capable of engagement, even when an in-range move
        // selector currently resolves to `hold` and the committed leaf has no
        // action. A transition to a genuinely idle or position-goal path still
        // clears it here.
        let retains_target = target.is_some() && engages_active(&brain);
        brain.acquired_target = match target {
            Some(target) if retains_target => Some(target.entity),
            _ => None,
        };

        // Resolved engagement remains the facing policy for ordinary chase and
        // action paths. Committed actionless aim is handled separately below.
        let engaged = target.is_some()
            && programs
                .with_entry_scope(snap.id, |bound, scope| engages_path(bound, scope, &brain))
                .unwrap_or(false);

        // A nested offense phase can intentionally hold at an authored standoff
        // before its leaf exposes an action. It is still committed to the
        // target: keep its aim moving even though its resolved current verb is
        // `hold`, which `engages_path` correctly leaves false.
        let committed_aim = retains_target && matches!(motion, Some(MotionVerb::Hold));
        let facing_direction = facing_direction(
            steering,
            engaged,
            committed_aim,
            target_perception,
            agent_steering::path_state(registry, snap.id),
        );
        // If no horizontal yaw is derivable (for example, melee contact's
        // vertical eye-to-aim segment), apply leaves the transform untouched;
        // the firing check therefore reads that unchanged heading.
        let post_slew_yaw = facing_direction
            .and_then(|direction| {
                slewed_yaw_toward(snap.rotation, direction, FACING_TURN_RATE * tick_dt)
            })
            .unwrap_or_else(|| yaw_from_rotation(snap.rotation));
        let post_slew_facing_is_within_tolerance = target_perception.is_some_and(|perception| {
            yaw_within_attack_tolerance(post_slew_yaw, perception.target_aim - perception.enemy_eye)
        });

        // (4) Attack: the active firing leaf latches one graph-wide contact
        // attack on its first clear dwell tick. Its own cooldown must have
        // elapsed, the SELECTED target must be inside its `maxRange`, and it
        // must still be alive. The LOS and facing gates read this tick's shared
        // debounced perception and post-slew heading respectively.
        // The range gate lets a graph declare the action without making it
        // connect from across the room.
        // An unresolved action name configures no range and no damage, so it
        // never attacks.
        // Gating on the selected target's Health stops attack/event spam against
        // an already-dead but still-present pawn and prevents damaging a
        // different co-op pawn than the one this enemy chose.
        let entered = brain.take_entry_pending();
        let mut attacked = false;
        let mut attack_damage = None;
        if let Some(firing_leaf_depth) = brain.active_depth().checked_sub(1)
            && let (Some(target), Some(distance)) = (target, selected_distance)
            && let Some((attack_name, attack)) = programs
                .with_entry_scope(snap.id, |bound, scope| {
                    action_for_path(bound, scope, &brain).and_then(|action| match action {
                        ActionVerb::Attack(name) => brain
                            .graph
                            .attacks
                            .get(name)
                            .cloned()
                            .map(|attack| (name.clone(), attack)),
                    })
                })
                .flatten()
            && let (Some(damage), Some(max_range), Some(cooldown_ms)) =
                (attack.damage, attack.max_range, attack.cooldown_ms)
            && brain.activity_attack_count(firing_leaf_depth) == Some(0)
            && distance <= max_range
            && brain
                .attack_cooldown_remaining_ms
                .get(&attack_name)
                .copied()
                .unwrap_or(0.0)
                <= 0.0
            && selected_target_alive(registry, target.entity)
            && perception::fire_gate(target_perception)
            && post_slew_facing_is_within_tolerance
        {
            attacked = true;
            attack_damage = Some(damage);
            brain
                .attack_cooldown_remaining_ms
                .insert(attack_name.clone(), cooldown_ms);
            brain.record_successful_attack_fire();
        }

        let state_changed = graph_reseated || transitioned || entered.is_some();
        let announce_entry = brain.take_entry_event_pending() && entered.is_some();
        let on_enter = if announce_entry {
            brain
                .active_depth()
                .checked_sub(1)
                .and_then(|depth| brain.activity_at_depth(depth))
                .and_then(|(_, activity)| activity.on_enter.clone())
        } else {
            None
        };
        let standoff_distance = programs
            .with_entry_scope(snap.id, |bound, scope| {
                action_for_path(bound, scope, &brain)
                    .map(|action| brain.graph.standoff_distance_for_action(Some(action)))
            })
            .flatten()
            .unwrap_or_else(|| brain.graph.standoff_distance_for_action(None));
        outcomes.push(EnemyOutcome {
            id: snap.id,
            position: snap.position,
            target,
            enemy_eye_offset,
            target_aim,
            prior_acquired_target,
            graph_reseated,
            state_changed,
            attacked,
            attack_damage,
            prior_standoff_distance,
            standoff_distance,
            on_enter,
            steering,
            engaged,
            facing_direction,
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

        // Facing (yaw-only): compute chose this direction from the same
        // start-of-tick path state it used for the attack gate. Reusing it here
        // keeps the predicted post-slew heading and the transform write exactly
        // aligned. A committed hold-at-standoff aim therefore turns every tick
        // even before its firing leaf exposes an action.
        if let Some(direction) = outcome.facing_direction
            && let Ok(mut transform) = registry.get_component::<Transform>(outcome.id).cloned()
            && let Some(slewed_yaw) =
                slewed_yaw_toward(transform.rotation, direction, FACING_TURN_RATE * tick_dt)
        {
            transform.rotation = Quat::from_rotation_y(slewed_yaw);
            let _ = registry.set_component(outcome.id, transform);
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
                        amount: outcome.attack_damage.unwrap_or(0.0),
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
            let animation_name = animation_for_path(&outcome.brain, locomotion_intent.moving);
            if let Some(name) = animation_name {
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
