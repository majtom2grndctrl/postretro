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
// For each admitted live brain, exactly one authored-state condition suppresses
// guard evaluation: a closed aggro gate. It stands the brain down to its graph's
// `initial` state with steering cleared and reads neither targeting nor guards.
// Everything else — including having no target at all — evaluates the whole
// guard set, with the no-target
// facts (`@brain.hasTarget` false, `@brain.targetDistance` at its sentinel)
// projected into the scope. That is what lets a sealed-closet enemy that gets
// shot flinch on an authored interrupt while it has nobody to chase.

use std::borrow::Cow;
use std::collections::{HashMap, HashSet};

use glam::{Quat, Vec3};

mod apply;
mod brain_programs;
mod brain_scope;
mod candidate_scope;
mod combat_slots;
mod compute;
mod engine_floor;
mod facing;
mod graph_eval;
mod perception;
mod steering;
mod targeting;

#[cfg(test)]
#[path = "../ai_tests.rs"]
mod ai_tests;

use crate::collision::CollisionWorld;
use crate::nav::NavGraph;
use crate::sim::EnemyProjectilePresentationSpawn;
use crate::weapon::ProjectileLaunch;
use brain_programs::BrainPrograms;
use combat_slots::resolve_combat_slots;
use engine_floor::SteeringIntent;
pub(crate) use graph_eval::{locomotion_animation, rest_animation};
use perception::LosGraceState;
use postretro_entities::components::brain::BrainComponent;
use postretro_entities::components::health::HealthComponent;
use postretro_entities::{
    ComponentKind, ComponentValue, DeferredEffectComponent, DeferredEffectKind, EntityId,
    EntityRegistry, FactionRegistry, Transform,
};
use postretro_scripting_core::data_descriptors::EntityTypeDescriptor;
use targeting::TargetPawn;

#[cfg(test)]
use apply::should_switch_animation;
#[cfg(test)]
use brain_scope::BrainFacts;
#[cfg(test)]
use engine_floor::POSITION_GOAL_ARRIVAL_EPSILON;
#[cfg(test)]
use graph_eval::{engages_active, select_transition_path, steering_for};
#[cfg(test)]
use postretro_entities::components::health::{
    DamageContext, DamageProducer, apply_damage_with_context,
};
#[cfg(test)]
use postretro_foundation::DamagePayload;
#[cfg(test)]
use targeting::{acquisition_due, select_target, target_candidate, target_offers};

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
/// Optional per-archetype retaliation tolerance. This engine-owned storage is
/// intentionally not an authored guard vocabulary; candidate guards consume
/// only the resolved `@candidate.tolerance` fact.
pub(crate) const ARCHETYPE_TOLERANCE_STATE_FIELD: &str = "archetype_tolerance";
/// Compatibility tolerance for a relationship without authored pair or
/// archetype data. No finite normal damage total can exceed it, so retaliation
/// remains inert until content deliberately lowers a tolerance.
pub(crate) const DEFAULT_RETALIATION_TOLERANCE: f32 = f32::MAX;
/// Host-owned brain-bearing enemies begin in faction one. Player pawns leave
/// the emergent state field absent and therefore read as faction zero.
#[cfg(test)]
pub(crate) const ENEMY_DEFAULT_FACTION: f32 = postretro_entities::DEFAULT_ENEMY_FACTION_INDEX;

/// Minimum XZ speed (units/sec) the agent must exceed for "moving" behavior:
/// above it the enemy orients to its velocity and a locomotion state plays its
/// own travel animation; at or below it the enemy is treated as stopped, faces
/// its target, and a locomotion state substitutes the graph's rest animation. A
/// shared epsilon keeps facing and locomotion animation in agreement.
const MOVE_SPEED_EPSILON: f32 = 0.05;

#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct LocomotionIntent {
    moving: bool,
    speed_xz_sq: f32,
}

impl LocomotionIntent {
    const STOPPED: Self = Self {
        moving: false,
        speed_xz_sq: 0.0,
    };

    pub(super) fn from_velocity(velocity: Vec3) -> Self {
        let speed_xz_sq = velocity.x * velocity.x + velocity.z * velocity.z;
        Self {
            moving: speed_xz_sq > MOVE_SPEED_EPSILON * MOVE_SPEED_EPSILON,
            speed_xz_sq,
        }
    }
}

/// Per-enemy snapshot captured under the immutable iterator borrow so the
/// mutable writes (steering, damage, animation) happen after the walk completes.
/// The compute pass CONSUMES these — the brain is moved into its outcome rather
/// than cloned a second time, which matters because a brain carries its graph.
pub(super) struct EnemySnapshot {
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
    /// `true` when an attack fired this tick (event raised; projectile contact
    /// damage arrives in a later simulation stage).
    attacked: bool,
    /// Fire-time resolution already selected at the one fire-latch seam. The
    /// apply pass never re-derives an action from a potentially changed path.
    attack: Option<AttackOutcome>,
    /// The selected offense action's standoff before and after this tick's
    /// transition. Combat slots are path-relative, not root-graph-relative.
    pub(super) prior_standoff_distance: f32,
    pub(super) standoff_distance: f32,
    /// The entered state's authored `on_enter` address, present only on the tick
    /// the brain entered it.
    on_enter: Option<String>,
}

/// One successful fire-latch's engine-owned resolution. Contact attacks keep
/// their direct-damage path; weapon attacks carry the launch materialized by
/// the apply pass, after the immutable evaluator has released its registry
/// borrow.
pub(super) enum AttackOutcome {
    Contact {
        damage: f32,
    },
    Projectile {
        launch: Box<ProjectileLaunch>,
        /// The resolved weapon descriptor is the presentation class. Enemy
        /// entities never stand in for a materialized wieldable descriptor.
        descriptor_class: String,
    },
}

/// Host-only output from the AI stage. Event addresses retain their existing
/// post-tick dispatch path, while standalone enemy projectile ids carry the
/// resolved weapon class to the listen-host presentation mirror.
pub(crate) struct AiTickResult {
    pub(crate) events: Vec<Cow<'static, str>>,
    pub(crate) projectile_spawns: Vec<EnemyProjectilePresentationSpawn>,
}

/// Borrowed world data read by one host-side AI tick.
///
/// The descriptor slice and its generation are one snapshot: callers must
/// obtain both from the same data-registry borrow. Keeping the world data
/// borrowed makes this a stack-only view with no per-tick allocation or
/// ownership transfer.
pub(crate) struct AiTickInputs<'a> {
    pub(crate) nav_graph: Option<&'a NavGraph>,
    pub(crate) collision_world: Option<&'a CollisionWorld>,
    pub(crate) descriptors: &'a [EntityTypeDescriptor],
    pub(crate) descriptor_generation: u64,
    /// Resolved manifest faction relationships. The App borrows this from the
    /// same `DataRegistry` snapshot as descriptors, so a tick cannot observe a
    /// new faction matrix beside stale content.
    pub(crate) factions: &'a FactionRegistry,
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
    let factions = FactionRegistry::default();
    run_ai_tick_with_navigation_and_impact(
        registry,
        runtime,
        tick_dt,
        AiTickInputs {
            nav_graph,
            collision_world,
            descriptors: &[],
            descriptor_generation: 0,
            factions: &factions,
        },
        |_| {},
    )
    .events
}

pub(crate) fn run_ai_tick_with_navigation_and_impact(
    registry: &mut EntityRegistry,
    runtime: &mut AiRuntime,
    tick_dt: f32,
    inputs: AiTickInputs<'_>,
    on_impact: impl FnMut(&mut EntityRegistry),
) -> AiTickResult {
    let AiTickInputs {
        nav_graph,
        collision_world,
        descriptors,
        descriptor_generation,
        factions,
    } = inputs;
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
    programs.sync(registry, descriptors, descriptor_generation, warned);

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
                .get_component::<HealthComponent>(id)
                .is_ok_and(crate::scripting_systems::health::is_depleted)
                || registry
                    .get_component::<DeferredEffectComponent>(id)
                    .is_ok_and(|effects| {
                        effects.inert
                            || effects
                                .pending
                                .iter()
                                .any(|effect| effect.kind == DeferredEffectKind::Despawn)
                    })
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

    let mut outcomes = compute::evaluate(
        registry,
        snapshots,
        programs,
        reseat_warned,
        los_grace,
        tick_dt,
        dt_ms,
        nav_graph,
        collision_world,
        factions,
    );

    resolve_combat_slots(&mut outcomes, nav_graph, collision_world);

    apply::apply_outcomes(
        registry,
        outcomes,
        tick_dt,
        warned,
        blocked_warned,
        on_impact,
    )
}
