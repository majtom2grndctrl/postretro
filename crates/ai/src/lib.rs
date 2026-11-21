// Engine-owned enemy brain tick evaluates authored behavior graphs.
// See: context/lib/entity_model.md §7c · context/lib/scripting.md §11

#![deny(unsafe_code)]

use std::collections::{HashMap, HashSet};
#[cfg(any(test, feature = "test-support"))]
use std::{borrow::Cow, cell::RefCell};

use glam::{Quat, Vec3};

mod apply;
mod attack_turns;
mod brain_programs;
mod brain_scope;
mod candidate_scope;
mod clip_clock;
mod combat_slots;
mod compute;
mod decide;
mod engaged_motion;
mod engine_floor;
mod facing;
mod graph_eval;
mod heading;
mod launch;
mod leap;
mod pawn_index;
mod perception;
mod steering;
mod targeting;
mod travel_variants;
mod weapon_controller;

#[cfg(test)]
#[path = "ai_tests.rs"]
mod ai_tests;

#[cfg(any(test, feature = "test-support"))]
#[doc(hidden)]
pub use postretro_entities as __postretro_entities;
pub(crate) use postretro_physics::collision;
#[cfg(test)]
pub(crate) use postretro_physics::{kinematic_mover, movement};
#[cfg(any(test, feature = "test-support"))]
#[doc(hidden)]
pub use postretro_sim as __postretro_sim;
pub(crate) use postretro_sim::{
    agent_steering, ai_host, combat_positioning, emission, nav, scripting, scripting_systems, sim,
    weapon,
};
#[cfg(test)]
pub(crate) use postretro_sim::{
    alloc_probe, health, impact_effects, impact_policy, sprite_collection,
};

// Test-only: makes `alloc_probe` count allocations in this test binary.
// See: context/lib/testing_guide.md §Resource bounds
#[cfg(test)]
#[global_allocator]
static ALLOCATOR: alloc_probe::CountingAllocator = alloc_probe::CountingAllocator;

#[cfg(test)]
mod alloc_probe_tests {
    use crate::alloc_probe::AllocSnapshot;

    /// Positive control: the probe must see a real allocation, or every
    /// zero-allocation assertion in this crate is dead.
    #[test]
    fn probe_counts_a_real_allocation() {
        let snapshot = AllocSnapshot::arm();
        let v: Vec<u64> = std::hint::black_box(Vec::with_capacity(16));
        let allocs = snapshot.allocs_since();
        drop(v);
        assert!(allocs > 0, "probe must observe a Vec allocation");
    }
}

#[cfg(any(test, feature = "test-support"))]
use crate::ai_host::SimAiHost;
use crate::ai_host::{AiHost, AiRun, AiTickInputs, AiTickResult, ai_stage};
use crate::sim::cpu_stages::SimStage;
#[cfg(any(test, feature = "test-support"))]
use crate::{collision::CollisionWorld, nav::NavGraph};
use brain_programs::BrainPrograms;
use combat_slots::resolve_combat_slots;
use engine_floor::SteeringIntent;
use perception::LosGraceState;
use postretro_entities::components::brain::BrainComponent;
use postretro_entities::{
    ComponentKind, ComponentValue, EntityId, EntityRegistry, LiveFactionSentiment, Transform,
};
#[cfg(any(test, feature = "test-support"))]
use postretro_entities::{FactionRegistry, FactionSentimentState};
use postretro_foundation::{ARCHETYPE_TOLERANCE_STATE_FIELD, FACTION_STATE_FIELD};
#[cfg(test)]
use postretro_foundation::{locomotion_animation, rest_animation};
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
use postretro_entities::DEFAULT_ENEMY_FACTION_INDEX as ENEMY_DEFAULT_FACTION;
#[cfg(test)]
use postretro_entities::components::health::{
    DamageContext, DamageProducer, apply_damage_with_context,
};
#[cfg(test)]
use postretro_foundation::DamagePayload;
#[cfg(test)]
use targeting::{scanned_offers, select_target, target_candidate};

/// Event name fired once per enemy attack that lands this tick. Mirrors the
/// weapon-fire event precedent (`"activate"`/`"impact"`): the tick returns the
/// names it raised and the app drains them through the sequence-aware named
/// dispatcher after the tick loop settles.
pub const ENEMY_ATTACK_EVENT: &str = "enemyAttack";
const ENEMY_ATTACK_SOURCE_ID: &str = "enemy.attack";

/// Compatibility tolerance for a relationship without authored pair or
/// archetype data. No finite normal damage total can exceed it, so retaliation
/// remains inert until content deliberately lowers a tolerance.
pub const DEFAULT_RETALIATION_TOLERANCE: f32 = f32::MAX;
/// Minimum XZ speed (units/sec) the agent must exceed for "moving" behavior:
/// above it the enemy orients to its velocity and a locomotion state plays its
/// own travel animation; at or below it the enemy is treated as stopped, faces
/// its target, and a locomotion state substitutes the graph's rest animation. A
/// shared epsilon keeps facing and locomotion animation in agreement.
const MOVE_SPEED_EPSILON: f32 = 0.05;

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct LocomotionIntent {
    moving: bool,
    speed_xz_sq: f32,
}

impl LocomotionIntent {
    const STOPPED: Self = Self {
        moving: false,
        speed_xz_sq: 0.0,
    };

    pub(crate) fn from_velocity(velocity: Vec3) -> Self {
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
pub(crate) struct EnemySnapshot {
    id: EntityId,
    position: Vec3,
    rotation: Quat,
    brain: BrainComponent,
    /// Ticks since this brain's last run: its clocks advance by all of them.
    span: u32,
    /// The brain comes back to the AI set this run (it was held, quiescent,
    /// dead or inventory-owned last tick): it decides, whatever its phase.
    returned: bool,
    /// A dormant run's decide, which replaces the 15 Hz phase; `None` on an
    /// ordinary run.
    dormant_decide: Option<bool>,
}

/// One enemy's resolved outcome after evaluating its brain this tick, applied in
/// a second pass under `&mut registry`.
pub(crate) struct EnemyOutcome {
    pub(crate) id: EntityId,
    /// Ticks since this brain's last run; its timers covered all of them.
    pub(crate) span: u32,
    /// A dormant run: facing slews over the whole span. Any other run, a
    /// move-up included, slews one tick.
    pub(crate) dormant: bool,
    /// This tick decided: acquisition, guards, selectors and combat-slot
    /// choice ran. Off it, the enemy acts on its last decide's choices.
    pub(crate) decide: bool,
    /// This enemy's position as snapshotted, carried forward so combat-slot
    /// resolution needs nothing but the outcomes.
    pub(crate) position: Vec3,
    pub(crate) target: Option<TargetPawn>,
    /// Canonical enemy-eye/target-aim LOS endpoints. Passing them as data keeps
    /// combat positioning registry-decoupled and aligned with the fire gate.
    pub(crate) enemy_eye_offset: Vec3,
    pub(crate) target_aim: Option<Vec3>,
    pub(crate) brain: BrainComponent,
    /// This tick's resolved motion verb, after a flinch's stagger plants it.
    pub(crate) motion: Option<postretro_foundation::MotionVerb>,
    steering: SteeringIntent,
    /// A `leap` travelling to its spot: its `MoveTo` is paced to the clip
    /// moment. `None` for any other steering.
    leap: Option<leap::LeapTravel>,
    /// A leap entered this tick whose spot no route query resolves, near it
    /// or on its line back to the enemy: apply warns.
    leap_spot_off_mesh: bool,
    /// `true` when the selected state is ENGAGED with the target — it chases it
    /// or acts on it (`graph_eval::engages`). Drives facing and combat-slot
    /// participation; the destination writes key on `steering` instead.
    pub(crate) engaged: bool,
    /// The facing direction evaluated in the compute pass and written in apply.
    /// Carrying it across the pass boundary lets the fire gate inspect this
    /// tick's exact post-slew heading rather than the previous tick's rotation.
    facing_direction: Option<Vec3>,
    /// The directional travel clip this tick's facing and travel ask for,
    /// before the mesh is checked for it.
    travel_variant: postretro_entities::components::brain::TravelVariant,
    pub(crate) combat_slot: Option<Vec3>,
    /// The rotated ring an engaged `strafe` or `reposition` proposes in place
    /// of the shared ring. Its steering resolves from the claim.
    pub(crate) engaged_ring: Option<crate::combat_positioning::SlotRing>,
    /// The target this brain held BEFORE this tick's evaluation — the incumbency
    /// test for combat-slot retention.
    pub(crate) prior_acquired_target: Option<EntityId>,
    /// A replacement graph or invalid restored path was reseated this tick.
    /// Its state identity was resolved by name even when the resulting numeric
    /// index stayed equal.
    pub(crate) graph_reseated: bool,
    /// `true` when the graph state changed this tick; the apply pass uses this
    /// with locomotion intent changes to decide whether to switch animation.
    state_changed: bool,
    /// Immutable fire resolution selected by compute. The mutable cooldown and
    /// activity-count commit waits for apply to revalidate the target against
    /// earlier outcomes in the same batch.
    attack: Option<PendingAttack>,
    /// Current selected-target aim for an already accepted weapon sequence.
    weapon_aim: Option<weapon_controller::WeaponAim>,
    /// The selected offense action's standoff before and after this tick's
    /// transition. Combat slots are path-relative, not root-graph-relative.
    pub(crate) prior_standoff_distance: f32,
    pub(crate) standoff_distance: f32,
    /// The activity entered this tick, present only on the tick the brain
    /// entered it.
    entered: Option<EnteredActivity>,
    /// The attack a standing rest's first open queued row waits on, by
    /// attack index: the rest's request this tick, unless a mark outranks it.
    queued_attack: Option<u16>,
    /// An unmarked turn granted this tick, by attack index. Released in apply
    /// unless that attack starts this tick.
    fresh_unmarked_grant: Option<u16>,
    /// A flinch played over an activity routing flinches (`staggerTo`) as
    /// this tick began, even one a transition this tick then left: apply
    /// cancels the burst in flight.
    flinched: bool,
}

/// An activity the brain entered this tick: its authored `on_enter` address,
/// if any, and its root-to-leaf index path in the brain's graph.
pub(crate) struct EnteredActivity {
    on_enter: Option<String>,
    path: Vec<usize>,
}

/// One successful fire-latch's engine-owned resolution. Contact attacks keep
/// their direct-damage path; weapon attacks propose controller input. Apply
/// advances the persistent component after the immutable registry borrow ends.
pub(crate) struct PendingAttack {
    attack_name: String,
    cooldown_ms: f32,
    effect: AttackOutcome,
}

pub(crate) enum AttackOutcome {
    Contact {
        damage: f32,
        /// The attack's damage type by name; `None` deals the mod default.
        damage_type: Option<String>,
    },
    Projectile {
        request: weapon_controller::WeaponAttackRequest,
    },
}

/// The AI tick's run-long state, owned by `App` across ticks.
///
/// Bound guards, canonical weapon components, and diagnostics outlive a tick.
/// Their lifecycle is reconciled together before compute: newly seen graphs
/// bind, changed descriptors cancel old execution, and dead actors are pruned.
pub struct AiRuntime {
    /// The brains in the AI set last tick, run or skipped by the schedule.
    presence: attack_turns::Presence,
    /// This tick's skipped holders. Reused storage.
    skipped: Vec<attack_turns::SkippedHolder>,
    /// Ticks between a brain's decides: [`decide::DECIDE_INTERVAL_TICKS`]
    /// outside unit suites that pin graph semantics a tick at a time. A brain
    /// also decides off its phase on a reseat, with no selector memo for its
    /// path (its first run, a run after a closed aggro gate), and on its
    /// return to the AI set.
    decide_interval: u64,
    /// An NPC without a target takes the best candidate on sight instead of
    /// noticing through its awareness meters: today's 60 Hz semantics, which
    /// the graph, turn and clip unit suites pin. Production always meters.
    notice_on_sight: bool,
    /// Each brain's selector rows from its last decide, for the ticks between.
    /// Host-only, pruned with live brains.
    selector_memo: HashMap<EntityId, brain_scope::SelectorChoices>,
    /// Route-search scratch for the pass's reachability and positioning
    /// queries. Contents never carry between queries.
    nav_scratch: crate::nav::NavScratch,
    /// This pass's targetable pawns by faction, rebuilt every pass.
    pawns: pawn_index::PawnIndex,
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
    /// Canonical weapon components retained across attacks and descriptor sync.
    weapons: weapon_controller::AiWeaponControllers,
    /// The attack-turn tick clock, budgets and arbitration scratch. Turns
    /// themselves live on brains.
    turns: attack_turns::AttackTurnRuntime,
}

impl AiRuntime {
    /// Cancel pending private weapon work on input/world suspension, retaining
    /// installed tuning, paid resources and recovery already owed.
    pub fn cancel_weapon_activations(&mut self) {
        self.weapons.cancel_activations();
    }

    /// Drop the state that names the unloading level's entities: refreshing
    /// attack-turn counts, the turn roster, and the running attack pools'
    /// budgets. Call on level unload, before the next level's first tick.
    pub fn clear_level_state(&mut self) {
        self.turns.clear_level_state();
        self.presence.clear();
    }

    pub fn new() -> Self {
        Self {
            nav_scratch: crate::nav::NavScratch::new(),
            pawns: pawn_index::PawnIndex::default(),
            warned: HashSet::new(),
            blocked_warned: HashSet::new(),
            reseat_warned: HashSet::new(),
            programs: BrainPrograms::new(),
            los_grace: HashMap::new(),
            weapons: weapon_controller::AiWeaponControllers::default(),
            turns: attack_turns::AttackTurnRuntime::default(),
            presence: attack_turns::Presence::default(),
            skipped: Vec::new(),
            selector_memo: HashMap::new(),
            decide_interval: decide::DECIDE_INTERVAL_TICKS,
            notice_on_sight: false,
        }
    }
}

#[cfg(any(test, feature = "test-support"))]
impl AiRuntime {
    /// A runtime whose brains decide every tick and take the best candidate
    /// on sight: the 60 Hz granularity and first-sight acquisition the graph,
    /// turn and clip unit suites pin. Production decides every
    /// [`decide::DECIDE_INTERVAL_TICKS`] and notices through the awareness
    /// meters; the decide, awareness and stress rows run that.
    pub fn deciding_every_tick() -> Self {
        Self {
            decide_interval: 1,
            notice_on_sight: true,
            ..Self::new()
        }
    }

    /// A production-cadence runtime whose brains take the best candidate on
    /// sight: for rows about decide cadence, turns or engaged motion that
    /// predate the awareness meters and are not about noticing.
    pub fn noticing_on_sight() -> Self {
        Self {
            notice_on_sight: true,
            ..Self::new()
        }
    }
}

impl Default for AiRuntime {
    fn default() -> Self {
        Self::new()
    }
}

/// Bind an application-owned runtime into simulation's injected AI callback.
/// Expanding at the call site lets Rust infer the callback's independent
/// registry, world-input, and host borrow lifetimes.
#[macro_export]
macro_rules! tick_runner {
    ($runtime:expr) => {
        |registry, tick_dt, inputs, host| {
            $crate::run_ai_tick_with_host(registry, $runtime, tick_dt, inputs, host)
        }
    };
}

/// Adapt `postretro-sim`'s unit-test crate identity to AI's normal sim
/// dependency. Cargo compiles those as distinct crate instances in the
/// sim(dev) -> AI -> sim(normal) cycle, even though their lower-layer entity,
/// collision, and descriptor values are shared.
#[cfg(any(test, feature = "test-support"))]
#[doc(hidden)]
#[macro_export]
macro_rules! test_tick_runner {
    ($runtime:expr) => {
        $crate::test_tick_runner!($runtime, None)
    };
    // `$clips` is an `Option<&HitZoneStore>` of AI's own sim build, loaded
    // from the same models as the caller's store, for clip-timed attacks.
    ($runtime:expr, $clips:expr) => {
        |registry, tick_dt, inputs, host| {
            let rebuilt_nav = inputs.nav_graph.map(|graph| {
                $crate::__postretro_sim::nav::NavGraph::from_section(&graph.to_section_for_test())
            });
            let external_inputs = $crate::__postretro_sim::ai_host::AiTickInputs {
                nav_graph: rebuilt_nav.as_ref(),
                // The runner bridges two builds of `postretro_sim`, whose dock
                // tables are different types; a gate test drives `ai` directly.
                mover_nav: $crate::__postretro_sim::nav::MoverNav::none(),
                collision_world: inputs.collision_world,
                descriptors: inputs.descriptors,
                descriptor_generation: inputs.descriptor_generation,
                factions: inputs.factions,
                faction_sentiment: inputs.faction_sentiment,
                // The clip store is a type of the other `postretro_sim` build,
                // so the caller's cannot cross this bridge: without one built
                // in AI's build, every clip runs unresolved.
                clips: $clips,
                // The stage frame is the other build's `SimStage` too: AI
                // sub-stages report only through the same-build runner.
                schedule: Default::default(),
                cpu: None,
            };
            let mut on_impact = |registry: &mut $crate::__postretro_entities::EntityRegistry| {
                host.on_impact(registry)
            };
            let mut concrete_host =
                $crate::__postretro_sim::ai_host::SimAiHost::new(&mut on_impact);
            let result = $crate::run_ai_tick_with_host(
                registry,
                $runtime,
                tick_dt,
                external_inputs,
                &mut concrete_host,
            );
            (
                result.events,
                result
                    .projectile_spawns
                    .into_iter()
                    .map(|spawn| (spawn.projectile, spawn.descriptor_class))
                    .collect::<Vec<_>>(),
            )
        }
    };
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
/// 1. Tick cooldowns and every active activity clock. Close a pending clip
///    moment the mesh was switched off since the brain's own last switch, then
///    advance the clip clock at the previous tick's playback rate.
/// 2. Evaluate outer-to-inner transition rows, `"*"` before source-keyed rows,
///    declaration order, first true wins. Every guard is evaluated every armed
///    tick; a closed aggro gate is the sole exception, standing the brain down
///    to `initial` and skipping evaluation entirely.
/// 3. On entry, reset the entered path suffix. Raise its leaf `onEnter` unless
///    this is the fresh spawn's initial seating.
/// 4. Latch-fire an active leaf's action on its first dwell tick that passes its
///    cooldown, range, live-target, LOS, and post-slew-facing gates. An
///    unmarked attack ready to fire requests an attack turn; the hard gate
///    drops any drawn attack whose enemy holds no turn for it. An action fired
///    `at` a clip event instead proposes only on the tick its clip reaches the
///    event, once per entry; a launch closes the moment, and a changed drawn
///    state restarts the clip clock first.
/// 5. On an activity change, locomotion stop/resume, or an airborne launch's
///    `jump_loop` starting or ending, request the one animation state resolved
///    from the active nested path, or `jump_loop` while it plays. A switch
///    absorbed since compute closes a pending moment off an entry tick, and
///    switching the mesh back onto the clip clock's state restarts the clock.
///
/// Between compute and apply run the batch stages, across all enemies: combat
/// slots, then attack-turn arbitration (releases, requests, grants, the hard
/// gate). Apply then commits each outcome and releases an unmarked grant whose
/// attack did not start.
#[cfg(any(test, feature = "test-support"))]
pub fn run_ai_tick(
    registry: &mut EntityRegistry,
    runtime: &mut AiRuntime,
    tick_dt: f32,
) -> Vec<Cow<'static, str>> {
    run_ai_tick_with_navigation(registry, runtime, tick_dt, None, None)
}

#[cfg(any(test, feature = "test-support"))]
pub(crate) fn run_ai_tick_with_navigation(
    registry: &mut EntityRegistry,
    runtime: &mut AiRuntime,
    tick_dt: f32,
    nav_graph: Option<&NavGraph>,
    collision_world: Option<&CollisionWorld>,
) -> Vec<Cow<'static, str>> {
    let factions = FactionRegistry::default();
    let faction_sentiment = RefCell::new(FactionSentimentState::default());
    run_ai_tick_with_navigation_and_impact(
        registry,
        runtime,
        tick_dt,
        AiTickInputs {
            nav_graph,
            mover_nav: crate::nav::MoverNav::none(),
            collision_world,
            descriptors: &[],
            descriptor_generation: 0,
            factions: &factions,
            faction_sentiment: &faction_sentiment,
            clips: None,
            schedule: Default::default(),
            cpu: None,
        },
        |_| {},
    )
    .events
    .into_iter()
    .filter_map(|emission| emission.address)
    .collect()
}

#[cfg(any(test, feature = "test-support"))]
pub(crate) fn run_ai_tick_with_navigation_and_impact(
    registry: &mut EntityRegistry,
    runtime: &mut AiRuntime,
    tick_dt: f32,
    inputs: AiTickInputs<'_>,
    mut on_impact: impl FnMut(&mut EntityRegistry),
) -> AiTickResult {
    let mut host = SimAiHost::new(&mut on_impact);
    run_ai_tick_with_host(registry, runtime, tick_dt, inputs, &mut host)
}

pub fn run_ai_tick_with_host<H>(
    registry: &mut EntityRegistry,
    runtime: &mut AiRuntime,
    tick_dt: f32,
    inputs: AiTickInputs<'_>,
    host: &mut H,
) -> AiTickResult
where
    H: AiHost,
{
    let AiTickInputs {
        nav_graph,
        mover_nav,
        collision_world,
        descriptors,
        descriptor_generation,
        factions,
        faction_sentiment,
        clips,
        cpu,
        schedule,
    } = inputs;
    let dt_ms = tick_dt.max(0.0) * 1000.0;

    // Reconcile the bound-guard side-table with the registry's live brains
    // before anything reads it: this is the single lifecycle hook covering
    // spawn, despawn, and a wholesale deserialize.
    let AiRuntime {
        nav_scratch,
        pawns,
        warned,
        blocked_warned,
        reseat_warned,
        programs,
        los_grace,
        weapons,
        turns,
        presence,
        skipped,
        selector_memo,
        decide_interval,
        notice_on_sight,
    } = runtime;
    turns.begin_tick(dt_ms, registry);
    turns.sync_pools(attack_turns::running_attack_pools(registry));
    programs.sync(registry, descriptors, descriptor_generation, warned);
    if let Some(clips) = clips {
        programs.warn_unclipped_moments(registry, clips, tick_dt, warned);
    }
    weapons.sync(registry, programs, |actor| {
        host.is_quiescent(registry, actor)
    });
    weapons.begin_tick(dt_ms);

    // Bound the blocked-warn latch to entities that still carry a brain: the
    // side-table `sync` just reconciled is the authoritative live set, so this
    // is where the pruning is free. Without it a wave-spawning level accumulates
    // one entry per enemy that ever blocked, for the process lifetime. A reused
    // entity id may report once more, which is the right answer for what is a
    // different enemy.
    blocked_warned.retain(|id| programs.get(*id).is_some());
    reseat_warned.retain(|id| programs.get(*id).is_some());
    los_grace.retain(|id, _| programs.get(*id).is_some());
    selector_memo.retain(|id, _| programs.get(*id).is_some());

    // Pass 1: snapshot every brain-bearing enemy the schedule runs under the
    // immutable borrow. One it skips stays in the AI set as a holder only:
    // no clone, no evaluation.
    presence.begin();
    skipped.clear();
    let mut remapped_while_skipped = Vec::new();
    let snapshots: Vec<EnemySnapshot> = registry
        .iter_with_kind(ComponentKind::Brain)
        .filter_map(|(id, value)| {
            // A terminal impact effect or queued despawn leaves the id live
            // long enough for a same-group playAnim to address it. AI must not
            // overwrite that presentation request or keep steering/attacking.
            if registry.is_inventory_owned(id) || host.is_quiescent(registry, id) {
                return None;
            }
            let ComponentValue::Brain(brain) = value else {
                return None;
            };
            let present_last_tick = presence.was_present(id);
            let run = schedule.run(id);
            if run == AiRun::Skip {
                // A skip keeps a brain in the set only if it was in it: one
                // returning from outside comes back on its next run, holding
                // nothing.
                if present_last_tick {
                    presence.mark(id);
                    let mut held = brain.attack_turns;
                    held.request = None;
                    let before = held;
                    turns.remap_brain(&mut held);
                    if held != before {
                        remapped_while_skipped.push(id);
                    }
                    skipped.push(attack_turns::SkippedHolder {
                        id,
                        pool: attack_turns::attack_pool_of(&turns.budgets, brain),
                        turns: held,
                    });
                }
                return None;
            }
            // Marked present only once it can run: a brain with no transform
            // is neither snapshotted nor skipped, so it is out of the set.
            let transform = registry.get_component::<Transform>(id).ok()?;
            presence.mark(id);
            let mut brain = brain.clone();
            let returned =
                attack_turns::returns_to_set(&brain.attack_turns, turns.tick, present_last_tick);
            let span = attack_turns::admit(&mut brain.attack_turns, turns.tick, present_last_tick);
            turns.remap_brain(&mut brain.attack_turns);
            Some(EnemySnapshot {
                id,
                position: transform.position,
                rotation: transform.rotation,
                brain,
                span,
                returned,
                dormant_decide: match run {
                    AiRun::RunDormant { decide } => Some(decide),
                    AiRun::Run | AiRun::Skip => None,
                },
            })
        })
        .collect();
    // A token shift reaches a skipped holder's stored turns now: its next run
    // reads them under the new ids. Rare (a reload that moves tokens).
    for id in remapped_while_skipped {
        if let Ok(brain) = registry.get_component::<BrainComponent>(id) {
            let mut brain = brain.clone();
            turns.remap_brain(&mut brain.attack_turns);
            let _ = registry.set_component(id, brain);
        }
    }
    // The turn facts guards read this tick describe the line as it stood
    // after the previous tick.
    let attack_turns::AttackTurnRuntime {
        budgets, previous, ..
    } = &mut *turns;
    let run_count = snapshots.len();
    previous.rebuild(
        budgets,
        snapshots
            .iter()
            .enumerate()
            .map(|(slot, snapshot)| attack_turns::LineEntry {
                entity: snapshot.id,
                slot: slot as u32,
                pool: attack_turns::attack_pool_of(budgets, &snapshot.brain),
                turns: &snapshot.brain.attack_turns,
            })
            .chain(
                skipped
                    .iter()
                    .enumerate()
                    .map(|(index, holder)| attack_turns::LineEntry {
                        entity: holder.id,
                        slot: (run_count + index) as u32,
                        pool: holder.pool,
                        turns: &holder.turns,
                    }),
            ),
    );

    // Every enemy's candidates, read once for the pass. Built here, after the
    // previous pass's apply and before compute borrows the registry for the
    // rest of it. Unlike the snapshots, quiescent brains stay: they remain
    // candidates.
    {
        let _stage = ai_stage(cpu, SimStage::AiTargeting);
        if snapshots.is_empty() {
            // No enemy selects this pass: skip the registry walk.
            pawns.clear();
        } else {
            pawns.rebuild(registry);
        }
    }

    // The live view exists only for compute. `apply_outcomes` can invoke an
    // impact policy which mutates the overlay, so retaining this Ref across the
    // apply pass would turn an ordinary same-tick write into a RefCell panic.
    // NPC sight sees movers at this tick's pose; a closed door hides a target.
    let sight_world = collision_world.map(|world| {
        crate::collision::moving::CombinedCollisionWorld::new(
            world,
            mover_nav.movers,
            mover_nav.poses,
        )
    });
    let mut outcomes = {
        let faction_sentiment = faction_sentiment.borrow();
        let live_factions = LiveFactionSentiment::new(factions, &faction_sentiment);
        compute::evaluate(
            registry,
            snapshots,
            programs,
            reseat_warned,
            los_grace,
            tick_dt,
            dt_ms,
            nav_graph,
            mover_nav.gate,
            sight_world.as_ref(),
            &live_factions,
            turns,
            clips,
            cpu,
            nav_scratch,
            pawns,
            selector_memo,
            *decide_interval,
            *notice_on_sight,
        )
    };

    resolve_combat_slots(
        registry,
        &mut outcomes,
        nav_graph,
        mover_nav,
        collision_world,
        cpu,
        nav_scratch,
    );

    let _apply_stage = ai_stage(cpu, SimStage::AiApply);
    attack_turns::arbitrate_attack_turns(&mut outcomes, skipped, programs, turns, weapons);

    apply::apply_outcomes(
        registry,
        outcomes,
        tick_dt,
        warned,
        blocked_warned,
        weapons,
        turns,
        host,
    )
}
